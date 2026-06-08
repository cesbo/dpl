use std::{
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
        app::AppUnit,
        db::{
            DbServerUnit,
            DbUnit,
        },
        domain::DomainUnit,
        http_server::HttpServerUnit,
    },
    log::{
        DeployConsole,
        fmt_ago,
        fmt_duration,
        print_field,
    },
    state::{
        DeployFailure,
        DeployLockGuard,
        DeployStage,
        DeployState,
        DeployStatus,
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
        .bump_version()
        .map_err(|e| DeployError::step_prepare("bump version", e))
        .with_context(|| format!("deploy unit '{name}'"))?;

    let log_path = ctx.build_log_path(name);
    let _ = crate::log::cri_log::remove_all(&log_path);

    let console = DeployConsole::open(name, version);

    let result: std::result::Result<(), DeployError> = match unit {
        UnitConfig::App(app_config) => {
            // validated Some in the pre-flight match
            AppUnit::new(ctx, name, app_config).deploy(&mut state, version, input.unwrap())
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
            let cause = match err.failure() {
                Some((stage, message)) => {
                    state.set_failed(stage, message.clone());
                    message
                }
                None => String::new(),
            };
            console.finish_err(&cause, &log_path);
            Err(anyhow::Error::new(DeployError::Reported))
        }
    }
}

pub fn inspect(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let state = DeployState::load(ctx, name).with_context(|| format!("inspect unit '{name}'"))?;
    let now = Utc::now();

    const ACTIVE_VERSION: &str = "Active version";
    const LATEST_DEPLOY: &str = "Latest deploy";

    print_field("Unit", unit.kind());
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
        }
        DeployStatus::Check => {
            let info = format!(
                "Version {} starting · since {}",
                state.last_version,
                fmt_ago(&now, &state.updated_at)
            );
            print_field(LATEST_DEPLOY, info);
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
        UnitConfig::DbServer(config) => DbServerUnit::new(ctx, name, config).start(),
        UnitConfig::HttpServer(config) => HttpServerUnit::new(ctx, name, config).start(),
        UnitConfig::Db(_) | UnitConfig::Domain(_) => {
            bail!("{} unit has no runtime container", unit.kind())
        }
    };

    result.with_context(|| format!("start unit '{name}'"))
}

/// Stop a unit's container.
pub fn stop(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let result = match unit {
        UnitConfig::App(config) => AppUnit::new(ctx, name, config).stop(),
        UnitConfig::DbServer(config) => DbServerUnit::new(ctx, name, config).stop(),
        UnitConfig::HttpServer(config) => HttpServerUnit::new(ctx, name, config).stop(),
        UnitConfig::Db(_) | UnitConfig::Domain(_) => {
            bail!("{} unit has no runtime container", unit.kind())
        }
    };

    result.with_context(|| format!("stop unit '{name}'"))
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
