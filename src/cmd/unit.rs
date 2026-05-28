use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use anyhow::{
    Context,
    Result,
};
use clap::Subcommand;

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
    log::DeployLog,
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Trigger a deploy from a tar.gz archive (stdin if path is omitted)
    Deploy {
        /// Unit name
        name: String,
        /// Path to tar.gz archive; reads from stdin if omitted
        path: Option<PathBuf>,
    },
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Deploy { name, path } => {
            let name = ResourceName::new(name)?;
            deploy(ctx, &name, path.as_deref())
        }
    }
}

pub fn check(ctx: &MainContext, name: &str) -> Result<()> {
    let name = ResourceName::new(name)?;
    let _ = load_unit(ctx, &name)?;
    println!("ok");
    Ok(())
}

pub(crate) fn deploy(ctx: &MainContext, name: &ResourceName, path: Option<&Path>) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    match (&unit, path) {
        (UnitConfig::App(_), _) => {}
        (UnitConfig::Db(_), _) => {}
        (UnitConfig::DbServer(_), _) => {}
        (UnitConfig::HttpServer(_), _) => {}
        (UnitConfig::Domain(_), _) => {}
    }

    let unit_dir = name.unit_dir(ctx);
    let (_guard, mut state) =
        DeployState::acquire(&unit_dir).with_context(|| format!("acquire unit '{name}'"))?;

    let prev_build = state.latest_build.version;
    if prev_build != 0 {
        let prev_log = build_log_path(&unit_dir, prev_build);
        if let Err(err) = fs::remove_file(&prev_log)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("remove previous build log {}: {err}", prev_log.display());
        }
    }

    let version = state
        .bump_version()
        .map_err(DeployError::from)
        .with_context(|| format!("deploy unit '{name}'"))?;

    let log_path = build_log_path(&unit_dir, version);
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
            let app = AppUnit::new(ctx, name, app_config);
            match path {
                Some(path) => match fs::File::open(path) {
                    Ok(file) => app.deploy(&mut state, version, file),
                    Err(err) => Err(DeployError::unit("open archive", err)),
                },
                None => app.deploy(&mut state, version, io::stdin().lock()),
            }
        }
        UnitConfig::Db(db_config) => DbUnit::new(ctx, name, db_config).deploy(&mut state),
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

fn build_log_path(unit_dir: &Path, version: u32) -> PathBuf {
    unit_dir.join("log").join(format!("build-{version}.log"))
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

fn load_unit(ctx: &MainContext, name: &ResourceName) -> Result<UnitConfig> {
    let unit = UnitConfig::load(ctx, name)?;

    unit.validate_references(ctx)
        .with_context(|| format!("unit '{name}': broken reference chain"))?;

    Ok(unit)
}
