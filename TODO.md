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

## Phase 3: CLI Installation Wizard (removed)

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

## Phase 7: Podman Build & Async Deploy API ✅

- [x] Change deploy dir timestamp format to compact `deploy_<YYYYMMDDHHMMSS>`
- [x] `Deploy()` returns `DeployResult{Dir, Timestamp}` struct, creates `logs/` subdir
- [x] Deploy status file on disk: `deploy_<ts>/status` (`building`, `done`, `failed`)
- [x] `internal/app/status.go` — `WriteStatus()` / `ReadStatus()` helpers + tests
- [x] Create `internal/podman` — wrapper for `podman build` command
- [x] `BuildOpts` struct + `Build(ctx, opts)` — runs `podman build` with `--secret id=build-sh-N`
- [x] Tag image as `localhost/<name>:<timestamp>`
- [x] Write build output to `deploy_<ts>/logs/build.log` (no HTTP streaming)
- [x] Integration tests (guarded with `//go:build integration`)
- [x] `POST /deploy/{name}` returns `202 Accepted` + JSON `{"deploy_id": "deploy_<ts>"}`
- [x] Podman build runs in a background goroutine; status file updated on completion
- [x] `GET /deploy/{name}/{deployID}/status` — returns JSON deploy state
- [x] `GET /deploy/{name}/{deployID}/logs?offset=N` — returns build log from byte offset
- [x] `X-Offset` response header for incremental log polling
- [x] All GET endpoints share the same Bearer token auth
- [x] Server tests updated for 202 + JSON + new endpoints

## Phase 7.1: Sequential Versioning & Deploy Lock ✅

- [x] Replace timestamp-based deploy IDs with sequential version numbers
- [x] `internal/app/version.go` — `ReadVersion()` / `WriteVersion()` helpers + tests
- [x] Version stored in `<entityDir>/version.txt`, starts at 1 for first deploy
- [x] `Deploy()` accepts `version int` parameter; dir is `deploy_<N>`, image tag `localhost/<name>:<N>`
- [x] `DeployResult.Timestamp` → `DeployResult.Version int`
- [x] Per-entity `sync.Mutex` in server (`entityLocker`) protects version read + check + increment
- [x] `POST /deploy/{name}` returns `409 Conflict` if previous deploy is still `building`
- [x] `deployIDPattern` relaxed to `^deploy_\d+$`
- [x] All tests updated: deploy_test, server_test (409 + sequential versions), podman/build_test, generate_test
- [x] Golden files updated for version-based image tags

## Phase 8: systemd Service Management ✅

- [x] Create `internal/systemd` — wrapper for `systemctl` commands
- [x] Write generated `.service` file to systemd directory
- [x] `systemctl daemon-reload`, `enable`, `restart` the service
- [x] Random free port allocation for host-side mapping, persisted in service file
- [x] Integration tests

## Phase 9: End-to-End Flow

- [ ] Wire everything together: HTTP handler → app.Deploy → podman → systemd
- [ ] Test full deploy cycle on a real server (manual / CI)
- [ ] Error handling & cleanup on failure (remove partial artifacts)

## Phase 10: Polish & Hardening

- [ ] Structured logging throughout all packages
- [ ] Timeouts for podman build & HTTP requests
- [ ] README with usage instructions and config.yaml example

---

## Future (not in scope for MVP)

- [ ] `internal/domain` — domain config, nginx generation, TLS/ACME
- [ ] `internal/static` — static site entity: config, templates, deploy
- [ ] `internal/database` — managed database services
- [ ] `public_url` for apps to be exposed outside (nginx config generation)
