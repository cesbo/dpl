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
use chrono::Utc;

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
        DeployLog,
        fmt_ago,
        fmt_duration,
        print_field,
    },
    state::{
        DeployFailure,
        DeployStage,
        DeployState,
        DeployStateGuard,
        DeployStatus,
    },
    timers::{
        TimerState,
        TimerStatus,
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
    let log = match DeployLog::open(&log_path, name, version) {
        Ok(log) => log,
        Err(err) => {
            let err = DeployError::step_prepare("open deploy log", err);
            if let Some((stage, message)) = err.failure() {
                state.set_failed(stage, message);
            }
            return Err(anyhow::Error::new(err).context(format!("deploy unit '{name}'")));
        }
    };
    let _default = log.set_default();

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
            log.finish_ok();
            Ok(())
        }
        Err(err) => {
            // Record the failing stage and cause before converting to Reported.
            if let Some((stage, message)) = err.failure() {
                state.set_failed(stage, message);
            }
            tracing::debug!("deploy failed: {:#}", anyhow::Error::new(err));
            log.finish_err();
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
                    format!(" deployed {}", fmt_ago(now, state.updated_at))
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
                fmt_ago(now, state.updated_at)
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
                fmt_ago(now, state.updated_at)
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
    for (timer, run) in &timers.timers {
        let mut items = Vec::new();

        match run.status {
            TimerStatus::Running => {
                items.push(format!(
                    "{} {}",
                    console::style("started").yellow(),
                    fmt_ago(now, run.last_run_at)
                ));
            }
            TimerStatus::Success => {
                items.push(format!(
                    "{} {}",
                    console::style("success").green(),
                    fmt_ago(now, run.last_run_at)
                ));
                if let Some(v) = run.duration_ms {
                    let v = Duration::from_millis(v);
                    let v = fmt_duration(v);
                    items.push(format!("in {v}"));
                }
            }
            TimerStatus::Failed => {
                items.push(format!(
                    "{} {}",
                    console::style("failed").red(),
                    fmt_ago(now, run.last_run_at)
                ));
                if let Some(v) = run.duration_ms {
                    let v = Duration::from_millis(v);
                    let v = fmt_duration(v);
                    items.push(format!("in {v}"));
                }
                if let Some(at) = run.last_success_at {
                    let v = fmt_ago(now, at);
                    items.push(format!("last success {v}"))
                }
                if let Some(failure) = &run.failure {
                    items.push(format!("fails {}", failure.count));
                    items.push(failure.error.clone());
                }
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
    let unit = load_unit(ctx, name)?;

    let UnitConfig::App(config) = unit else {
        bail!("{} unit has no timers", unit.kind());
    };

    let (_timer_lock, mut timers) = TimersState::acquire(ctx, name)
        .with_context(|| format!("acquire timers for unit '{name}'"))?;

    let Some(_deploy_log) = DeployStateGuard::try_acquire(ctx, name)
        .with_context(|| format!("acquire unit '{name}'"))?
    else {
        let prev = timers.timers.get(timer_name).cloned();
        let msg = format!("unit '{name}' busy: deploy in progress");
        tracing::info!("{msg}; recording skip for timer '{timer_name}'");
        timers.set_timer_state(
            timer_name,
            TimerState::failed(prev.as_ref(), Utc::now(), Duration::default(), msg),
        );

        return Ok(());
    };

    AppUnit::new(ctx, name, config)
        .run_timer(&mut timers, timer_name)
        .with_context(|| format!("run timer '{timer_name}' on unit '{name}'"))
}

/// Print the failure line for a `Failed` build and point at the relevant log.
fn print_failure(ctx: &MainContext, name: &UnitName, failure: &DeployFailure) {
    print_field("Error", &failure.error);

    // A runtime failure (the health check) lives in the container's own log;
    // every earlier stage is in the build log.
    println!();
    if failure.stage == DeployStage::Startup {
        let runtime_log_name = format!("{}.log", name.scoped_unit_name());
        let runtime_log_path = Path::new(crate::podman::PODMAN_LOG_DIR).join(runtime_log_name);
        print_field("Runtime log", runtime_log_path.display());
    } else {
        let build_log_path = ctx.build_log_path(name);
        print_field("Build log", build_log_path.display());
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
