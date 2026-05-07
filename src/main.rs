mod archive;
mod auth;
mod cmd;
mod config;
mod context;
mod deploy;
mod error;
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
use tokio::{
    net::TcpListener,
    signal,
};
use tracing::info;

pub use self::context::MainContext;
use self::{
    deploy::{
        DeployService,
        deploy_router,
    },
    error::{
        exit_with_error,
        exit_with_stderr,
    },
};
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

// Temporary variables, will be removed when MainContext will be finished
static CONTEXT: OnceLock<MainContext> = OnceLock::new();

pub fn context() -> &'static MainContext {
    CONTEXT.get().expect("context not initialized")
}

pub fn config() -> &'static MainConfig {
    &context().config
}

fn load_main_context(path: &Path) -> Result<(), context::ContextError> {
    let ctx = MainContext::load(path)?;
    CONTEXT.set(ctx).expect("context already initialized");
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        let result = match cmd {
            Command::Init => cmd::init::run(),
            Command::Secret(args) => match MainContext::load(&cli.config) {
                Ok(ctx) => cmd::secret::run(&ctx, args),
                Err(err) => Err(err.into()),
            },
            Command::Unit(args) => match MainContext::load(&cli.config) {
                Ok(ctx) => cmd::unit::run(&ctx, args),
                Err(err) => Err(err.into()),
            },
        };

        if let Err(err) = result {
            exit_with_stderr(err.as_ref());
        }

        return;
    }

    if let Err(err) = load_main_context(&cli.config) {
        exit_with_stderr(&err);
    }

    if let Err(err) = run().await {
        exit_with_error(err.as_ref());
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    log::init_tracing();

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
