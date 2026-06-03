# dpl - Deploy CLI

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

- Never run `cargo fmt` - the project uses custom rustfmt rules.
- Rust edition is `2024`.

## Architecture

### Separation of Concerns

- **`cmd/`** (`cmd/unit.rs`, `cmd/db.rs`, `cmd/secret.rs`) - clap subcommand
  surface (top-level `deploy`/`check`/`inspect`/`start`/`stop` live in
  `cmd/unit.rs`). Parses flags, prompts for missing input, calls into the
  unit/secret layer. `start`/`stop` are the container-lifecycle commands the
  generated systemd service invokes via `ExecStart`/`ExecStop`; they route
  through per-unit `start()`/`stop()` methods (which build the `podman run`
  argv and `exec` it, so the service file carries no podman logic).
- **Unit implementations** (`deploy/unit/app/`, `deploy/unit/db/`,
  `deploy/unit/domain/`) - "what does a deploy / install of this kind actually
  do". All build, render, and systemd logic lives here. For db units,
  `DbServerEngine` owns every `podman exec` invocation - `ping`/`create_database`
  (`sql.rs`), `dump`/`restore` (`backup.rs`), `console` (`console.rs`); `cmd/db.rs`
  only resolves config/secrets and picks the login (e.g. `--root`), never spawns
  the client itself.
- **Deploy state** (`state.rs`) - on-disk `.deploy.state` (JSON) and the
  `.deploy.lock` advisory `flock`. Acquired before any unit deploy runs. Timer
  run records live separately in `.timers.state` with their own `.timers.lock`
  (`timers.rs`).

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
3. `DeployState` is rewritten to `{unit_dir}/.deploy.state` at each phase
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
- Auth is out of scope - `dpl` runs locally (typically as root). Sensitive
  values live in `{base}/.secrets/` encrypted with an AES-256-GCM master key.
- Entry points are CLI subcommands (`dpl deploy`, `dpl start`, `dpl stop`,
  `dpl timer`, `dpl db ...`, `dpl secret ...`).
- `dpl deploy` installs and starts the generated systemd unit for app
  and db-server units, and creates the database (via `podman exec` against
  the running server) for db units. Other unit types render artifacts but
  don't yet install services.
- The systemd service is a thin supervisor: its `ExecStart`/`ExecStop` only
  call `dpl start <name>` / `dpl stop <name>`. Those commands own the
  container lifecycle (log dir + `dpl` network setup, db-dependency wait
  gates, the `podman run` argv which `dpl start` `exec`s to stay the
  `MAINPID` for `Type=notify`, and `podman stop`/`rm`). No `ExecStartPre`,
  inline `podman run`, or secret `Environment=` lives in the unit file.
  Timers are driven by cron, not systemd: deploying an app with enabled timers
  writes `/etc/cron.d/{scoped_unit_name}` (one line per timer, no file when
  there are none), each line calling `dpl timer <unit> <timer>` (no inline
  `podman exec`). `TimerConfig::schedule` is a standard 5-field cron expression
  parsed with `croner` at config load. `dpl timer` takes the per-unit timer
  lock (`.timers.lock`, blocking, so concurrent runs serialize) and records each
  run into `.timers.state` (keyed by timer name) for `dpl inspect` — marked
  `running` while in flight, then overwritten with `success`/`failed` and the
  run's duration. It checks the deploy lock non-blocking: while a deploy holds
  it, the run is skipped and the reason recorded as a failed entry; otherwise it
  holds the deploy lock for the run so a deploy can't replace the container
  underneath it.
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
- `anyhow` is confined to the console layer (`src/cmd/*`). Everywhere else
  (`deploy/`, `podman/`, `config/`, `secret/`, …) functions return typed errors
  (`thiserror` enums or `io::Result`); map foreign errors with
  `.map_err(io::Error::other)` rather than reaching for `anyhow`. `src/cmd`
  lifts those into `anyhow::Result` with `.with_context(...)` at the CLI edge.
- Prefer `tracing` macros (`info!`, `error!`, `debug!`) over `println!`.
- Keep modules focused: one concern per file.
- Function argument order: context/destination (`&Path`, config refs) → subject/data → mutable/owned state → options/callbacks.

## Design Decisions

@.claude/build-vs-buy.md
