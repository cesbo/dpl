# dpl

`dpl` is a deploy server written in Rust.

## Current Status

What works now:

- `app` entities
- Bearer auth from `auth.yaml`
- `POST /deploy/{name}` to start a deploy
- `GET /deploy/{name}/state` to check deploy state
- generated build artifacts in `deploy_N/artifacts/`
- build logs in `deploy_N/log/build.log`
- static file export from the built image into `deploy_N/exports/`

What is not done yet:

- starting or restarting the generated systemd service
- an HTTP endpoint for reading build logs
- entity types `domain`, `static`, and `database`
- hashed auth keys
- timers to execute scripts in app containers

The project is already useful for preparing a deploy and building an image.
It is not yet a full end-to-end replacement for the old tool.

## What You Need

- Linux with podman and systemd

Default base directory is `/opt/dpl`. You can change it with the `DPL_BASE` environment variable.

The server reads its own config from:

```text
/opt/dpl/config.yaml
```

If this file does not exist, `dpl` uses these defaults:

```yaml
server:
  addr: 0.0.0.0
  port: 3000
```

## Auth Config

All `/deploy` routes need a Bearer token. Tokens are stored in a config file:

```text
/opt/dpl/auth.yaml
```

Example:

```yaml
keys:
  - name: deploy-key
    token: secret-token
    apps: [myapp]
    disabled: false
```

Fields:

- `name` - must be unique inside `auth.yaml`
- `token` - plain-text Bearer token
- `apps` - list of allowed app names. Use `"*"` to allow all apps
- `disabled` - if `true`, the key is rejected

## Entity Config

Each entity has its own directory under the base directory:

```text
/opt/dpl/{name}/
```

For example, an entity named `myapp` lives in `/opt/dpl/myapp/`. This path is referred to as `entity_dir` below.

Common files in every entity directory:

- `config.yaml` - entity config. The `type` field selects the entity kind (`app`, `domain`, `static`, `database`)
- `state.yaml` - stores `version`, `status`, and optional `last_error`

## App Entity

An app entity represents a containerized application. When you deploy an app, `dpl` receives a `.tar.gz` archive with your source code, generates a `containerfile` from your config, builds a podman image, and optionally exports static files from the built image.

### Configuration file

File:

```text
/opt/dpl/myapp/config.yaml
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
    script: |
      npm run build

runtime:
  env:
    NODE_ENV: production
  init: |
    test -d /app/dist
  cmd: node server.js

volumes:
  - source: myapp-data
    path: /app/data

exports:
  - source: /app/public
    url: /static

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
- `timers` - periodic scripts to run in the container (not implemented yet)

Runtime fields:

- `env` - environment variables
- `init` - optional shell script that runs before `cmd`
- `cmd` - main start command

Build layer fields:

- `files` - paths copied from the extracted archive into `/app`. Use `"*"` to copy all files
- `env` - build-time environment variables for the layer script
- `script` - shell script for the layer. If missing, the layer only copies files

### Files

- `{entity_dir}/port.txt` - persisted host port for the entity
- `{entity_dir}/deploy_{version}/` - versioned deploy directory (referred to as `deploy_dir` below)

deploy_dir layout:

- `{deploy_dir}/app.tar.gz` - uploaded archive
- `{deploy_dir}/app/` - extracted archive
- `{deploy_dir}/artifacts/` - holds generated deploy files
- `{deploy_dir}/exports/` - static files exported from the built image
- `{deploy_dir}/log/build.log` - build log with podman build output

## Start DPL

```bash
dpl
```

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
  "last_error": "failed to build image: podman build exited with exit status: 125"
}
```

Possible status values:

- `idle`
- `building`
- `ready`
- `failed`

## HTTP API

### `POST /deploy/{name}`

Starts a new deploy for one app.

Request body:

- raw `.tar.gz` bytes

Response:

- `202 Accepted` with `{ "version": N, "status": "building" }`

Errors:

- `401` - missing or invalid `Authorization` header
- `403` - token is valid but not allowed for this app
- `404` - app config does not exist
- `409` - another deploy for the same app is already building

### `GET /deploy/{name}/state`

Reads the current state from disk.

Response body:

```json
{
  "version": 1,
  "status": "ready"
}
```

## Notes

- The app config is read fresh on each deploy request.
- The auth config is loaded once on the first protected request. Restart `dpl` after changing `auth.yaml`.
- Deploy state is stored on disk, not in memory.
- If the archive has a single top-level folder, `dpl` flattens it after extraction.
- Exported files are copied from the built image after a successful build.
- `timers` field is accepted in the config, but the deploy pipeline does not use it yet.

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
cargo run
```
