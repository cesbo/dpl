# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

dpl is a single static-binary deploy CLI that manages local units (`app`, `db-server`, `db`, `domain`, `http-server`) on one host. It uses `podman` to run containers and `systemd` to supervise them. It runs as root, is not a daemon, and has no server-side component.

This project is in active development; backward compatibility is NOT required. Make breaking changes freely whenever they improve the design - do not preserve legacy behavior, deprecated APIs, or compatibility shims.

## Commands

- Build: `cargo build` (release is slow: `[profile.release]` uses LTO + `panic=abort`).
- Test: `cargo test`. Single test by name substring: `cargo test validate_references_full_chain`. Tests are inline `#[cfg(test)] mod tests`; there is no `tests/` directory.
- Lint: `cargo clippy` - keep it clean before finishing. Edition 2024.
- Run the CLI: `cargo run -- --base <scratch-dir> <subcommand>`.
- Most logic is unit-tested against temp dirs and needs nothing installed. Exercising a real deploy needs `podman` + `systemd` on the host.

Formatting: there is no checked-in `rustfmt.toml`, but imports are hand-formatted as merged-by-crate, one item per line, with std / external / `crate` in separate `use` blocks. Do not run `cargo fmt` blindly - on stable it reformats every import block and creates unrelated churn. Match the surrounding import style by hand.

## Architecture

The unit model plus typed cross-unit references is the novel core; preserve it across refactors (see Design Decisions).

- **Units.** `UnitConfig` (`src/deploy/unit/mod.rs`) is a `type:`-tagged enum with five variants: `app` (build and run a container), `db-server` (a DBMS container), `db` (a database created on a server via `podman exec`), `http-server` (an nginx container), `domain` (an nginx vhost/proxy). One unit = one `conf/{name}.yaml`; `list_units` enumerates `conf/*.yaml`.
- **Typed references.** `env` values embed `${unit:key}` and `${secret:name}` tokens, parsed in `src/config/env/value.rs` and resolved in `src/reference.rs`, which threads a `Location` trail for precise errors. Each unit type implements `resolve_export(key)` (app: `url`/`socket`/`export`; db: `url`/`host`/`port`/`user`/`password`/`name`). `dpl check` validates the entire reference graph (`validate_references`) without deploying. An app's database dependencies are derived from its `${db:...}` refs, not declared.
- **Deploy.** `cmd/unit::deploy` dispatches on the unit type to a per-type `deploy` in `src/deploy/unit/{app,db,domain,http_server}`. The app path: take the deploy lock and bump the version, extract the tar, render MiniJinja artifacts (`containerfile`, `build-N.sh`, `run.sh`, `.service`), `podman build`, then install the systemd service (registering timers into the unit's timer state and copying static exports into the nginx `dpl-www` volume). The other types have lighter flows. Outcomes and failures are written to the deploy state file.
- **dpl delegates lifecycle to itself.** The generated `.service` runs `dpl start` / `dpl stop` as its `ExecStart` / `ExecStop`. Timers are fired by the in-process scheduler (`src/scheduler/`): `dpl serve` polls every unit's timer state and runs due timers via `dpl timer <unit> <name>` (which `podman exec`s the script in the container). So `start`/`stop`/`timer` are CLI subcommands invoked by systemd and the scheduler, not only by humans.
- **Paths.** Everything under `{base}` (default `/opt/dpl`) is addressed through `MainContext` accessors in `src/context.rs`. Never hardcode `{base}/...`; add or extend an accessor. The layout is grouped by kind, not per-unit: `conf/{name}.yaml`, `state/{name}--{deploy,timers}.{json,lock}`, `log/{name}.log`, `secrets/`, `backup/` (see README "Layout"). System-side artifacts live outside base: `/etc/systemd/system/dpl--*.service`, `/var/log/podman/`, podman network `dpl`.
- **Secrets** (`src/secret.rs`): AES-256-GCM, `secrets/master.key` plus one JSON file per secret. Plaintext is resolved as close to use as possible - inlined into `run.sh` for app units, passed as `-e VAR=value` by `dpl start` for db-server units (never written into the unit file).
- **State and locking** (`src/state.rs`, `src/timers.rs`): per-unit deploy and timer state are JSON written atomically (temp file then rename); concurrency is guarded by `flock(2)` on `state/{name}--*.lock`, released on drop.
- **Errors:** `thiserror` enums per module for library/domain errors; `anyhow` (`Context`, `bail!`) in `src/cmd/`. `DeployError::Reported` means "already shown to the user", so `main` skips the duplicate report.

## Testing conventions

- Inline `#[cfg(test)] mod tests`; use `tempfile::TempDir` for the filesystem.
- Write unit configs with the test-only `MainContext::write_test_unit(name, yaml)` helper (it routes through `config_path`). Never hand-build `{base}/{name}/...` paths in tests, so they cannot drift from the real layout.

## Code conventions

- In comments use regular hyphen - for dashes, not em dash — or en dash –
- Keep comments lean: state the intent in a line or two, not the implementation. The code already shows *how*; the comment exists for the *why* that isn't obvious. Don't narrate each branch or restate what the next line does.

## Design Decisions

@.claude/build-vs-buy.md
