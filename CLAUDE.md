# dpl — Deploy Server

Deploy server: accepts deploy archives over HTTP, generates build artifacts from MiniJinja templates, runs `podman build`, exports static files from the built image when configured, and stores deploy state on disk.

## Commands

```bash
cargo build          # Build
cargo run            # Run server (config.yaml or default 0.0.0.0:3000)
cargo test           # Run all tests
cargo test <name>    # Single test (e.g. cargo test render_templates)
cargo clippy         # Lint
```

- Never run `cargo fmt` — the project uses custom rustfmt rules.
- Rust edition is `2024`.

## Project Structure

```
src/
  main.rs                  # Axum server setup, routes, signal handling
  config.rs                # Global server config (DPL_BASE, bind address)
  archive.rs               # tar.gz extraction into deploy workspace
  error.rs                 # Top-level errors: ConfigError, ArtifactError, archive errors
  log.rs                   # Tracing/logging setup
  auth/
    mod.rs                 # Auth module root
    model.rs               # AuthKey model, auth.yaml parsing
    middleware.rs           # Axum auth middleware (Bearer tokens)
    error.rs               # Auth errors
  deploy/
    mod.rs                 # Deploy module root
    config.rs              # Load entity config (config.yaml inside entity directory)
    handlers.rs            # HTTP handlers: POST /deploy/{name}, GET /deploy/{name}/state
    service.rs             # DeployService — orchestrates entity lifecycle
    entity.rs              # Entity enum
    state.rs               # Deploy state persistence (on-disk)
    error.rs               # DeployError
    app_entity/
      mod.rs               # AppEntity implementation
      model.rs             # App entity config model
      artifacts.rs         # MiniJinja template rendering for build artifacts
      podman.rs            # podman build / image export
      port.rs              # Port allocation
```

## Architecture

### Separation of Concerns

- **`DeployService`** (`deploy/service.rs`) — "is the entity busy?", "which entity method to call?". Orchestration only.
- **Entity implementations** (e.g. `deploy/app_entity/`) — "what does a deploy of this kind actually do". All build logic lives here.

Keep this split when adding functionality.

### Request Flow

1. `POST /deploy/{name}` receives a tar.gz archive
2. Handler extracts archive into a versioned workspace, returns `{ "name": ..., "version": N }` immediately
3. Background task runs: template rendering -> `podman build` -> optional static file export -> state update

### Error Handling

Errors use `thiserror` with two layers:
- **`crate::error`** — `ConfigError`, `ArtifactError`, archive errors. Paths captured alongside `io`/`serde_yaml`/`minijinja` sources.
- **`deploy::error::DeployError`** — deploy-specific errors.

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| axum | HTTP framework |
| minijinja | Template rendering for build artifacts |
| serde / serde_yaml | Config and model (de)serialization |
| thiserror | Error types |
| tokio | Async runtime |
| tracing | Structured logging |

### Current Limitations

- Only `app` entities are implemented (no other entity types yet).
- Auth: plain Bearer tokens from `{DPL_BASE}/auth.yaml`.
- API: only `POST /deploy/{name}` and `GET /deploy/{name}/state`.
- Systemd service file is generated but not installed or restarted.

## Coding Style

- Use `thiserror` for error types; include file paths in error context where applicable.
- Prefer `tracing` macros (`info!`, `error!`, `debug!`) over `println!`.
- Keep modules focused: one concern per file.
