# dpl — Deploy CLI

CLI tool that manages local units (`app`, `db-server`, `db`, `domain`),
renders build artifacts from MiniJinja templates, runs `podman build`,
exports static files when configured, and installs systemd services. State
lives on disk in each unit's directory.

## Commands

```bash
cargo build          # Build
cargo test           # Run all tests
cargo test <name>    # Single test (e.g. cargo test render_templates)
cargo clippy         # Lint
cargo run -- --base /path/to/base <command> ...
```

- Never run `cargo fmt` — the project uses custom rustfmt rules.
- Rust edition is `2024`.

## Architecture

### Separation of Concerns

- **`cmd/`** (`cmd/unit.rs`, `cmd/db.rs`, `cmd/secret.rs`) — clap subcommand
  surface (top-level `deploy`/`check`/`inspect` live in `cmd/unit.rs`). Parses
  flags, prompts for missing input, calls into the unit/secret layer.
- **Unit implementations** (`deploy/unit/app/`, `deploy/unit/db/`,
  `deploy/unit/domain/`) — "what does a deploy / install of this kind actually
  do". All build, render, and systemd logic lives here. For db units,
  `DbServerEngine` owns every `podman exec` invocation — `ping`/`create_database`
  (`sql.rs`), `dump`/`restore` (`backup.rs`), `console` (`console.rs`); `cmd/db.rs`
  only resolves config/secrets and picks the login (e.g. `--root`), never spawns
  the client itself.
- **Deploy state** (`deploy/state.rs`) — on-disk `.state.json` and the
  `.deploy.lock` advisory `flock`. Acquired before any unit deploy runs.

Keep this split when adding functionality.

### Deploy Flow (app unit)

1. `dpl deploy <name> [path]` loads `UnitConfig`, calls
   `validate_references` against the current secrets and referenced units,
   then `DeployState::acquire` takes the `flock` on `{unit_dir}/.deploy.lock`.
2. `AppUnit::deploy` bumps the version, writes the archive to
   `{deploy_dir}/app.tar.gz`, renders artifacts (`containerfile`, `run.sh`,
   `build-N.sh`, systemd service), runs `podman build`, optionally exports
   static files, and (re)installs the systemd service via `systemctl`. An app
   with no `runtime` is a static build-and-export unit: it builds + exports
   only, skipping `run.sh`, the systemd service, the health check, and timers
   (the `port` lives inside `runtime`, so static units have none).
3. `DeployState` is rewritten to `{unit_dir}/.state.json` at each phase
   transition; failures land as `status: failed`, with `phase` recording the
   `log::phase` active at the failure (e.g. `building app image` →
   `{unit_dir}/build.log`, `waiting for app` → `/var/log/podman/{scoped}.log`).

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| clap | CLI parsing (`derive`, subcommands) |
| dialoguer | Interactive prompts for missing flags |
| minijinja | Template rendering for build artifacts |
| serde / serde_yaml | Config and model (de)serialization |
| aes-gcm | AES-256-GCM for the secrets store |
| fs4 | Advisory `flock(2)` for `.deploy.lock` |
| thiserror / anyhow | Error types (`thiserror` for library, `anyhow` for CLI) |
| tracing | Structured logging |

### Current Limitations

- Unit types implemented: `app`, `db-server`, `db`, `domain`.
- Auth is out of scope — `dpl` runs locally (typically as root). Sensitive
  values live in `{base}/.secrets/` encrypted with an AES-256-GCM master key.
- Entry points are CLI subcommands (`dpl deploy`, `dpl db ...`,
  `dpl secret ...`). No HTTP surface.
- `dpl deploy` installs and starts the generated systemd unit for app
  and db-server units, and creates the database (via `podman exec` against
  the running server) for db units. Other unit types render artifacts but
  don't yet install services.
- `dpl db backup [path]` streams SQL via `podman exec` as the `db` unit's
  login user (`deploy/unit/db/backup.rs`). `path` defaults to `-` (stdout); a
  `.gz` destination or `-z` gzips the output.
- Restore happens through `dpl deploy <db-name> [backup]`, not a separate
  command (`DbUnit::deploy` in `deploy/unit/db/database.rs`). It brings the
  server up, refuses if the database already exists (delete manually to
  re-import), then creates the database and replays the dump. Gzip is
  auto-detected, so `.sql` and `.sql.gz` both work; `path` may be a file, `-`
  for stdin, or omitted (provision only).

## Coding Style

- Use `thiserror` for error types; include file paths in error context where applicable.
- Prefer `tracing` macros (`info!`, `error!`, `debug!`) over `println!`.
- Keep modules focused: one concern per file.
- Function argument order: context/destination (`&Path`, config refs) → subject/data → mutable/owned state → options/callbacks.

## Design Decisions

@.claude/build-vs-buy.md
