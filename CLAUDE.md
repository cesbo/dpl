# dpl — Deploy Server

Deploy server: accepts deploy archives over HTTP, generates build artifacts from MiniJinja templates, runs `podman build`, exports static files from the built image when configured, and stores deploy state on disk.

## Commands

```bash
cargo build          # Build
cargo run -- -c config.yaml  # Run server with config file
cargo test           # Run all tests
cargo test <name>    # Single test (e.g. cargo test render_templates)
cargo clippy         # Lint
```

- Never run `cargo fmt` — the project uses custom rustfmt rules.
- Rust edition is `2024`.

## Architecture

### Separation of Concerns

- **`DeployService`** (`deploy/service.rs`) — "is the entity busy?", "which entity method to call?". Orchestration only.
- **Entity implementations** (e.g. `deploy/app_entity/`) — "what does a deploy of this kind actually do". All build logic lives here.

Keep this split when adding functionality.

### Request Flow

1. `POST /deploy/{name}` receives a tar.gz archive
2. Handler extracts archive into a versioned workspace, returns `{ "name": ..., "version": N }` immediately
3. Background task runs: template rendering -> `podman build` -> optional static file export -> state update

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

- Entity types implemented: `app`, `domain`.
- Auth: plain Bearer tokens from `{config.base}/auth.yaml`.
- API: `POST /deploy/{name}`, `GET /deploy/{name}/state`, `GET /deploy/{name}/log`.
- Systemd service file is generated but not installed or restarted.

## Coding Style

- Use `thiserror` for error types; include file paths in error context where applicable.
- Prefer `tracing` macros (`info!`, `error!`, `debug!`) over `println!`.
- Keep modules focused: one concern per file.
- Function argument order: context/destination (`&Path`, config refs) → subject/data → mutable/owned state → options/callbacks.
