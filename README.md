# dpl

`dpl` is a single-binary deploy CLI. It manages local units (apps, database
servers, databases, HTTP servers, domains, Cloudflare tunnels), keeps encrypted
secrets on disk, and performs one-shot deploys: extract a `.tar.gz`, render
artifacts from MiniJinja templates, run `podman build`, then hand the container
off to the `dpl serve` daemon to run.

Most `dpl` commands run to completion in the foreground. The exception is
`dpl serve`: a long-running daemon that runs the in-process timer scheduler
and supervises every unit's container - starting it and restarting it if it
exits. Liveness is read from `podman` (a `podman events` watcher plus a poll
backstop), not from child processes, so `dpl serve` can be restarted or
upgraded without stopping the containers it supervises. Global teardown is the
explicit `dpl down`.

## Requirements

- podman 4.8+

## Layout

`<base>` is the base directory, default `/opt/dpl`, override with `DPL_BASE`.
Configs and secrets are grouped by kind; runtime state and logs live in a
per-unit directory:

- `<base>/conf/{name}.yaml` - unit config (`type:` selects the variant)
- `<base>/state/{name}/` - per-unit runtime state, locks, and logs:
  - `deploy.json` - deploy state (unit kind, active version, last status)
  - `deploy.lock` - advisory `flock(2)` held for the whole lifetime of the
    foreground `dpl deploy` process, and released only when that process exits
    (however it exits). While held it contains the holder's pid, which is what
    `unit busy` and `dpl inspect` report. Restarting `dpl serve` does not
    release it - serve never holds it. Do not delete the file to clear a stuck
    deploy; kill the pid it names
  - `timers.json` - timer run state
  - `timers.lock` - advisory `flock(2)` held while a timer runs
  - `.env-v{N}.json` - encrypted runtime env, one per retained version,
    written at deploy and decrypted by `dpl start`
  - `conf/` (http-server only) - nginx `conf.d` source, bind-mounted read-only:
    `00-dpl.conf` plus each dependent domain's `<domain>.conf`
  - `www/` (http-server only) - static-export root, bind-mounted read-only at
    `/var/www`; holds each served app's `<app>_<version>/` tree, copied in at
    domain deploy
  - `log/build.log` - last build: captured podman build/restore output, CRI
    `k8s-file` format; cleared at the start of each deploy
  - `log/runtime.log` - container stdout/stderr, captured by `dpl start`, CRI
    `k8s-file` format; rotated at 20mb. For an app it is a symlink to
    `log/runtime-v{N}.log` of the version in service (one file per retained
    version, dropped with the version); other kinds write it directly. For
    `http-server`, only stderr is written here.
  - `log/access.log` (http-server only) - nginx access log captured from stdout
    as JSONL without a CRI prefix; non-object stdout lines are dropped; rotated
    at 200mb by default
  - `log/timers.log` - all timer run output for the unit, CRI `k8s-file`
    format with the timer name as a label column; rotated at 20mb
- `<base>/state/serve.pid` - PID file and single-instance lock for `dpl serve`
- `<base>/secrets/` - encrypted secrets (`master.key` plus `{name}.json`)
- `<base>/backup/` - database dumps written before a destructive drop

Containers join the podman network `dpl`. An app's container is named
`dpl--<name>--<version>` and carries the DNS alias `dpl--<name>`, which stays
the same across versions and is what `${<app>:url}` resolves to. Every other
unit kind (`db-server`, `http-server`, `cloudflare-tunnel`) runs as
`dpl--<name>`. An app container still named `dpl--<name>` by an older dpl is
adopted as the active version until its next deploy.

## CLI

```bash
DPL_BASE=/opt/dpl dpl <command> [args]
```

| Command | Purpose |
|---------|---------|
| `dpl check <name>`   | Validate config and reference graph |
| `dpl deploy <name> [path]` | Deploy a unit. App: `.tar.gz` (or `-`). Db: optional SQL dump to restore |
| `dpl rollback <name> [version]` | Roll an app back to a retained earlier version (default: the previous one) |
| `dpl undeploy <name>` | Remove a unit's active deployment from service |
| `dpl status` | One line per unit: kind, version, deploy status, container state (`RUN`) |
| `dpl inspect <name>` | Print deploy and runtime state |
| `dpl upgrade` | Upgrade dpl to the latest release (needs root) |
| `dpl serve` | Run the long-lived local serve process |
| `dpl down` | Stop serve and tear down all supervised containers (keeps deploy state) |
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
  `/app` (`"*"` = all). `script` is optional. Unchanged layers are reused on
  the next deploy; a changed file, script, or build env rebuilds that layer
  and everything after it
- `runtime` - the long-running service: `cmd`, optional `port` (listened on,
  also used for the readiness check and `${app:url}`/`${app:socket}` refs),
  optional `init` pre-start script, and `env`. The process must bind a wildcard
  address (`0.0.0.0` or `::`) on `port`: nginx reaches the container over the
  `dpl` podman network, so a loopback-only bind fails the readiness check even
  though the app works inside the container
- `volumes` - persistent storage (survives redeploys). `path` must be an
  absolute container path and cannot be `/`
- `exports` - copies files from the built image into the http-server's
  on-host `www/` directory, which nginx bind-mounts read-only at `/var/www`.
  Domain units can serve them via `${<unit>:export}`. `source` must be an
  absolute path inside the image and cannot be `/`
- `timers` - periodic in-container scripts
- `strategy` - how a new version takes over: `replace` (default) stops the
  running version, then starts the new one; `parallel` starts the new one
  next to it and switches once it is ready (see
  [Parallel deploys](#parallel-deploys))
- `expect` - optional check that the archive is the intended project, run after
  extract and before the image build. `path` is an exact relative path in the
  extracted tree (not a glob; a single wrapping folder is already flattened).
  `contains` is a required literal substring, matched as bytes, not as a regex
  or JSON. Point it at a file the project already has (`package.json`,
  `Cargo.toml`, `go.mod`). A missing file, a directory, a symlink, a file over
  1 MiB, or a missing needle fails the deploy and leaves the running version
  untouched. Rollback does not re-check. Include enough context that the needle
  is not a prefix of a different name (`"name": "foo-app"`, not `foo`)

```yaml
expect:
  path: package.json
  contains: '"name": "foo-app"'
```

### Static sites (no runtime)

Omit `runtime` to make a build-and-export unit. It runs its `builds` inside a
podman image and copies `exports` into the http-server's on-host `www/`
directory - there is no command, no port, no service, and no health check. Use
it for static site generators (e.g. `npm run build`) whose output a `domain`
unit then serves via `${<unit>:export}`. Without a runtime, `timers` are
skipped and `${<unit>:url}`/`${<unit>:socket}` are unavailable (only
`${<unit>:export}`).

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

### Workers (no port)

Omit `runtime.port` for a service that listens on nothing - a queue consumer, a
poller, a cron host. It is a normal supervised container (`cmd`, `init`, `env`,
`volumes` and `timers` all apply), with two differences: the image gets no
`EXPOSE`, and readiness is "the container started and stayed up" instead of a
port probe - three consecutive `running` observations 800ms apart, under the
same 90s ceiling. An earlier exit fails the deploy with the container's exit
code and how long it ran. `${<unit>:url}` and `${<unit>:socket}` are
unavailable, since there is nothing to connect to.

```yaml
type: app
image: python:3.12-alpine

builds:
  - files: ["*"]
    script: pip install -r requirements.txt

runtime:
  env:
    DB_URL: ${app-db:url}
  cmd: python worker.py
```

### Env templates

`env` values may be plain strings or templates:

- `${secret:<name>}` - decrypted plaintext of a secret (see [Secrets](#secrets))
- `${<db-unit>:<key>}` - export from a `db` unit. Keys: `name`, `user`,
  `password`, `host`, `port`, `url`. The referenced db is added as a startup
  dependency automatically (no separate `databases:` list)
- `${<app-unit>:<key>}` - export from an `app` unit. Keys: `url`
  (`http://dpl--<name>:<port>`, the container's stable DNS alias), `socket`
  (without scheme), `export` (static export path inside the nginx container).
  Env refs always resolve to the alias; a domain route to a
  `strategy: parallel` app is instead pinned to the version in service
  (`http://dpl--<name>--<version>:<port>`) and re-rendered at the switch

References are validated by `dpl check`.

## Domain units

A `domain` unit is an nginx `server` block written into an `http-server`
unit's `conf/` and reloaded there. It is not a container.

```yaml
type: domain
server: web             # http-server unit that serves it
hosts:
  - example.com
routes:
  - location: /
    kind: reverse_proxy
    target: ${myapp:url}
```

Optional `proxy` names the trusted proxy in front of nginx. Its addresses are
allowlisted with `set_real_ip_from` and the client IP is taken from the proxy's
header (`real_ip_header`):

```yaml
proxy:
  type: cloudflare
```

| `type` | Trusted addresses | Client IP header |
|--------|-------------------|------------------|
| `cloudflare` | Cloudflare's public IP list, fetched at every deploy | `CF-Connecting-IP` |
| `fastly` | Fastly's public IP list, fetched at every deploy | `Fastly-Client-IP` |
| `custom` | `proxies:` list of CIDRs, inline | `header:` |
| `cloudflare-tunnel` | the `dpl` podman network subnet(s) | `CF-Connecting-IP` |

Use `cloudflare-tunnel` when the domain is served through a
[`cloudflare-tunnel`](#cloudflare-tunnel) unit: the peer nginx sees is the
`cloudflared` container on the `dpl` network, not Cloudflare's public ranges.
The allowlist is the whole container subnet, so the `http-server` must publish
nothing on the host (`listen: false`) - the domain deploy refuses otherwise. A
published port would deliver host-side traffic from inside that subnet
(rootless podman proxies every connection; rootful masquerades loopback),
letting such a client spoof `CF-Connecting-IP`.
A list fetch failure aborts the deploy rather than rendering an empty
allowlist.

With `proxy` set, the `X-Forwarded-Proto` passed to `reverse_proxy` upstreams
is taken from the proxy's `X-Forwarded-Proto` header, but only on requests
that arrived from an allowlisted address and carried the client IP header
(`http`/`https` only, anything else falls back to nginx's own `$scheme`).
Without `proxy`, or from any other peer, it is always nginx's `$scheme`.

## HTTP server units

An `http-server` unit runs nginx and publishes HTTP on the host. nginx listens
on container port 80 only and does not terminate TLS. For security the server
must sit behind a proxy (Cloudflare, Fastly, or another); providing HTTPS is
the proxy's job. An `https_port` field is rejected.

```yaml
type: http-server
image: docker.io/library/nginx:stable
listen: "[2001:db8::1]:80"
access_log:
  max_size_mb: 200
  max_files: 1
```

- `listen` - host address and port to publish to nginx's container port 80.
  A quoted socket address: `"[2001:db8::1]:80"` or `"192.0.2.1:8080"`. The
  address is one assigned to the host, not an interface name, so a port opened
  for one interface's IPv6 is not reachable on the others. Omit it to publish
  port 80 on every interface. Set `false` (or `null`) to publish nothing on
  the host: nginx stays reachable on the `dpl` network as
  `http://dpl--<name>:80`. A link-local address with a zone
  (`[fe80::1%eth0]:80`) is rejected: podman binds an address, not an interface
- `access_log.max_size_mb` - access log rotation threshold in MiB
  (default: `200`)
- `access_log.max_files` - number of archived access log files to keep
  (`access.log.1`, `access.log.2`, ...; default: `1`)

## Cloudflare Tunnel

A `cloudflare-tunnel` unit runs Cloudflare's `cloudflared` connector as a
supervised container on the `dpl` network. Use it when the host has no public
address, or to hide the origin: the connector dials out to Cloudflare, TLS
terminates at Cloudflare's edge, and the host publishes no inbound ports.

```yaml
type: cloudflare-tunnel
secret: cf-tunnel-token
image: docker.io/cloudflare/cloudflared:latest   # default
```

- `secret` - secret holding the tunnel token from the Zero Trust dashboard
- `image` - connector image (default: `docker.io/cloudflare/cloudflared:latest`)

Operator flow:

1. In the Zero Trust dashboard create a tunnel (Networks -> Tunnels ->
   Cloudflared connector) and copy its token.
2. `dpl secret create cf-tunnel-token -` with the token on stdin.
3. Write `conf/<name>.yaml` as above.
4. `dpl deploy <name>`. It pulls the image if missing, snapshots the token into
   the version's encrypted runtime env, hands the container to `dpl serve` and
   waits for readiness. On success it prints the service URL format to enter
   in the dashboard: `http://dpl--<http-server>:80`.
5. In the dashboard add a Public Hostname per site pointing at that URL.
   Cloudflare creates the DNS record.

Notes:

- The token reaches the container as env `TUNNEL_TOKEN` from the podman process
  env, never on argv. It is stored in `state/<name>/.env-v{N}.json`.
- `cloudflared` output lands in `state/<name>/log/runtime.log`. Readiness only
  means the connector process stayed up, not that it registered with
  Cloudflare: a token it rejects at once fails the deploy; one revoked later
  shows up in the log, and serve restarts the connector with backoff.
- Companion settings: `listen: false` on the `http-server` unit so the host
  publishes nothing, and `proxy: {type: cloudflare-tunnel}`
  on each `domain` served through the tunnel so nginx trusts peers on the
  `dpl` network (the connector) and logs the real client IP.

## Database units

- **`db-server`** - containerized DBMS (PostgreSQL, MariaDB, or MySQL). One
  engine instance, one root password. Physical data lives in a Podman named
  volume with the scoped unit name.
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
dpl db console db-main                # superuser client on the db-server
dpl db backup app1 app1.sql           # dump to file
dpl db backup app1 app1.sql.gz        # .gz → gzip
dpl db backup app1 - | gzip > out.gz  # stdout when path is omitted/`-`
```

`dpl db console` runs `psql` / `mariadb` / `mysql` inside the running
`db-server` via `podman exec -it`. Given a `db-server` unit, it connects as the
engine superuser without selecting a database.

`dpl db backup` runs the engine's dump client as the db's login user. Client
messages go to stderr so stdout stays clean for piping.

`dpl deploy <db> <backup>` refuses if the database already exists (delete it
manually to re-import). The reader is gzip-detected by magic bytes, so `.sql`
and `.sql.gz` both work.

## Secrets

Secrets are encrypted files under `<base>/secrets/`, decrypted and
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
`dpl secret create` creates `<base>/secrets/master.key` automatically.

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
dpl status                                          # all units at a glance
dpl inspect myapp                                   # deploy + runtime state
```

A deploy acquires `deploy.lock`, bumps the version, renders artifacts, runs
`podman build`, exports configured files, marks the build for startup and
nudges `dpl serve` to bring the container up (SIGHUP), waits for the health
check, then marks it ready and prints the elapsed time. Non-zero exit on any
failure.

### Rollback

```bash
dpl rollback myapp                                  # back to the previous version
dpl rollback myapp 6                                # back to retained version 6
```

An app keeps the image and encrypted env of its previous good version (one by
default). A rollback retags that image as a new version, re-encrypts the
retained env under it, then runs the normal hand-off (per `strategy`) with
the usual readiness check. Versions only go forward; `dpl inspect` shows the
new one as `rollback of N`. `dpl undeploy` removes the active version only;
older retained images stay until the next successful deploy prunes them.

Limits: app units only. The env is the one the source version ran with - a
secret rotated since is not picked up. The yaml is not versioned: port,
volumes, timers and the readiness criterion come from the current file, while
the command is baked into the image.

### Parallel deploys

With `strategy: parallel` the new version is started next to the running one
instead of after it. The deploy builds as usual, then hands the new container
to `dpl serve` without the `dpl--<name>` DNS alias and waits for the usual
readiness check on it, while the old version keeps serving. Once ready, the
alias is attached to the new container (a podman network disconnect and
reconnect, the process is not restarted), the new version becomes active,
dependent domains are re-rendered against its container and nginx reloaded,
and only then is the old version stopped and the usual pruning done.
`dpl inspect` shows `Candidate: N (checking)` during the window, and the
candidate writes `log/runtime-v<N>.log` while `runtime.log` still points at
the old version until the switch.

A candidate that fails readiness (or whose alias cannot be attached) is
stopped and the old version is untouched: still active, still supervised.
`dpl inspect` shows the failure; the next deploy or rollback discards the
candidate's image and env. A domain that fails to re-render after the switch
reverts it: the old version is active again, the domains are re-rendered
against it, then the candidate is stopped. `dpl rollback` follows the strategy
too.

Limits: app units with a `runtime` only (a static unit always behaves as
`replace`). Both versions run until the old one is stopped, so named `volumes`
are mounted by both, timers run in the active one, and two versions need twice
the resources for up to the 90s readiness budget. Env refs `${<app>:url}` from
other apps follow the alias, so both versions answer them between the alias
attach and the stop of the old one; domain routes are pinned to the version in
service and switch exactly at the nginx reload (see
[Env templates](#env-templates)). An app whose container is still named
`dpl--<name>` by an older dpl must be deployed once by this dpl before it is
switched to `parallel`: the pin names `dpl--<name>--<version>`, which does not
exist yet.

### Readiness

Every deploy that hands a container to `dpl serve` then waits for it, and the
wait is bounded: **90s total, with a 5s cap on each individual probe**. A probe
that does not answer costs one attempt, not the deploy.

| Unit | Ready when | Probe |
|------|-----------|-------|
| `app` with `runtime.port` | a `LISTEN` socket on that port bound to `0.0.0.0` or `::` | `podman exec <ctr> sh -c 'cat /proc/net/tcp /proc/net/tcp6'` every 800ms |
| `app` without `runtime.port` | 3 consecutive `running` observations | `podman container inspect` every 800ms |
| `app` with no `runtime` | immediately (nothing runs) | none |
| `cloudflare-tunnel` | 3 consecutive `running` observations | `podman container inspect` every 800ms |
| `http-server` | a `LISTEN` socket on container port 80 (regardless of `listen`) | same as a port app |
| `db-server` | the engine accepts the root login | `podman exec` + `SELECT 1` every 800ms, 10s per call, 60s total |
| `db` | the same ping against that database | `dpl db wait --timeout`, default 60s |

A loopback-only listener is deliberately **not** accepted: nginx reaches the
container over the `dpl` network. The port probe needs a shell and `/proc` in
the image.

While waiting, the deploy line states the container, the criterion and the
budget, and restates it with elapsed time (plus the last probe error, if any)
every 10s. `dpl inspect` shows the same criterion for a unit sitting in
`check`.

Failure messages and what they mean:

| Message | Cause |
|---------|-------|
| `container exited with code N (ran …)` | the command died; see `state/{name}/log/runtime.log` |
| `container is running but not listening on any port` | the command never bound the port |
| `port N not reachable; found …` | bound, but on loopback or another port - the sockets found are listed |
| `the container was never created` | serve never started it: check `dpl serve` is running, and the unit's `start_after` db-servers |
| `readiness probe … failed: …` | podman itself did not answer - the host, not the app, is the problem |

`dpl inspect` prints aligned fields: the unit kind, the active version, the
latest deploy when it is not `ready`, then a per-kind runtime block and the
timers. For an app the kind carries its derived form - `app (service)` when
`runtime.port` is set, `app (worker)` for a runtime without one, `app (static)`
with no runtime at all. Deploy status is one of `idle`, `building`, `check`,
`ready`, `failed`; on `failed` the failing stage and message are printed with a
pointer to the log.

## Serve

`dpl serve` is the long-running process for a host. It is intended to be run by
systemd and should have exactly one instance per `<base>`; it holds
`state/serve.pid` as both a PID file and an exclusive `flock(2)` lock.

Deploys do not start containers directly. They mark the unit `check`, then
SIGHUP the running `dpl serve` process. Serve reconciles deploy state, starts
`check` runtime units once via child `dpl start <unit>` processes for startup
verification, restarts the version in service with backoff if it exits (also
while a parallel candidate is checking, or after one failed), and stops
supervised containers on shutdown.
Use `dpl undeploy <unit>` to remove a unit from service; it clears the active
deployment state so serve and timers stop treating the unit as desired.

Serve also runs the timer loop. Due app timers are executed through
`dpl timer <unit> <timer>`, which runs the timer script inside the running
container and writes output to `<base>/state/{name}/log/timers.log`.

## Troubleshooting

**A deploy sits on `waiting for app '<name>' …`.** The line names the container,
the readiness criterion and the budget, and refreshes every 10s. It cannot run
past the budget (90s; a db-server 60s), so a wait that outlives it means the
process is blocked elsewhere - almost always podman on the host.

1. `dpl inspect <name>` - for a unit in `check` it prints `Waiting for` (the
   criterion), `Startup budget`, whether a deploy still holds `deploy.lock` and
   its pid, and the runtime log path. `stalled - no deploy holds deploy.lock`
   means the deploy that handed the unit off is gone.
2. `podman ps`, then `podman logs dpl--<name>--<version>` (an app; other kinds
   are `dpl--<name>`), or read `<base>/state/{name}/log/runtime.log` directly.
3. `ps -ef | grep 'podman exec'` - a wedged host-side podman is the classic
   cause. dpl now kills its own probe after 5s and reports
   `readiness probe … failed`, so a wedge shows up as that message rather than
   as an unexplained wait.
4. Ctrl-C (or `kill <pid>`) aborts a deploy. Aborting releases `deploy.lock`
   immediately. The unit is left in `check` or `building`; the next
   `dpl deploy` or `dpl rollback` discards that version.

**`unit busy`.** Another process holds the unit's `deploy.lock`; the message
names its pid. A killed deploy no longer wedges a unit: the next
`DeployState::acquire` finds the interrupted `building` status, records it as
failed and continues.

## Notes

- If the archive has a single top-level folder, `dpl` flattens it after extraction.
- Every control-plane podman call is wall-clock bounded (15s for metadata calls,
  120s for `stop`/`rm`, 30 min for `pull`/`cp`). `podman build` and timer
  scripts are the deliberate exceptions: both legitimately run for a long time
  and both stream into a log.

## Development

```bash
cargo build
cargo test
cargo clippy
DPL_BASE=/opt/dpl cargo run -- deploy myapp ./build.tar.gz
```

Never run `cargo fmt` - the project uses custom rustfmt rules. Rust edition
is `2024`.
