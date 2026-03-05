# dpl — Development Plan

## Entity Model

All managed entities live in `/opt/dpl/<name>/config.yaml`.
Each config has a `type` field that determines the entity kind:

| type | example dir | description |
|------|-------------|-------------|
| `app` | `/opt/dpl/profile/` | Web app running in a Podman container |
| `domain` | `/opt/dpl/example.com/` | Domain config (nginx, TLS) |
| `static` | `/opt/dpl/landing/` | Static files with own deploy logic |
| `database` | `/opt/dpl/profile-db/` | Managed database service |

Each entity type is a separate Go package with its own config struct, templates, and deploy logic.

---

## Phase 1: Project Skeleton ✅

- [x] Initialize Go module (`go mod init dpl`), create directory structure
- [x] Create `cmd/dpl/main.go` — entry point with `init` / server mode dispatch
- [x] Add `slog`-based logger setup

## Phase 2: App Configuration ✅

- [x] Define `internal/app` — config struct with yaml tags (including `tokens` array)
- [x] Implement `app.LoadConfig(dir)` — read and validate `config.yaml` (type=app)
- [x] Write unit tests with sample config files in `testdata/`

## Phase 3: `dpl init` — CLI Wizard ✅

- [x] Create `internal/wizard` — interactive prompts (port)
- [x] Generate dpl's own systemd service file with env vars (`DPL_PORT`)
- [x] Write the service file to `/etc/systemd/system/dpl.service` (or user-specified path)

## Phase 4: HTTP Server & Auth ✅

- [x] Create `internal/server` — HTTP server with graceful shutdown
- [x] Common config loader: read `type` field from config.yaml, dispatch to entity package
- [x] Implement Bearer token auth middleware (validate against entity's `tokens`)
- [x] Implement `POST /deploy/{name}` route (stub handler, accept archive)
- [x] Write handler tests (auth, routing, error codes)

## Phase 5: App Templates & Generation ✅

- [x] Embedded templates inside `internal/app` (`embed.FS`)
- [x] `build_sh.tmpl` — build script with heredoc env vars (UUID delimiters)
- [x] `run_sh.tmpl` — runtime script with heredoc env vars + init script + exec
- [x] `containerfile.tmpl` — Containerfile with secret mount for build.sh
- [x] `service.tmpl` — systemd unit for the app container
- [x] Render functions: `app.GenerateBuildScripts`, `app.GenerateRunSh`, etc.
- [x] Golden-file tests for each template in `testdata/`

## Phase 5.1: Build Layers Refactoring ✅

- [x] Replace `BuildConfig.Script` with `BuildConfig.Layers []BuildLayer`
- [x] Each layer has optional `name`, `files`, `env` and required `script`
- [x] Per-layer env (no global `build.env`)
- [x] `GenerateBuildSh` → `GenerateBuildScripts` (returns `[]BuildScript`)
- [x] Containerfile: per-layer `COPY` (from `app/` subdir) + `RUN --mount=type=secret,id=build-sh-N`
- [x] Layer without `files` → only `RUN`, no `COPY`; `files: ["."]` → `COPY app/ ./`
- [x] Updated all golden files, test fixtures, and validation

## Phase 6: App Deploy Pipeline ✅

- [x] Implement `app.Deploy()` — orchestrator tying together all steps
- [x] Create deploy dir: `/opt/dpl/<name>/deploy_<timestamp>/`
- [x] Unpack received archive to `deploy_<timestamp>/app/` subdirectory
- [x] Assemble build context (app/ + generated Containerfile, run.sh)
- [x] Generate per-layer build scripts (`build-sh-1`, `build-sh-2`, ...)
- [x] Call generate functions to produce build artifacts

## Phase 7: Podman Build

- [ ] Create `internal/podman` — wrapper for `podman build` command
- [ ] Build image with `--secret id=build-sh-N` for each layer build script
- [ ] Tag image as `localhost/<name>:<timestamp>`
- [ ] Stream build output back to HTTP response
- [ ] Integration tests (guarded with `//go:build integration`)
- [ ] Write build logs to `/opt/dpl/<name>/deploy_<timestamp>/logs/build.log`

## Phase 8: systemd Service Management

- [ ] Create `internal/systemd` — wrapper for `systemctl` commands
- [ ] Write generated `.service` file to systemd directory
- [ ] `systemctl daemon-reload`, `enable`, `restart` the service
- [ ] Random free port allocation for host-side mapping, persisted in service file
- [ ] Integration tests

## Phase 9: End-to-End Flow

- [ ] Wire everything together: HTTP handler → app.Deploy → podman → systemd
- [ ] Test full deploy cycle on a real server (manual / CI)
- [ ] Error handling & cleanup on failure (remove partial artifacts)

## Phase 10: Polish & Hardening

- [ ] Structured logging throughout all packages
- [ ] Timeouts for podman build & HTTP requests
- [ ] Concurrent deploy safety (lock per entity)
- [ ] README with usage instructions and config.yaml example

## Phase 11: Async Deploy API (202 + Polling)

- [ ] `POST /deploy/<name>` returns `202 Accepted` with `{"deploy_id": "deploy_<timestamp>"}`
- [ ] Deploy runs in a background goroutine
- [ ] `GET /deploy/<name>/<deploy_id>/status` — returns deploy state (`building`, `running`, `done`, `failed`)
- [ ] `GET /deploy/<name>/<deploy_id>/logs` — streams build log from file (`deploy_<timestamp>/logs/build.log`)
- [ ] Keep streaming (Phase 7) as default, async as opt-in (`Accept` header or query param)

---

## Future (not in scope for MVP)

- [ ] `internal/domain` — domain config, nginx generation, TLS/ACME
- [ ] `internal/static` — static site entity: config, templates, deploy
- [ ] `internal/database` — managed database services
- [ ] `public_url` for apps to be exposed outside (nginx config generation)
