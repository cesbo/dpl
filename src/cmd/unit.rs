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
    config::ResourceName,
    deploy::{
        DeployError,
        DeployState,
        UnitConfig,
        UnitReport,
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
        build_log_path,
    },
};

pub fn check(ctx: &MainContext, name: &str) -> Result<()> {
    let name = ResourceName::new(name)?;
    let _ = load_unit(ctx, &name)?;
    println!("ok");
    Ok(())
}

pub fn deploy(ctx: &MainContext, name: &ResourceName, path: Option<&Path>) -> Result<()> {
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

    let unit_dir = name.unit_dir(ctx);
    let (_guard, mut state) =
        DeployState::acquire(&unit_dir).with_context(|| format!("acquire unit '{name}'"))?;

    let version = state
        .bump_version()
        .map_err(DeployError::from)
        .with_context(|| format!("deploy unit '{name}'"))?;

    let log_path = build_log_path(&unit_dir);
    let log = match DeployLog::open(&log_path, name.as_str(), version) {
        Ok(log) => log,
        Err(err) => {
            state.set_error();
            return Err(
                anyhow::Error::new(DeployError::unit("open deploy log", err))
                    .context(format!("deploy unit '{name}'")),
            );
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
            DomainUnit::new(ctx, name.as_str(), domain_config).deploy(&mut state)
        }
    };

    match result {
        Ok(()) => {
            log.finish_ok();
            Ok(())
        }
        Err(err) => {
            state.set_error();
            tracing::debug!("deploy failed: {:#}", anyhow::Error::new(err));
            log.finish_err();
            Err(anyhow::Error::new(DeployError::Reported))
        }
    }
}

pub fn inspect(ctx: &MainContext, name: &str) -> Result<()> {
    let name = ResourceName::new(name)?;
    let unit = UnitConfig::load(ctx, &name)?;

    let report = match unit {
        UnitConfig::App(app_config) => AppUnit::new(ctx, &name, app_config)
            .inspect()
            .with_context(|| format!("inspect unit '{name}'"))?,
        other => UnitReport::new(name.as_str(), other.kind()),
    };

    let json = serde_json::to_string_pretty(&report).context("serialize report")?;
    println!("{json}");
    Ok(())
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
            .map_err(|err| DeployError::unit("open input", err))?
    };

    Ok(Some(input))
}

fn load_unit(ctx: &MainContext, name: &ResourceName) -> Result<UnitConfig> {
    let unit = UnitConfig::load(ctx, name)?;

    unit.validate_references(ctx)
        .with_context(|| format!("unit '{name}': broken reference chain"))?;

    Ok(unit)
}
