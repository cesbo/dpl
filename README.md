# dpl

`dpl` is a single-binary deploy CLI. It manages local units (apps, database
servers, databases, domains), keeps encrypted secrets on disk, and performs
one-shot deploys: extract a `.tar.gz`, render artifacts from MiniJinja
templates, run `podman build`, and (re)install the generated systemd service.

`dpl` is not a daemon. Every command runs to completion in the foreground.

## Requirements

- Linux with systemd
- podman

## Layout

- `{base_dir}` - base directory, default `/opt/dpl`, override with `--base`
- `{unit_dir}` - one per unit, at `{base_dir}/{name}/`
- `{base_dir}/.secrets/` - encrypted secrets

Each unit directory holds:

- `config.yaml` - unit config (`type:` selects the variant)
- `state.json` - deploy state (app units only)
- `.deploy.lock` - advisory `flock(2)` held during a deploy
- `log/build-{version}.log` - last build log

## CLI

```bash
dpl --base /opt/dpl <command> [args]
```

| Command | Purpose |
|---------|---------|
| `dpl check <name>`   | Validate config and reference graph |
| `dpl deploy <name> [path]` | Deploy a unit. App: `.tar.gz` (or `-`). Db: optional SQL dump to restore |
| `dpl inspect <name>` | Print runtime state as JSON |
| `dpl db wait\|console\|backup` | Database operations |
| `dpl secret create\|cat\|ls\|rm` | Manage encrypted secrets |

Append `--help` to any command for the full flag list.

## App unit

```yaml
type: app
image: node:22-alpine

builds:
  - files: ["package.json", "package-lock.json"]
    script: npm ci
  - files: ["*"]
    env:
      NODE_ENV: production
      NPM_TOKEN: ${secret:npm-token}
    script: npm run build

runtime:
  port: 3000
  env:
    DB_URL: ${app-db:url}
    API_TOKEN: ${secret:api-token}
  init: test -d /app/dist
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

- `builds` - ordered build layers. `files` lists archive paths copied into
  `/app` (`"*"` = all). `script` is optional
- `runtime` - the long-running service: `port` (listened on, also used for the
  readiness check and `${app:url}`/`${app:socket}` refs), `cmd`, optional
  `init` pre-start script, and `env`
- `volumes` - persistent storage (survives redeploys)
- `exports` - copies files from the built image into the shared nginx web
  volume so domain units can serve them
- `timers` - periodic in-container scripts

### Static sites (no runtime)

Omit `runtime` to make a build-and-export unit. It runs its `builds` inside a
podman image and copies `exports` into the shared nginx web volume — there is
no command, no port, no service, and no health check. Use it for static site
generators (e.g. `npm run build`) whose output a `domain` unit then serves via
`${<unit>:export}`. Without a runtime, `timers` are skipped and
`${<unit>:url}`/`${<unit>:socket}` are unavailable (only `${<unit>:export}`).

```yaml
type: app
image: node:22-alpine

builds:
  - files: ["*"]
    script: npm ci && npm run build

exports:
  - source: /app/dist
    path: /
```

### Env templates

`env` values may be plain strings or templates:

- `${secret:<name>}` - decrypted plaintext of a secret (see [Secrets](#secrets))
- `${<db-unit>:<key>}` - export from a `db` unit. Keys: `name`, `user`,
  `password`, `host`, `port`, `url`. The referenced db is added as a startup
  dependency automatically (no separate `databases:` list)
- `${<app-unit>:<key>}` - export from an `app` unit. Keys: `url`
  (`http://dpl-<name>:<port>`), `socket` (without scheme), `export` (static
  export path inside the nginx container)

References are validated by `dpl check`.

## Database units

- **`db-server`** - containerized DBMS (PostgreSQL, MariaDB, or MySQL). One
  engine instance, one root password
- **`db`** - a single database + login user inside an existing `db-server`

```yaml
# db-server
type: db-server
engine: postgresql      # or "mariadb" / "mysql"
version: 18-alpine
secret: db-server-password
```

```yaml
# db
type: db
server: db-main         # parent db-server unit name
user: app1              # defaults to the db unit name
secret: app1-db-password
```

The database name is the unit name.

### Commands

```bash
# Provision a DBMS / database. Pass a backup to restore in the same step.
dpl deploy db-main
dpl deploy app1
dpl deploy app1 app1.sql.gz
gunzip -c app1.sql.gz | dpl deploy app1 -

dpl db wait app1 --timeout 60         # block until reachable
dpl db console app1                   # interactive client (--root for superuser)
dpl db backup app1 app1.sql           # dump to file
dpl db backup app1 app1.sql.gz        # .gz → gzip
dpl db backup app1 - | gzip > out.gz  # stdout when path is omitted/`-`
```

`dpl db console` runs `psql` / `mariadb` / `mysql` inside the running
`db-server` via `podman exec -it`.

`dpl db backup` runs the engine's dump client as the db's login user. Client
messages go to stderr so stdout stays clean for piping.

`dpl deploy <db> <backup>` refuses if the database already exists (delete it
manually to re-import). The reader is gzip-detected by magic bytes, so `.sql`
and `.sql.gz` both work.

## Secrets

Secrets are encrypted files under `{base_dir}/.secrets/`, decrypted and
inlined into generated artifacts (`run.sh`, `build-N.sh`, db `Environment=`)
at deploy time.

```bash
dpl secret create db/prod-password                  # interactive; empty input generates a random 32-char value
echo -n 'topsecret' | dpl secret create foo -       # read stdin
dpl secret create foo ./payload.txt                 # read file
dpl secret cat foo                                  # print plaintext
dpl secret ls
dpl secret rm foo
```

Reference a secret from any `env` map with `${secret:<name>}`. The first
`dpl secret create` creates `{base_dir}/.secrets/master.key` automatically.

### Threat model

Encryption keeps plaintext out of `config.yaml`, source control, and ad-hoc
backups of just the unit directory. It does not protect against an attacker
with root on the deploy host: the master key sits next to the encrypted
files, and decrypted values end up inlined into the generated artifacts.

## Deploy

```bash
dpl check myapp                                     # validate config + references
git archive --format=tar.gz HEAD | dpl deploy myapp -
dpl deploy myapp ./build.tar.gz
dpl inspect myapp                                   # JSON status
```

A deploy acquires `.deploy.lock`, bumps the version, renders artifacts, runs
`podman build`, exports configured files, (re)installs the systemd service,
runs the health check, and prints the elapsed time. Non-zero exit on any
failure.

`dpl inspect` prints sections (`deploy`, `container` for app units). Each
field carries a `health` signal (`ok`, `warn`, `down`, `unknown`). Deploy
`status` is one of `idle`, `building`, `ready`, `failed`. On `failed`, an
`error` field carries the message.

## Notes

- If the archive has a single top-level folder, `dpl` flattens it after extraction.

## Development

```bash
cargo build
cargo test
cargo clippy
cargo run -- --base /opt/dpl deploy myapp ./build.tar.gz
```

Never run `cargo fmt` - the project uses custom rustfmt rules. Rust edition
is `2024`.
