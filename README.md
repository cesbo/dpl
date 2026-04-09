# dpl

`dpl` is a deploy server.

## Current Status

What works now:

- `app` entities
- Bearer auth from `auth.yaml`
- `POST /deploy/{name}` to start a deploy
- `GET /deploy/{name}/state` to read deploy state
- generated build artifacts in `deploy_N/artifacts/`
- build logs in `deploy_N/log/build.log`
- static file export from the built image into `deploy_N/exports/`

What is not done yet:

- starting or restarting the generated systemd service
- an HTTP endpoint for reading build logs
- entity types `domain`, `static`, and `database`
- hashed auth keys
- timers to execute scripts in app containers

This means the project is already useful for preparing a deploy and building an image.
It is not yet a full end-to-end replacement for the old tool.

## What You Need

- Linux with podman and systemd

Set `DPL_BASE` to choose the base directory.
If you do not set it, `dpl` uses `/opt/dpl`.

The server reads its own config from:

```text
{DPL_BASE}/config.yaml
```

If this file does not exist, `dpl` uses these defaults:

```yaml
server:
  addr: 0.0.0.0
  port: 3000
```

## Quick Start

### 1. Create the base folders

```bash
mkdir -p /opt/dpl/myapp
```

### 2. Create the server config

File:

```text
/opt/dpl/config.yaml
```

Content:

```yaml
server:
  addr: 0.0.0.0
  port: 3000
```

### 3. Create the auth config

All `/deploy` routes need a Bearer token.
Today, tokens are stored in plain text.

File:

```text
/opt/dpl/auth.yaml
```

Content:

```yaml
keys:
  - name: deploy-key
    token: open-token
    apps: [myapp]
    disabled: false
```

Notes:

- `name` must be unique inside `auth.yaml`
- `apps` can contain app names or `"*"`
- if `disabled` is `true`, the key is rejected

### 4. Create the app config

File:

```text
/opt/dpl/myapp/config.yaml
```

Content:

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
```

How this config works:

- `type` must be `app`
- `image` is the base image in the generated `containerfile`
- `port` is the app port your application listens on
- `build` is a list of build layers
- `runtime.cmd` is the main start command
- `runtime.init` is an optional shell script that runs before `cmd`
- `runtime.env` sets runtime environment variables
- `volumes` are written into the generated service file
- `exports` copies files from the built image into `deploy_N/exports/`

About build layers:

- `files` lists paths copied from the extracted archive into `/app`
- use `"*"` to copy the whole extracted archive
- `env` sets build-time environment variables for that layer script
- `script` is a shell script for that layer
- if `script` is missing, that layer only copies files

Unknown YAML fields are rejected.
This helps catch typos early.

### 5. Start the server

```bash
dpl
```

### 6. Pack your app and upload it

```bash
git archive --format=tar.gz HEAD | curl \
  -X POST \
  -H "Authorization: Bearer open-token" \
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

The request returns when the deploy is accepted.
The image build continues in the background.

### 7. Check deploy state

```bash
curl \
  -H "Authorization: Bearer open-token" \
  http://127.0.0.1:3000/deploy/myapp/state
```

Example response while the build is running:

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

- `202 Accepted` with `{ "name": "...", "version": N }`

Common errors:

- `401` if the `Authorization` header is missing or broken
- `403` if the token is valid but not allowed for this app
- `404` if the app config does not exist
- `409` if another deploy for the same app is already building

### `GET /deploy/{name}/state`

Reads the current state from disk.

Response body:

```json
{
  "version": 1,
  "status": "ready"
}
```

## Files Written By dpl

Current on-disk layout for one app looks like this:

```text
/opt/dpl/
  config.yaml
  auth.yaml
  myapp/
    config.yaml
    port.txt
    state.yaml
    deploy_1/
      app.tar.gz
      app/
      artifacts/
        containerfile
        build-1.sh
        build-2.sh
        run.sh
        app.service
      exports/
      log/
        build.log
```

Important files:

- `state.yaml` stores `version`, `status`, and optional `last_error`
- `port.txt` stores the chosen host port for this app
- `deploy_N/app.tar.gz` is the uploaded archive
- `deploy_N/app/` is the extracted archive
- `deploy_N/artifacts/` holds generated deploy files
- `deploy_N/log/build.log` stores Podman build output

`app.service` is generated, but the current code does not install or start it yet.

## Notes

- The app config is read fresh for each deploy request.
- The auth config is loaded once on the first protected request. Restart `dpl` after changing `auth.yaml`.
- Deploy state is stored on disk, not in memory.
- If the archive has one top-level folder, `dpl` flattens it after extraction.
- Exported files are copied from the built image after a successful build.
- `timers` exist in the Rust config model, but the current deploy pipeline does not use them yet.

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
