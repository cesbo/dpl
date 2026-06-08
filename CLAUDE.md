# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

dpl is a single static-binary deploy CLI that manages local units (`app`, `db-server`, `db`, `domain`, `http-server`) on one host. It uses `podman` to run containers and the `dpl serve` daemon supervises them.

This project is in active development; backward compatibility is NOT required. Make breaking changes freely whenever they improve the design - do not preserve legacy behavior, deprecated APIs, or compatibility shims.

## Commands

- Build: `cargo build` (release is slow: `[profile.release]` uses LTO + `panic=abort`).
- Test: `cargo test`. Single test by name substring: `cargo test validate_references_full_chain`. Tests are inline `#[cfg(test)] mod tests`; there is no `tests/` directory.
- Lint: `cargo clippy` - keep it clean before finishing. Edition 2024.
- Run the CLI: `cargo run -- --base <scratch-dir> <subcommand>`.
- Most logic is unit-tested against temp dirs and needs nothing installed. Exercising a real deploy needs `podman` + `systemd` on the host.

Formatting: do not use `cargo fmt` in this repository. Formatting is handled by rust-analyzer.

## Architecture

The unit model plus typed cross-unit references is the novel core; preserve it across refactors (see Design Decisions).

- **Units.** `UnitConfig` (`src/deploy/unit/mod.rs`) is a `type:`-tagged enum with five variants: `app` (build and run a container), `db-server` (a DBMS container), `db` (a database created on a server via `podman exec`), `http-server` (an nginx container), `domain` (an nginx vhost/proxy). One unit = one `conf/{name}.yaml`; `list_units` enumerates `conf/*.yaml`.
- **Typed references.** `env` values embed `${unit:key}` and `${secret:name}` tokens, parsed in `src/config/env/value.rs` and resolved in `src/reference.rs`, which threads a `Location` trail for precise errors. Each unit type implements `resolve_export(key)` (app: `url`/`socket`/`export`; db: `url`/`host`/`port`/`user`/`password`/`name`). `dpl check` validates the entire reference graph (`validate_references`) without deploying. An app's database dependencies are derived from its `${db:...}` refs, not declared.
- **Deploy.** `cmd/unit::deploy` dispatches on the unit type to a per-type `deploy` in `src/deploy/unit/{app,db,domain,http_server}`. The app path: take the deploy lock and bump the version, extract the tar, render MiniJinja artifacts (`containerfile`, `build-N.sh`, `run.sh`), `podman build`, copy static exports into the nginx `dpl-www` volume and register timers, then hand the container off to `dpl serve` (mark the deploy state `check` + SIGHUP serve), run the health check, and flip to `ready`. The other types have lighter flows. Outcomes and failures are written to the deploy state file.
- **`dpl serve` owns lifecycle.** The in-process serve loop (`src/serve/`) supervises containers and fires timers. The container supervisor reconciles every supervised unit, spawning `dpl start <unit>` as a child for `check`/`ready` units, restarting it if it exits, and stopping it on shutdown (`src/serve/supervisor.rs`). The supervised set comes only from deploy state - `DeployState::list` over `state/*--deploy.json` filtered by the `supervised` flag (set at the `check` hand-off by `set_check`, sticky after); the editable configs are never read, since they can drift ahead of what was actually deployed. The timer loop fires due timers via `dpl timer <unit> <name>` (which `podman exec`s the script in the container). Deploy marks a unit `check` and SIGHUPs serve to start it now (`src/serve::notify` reads `state/serve.pid`). So `start`/`stop`/`timer` are CLI subcommands invoked by `dpl serve`, not only by humans.
- **Paths.** Everything under `{base}` (default `/opt/dpl`) is addressed through `MainContext` accessors in `src/context.rs`. Never hardcode `{base}/...`; add or extend an accessor. The layout is grouped by kind, not per-unit: `conf/{name}.yaml`, `state/{name}--{deploy,timers}.{json,lock}`, `state/serve.pid` (also the serve process's single-instance flock), `log/{name}.log`, `secrets/`, `backup/` (see README "Layout"). System-side artifacts live outside base: a systemd unit for `dpl serve` itself, and the podman network `dpl`.
- **Secrets** (`src/secret.rs`): AES-256-GCM, `secrets/master.key` plus one JSON file per secret. Plaintext is resolved as close to use as possible - inlined into `run.sh` for app units, passed as `-e VAR=value` by `dpl start` for db-server units (never written into the unit file).
- **State and locking** (`src/state.rs`, `src/timers.rs`): per-unit deploy and timer state are JSON written atomically (temp file then rename); concurrency is guarded by `flock(2)` on `state/{name}--*.lock`, released on drop.
- **Errors:** `thiserror` enums per module for library/domain errors; `anyhow` (`Context`, `bail!`) in `src/cmd/`. `DeployError::Reported` means "already shown to the user", so `main` skips the duplicate report.

## Testing conventions

- Inline `#[cfg(test)] mod tests`; use `tempfile::TempDir` for the filesystem.
- Write unit configs with the test-only `MainContext::write_test_unit(name, yaml)` helper (it routes through `config_path`). Never hand-build `{base}/{name}/...` paths in tests, so they cannot drift from the real layout.

## Code conventions

- In comments use regular hyphen - for dashes, not em dash — or en dash –
- AI agents must keep comments concise and useful. Prefer no comment when the code is already self-explanatory. Add a comment only for non-obvious intent, invariants, external contracts, safety/lifecycle constraints, or IDE-facing API summaries.
- Comments should explain *why* or *what contract matters*, not narrate *how* the next line works. Keep doc comments to one short sentence by default, and keep inline comments to one line unless the surrounding behavior is genuinely subtle.

## Design Decisions

@.claude/build-vs-buy.md
