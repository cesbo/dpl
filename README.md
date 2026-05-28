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

### Directory Structure

- `{base_dir}` - base directory for all `dpl` files (default: `/opt/dpl`), set via `--base`
- `{unit_dir}` - unit directory: `{base_dir}/{unit_name}/`

## CLI

`dpl` exposes two top-level commands plus three subcommand groups. The
`--base` flag is global and defaults to `/opt/dpl`:

```bash
dpl --base /opt/dpl <command> [args]
```

| Command | Purpose |
|---------|---------|
| `dpl check <name>`   | Validate a unit's config and reference graph |
| `dpl inspect <name>` | Show a unit's runtime state as JSON |
| `dpl unit`           | Deploy units (`deploy`) |
| `dpl db`             | Bring up DB-server units, create databases, back them up (`init`, `create`, `wait`, `console`, `backup`, `restore`) |
| `dpl secret`         | Manage encrypted runtime secrets (`create`, `cat`, `rm`, `ls`) |

Run any command with `--help` for the full flag list.

## Unit Config

Each unit lives in its own directory `{unit_dir}` and has:

- `config.yaml` - unit config. The `type` field selects the variant: `app`,
  `db-server`, `db`, or `domain`
- `state.json` - serialized deploy state (`active_version` and `latest_build`).
  Only app deploys update this file
- `.deploy.lock` - advisory `flock(2)` held for the duration of a deploy so
  two `dpl deploy` invocations against the same unit can't race

## App Unit

An app unit represents a containerized application. `dpl deploy` accepts a
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

builds:
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
- `volumes` - persistent storage mounted into the container. Data in volumes
  survives redeploys
- `exports` - copies files from the built image into the shared nginx web
  volume so domain units can serve them
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
  `name`, `user`, `password`, `host`, `port`, `url`. The `db` unit must already
  exist. Every `db` unit referenced this way is automatically treated as a
  startup dependency: a `db wait` gate is added to the generated service and
  its reference chain is validated recursively (no separate `databases:` list)
- `${<app-unit>:<key>}` - export of a referenced `app` unit. Allowed keys:
  `url` (`http://dpl-<name>:<port>`, for `proxy_pass`), `socket`
  (`dpl-<name>:<port>` without scheme, for `uwsgi_pass`/`fastcgi_pass`), and
  `export` (the app's static export path inside the nginx container)

References are validated by `dpl check`.

## Database Units

Two unit types make up the database story:

- **`db-server`** - a containerized DBMS (PostgreSQL, MariaDB, or MySQL) managed as a
  systemd unit. Each `db-server` runs one engine instance and holds a single
  root password
- **`db`** - a single database + login user inside an existing `db-server`. A
  `db` unit is referenced by app units and exposes connection values
  (`name`, `user`, `password`, `host`, `port`, `url`)

### `db-server` config

```yaml
type: db-server
engine: postgresql      # or "mariadb", "mysql"
version: 18-alpine
secret: db-server-password
```

- `engine` - `postgresql`, `mariadb`, or `mysql`
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

Write the unit's `config.yaml` under `{base_dir}/{name}/` (see the schemas
above), then provision it with `dpl deploy`. `dpl db restore` is also a
valid from-scratch entry point — it brings the db-server up and creates the
database before streaming the dump.

```bash
# Bring up a containerized DBMS from {base_dir}/db-main/config.yaml.
dpl deploy db-main

# Create the database + login user inside the running db-server.
dpl deploy app1

# Block until the db answers a ping (default 60s timeout).
dpl db wait app1 --timeout 60

# Open an interactive SQL console as the db's login user (use --root for the superuser).
dpl db console app1

# Dump a database to a SQL file, or stdout when the path is omitted.
dpl db backup app1 app1.sql
dpl db backup app1 - | gzip > app1.sql.gz

# Replay a SQL dump into the database from a file, or stdin. Creates the db
# and starts the server if needed, so this works on a fresh host too.
dpl db restore app1 app1.sql
gunzip -c app1.sql.gz | dpl db restore app1
```

`dpl db console` opens the engine's interactive client (`psql`, `mariadb`, or
`mysql`) inside the running `db-server` via `podman exec -it`, connected to the
database as its login user (or the superuser with `--root`).

`dpl db backup` and `dpl db restore` run the engine's dump/restore client
(`pg_dump`/`psql`, `mariadb-dump`/`mariadb`, or `mysqldump`/`mysql`) inside the running `db-server`
via `podman exec`. They connect as the `db` unit's own login user, not the
superuser, and stream plain SQL with no compression. The `path` argument
defaults to `-`, which means stdout for `backup` and stdin for `restore`, so
you can pipe through `gzip` or any other tool. `restore` runs the same setup
flow as `dpl deploy <db>` first — db-server up, database created if
missing — and then replays the dump on top of whatever is already there
(no DROP/CREATE).

Progress and the client's own messages (for example PostgreSQL `NOTICE` lines
or restore errors) go to stderr, so stdout stays clean for piping. On failure
those messages are already on screen and the final error only adds the exit
status.

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
the generated systemd unit at `dpl deploy` time.

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
dpl check myapp
```

Parses `{unit_dir}/config.yaml` and resolves every reference: each
`${secret:...}` must exist and each `${<db>:<key>}` must use a known export.
Referenced `db` units are validated recursively through their db-server and
secret. Exits non-zero on the first problem.

### Trigger a deploy

```bash
git archive --format=tar.gz HEAD | dpl deploy myapp
# or, from a file:
dpl deploy myapp ./build.tar.gz
```

`dpl deploy` acquires `{unit_dir}/.deploy.lock`, bumps the version,
renders artifacts, runs `podman build`, exports any configured files,
(re)installs the systemd service, runs the health check, and prints the
elapsed time. The command exits non-zero if any step fails.

### Inspect deploy state

```bash
dpl inspect myapp
```

Prints a structured JSON report grouped into sections. App units report a
`deploy` section (on-disk deploy state) and a `container` section (live
`podman container inspect` data). Each field carries a `health` signal
(`ok`, `warn`, `down`, or `unknown`) for machine consumers:

```json
{
  "name": "myapp",
  "kind": "app",
  "sections": [
    {
      "title": "deploy",
      "fields": [
        { "label": "version", "value": "3", "health": "ok" },
        { "label": "status", "value": "ready", "health": "ok" },
        { "label": "active", "value": "3", "health": "unknown" }
      ]
    },
    {
      "title": "container",
      "fields": [
        { "label": "state", "value": "running", "health": "ok" },
        { "label": "started", "value": "2026-05-25T08:00:00Z", "health": "unknown" },
        { "label": "restarts", "value": "0", "health": "unknown" },
        { "label": "image", "value": "localhost/myapp:3", "health": "unknown" }
      ]
    }
  ]
}
```

Possible deploy `status` values:

- `idle`
- `building`
- `ready`
- `failed`

When `status` is `failed`, an `error` field in the `deploy` section carries
the failure message. Non-app units report only `name` and `kind`.

The full build log is at `{unit_dir}/log/build-{version}.log`.

## Notes

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
cargo run -- --base /opt/dpl deploy myapp ./build.tar.gz
```
