# dpl

`dpl` is a deploy server.

## Requirements

- linux - recommended Fedora 42
- systemd
- podman
- nginx

### Directory Structure

- `{base_dir}` - base directory for all `dpl` files (default: `/opt/dpl`), set via `base` in config
- `{unit_dir}` - unit directory: `{base_dir}/{unit_name}/`
- `{deploy_dir}` - deploy directory for one version: `{unit_dir}/deploy_{version}/`

## Initial Setup

Run the interactive wizard as root to prepare a fresh host:

```bash
sudo dpl init
```

The wizard asks for the base directory, bind address, port, and one access
token (name + value), then:

- creates main config in `{base_dir}/config.yaml` and auth config in `{base_dir}/auth.yaml`
- writes `/etc/systemd/system/dpl.service`
- optionally runs `systemctl enable --now dpl`

## Main Config

The server reads its config from a YAML file. The path is set with `--config` / `-c` (default: `/opt/dpl/config.yaml`).

```bash
dpl -c /opt/dpl/config.yaml
```

Example:

```yaml
base: /opt/dpl
server:
  addr: 0.0.0.0
  port: 3000
```

Fields:

- `base` - base directory for all `dpl` files (default: `/opt/dpl`). Referred to as `{base_dir}`
- `server.addr` - bind address (default: `0.0.0.0`)
- `server.port` - port (default: `3000`)


## Auth Config

All `/deploy` routes need a Bearer token. Tokens are stored in:

```text
{base_dir}/auth.yaml
```

Example:

```yaml
keys:
  - name: deploy-key
    token: secret-token
    apps: ["myapp"]
    disabled: false
```

Fields:

- `name` - must be unique inside `auth.yaml`
- `token` - plain-text Bearer token
- `apps` - list of allowed app names. Use `"*"` to allow all apps
- `disabled` - if `true`, the key is rejected

## Unit Config

Each unit uses `{unit_dir}`. Common files in every unit directory:

- `config.yaml` - unit config. Currently only `type: app` is implemented
- `state.yaml` - serialized deploy state stored on disk (`active_version` and `latest_build`)

## App Unit

An app unit represents a containerized application. When you deploy an app, `dpl` receives a `.tar.gz` archive with your source code, generates a `containerfile` from your config, builds a podman image, and optionally exports static files from the built image.

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
      NPM_TOKEN: !secret npm-token
    script: |
      npm run build

runtime:
  env:
    NODE_ENV: production
    DB_PASS: !secret db/prod-password
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
- `port` - port your application listens on
- `build` - list of build layers (see below)
- `runtime` - runtime configuration (see below)
- `volumes` - persistent storage mounted into the container. Data in volumes survives redeploys
- `exports` - copies files from the built image into `{deploy_dir}/exports/`
- `timers` - periodic scripts to run in the container

Runtime fields:

- `env` - environment variables. Values may be plain strings or `!secret <name>` to pull a value from an encrypted secret. See [Secrets](#secrets)
- `init` - optional shell script that runs before `cmd`
- `cmd` - main start command

Build layer fields:

- `files` - paths copied from the extracted archive into `/app`. Use `"*"` to copy all files
- `env` - build-time environment variables for the layer script. Supports `!secret <name>` the same way as `runtime.env`
- `script` - shell script for the layer. If missing, the layer only copies files

### Files

App-specific files in `{unit_dir}`:

- `{unit_dir}/port.txt` - persisted host port for the unit
- `{unit_dir}/deploy_{version}/` - versioned deploy directory. Referred to as `{deploy_dir}`

`{deploy_dir}` layout:

- `{deploy_dir}/app.tar.gz` - uploaded archive
- `{deploy_dir}/app/` - extracted archive
- `{deploy_dir}/artifacts/` - generated deploy files such as `containerfile`, `run.sh`, `build-N.sh`, and systemd service files
- `{deploy_dir}/exports/` - static files exported from the built image
- `{deploy_dir}/log/build.log` - build log with podman build output

## Secrets

Runtime values that should not live in `config.yaml` (DB passwords, API keys, signing secrets) are stored as encrypted files under `{base_dir}/.secrets/` and exposed to the container as environment variables.

Storage layout:

- `{base_dir}/.secrets/master.key` - 32-byte AES-256-GCM master key, mode `0600`
- `{base_dir}/.secrets/<name>.bin` - encrypted secret, mode `0600`. Subdirectories are allowed (`db/prod-password.bin`)

Reference a secret from a unit config with the `!secret` YAML tag inside any `env` map (build layer or runtime):

```yaml
runtime:
  env:
    ALLOWED_HOSTS: app.example.com
    SECRET_KEY: !secret secret_name
    DB_PASS: !secret db/prod-password
```

The tag value is the secret name (matching the `<name>` used with `dpl secret set`). Plain strings and tagged secrets can be mixed freely in the same `env` map. At deploy time `dpl` decrypts each tagged value and inlines the plaintext into the generated `run.sh` (or `build-N.sh` for build-layer envs).

### CLI

```bash
dpl secret set db/prod-password                          # interactive prompt; Enter to generate
echo -n 'topsecret' | dpl secret set db/prod-password -  # read from stdin
dpl secret set db/prod-password ./payload.txt            # read from a file

dpl secret cat db/prod-password                          # print plaintext to stdout
dpl secret list                                          # print secret names
dpl secret rm db/prod-password                           # delete
```

`secret set` takes an optional source:

- omitted - prompts for the value (terminal echo off). An empty input generates a random 32-character alphanumeric secret and prints it once
- `-` - reads stdin to EOF; a single trailing `\n` is stripped
- any other value - treated as a file path

The first `dpl secret set` creates `{base_dir}/secrets.key` automatically.

### Backup

`{base_dir}/secrets.key` and `{base_dir}/secrets/` live together inside `{base_dir}`. `rsync -a {base_dir}/ host2:{base_dir}/` is sufficient to restore secrets on a new host - no host- or TPM-bound material is involved.

### Threat model

The encryption keeps plaintext out of `config.yaml`, source control, and ad-hoc backups of just the unit directory. It does not protect against an attacker with root on the deploy host: the master key sits next to the encrypted files, and decrypted values are inlined into the generated `run.sh` and the built image.

## Deploy

### Start deploy

```bash
git archive --format=tar.gz HEAD | curl \
  -X POST \
  -H "Authorization: Bearer secret-token" \
  --data-binary @- \
  http://127.0.0.1:3000/deploy/myapp
```

Example response:

```json
{
  "version": 1,
  "status": "building"
}
```

The request returns as soon as the deploy is accepted.
The image build continues in the background.

### Check deploy state

```bash
curl \
  -H "Authorization: Bearer secret-token" \
  http://127.0.0.1:3000/deploy/myapp/state
```

Example response during the build:

```json
{
  "version": 1,
  "status": "building"
}
```

Example response after a failed build:

```json
{
  "version": 1,
  "status": "failed",
  "error": "failed to build image: podman build exited with exit status: 125"
}
```

If an app is currently active, the state also includes `active_version`.

Possible `latest_build.status` values:

- `idle`
- `building`
- `ready`
- `failed`

### Read build log

```bash
curl \
  -H "Authorization: Bearer secret-token" \
  http://127.0.0.1:3000/deploy/myapp/log
```

The response body is the current build log from `{deploy_dir}/log/build.log`.
The `X-Offset` response header contains the current log size in bytes; pass it back as `?offset=<value>` to fetch only newly appended output.

## HTTP API

See [`openapi.yaml`](openapi.yaml) for the full API specification.

## Notes

- The app config is read fresh on each deploy request.
- The auth config is loaded once on the first protected request. Restart `dpl` after changing `auth.yaml`.
- Deploy state is stored on disk, not in memory.
- If the archive has a single top-level folder, `dpl` flattens it after extraction.

## Development

Build:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Run the server:

```bash
cargo run -- --config config.yaml
```
