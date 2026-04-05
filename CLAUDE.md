# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`dpl` is a Rust rewrite of an existing deploy tool. It accepts deploy archives over HTTP, unpacks them, generates build artifacts (Containerfile, build scripts, run script, systemd service unit) from Jinja templates, and drives `podman` + `systemd` to run the resulting service. The Rust port is in-progress; see `todo-rust.md` for the scope and roadmap, and `techdoc.md` for the full behavioural spec of the legacy system being replicated.

## Commands

- Build: `cargo build`
- Run server: `cargo run` (binds using `config.yaml` or defaults to `0.0.0.0:3000`)
- Tests: `cargo test`
- Single test: `cargo test <name>` (e.g. `cargo test prepare_creates_first_deploy_layout_and_artifacts`)
- Clippy: `cargo clippy`
- **Do not run `cargo fmt`** (per `.github/copilot-instructions.md`).

Edition is `2024`.

## Runtime layout

All state lives under a base directory, configured by env `DPL_BASE` (default `/opt/dpl`):

- `{DPL_BASE}/config.yaml` — server config (`server.addr`, `server.port`). Missing file = defaults.
- `{DPL_BASE}/{entity_name}/config.yaml` — per-entity config (typed via `type:` discriminator: `app`, `domain`, `static`, `database`).
- `{DPL_BASE}/{entity_name}/version.txt` — monotonically increasing reserved version.
- `{DPL_BASE}/{entity_name}/deploy_{N}/` — per-deploy workspace containing `status.txt`, `logs/`, `app/`, and generated artifacts (`Containerfile`, `run.sh`, `build-N.sh`, service file).

`crate::config::ENV` is a `LazyLock<EnvConfig>` holding the resolved `base_dir`.

## Architecture

Entry point `src/main.rs` starts an axum server with graceful shutdown on Ctrl-C and initializes tracing. Business logic lives in `src/deploy/`.

Key modules:

- `deploy::entity` — `DeployEntity` enum + `EntityType`. `DeployEntity::load` reads the entity's `config.yaml`, dispatches on `type:`, and constructs the concrete entity. Currently only `App` is implemented.
- `deploy::app_entity` — `AppEntity` (loaded app config) and the artifact generation pipeline. `AppConfig` (in `model.rs`) uses `serde(deny_unknown_fields)` and mirrors the legacy YAML schema: `image`, `port`, `build: [{files, env, script}]`, `runtime: {env, init, cmd}`, optional `domain`, `route`, `volumes`, `public`.
- `deploy::app_entity::artifacts` — `ArtifactsContext` renders four MiniJinja templates embedded via `include_str!` from `templates/` (`containerfile.jinja`, `build.sh.jinja`, `run.sh.jinja`, `servicefile.jinja`). Templates register a `cuid()` global function. `Environment` is a `LazyLock` with `keep_trailing_newline`, `trim_blocks`, `lstrip_blocks` enabled.
- `deploy::version` — `get_entity_version` / `reserve_entity_version` (read-increment-write on `version.txt`; no file = version 0).
- `deploy::status` — reads/writes `status.txt` as one of `idle | building | ready | failed`.
- `deploy::service` — `DeployService` is a pure orchestrator. It only decides *which* entity-specific logic to run, serialises per-entity operations via an `EntityLockRegistry` (map of entity name → `Arc<tokio::sync::Mutex>`) to ensure an entity is free, loads the `DeployEntity`, and dispatches to the entity's own `prepare`/deploy methods. The orchestrator does **not** know what each entity needs — it never creates deploy dirs, writes status files, or generates artifacts directly.
- Entity-level logic lives on each entity type. For apps, `AppEntity::prepare` owns the full preparation of an app deploy: checking status, reserving the next version, creating `deploy_{N}/`, allocating a port, rendering and saving artifacts, etc. Each new entity type (`domain`, `static`, `database`) will implement its own `prepare`/deploy methods.

When adding functionality, keep this split: put "is the entity busy / which entity method do I call" concerns in `DeployService`; put "what does a deploy of *this kind* of entity actually do" concerns on the entity itself.

Errors use `thiserror`: top-level `crate::error` (`ConfigError`, `ArtifactError`) and `deploy::error::DeployError` / `deploy::app_entity::error::AppEntityError`. Paths are always captured in error variants alongside the underlying `io`/`serde_yaml`/`minijinja` source.

## Conventions

- Async I/O via `tokio::fs`; sync stdlib `fs` only appears in tests.
- Module layout uses private submodules with `pub use` re-exports through `mod.rs` (see `src/deploy/mod.rs`); keep the same pattern when adding modules.
- Deploy-related identifiers use the `deploy_{N}` / `deploy_id` convention.
