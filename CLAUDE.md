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
- **Deploy state** (`deploy/state.rs`) — on-disk `state.json` and the
  `.deploy.lock` advisory `flock`. Acquired before any unit deploy runs.

Keep this split when adding functionality.

### Deploy Flow (app unit)

1. `dpl deploy <name> [path]` loads `UnitConfig`, calls
   `validate_references` against the current secrets and referenced units,
   then `DeployState::acquire` takes the `flock` on `{unit_dir}/.deploy.lock`.
2. `AppUnit::deploy` bumps the version, writes the archive to
   `{deploy_dir}/app.tar.gz`, renders artifacts (`containerfile`, `run.sh`,
   `build-N.sh`, systemd service), runs `podman build`, optionally exports
   static files, and (re)installs the systemd service via `systemctl`.
3. `DeployState` is rewritten to `{unit_dir}/state.json` at each phase
   transition; failures land as `status: failed` with an `error` string.

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
- `dpl db backup`/`dpl db restore` stream plain SQL through `podman exec` as
  the `db` unit's login user (engine `dump`/`restore` methods in
  `deploy/unit/db/backup.rs`, mirroring `create_database` in `sql.rs`). The
  `path` arg defaults to `-` (stdout/stdin); no compression, no managed backup
  directory, no DROP/CREATE — restore runs `DbUnit::deploy()` first (server
  up + empty database provisioned if missing) and then replays the dump on
  top of whatever is already there. The
  child's stderr is drained on a separate thread and streamed live above the
  spinner (the `on_stderr` callback wired to `spinner::stderr_sink`); this also
  prevents a chatty client from deadlocking by filling its stderr pipe while
  the data pipe is busy. On a non-zero exit the streamed output is the detail,
  so the returned error only carries the exit status.

## Coding Style

- Use `thiserror` for error types; include file paths in error context where applicable.
- Prefer `tracing` macros (`info!`, `error!`, `debug!`) over `println!`.
- Keep modules focused: one concern per file.
- Function argument order: context/destination (`&Path`, config refs) → subject/data → mutable/owned state → options/callbacks.

## Design Decisions

@.claude/build-vs-buy.md
