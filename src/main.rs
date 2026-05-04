mod archive;
mod auth;
mod cmd;
mod config;
mod deploy;
mod log;
mod model;
mod secret;
mod validate;

use std::{
    error::Error,
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        OnceLock,
    },
};

use axum::{
    Router,
    middleware,
};
use clap::{
    Parser,
    Subcommand,
};
use deploy::{
    DeployService,
    deploy_router,
};
use tokio::{
    net::TcpListener,
    signal,
};
use tracing::info;

use crate::model::MainConfig;

#[derive(Parser)]
struct Cli {
    /// Configuration file
    #[arg(
        long = "config",
        short = 'c',
        default_value = "/opt/dpl/config.yaml",
        global = true
    )]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive wizard for initial server setup
    Init,
    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
    /// Manage units
    Unit(cmd::unit::Args),
}

static CONFIG: OnceLock<MainConfig> = OnceLock::new();

pub fn config() -> &'static MainConfig {
    CONFIG.get().expect("config not initialized")
}

pub fn load_main_config(path: &Path) -> Result<(), config::ConfigError> {
    let main_config = config::load_config(path)?;
    CONFIG.set(main_config).expect("config already initialized");
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        let mut src = e.source();
        while let Some(s) = src {
            eprintln!("  caused by: {s}");
            src = s.source();
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Init) => return cmd::init::run(),
        Some(Command::Secret(args)) => return cmd::secret::run(args, &cli.config),
        Some(Command::Unit(args)) => return cmd::unit::run(args, &cli.config),
        None => {}
    }

    log::init_tracing();

    load_main_config(&cli.config)?;

    let service = Arc::new(DeployService::default());
    let deploy_routes = deploy_router().route_layer(middleware::from_fn_with_state(
        service.clone(),
        auth::authorize_request,
    ));

    let bind_target = format!("{}:{}", config().server.addr, config().server.port);
    let listener = TcpListener::bind(&bind_target).await?;
    let addr = listener.local_addr()?;
    let app = Router::new()
        .nest("/deploy", deploy_routes)
        .with_state(service);

    info!(%addr, "server started");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("server stopped");

    Ok(())
}

async fn shutdown_signal() {
    signal::ctrl_c().await.unwrap();
}
