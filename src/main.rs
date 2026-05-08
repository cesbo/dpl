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
    path::PathBuf,
    sync::Arc,
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
    net::{
        TcpListener,
        ToSocketAddrs,
    },
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

#[derive(Parser)]
struct Cli {
    /// Base directory (contains config.yaml, .tokens/, .secrets/, unit dirs)
    #[arg(long = "base", default_value = "/opt/dpl", global = true)]
    base: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive wizard for initial server setup
    Init,
    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
    /// Manage HTTP API access tokens
    Token(cmd::token::Args),
    /// Manage units
    Unit(cmd::unit::Args),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        let result = match cmd {
            Command::Init => cmd::init::run(),
            Command::Secret(args) => match MainContext::load(&cli.base) {
                Ok(ctx) => cmd::secret::run(&ctx, args),
                Err(err) => Err(err.into()),
            },
            Command::Token(args) => match MainContext::load(&cli.base) {
                Ok(ctx) => cmd::token::run(&ctx, args),
                Err(err) => Err(err.into()),
            },
            Command::Unit(args) => match MainContext::load(&cli.base) {
                Ok(ctx) => cmd::unit::run(&ctx, args),
                Err(err) => Err(err.into()),
            },
        };

        if let Err(err) = result {
            exit_with_stderr(err.as_ref());
        }

        return;
    }

    let ctx = match MainContext::load(&cli.base) {
        Ok(v) => v,
        Err(err) => exit_with_stderr(&err),
    };

    let service = DeployService::new(cli.base);
    let addr = format!("{}:{}", ctx.config.server.addr, ctx.config.server.port);

    if let Err(err) = run(service, addr).await {
        exit_with_error(err.as_ref());
    }
}

async fn run<A>(service: DeployService, addr: A) -> Result<(), Box<dyn Error>>
where
    A: ToSocketAddrs,
{
    log::init_tracing();

    let service = Arc::new(service);
    let deploy_routes = deploy_router().route_layer(middleware::from_fn_with_state(
        service.clone(),
        auth::authorize_request,
    ));

    let listener = TcpListener::bind(addr).await?;
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
