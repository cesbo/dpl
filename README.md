# dpl

`dpl` is a CLI deploy tool. It manages local units (apps, database servers,
databases, domains), keeps encrypted secrets on disk, and performs one-shot
deploys: extract a `.tar.gz`, render build artifacts from MiniJinja templates,
run `podman build`, optionally export static files, and (re)install the
generated systemd service.

`dpl` is not a daemon — every command runs to completion in the foreground.

## Requirements

- linux - recommended Fedora 42
- systemd
- podman
- nginx

### Directory Structure

- `{base_dir}` - base directory for all `dpl` files (default: `/opt/dpl`), set via `--base`
- `{unit_dir}` - unit directory: `{base_dir}/{unit_name}/`
- `{deploy_dir}` - deploy directory for one version of an app: `{unit_dir}/deploy_{version}/`

## CLI

`dpl` exposes three subcommand groups. The `--base` flag is global and
defaults to `/opt/dpl`:

```bash
dpl --base /opt/dpl <group> <command> [args]
```

| Group | Purpose |
|-------|---------|
| `dpl unit`   | Validate, deploy, and inspect units (`check`, `deploy`, `state`) |
| `dpl db`     | Bring up DB-server units and create databases (`init`, `create`, `wait`) |
| `dpl secret` | Manage encrypted runtime secrets (`create`, `cat`, `rm`, `ls`) |

Run any command with `--help` for the full flag list.

## Unit Config

Each unit lives in its own directory `{unit_dir}` and has:

- `config.yaml` - unit config. The `type` field selects the variant: `app`,
  `db-server`, `db`, or `domain`
- `state.yaml` - serialized deploy state (`active_version` and `latest_build`).
  Only app deploys update this file
- `.deploy.lock` - advisory `flock(2)` held for the duration of a deploy so
  two `dpl unit deploy` invocations against the same unit can't race

## App Unit

An app unit represents a containerized application. `dpl unit deploy` accepts a
`.tar.gz` archive with the source code, generates a `containerfile` from the
config, builds a podman image, and optionally exports static files from the
built image.

### Configuration file

File:

```text
{unit_dir}/config.yaml
```

Example:

```yaml
type: app
image: node:22-alpine
port: 3000

build:
  - files: ["package.json", "package-lock.json"]
    script: |
      npm ci

  - files: ["*"]
    env:
      NODE_ENV: production
      NPM_TOKEN: ${secret:npm-token}
    script: |
      npm run build

runtime:
  env:
    NODE_ENV: production
    DB_URL: ${app-db:url}
    API_TOKEN: ${secret:api-token}
  init: |
    test -d /app/dist
  cmd: node server.js

databases:
  - app-db

volumes:
  - source: myapp-data
    path: /app/data

exports:
  - source: /app/public
    path: /static

timers:
  - name: cleanup
    schedule: "0 3 * * *"
    script: node cleanup.js
```

Fields:

- `type` - must be `app`
- `image` - base image for the generated `containerfile`
- `port` - port the application listens on
- `build` - list of build layers (see below)
- `runtime` - runtime configuration (see below)
- `databases` - list of `db` unit names this app depends on. Their values are
  available to `runtime.env` and `build.env` via the `${<db-unit>:<key>}`
  template (see [Database Units](#database-units))
- `volumes` - persistent storage mounted into the container. Data in volumes
  survives redeploys
- `exports` - copies files from the built image into `{deploy_dir}/exports/`
- `timers` - periodic scripts to run in the container

Runtime fields:

- `env` - environment variables. Values may be plain strings or template
  expressions (see [Env templates](#env-templates))
- `init` - optional shell script that runs before `cmd`
- `cmd` - main start command

Build layer fields:

- `files` - paths copied from the extracted archive into `/app`. Use `"*"` to
  copy all files
- `env` - build-time environment variables for the layer script. Supports the
  same templates as `runtime.env`
- `script` - shell script for the layer. If missing, the layer only copies files

### Env templates

`env` values support two template forms:

- `${secret:<name>}` - inlined plaintext of an encrypted secret. See
  [Secrets](#secrets)
- `${<db-unit>:<key>}` - export of a referenced `db` unit. Allowed keys:
  `name`, `user`, `password`, `host`, `port`, `url`. The `db` unit must appear
  in this app's `databases:` list and must already exist

References are validated by `dpl unit check`.

### Files

App-specific files in `{unit_dir}`:

- `{unit_dir}/port.txt` - persisted host port for the unit
- `{unit_dir}/deploy_{version}/` - versioned deploy directory. Referred to as
  `{deploy_dir}`

`{deploy_dir}` layout:

- `{deploy_dir}/app.tar.gz` - uploaded archive
- `{deploy_dir}/app/` - extracted archive
- `{deploy_dir}/artifacts/` - generated deploy files such as `containerfile`,
  `run.sh`, `build-N.sh`, and systemd service files
- `{deploy_dir}/exports/` - static files exported from the built image
- `{deploy_dir}/log/build.log` - build log with podman build output

## Database Units

Two unit types make up the database story:

- **`db-server`** - a containerized DBMS (PostgreSQL or MariaDB) managed as a
  systemd unit. Each `db-server` runs one engine instance and holds a single
  root password
- **`db`** - a single database + login user inside an existing `db-server`. A
  `db` unit is referenced by app units and exposes connection values
  (`name`, `user`, `password`, `host`, `port`, `url`)

### `db-server` config

```yaml
type: db-server
engine: postgresql      # or "mariadb"
version: 18-alpine
secret: db-server-password
```

- `engine` - `postgresql` or `mariadb`
- `version` - image tag. Verified against the registry with `podman manifest inspect`
- `secret` - name of an existing `dpl secret` holding the root password

### `db` config

```yaml
type: db
server: db-main         # name of an existing db-server unit
user: app1
secret: app1-db-password
```

- `server` - name of the parent `db-server` unit
- `user` - SQL login that owns the database (defaults to the db unit name)
- `secret` - name of an existing `dpl secret` holding the user's password

The database name is the unit name itself.

### Commands

```bash
# Bring up a containerized DBMS. Prompts for any flag you omit.
dpl db init db-main --engine postgresql --version 18-alpine --secret db-server-password

# Create a database + user inside an existing db-server.
dpl db create app1 --db-server db-main --user app1 --secret app1-db-password

# Block until the db answers a ping (default 60s timeout).
dpl db wait app1 --timeout 60
```

`dpl db init` writes `{base_dir}/{name}/config.yaml`, renders a systemd
service file from the unit's templates with the root password inlined,
reloads systemd, and runs `systemctl enable --now`.

`dpl db create` executes the engine-specific SQL to create the user and the
database inside the running `db-server` via `podman exec`.

## Secrets

Runtime values that should not live in `config.yaml` (DB passwords, API keys,
signing secrets) are stored as encrypted files under `{base_dir}/.secrets/`
and inlined into generated artifacts at deploy time.

Storage layout:

- `{base_dir}/.secrets/master.key` - 32-byte AES-256-GCM master key, mode `0600`
- `{base_dir}/.secrets/<name>.bin` - encrypted secret, mode `0600`.
  Subdirectories are allowed (`db/prod-password.bin`)

Reference a secret from a unit config with the `${secret:<name>}` template:

```yaml
runtime:
  env:
    ALLOWED_HOSTS: app.example.com
    SECRET_KEY: ${secret:secret_name}
    DB_PASSWORD: ${secret:db/prod-password}
```

The tag value is the secret name (matching the `<name>` used with
`dpl secret create`). Plain strings and tagged secrets can be mixed freely in
the same `env` map. At deploy time `dpl` decrypts each tagged value and
inlines the plaintext into the generated `run.sh` (or `build-N.sh` for
build-layer envs). For `db-server` units, the root password is inlined into
the generated systemd unit at `dpl db init` time.

### CLI

```bash
dpl secret create db/prod-password                          # interactive prompt; Enter to generate
echo -n 'topsecret' | dpl secret create db/prod-password -  # read from stdin
dpl secret create db/prod-password ./payload.txt            # read from a file

dpl secret cat db/prod-password                          # print plaintext to stdout
dpl secret ls                                            # print secret names
dpl secret rm db/prod-password                           # delete
```

`secret create` takes an optional source:

- omitted - prompts for the value (terminal echo off). An empty input generates
  a random 32-character alphanumeric secret and prints it once
- `-` - reads stdin to EOF; a single trailing `\n` is stripped
- any other value - treated as a file path

The first `dpl secret create` creates `{base_dir}/.secrets/master.key`
automatically.

### Threat model

The encryption keeps plaintext out of `config.yaml`, source control, and
ad-hoc backups of just the unit directory. It does not protect against an
attacker with root on the deploy host: the master key sits next to the
encrypted files, and decrypted values are inlined into the generated `run.sh`,
systemd units, and the built image.

## Deploy

### Validate a unit

```bash
dpl unit check myapp
```

Parses `{unit_dir}/config.yaml` and resolves every reference: each
`${secret:...}` must exist, each unit named in `databases:` must be a `db`
unit, and each `${<db>:<key>}` must use a known export. Exits non-zero on the
first problem.

### Trigger a deploy

```bash
git archive --format=tar.gz HEAD | dpl unit deploy myapp
# or, from a file:
dpl unit deploy myapp ./build.tar.gz
```

`dpl unit deploy` acquires `{unit_dir}/.deploy.lock`, bumps the version,
renders artifacts, runs `podman build`, exports any configured files,
(re)installs the systemd service, runs the health check, and prints the
elapsed time. The command exits non-zero if any step fails.

### Inspect deploy state

```bash
dpl unit state myapp
```

Output:

```text
version: 3
status:  ready
active:  3
```

Possible `status` values:

- `idle`
- `building`
- `ready`
- `failed`

When `status` is `failed`, an `error:` line shows the failure message.

The full build log is at `{deploy_dir}/log/build.log`.

## Notes

- The unit config is read fresh on each `dpl` invocation.
- Deploy state is stored on disk in `{unit_dir}/state.yaml`.
- The busy lock at `{unit_dir}/.deploy.lock` is held via `flock(2)` for the
  duration of a deploy; the kernel releases it if `dpl` crashes.
- If the archive has a single top-level folder, `dpl` flattens it after
  extraction.

## Development

Build:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Run a deploy from the source tree:

```bash
cargo run -- --base /opt/dpl unit deploy myapp ./build.tar.gz
```
