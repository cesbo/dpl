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
    bail,
};
use clap::Subcommand;

use crate::{
    MainContext,
    config::ResourceName,
    deploy::{
        DeployState,
        UnitConfig,
        unit::app::AppUnit,
    },
    log::fmt_elapsed,
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

fn deploy(ctx: &MainContext, name: &ResourceName, path: Option<&Path>) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let UnitConfig::App(app_config) = unit else {
        bail!("deploy not allowed for unit '{name}'");
    };

    let unit_dir = name.unit_dir(ctx);
    let (_guard, state) =
        DeployState::acquire(&unit_dir).with_context(|| format!("acquire unit '{name}'"))?;

    let app = AppUnit::new(ctx, name.as_str(), app_config);

    let (final_state, log) = match path {
        Some(path) => {
            let file = fs::File::open(path).context("open archive")?;
            app.deploy(state, file)
        }
        None => {
            let stdin = io::stdin().lock();
            app.deploy(state, stdin)
        }
    }
    .with_context(|| format!("deploy unit '{name}'"))?;

    let elapsed = fmt_elapsed(log.elapsed());
    let version = final_state.latest_build.version;

    if let Some(err) = &final_state.latest_build.error {
        bail!("deploy failed (version {version}) after {elapsed}: {err}");
    }

    println!("deploy ok (version {version}) in {elapsed}");
    Ok(())
}

pub fn inspect(ctx: &MainContext, name: &str) -> Result<()> {
    let name = ResourceName::new(name)?;
    let unit = UnitConfig::load(ctx, &name)?;

    let UnitConfig::App(_) = unit else {
        bail!("inspect not yet supported for unit '{name}'");
    };

    let unit_dir = name.unit_dir(ctx);
    let state = DeployState::load(&unit_dir).context("load deploy state")?;
    let build = &state.latest_build;
    let status = format!("{:?}", build.status).to_lowercase();

    println!("version: {}", build.version);
    println!("status:  {status}");
    if let Some(active) = state.active_version {
        println!("active:  {active}");
    }
    if let Some(err) = &build.error {
        println!("error:   {err}");
    }

    Ok(())
}

fn load_unit(ctx: &MainContext, name: &ResourceName) -> Result<UnitConfig> {
    let unit = UnitConfig::load(ctx, name)?;

    unit.validate_references(ctx)
        .with_context(|| format!("unit '{name}': broken reference chain"))?;

    Ok(unit)
}
