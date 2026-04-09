# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`dpl` is a deploy server written in Rust.
It accepts deploy archives over HTTP, saves them into a per-deploy workspace, generates build artifacts from MiniJinja templates, runs `podman build`, exports static files from the built image when configured, and stores deploy state on disk.

Current implementation status:

- only `app` entities are implemented;
- auth uses plain Bearer tokens from `{DPL_BASE}/auth.yaml`;
- HTTP API currently exposes `POST /deploy/{name}` and `GET /deploy/{name}/state`;
- a systemd service file is generated, but the current code does not install or restart it yet.

## Commands

- Build: `cargo build`
- Run server: `cargo run` (binds using `config.yaml` or defaults to `0.0.0.0:3000`)
- Tests: `cargo test`
- Single test: `cargo test <name>` (e.g. `cargo test render_templates`)
- Clippy: `cargo clippy`
- Never run `cargo fmt`

Edition is `2024`.

## Runtime layout

All state lives under a base directory, configured by env `DPL_BASE` (default `/opt/dpl`):

- `{DPL_BASE}/config.yaml` — server config (`server.addr`, `server.port`). Missing file = defaults.
- `{DPL_BASE}/auth.yaml` — auth keys used by all `/deploy` routes.
- `{DPL_BASE}/{entity_name}/config.yaml` — per-entity config (typed via `type:` discriminator: `app`, `domain`, `static`, `database`).
- `{DPL_BASE}/{entity_name}/port.txt` — persisted host port for the entity.
- `{DPL_BASE}/{entity_name}/state.yaml` — current deploy state (`version`, `status`, optional `last_error`).
- `{DPL_BASE}/{entity_name}/deploy_{N}/` — per-deploy workspace containing `app.tar.gz`, extracted `app/`, generated `artifacts/`, optional `exports/`, and `log/build.log`.

`crate::config::ENV` is a `LazyLock<EnvConfig>` holding the resolved `base_dir`.

## Architecture

Entry point `src/main.rs` starts an axum server with graceful shutdown on Ctrl-C and initializes tracing. Business logic lives in `src/deploy/`.

Key modules:

- `archive` — extracts uploaded `.tar.gz` archives into the deploy workspace and flattens a single top-level directory after extraction.
- `auth` — lazily loads `{DPL_BASE}/auth.yaml` once, checks Bearer tokens by exact plain-text match, and verifies the requested app is allowed by `apps`.
- `deploy::entity` — `DeployEntity` enum + `EntityType`. `DeployEntity::load` reads the entity's `config.yaml`, dispatches on `type:`, and constructs the concrete entity. Currently only `App` is implemented.
- `deploy::app_entity` — `AppEntity` owns app deploy execution. `deploy()` loads `state.yaml`, rejects parallel builds for the same entity, bumps the version, writes `building`, prepares the deploy workspace, and spawns the blocking build worker.
- `deploy::app_entity::model` — `AppConfig` uses `serde(deny_unknown_fields)` and currently supports `image`, `port`, `build`, `runtime`, `volumes`, `exports`, and `timers`.
- `deploy::app_entity::artifacts` — `ArtifactsContext` renders four MiniJinja templates embedded via `include_str!` from `templates/` (`containerfile.jinja`, `build.sh.jinja`, `run.sh.jinja`, `servicefile.jinja`). Templates register a `cuid()` global function. `Environment` is a `LazyLock` with `keep_trailing_newline`, `trim_blocks`, `lstrip_blocks` enabled.
- `deploy::app_entity::podman` — runs `podman build`, streams build output into the deploy log, and copies configured exports out of the built image into `deploy_{N}/exports/`.
- `deploy::state` — reads and writes `{entity}/state.yaml` with statuses `idle | building | ready | failed`.
- `deploy::service` — `DeployService` is a thin orchestrator. It serialises per-entity operations via a map of `Arc<tokio::sync::Mutex<()>>`, loads the entity, and dispatches to the entity-specific deploy logic.

Important current behavior:

- the deploy endpoint returns `{ "name": ..., "version": N }` after the deploy is accepted;
- the heavy build runs in a background task;
- the generated service file is not applied to systemd yet;
- `state()` reads only the current entity state, not per-version history.

When adding functionality, keep this split: put "is the entity busy / which entity method do I call" concerns in `DeployService`; put "what does a deploy of *this kind* of entity actually do" concerns on the entity itself.

Errors use `thiserror`: top-level `crate::error` (`ConfigError`, `ArtifactError`, archive errors) and `deploy::error::DeployError`. Paths are captured in config and artifact errors alongside the underlying `io`/`serde_yaml`/`minijinja` source where applicable.

## Conventions

- Async I/O via `tokio::fs`; sync stdlib `fs` only appears in tests.
- Module layout uses private submodules with `pub use` re-exports through `mod.rs` (see `src/deploy/mod.rs`); keep the same pattern when adding modules.
- Deploy-related identifiers use the `deploy_{N}` / `deploy_id` convention.
