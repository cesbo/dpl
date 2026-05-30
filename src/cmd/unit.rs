use std::{
    fs,
    io::{
        self,
        Read,
    },
    path::Path,
};

use anyhow::{
    Context,
    Result,
    bail,
};

use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        DeployState,
        DeployStatus,
        Stage,
        UnitConfig,
        unit::{
            app::AppUnit,
            db::{
                DbServerUnit,
                DbUnit,
            },
            domain::DomainUnit,
            http_server::HttpServerUnit,
        },
    },
    log::{
        DeployLog,
        error_mark,
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
            state.set_error(&err);
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
            state.set_error(&err);
            tracing::debug!("deploy failed: {:#}", anyhow::Error::new(err));
            log.finish_err();
            Err(anyhow::Error::new(DeployError::Reported))
        }
    }
}

pub fn inspect(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let state = DeployState::load(ctx, name).with_context(|| format!("inspect unit '{name}'"))?;

    println!("Unit:    {name} ({})", unit.kind());
    match state.active_version {
        Some(active) => println!("Active:  {}", console::style(active).green()),
        None => println!("Active:  {}", console::style("none").red()),
    }
    println!();

    let build = &state.latest_build;
    match build.status {
        // Nothing deployed (or a fresh unit with no `.state.json`); the build
        // version is meaningless here, so don't print it.
        DeployStatus::Idle => {
            println!("no deploys yet");
            return Ok(());
        }
        DeployStatus::Building => println!("build #{} in progress", build.version),
        DeployStatus::Failed => print_failure(
            ctx,
            name,
            build.version,
            build.stage,
            build.error.as_deref(),
        ),
        DeployStatus::Ready => {}
    }

    // Per-unit runtime detail. Run it whenever a version is live.
    if state.active_version.is_none() {
        return Ok(());
    }

    match unit {
        UnitConfig::App(app_config) => {
            AppUnit::new(ctx, name, app_config)
                .inspect()
                .with_context(|| format!("inspect unit '{name}'"))?;
        }
        UnitConfig::Db(_) => {}
        UnitConfig::DbServer(_) => {}
        UnitConfig::Domain(_) => {}
        UnitConfig::HttpServer(_) => {}
    }

    Ok(())
}

/// Print the failure line for a `Failed` build and point at the relevant log.
fn print_failure(
    ctx: &MainContext,
    name: &UnitName,
    version: u32,
    stage: Option<Stage>,
    error: Option<&str>,
) {
    match stage {
        Some(stage) => println!("{} build #{version} failed ({stage})", error_mark()),
        None => println!("{} build #{version} failed", error_mark()),
    }

    if let Some(error) = error {
        println!("  {error}");
    }

    // A runtime failure (the health check) lives in the container's own log;
    // every earlier stage is in the build log.
    if stage == Some(Stage::Runtime) {
        let runtime_log = format!("/var/log/podman/{}.log", name.scoped_unit_name());
        println!("  runtime log: {}", runtime_log);
    } else {
        let build_log = ctx.build_log_path(name);
        println!("  build log: {}", build_log.display());
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
