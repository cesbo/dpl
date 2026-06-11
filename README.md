# dpl

`dpl` is a single-binary deploy CLI. It manages local units (apps, database
servers, databases, domains), keeps encrypted secrets on disk, and performs
one-shot deploys: extract a `.tar.gz`, render artifacts from MiniJinja
templates, run `podman build`, then hand the container off to the `dpl serve`
daemon to run.

Most `dpl` commands run to completion in the foreground. The exception is
`dpl serve`: a long-running daemon that runs the in-process timer scheduler
and supervises every unit's container - starting it, restarting it if it
exits, and stopping it on shutdown.

## Requirements

- podman

## Layout

`{base_dir}` is the base directory, default `/opt/dpl`, override with `DPL_BASE`.
Configs and secrets are grouped by kind; runtime state and logs live in a
per-unit directory:

- `{base_dir}/conf/{name}.yaml` - unit config (`type:` selects the variant)
- `{base_dir}/state/{name}/` - per-unit runtime state, locks, and logs:
  - `deploy.json` - deploy state (unit kind, active version, last status)
  - `deploy.lock` - advisory `flock(2)` held during a deploy
  - `timers.json` - timer run state
  - `timers.lock` - advisory `flock(2)` held while a timer runs
  - `.env-v{N}.json` - encrypted runtime env of the deployed version, written
    at deploy and decrypted by `dpl start`
  - `log/build.log` - last build: captured podman build/restore output, CRI
    `k8s-file` format; cleared at the start of each deploy
  - `log/runtime.log` - container stdout/stderr, captured by `dpl start`, CRI
    `k8s-file` format; rotated at 20mb
  - `log/timers.log` - all timer run output for the unit, CRI `k8s-file`
    format with the timer name as a label column; rotated at 20mb
- `{base_dir}/state/serve.pid` - PID file and single-instance lock for `dpl serve`
- `{base_dir}/secrets/` - encrypted secrets (`master.key` plus `{name}.json`)
- `{base_dir}/backup/` - database dumps written before a destructive drop

## CLI

```bash
DPL_BASE=/opt/dpl dpl <command> [args]
```

| Command | Purpose |
|---------|---------|
| `dpl check <name>`   | Validate config and reference graph |
| `dpl deploy <name> [path]` | Deploy a unit. App: `.tar.gz` (or `-`). Db: optional SQL dump to restore |
| `dpl undeploy <name>` | Remove a unit's active deployment from service |
| `dpl inspect <name>` | Print runtime state as JSON |
| `dpl serve` | Run the long-lived local serve process |
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
- `volumes` - persistent storage (survives redeploys). `path` must be an
  absolute container path and cannot be `/`
- `exports` - copies files from the built image into the shared nginx web
  volume so domain units can serve them. `source` must be an absolute path
  inside the image and cannot be `/`
- `timers` - periodic in-container scripts

### Static sites (no runtime)

Omit `runtime` to make a build-and-export unit. It runs its `builds` inside a
podman image and copies `exports` into the shared nginx web volume - there is
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

## HTTP server units

An `http-server` unit runs nginx and publishes HTTP on the host.

```yaml
type: http-server
image: docker.io/library/nginx:stable
http_port: 8080
https_port: false
```

- `http_port` - host HTTP port to publish to nginx's container port 80
  (default: `80`)
- `https_port` - host HTTPS port to publish to nginx's container port 443;
  omit it or set `false` to disable HTTPS publishing

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

Secrets are encrypted files under `{base_dir}/secrets/`, decrypted and
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
`dpl secret create` creates `{base_dir}/secrets/master.key` automatically.

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

A deploy acquires `deploy.lock`, bumps the version, renders artifacts, runs
`podman build`, exports configured files, marks the build for startup and
nudges `dpl serve` to bring the container up (SIGHUP), waits for the health
check, then marks it ready and prints the elapsed time. Non-zero exit on any
failure.

`dpl inspect` prints sections (`deploy`, `container` for app units). Each
field carries a `health` signal (`ok`, `warn`, `down`, `unknown`). Deploy
`status` is one of `idle`, `building`, `check`, `ready`, `failed`. On `failed`,
an `error` field carries the message.

## Serve

`dpl serve` is the long-running process for a host. It is intended to be run by
systemd and should have exactly one instance per `{base_dir}`; it holds
`state/serve.pid` as both a PID file and an exclusive `flock(2)` lock.

Deploys do not start containers directly. They mark the unit `check`, then
SIGHUP the running `dpl serve` process. Serve reconciles deploy state, starts
`check` runtime units once via child `dpl start <unit>` processes for startup
verification, restarts only `ready` units with backoff if they exit, and stops
supervised containers on shutdown.
Use `dpl undeploy <unit>` to remove a unit from service; it clears the active
deployment state so serve and timers stop treating the unit as desired.

Serve also runs the timer loop. Due app timers are executed through
`dpl timer <unit> <timer>`, which runs the timer script inside the running
container and writes output to `{base_dir}/state/{name}/log/timers.log`.

## Notes

- If the archive has a single top-level folder, `dpl` flattens it after extraction.

## Development

```bash
cargo build
cargo test
cargo clippy
DPL_BASE=/opt/dpl cargo run -- deploy myapp ./build.tar.gz
```

Never run `cargo fmt` - the project uses custom rustfmt rules. Rust edition
is `2024`.
