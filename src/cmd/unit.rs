use std::{
    collections::{
        BTreeMap,
        HashMap,
    },
    fs,
    io::{
        self,
        Read,
    },
    path::Path,
    time::Duration,
};

use anyhow::{
    Context,
    Result,
    bail,
};
use chrono::{
    Local,
    Utc,
};

use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        UnitConfig,
        app::{
            AppConfig,
            AppUnit,
        },
        cloudflare_tunnel::{
            CloudflareTunnelConfig,
            CloudflareTunnelUnit,
        },
        db::{
            DbServerConfig,
            DbServerUnit,
            DbUnit,
        },
        domain::{
            DomainConfig,
            DomainUnit,
        },
        http_server::HttpServerUnit,
        list_units,
        undeploy_container,
    },
    log::{
        DeployConsole,
        fmt_ago,
        fmt_duration,
        print_field,
        success_mark,
    },
    podman::{
        self,
        health,
    },
    state::{
        DeployFailure,
        DeployLockGuard,
        DeployStage,
        DeployState,
        DeployStatus,
        LockHolder,
    },
    timers::{
        TimerOutcome,
        TimerState,
        TimersState,
    },
};

pub fn check(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let _ = load_unit(ctx, name)?;
    println!("ok");
    Ok(())
}

pub fn deploy(ctx: &MainContext, name: &UnitName, path: Option<&Path>) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let input = match unit {
        UnitConfig::App(_) => {
            let input = open_input(path)?;
            if input.is_none() {
                bail!("archive path is required to deploy app unit (use `-` to read from stdin)")
            }
            input
        }
        UnitConfig::Db(_) => open_input(path)?,
        _ => {
            if path.is_some() {
                bail!("path argument is not allowed for {} unit", unit.kind());
            } else {
                None
            }
        }
    };

    let (_guard, mut state) =
        DeployState::acquire(ctx, name).with_context(|| format!("acquire unit '{name}'"))?;

    let version = state
        .begin_deploy(unit.kind())
        .map_err(|e| DeployError::step_prepare("begin deploy", e))
        .with_context(|| format!("deploy unit '{name}'"))?;

    let build_log_path = ctx.build_log_path(name);
    let _ = crate::log::cri_log::remove_all(&build_log_path);

    let console = DeployConsole::open(name, version);

    let result: std::result::Result<(), DeployError> = match unit {
        UnitConfig::App(app_config) => {
            // validated Some in the pre-flight match
            AppUnit::new(ctx, name, app_config).deploy(&mut state, version, input.unwrap())
        }
        UnitConfig::CloudflareTunnel(tunnel_config) => {
            CloudflareTunnelUnit::new(ctx, name, tunnel_config).deploy(&mut state)
        }
        UnitConfig::Db(db_config) => DbUnit::new(ctx, name, db_config).deploy(&mut state, input),
        UnitConfig::DbServer(db_server_config) => {
            DbServerUnit::new(ctx, name, db_server_config).deploy(&mut state)
        }
        UnitConfig::HttpServer(http_config) => {
            HttpServerUnit::new(ctx, name, http_config).deploy(&mut state)
        }
        UnitConfig::Domain(domain_config) => {
            DomainUnit::new(ctx, name, domain_config).deploy(&mut state)
        }
    };

    match result {
        Ok(()) => {
            console.finish_ok();
            Ok(())
        }
        Err(err) => {
            // Record the failing stage and cause, then surface the cause once on
            // the console before converting to Reported (main stays quiet).
            let mut log_path = build_log_path;
            let cause = match err.failure() {
                Some((stage, message)) => {
                    state.set_failed(stage, message.clone());
                    if stage == DeployStage::Startup {
                        log_path = ctx.runtime_log_path(name);
                    }
                    message
                }
                None => String::new(),
            };
            console.finish_err(&cause, &log_path);
            Err(anyhow::Error::new(DeployError::Reported))
        }
    }
}

/// One line per unit. `RUN` comes from a single `podman ps`; `?` means podman
/// did not answer, `-` a unit that has no container of its own.
pub fn status(ctx: &MainContext) -> Result<()> {
    let serve = match fs::read_to_string(ctx.serve_pid_path())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&pid| pid != 0 && super::down::process_alive(pid))
    {
        Some(pid) => format!("running (pid {pid})"),
        None => "not running".to_string(),
    };
    print_field("Serve", serve);
    println!();

    let containers = podman::run_podman(&[
        "ps",
        "-a",
        "--filter",
        "name=dpl--",
        "--format",
        "{{.Names}} {{.State}}",
    ])
    .map_err(|err| crate::log::warn(format!("podman did not answer: {err}")))
    .ok()
    .map(|out| {
        out.lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(n, s)| (n.to_string(), s.to_string()))
            .collect::<HashMap<_, _>>()
    });

    let mut units: BTreeMap<UnitName, (Option<UnitConfig>, Option<DeployState>)> = BTreeMap::new();
    for (name, config) in list_units(ctx, |_| true) {
        units.entry(name).or_default().0 = Some(config);
    }
    for (name, state) in DeployState::list(ctx) {
        units.entry(name).or_default().1 = Some(state);
    }

    let now = Utc::now();
    let rows: Vec<[String; 6]> = units
        .into_iter()
        .map(|(name, (config, state))| {
            let kind = match (&config, state.as_ref().and_then(|s| s.kind.clone())) {
                (Some(c), _) => c.kind_display(),
                (None, Some(kind)) => format!("{kind} (no config)"),
                (None, None) => "?".to_string(),
            };
            let (version, deploy, age) = match &state {
                Some(s) => (
                    s.active_version.map_or("-".to_string(), |v| format!("v{v}")),
                    deploy_status_name(s.last_status).to_string(),
                    fmt_ago(&now, &s.updated_at),
                ),
                None => ("-".into(), "idle".into(), "-".into()),
            };
            let run = run_column(
                state.as_ref().is_some_and(|s| s.supervised),
                containers.as_ref().map(|c| c.get(&name.scoped_unit_name())),
            );
            [name.to_string(), kind, version, deploy, run, age]
        })
        .collect();

    let header = ["UNIT", "KIND", "VERSION", "DEPLOY", "RUN", "AGE"].map(String::from);
    let mut widths = [0; 6];
    for row in std::iter::once(&header).chain(&rows) {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.len());
        }
    }
    let line = |row: &[String; 6]| {
        let cells: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        cells.join("  ").trim_end().to_string()
    };
    println!("{}", console::style(line(&header)).bold());
    for row in &rows {
        println!("{}", line(row));
    }
    Ok(())
}

/// `RUN` cell. `container`: outer `None` = podman did not answer, inner `None`
/// = no such container.
fn run_column(supervised: bool, container: Option<Option<&String>>) -> String {
    match (supervised, container) {
        (false, _) => "-".into(),
        (true, None) => "?".into(),
        (true, Some(None)) => "down".into(),
        (true, Some(Some(state))) => state.clone(),
    }
}

fn deploy_status_name(status: DeployStatus) -> &'static str {
    match status {
        DeployStatus::Idle => "idle",
        DeployStatus::Building => "building",
        DeployStatus::Check => "check",
        DeployStatus::Ready => "ready",
        DeployStatus::Failed => "failed",
    }
}

pub fn inspect(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let state = DeployState::load(ctx, name).with_context(|| format!("inspect unit '{name}'"))?;
    let now = Utc::now();

    // Taken while `unit` is still borrowed; used by the in-progress arms below.
    let readiness_port = readiness_port(&unit);

    const ACTIVE_VERSION: &str = "Active version";
    const LATEST_DEPLOY: &str = "Latest deploy";

    print_field("Unit", unit.kind_display());
    match state.active_version {
        Some(active) => {
            let deployed =
                if state.last_status == DeployStatus::Ready && state.last_version == active {
                    format!(" deployed {}", fmt_ago(&now, &state.updated_at))
                } else {
                    String::new()
                };
            let info = format!("{}{}", console::style(active).green(), deployed);
            print_field(ACTIVE_VERSION, info);
        }
        None => print_field(ACTIVE_VERSION, console::style("-").red()),
    }

    match state.last_status {
        DeployStatus::Idle => {
            print_field(LATEST_DEPLOY, "No deploys yet");
            return Ok(());
        }
        DeployStatus::Building => {
            let info = format!(
                "Version {} in progress · started {}",
                state.last_version,
                fmt_ago(&now, &state.updated_at)
            );
            print_field(LATEST_DEPLOY, info);
            print_field(
                "Deploy",
                match DeployLockGuard::holder(ctx, name) {
                    LockHolder::Held(pid) => format!("running{}", holder_pid(pid)),
                    // The next deploy recovers it; nothing to wait for here.
                    LockHolder::Free => "gone - the next deploy will clear this".to_string(),
                },
            );
            print_field("Build log", ctx.build_log_path(name).display());
        }
        DeployStatus::Check => {
            let info = format!(
                "Version {} starting · since {}",
                state.last_version,
                fmt_ago(&now, &state.updated_at)
            );
            print_field(LATEST_DEPLOY, info);
            print_field("Waiting for", health::criterion(readiness_port));
            print_field(
                "Startup budget",
                match DeployLockGuard::holder(ctx, name) {
                    LockHolder::Held(pid) => format!(
                        "{} · deploy waiting{}",
                        fmt_duration(health::BUDGET),
                        holder_pid(pid)
                    ),
                    // Nobody is waiting on this hand-off any more.
                    LockHolder::Free => format!(
                        "{} · stalled - no deploy holds deploy.lock",
                        fmt_duration(health::BUDGET)
                    ),
                },
            );
            print_field("Runtime log", ctx.runtime_log_path(name).display());
        }
        DeployStatus::Failed => {
            let stage = match &state.failure {
                Some(DeployFailure { stage, .. }) => format!(" during {}", stage),
                None => String::new(),
            };
            let info = format!(
                "Version {} build failed{} · {}",
                state.last_version,
                stage,
                fmt_ago(&now, &state.updated_at)
            );
            print_field(LATEST_DEPLOY, info);

            if let Some(failure) = &state.failure {
                print_failure(ctx, name, failure);
            }
        }
        DeployStatus::Ready => {}
    }

    // Per-unit runtime detail. Run it whenever a version is live.
    if state.active_version.is_none() {
        return Ok(());
    }

    match unit {
        UnitConfig::App(app_config) => {
            println!();
            AppUnit::new(ctx, name, app_config)
                .inspect()
                .with_context(|| format!("inspect unit '{name}'"))?;
        }
        UnitConfig::CloudflareTunnel(tunnel_config) => {
            println!();
            CloudflareTunnelUnit::new(ctx, name, tunnel_config).inspect();
        }
        UnitConfig::DbServer(db_server_config) => {
            println!();
            DbServerUnit::new(ctx, name, db_server_config).inspect();
        }
        UnitConfig::HttpServer(http_config) => {
            println!();
            HttpServerUnit::new(ctx, name, http_config).inspect();
        }
        UnitConfig::Db(_) => {}
        UnitConfig::Domain(_) => {}
    }

    let timers = TimersState::load(ctx, name).with_context(|| format!("inspect unit '{name}'"))?;
    if !timers.timers.is_empty() {
        println!();
        print_field("Timers", "");
        print_timers(now, &timers);
    }

    Ok(())
}

/// Print the last run of each timer (only app units record any).
fn print_timers(now: chrono::DateTime<Utc>, timers: &TimersState) {
    const DT_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

    for (timer, run) in &timers.timers {
        let mut items = Vec::new();

        let next_run = run
            .next_run
            .map(|f| format!("next {}", f.with_timezone(&Local).format(DT_FORMAT)));

        match &run.outcome {
            TimerOutcome::Idle => {
                items.push(format!("{}", console::style("idle").dim()));
                if let Some(next_run) = next_run {
                    items.push(next_run);
                }
            }
            TimerOutcome::Running => {
                items.push(format!(
                    "{} {}",
                    console::style("started").yellow(),
                    fmt_ago(&now, &run.last_run_at)
                ));
            }
            TimerOutcome::Success { duration_ms } => {
                items.push(format!(
                    "{} {}",
                    console::style("success").green(),
                    fmt_ago(&now, &run.last_run_at)
                ));
                items.push(format!(
                    "took {}",
                    fmt_duration(Duration::from_millis(*duration_ms))
                ));
                if let Some(next_run) = next_run {
                    items.push(next_run);
                }
            }
            TimerOutcome::Failed { duration_ms, error } => {
                items.push(format!(
                    "{} {}",
                    console::style("failed").red(),
                    fmt_ago(&now, &run.last_run_at)
                ));
                items.push(format!(
                    "took {}",
                    fmt_duration(Duration::from_millis(*duration_ms))
                ));
                items.push(format!("fails {}", run.consecutive_failures));
                if let Some(at) = &run.last_success_at {
                    items.push(format!("last success {}", fmt_ago(&now, at)));
                }
                if let Some(next_run) = next_run {
                    items.push(next_run);
                }
                items.push(format!("error: {}", error));
            }
            TimerOutcome::Invalid { error } => {
                items.push(format!("{}", console::style("invalid").red()));
                items.push(format!("error: {}", error));
            }
        }

        print_field(timer, items.join(" · "));
    }
}

/// Start a unit's container.
/// Units without a runtime container error out.
pub fn start(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let result = match unit {
        UnitConfig::App(config) => AppUnit::new(ctx, name, config).start(),
        UnitConfig::CloudflareTunnel(config) => {
            CloudflareTunnelUnit::new(ctx, name, config).start()
        }
        UnitConfig::DbServer(config) => DbServerUnit::new(ctx, name, config).start(),
        UnitConfig::HttpServer(config) => HttpServerUnit::new(ctx, name, config).start(),
        UnitConfig::Db(_) | UnitConfig::Domain(_) => {
            bail!("{} unit has no runtime container", unit.kind())
        }
    };

    result.with_context(|| format!("start unit '{name}'"))
}

/// Remove a unit's active deployment from service.
pub fn undeploy(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let (_guard, mut state) =
        DeployState::acquire(ctx, name).with_context(|| format!("acquire unit '{name}'"))?;
    let supervised = state.supervised;

    let outcome = state
        .undeploy()
        .with_context(|| format!("update deploy state for unit '{name}'"))?;

    crate::serve::notify(ctx);

    match outcome.kind.as_deref() {
        Some(AppConfig::KIND) => {
            if let Some(active_version) = outcome.active_version {
                AppUnit::undeploy(ctx, name, active_version);
            }
        }
        Some(DbServerConfig::KIND | CloudflareTunnelConfig::KIND)
            if outcome.active_version.is_some() =>
        {
            undeploy_container(ctx, name, outcome.active_version.unwrap());
        }
        Some(DomainConfig::KIND) => DomainUnit::undeploy(ctx, name),
        _ => {
            if supervised {
                crate::podman::stop_and_remove(name)
                    .with_context(|| format!("stop container '{name}'"))?;
            }
        }
    }

    if outcome.changed {
        eprintln!("{} undeployed '{name}'", success_mark());
    } else {
        eprintln!("{} '{name}' is already undeployed", success_mark());
    }

    Ok(())
}

/// Run one of a unit's timers.
pub fn timer(ctx: &MainContext, name: &UnitName, timer_name: &str) -> Result<()> {
    let (_timer_lock, mut timers) = TimersState::acquire(ctx, name)
        .with_context(|| format!("acquire timers for unit '{name}'"))?;

    let Some(_deploy_log) = DeployLockGuard::try_acquire(ctx, name)
        .with_context(|| format!("acquire unit '{name}'"))?
    else {
        // Record a skip only for a timer already registered in the state.
        if let Some(prev) = timers.timers.get(timer_name).cloned() {
            let msg = format!("unit '{name}' busy: deploy in progress");

            let now = Utc::now();
            let next_run = TimerState::next_occurrence(&prev.schedule, now);
            // The attempt starts now but can't run, so record it as a failed run.
            let skipped = prev.running(now, next_run).failed(Duration::default(), msg);
            timers.set_timer_state(timer_name, skipped);
        }

        return Ok(());
    };

    crate::serve::run_timer(ctx, name, &mut timers, timer_name)
        .with_context(|| format!("run timer '{timer_name}' on unit '{name}'"))
}

/// The port a unit's readiness probe looks for, or `None` for a unit whose
/// readiness is "the container stayed running".
fn readiness_port(unit: &UnitConfig) -> Option<u16> {
    match unit {
        UnitConfig::App(config) => config.runtime.as_ref().and_then(|r| r.port),
        UnitConfig::HttpServer(_) => Some(crate::deploy::http_server::HTTP_PORT),
        _ => None,
    }
}

fn holder_pid(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!(" (pid {pid})"),
        None => String::new(),
    }
}

/// Print the failure line for a `Failed` build and point at the relevant log.
fn print_failure(ctx: &MainContext, name: &UnitName, failure: &DeployFailure) {
    print_field("Error", &failure.error);

    // A runtime failure (the health check) lives in the container's runtime
    // log; every earlier stage is in the build log.
    println!();
    if failure.stage == DeployStage::Startup {
        print_field("Runtime log", ctx.runtime_log_path(name).display());
    } else {
        print_field("Build log", ctx.build_log_path(name).display());
    }
}

/// Open `path` as a deploy input. `-` means stdin.
fn open_input(path: Option<&Path>) -> Result<Option<Box<dyn Read>>, DeployError> {
    let Some(path) = path else {
        return Ok(None);
    };

    let input = if path.as_os_str() == "-" {
        Box::new(io::stdin().lock())
    } else {
        fs::File::open(path)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|err| DeployError::step_prepare("open input", err))?
    };

    Ok(Some(input))
}

fn load_unit(ctx: &MainContext, name: &UnitName) -> Result<UnitConfig> {
    let unit = UnitConfig::load(ctx, name)?;

    unit.validate_references(ctx)
        .with_context(|| format!("unit '{name}': broken reference chain"))?;

    Ok(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_column_distinguishes_down_unknown_and_no_container() {
        let running = "running".to_string();
        assert_eq!(run_column(false, Some(None)), "-");
        assert_eq!(run_column(true, None), "?");
        assert_eq!(run_column(true, Some(None)), "down");
        assert_eq!(run_column(true, Some(Some(&running))), "running");
    }
}
