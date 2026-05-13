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
    /// Validate a unit's config and secret references
    Check {
        /// Unit name
        name: String,
    },
    /// Trigger a deploy from a tar.gz archive (stdin if path is omitted)
    Deploy {
        /// Unit name
        name: String,
        /// Path to tar.gz archive; reads from stdin if omitted
        path: Option<PathBuf>,
    },
    /// Show deploy state for a unit
    State {
        /// Unit name
        name: String,
    },
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Check { name } => check(ctx, &name),
        Cmd::Deploy { name, path } => deploy(ctx, &name, path.as_deref()),
        Cmd::State { name } => state(ctx, &name),
    }
}

fn check(ctx: &MainContext, name: &str) -> Result<()> {
    let _ = load_unit(ctx, name, true)?;
    println!("ok");
    Ok(())
}

fn deploy(ctx: &MainContext, name: &str, path: Option<&Path>) -> Result<()> {
    let unit = load_unit(ctx, name, true)?;

    let UnitConfig::App(app_config) = unit else {
        bail!("deploy not allowed for unit '{name}'");
    };

    let unit_dir = ctx.base().join(name);
    let (_guard, state) =
        DeployState::acquire(&unit_dir).with_context(|| format!("acquire unit '{name}'"))?;

    let app = AppUnit::new(ctx, name, app_config);

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

fn state(ctx: &MainContext, name: &str) -> Result<()> {
    let _unit = load_unit(ctx, name, false)?;

    let unit_dir = ctx.base().join(name);
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

fn load_unit(ctx: &MainContext, name: &str, validate: bool) -> Result<UnitConfig> {
    super::check_unit_name(ctx, name, true)?;
    let unit = UnitConfig::load(ctx, name).context("load unit '{name}'")?;

    if validate {
        unit.validate_references(ctx)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("invalid reference in unit '{name}'"))?;
    }

    Ok(unit)
}
