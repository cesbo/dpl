mod archive;
mod artifacts;
mod auth;
mod config;
mod deploy;
mod log;
mod model;

use std::{
    error::Error,
    path::PathBuf,
    sync::{
        Arc,
        OnceLock,
    },
};

use axum::{
    Router,
    middleware,
};
use clap::Parser;
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
    #[arg(long = "version", short = None)]
    version: bool,
    #[arg(long = "config", short = 'c', default_value = "/opt/dpl/config.yaml")]
    config: PathBuf,
}

static CONFIG: OnceLock<MainConfig> = OnceLock::new();

pub fn config() -> &'static MainConfig {
    CONFIG.get().expect("config not initialized")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    if cli.version {
        println!("dpl {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    log::init_tracing();

    CONFIG
        .set(config::load_config::<MainConfig>(&cli.config)?)
        .unwrap();

    let service = Arc::new(DeployService::default());
    let deploy_routes = deploy_router().route_layer(middleware::from_fn_with_state(
        service.clone(),
        auth::authorize_request,
    ));

    let bind_target = format!("{}:{}", config().server.addr, config().server.port);
    let listener = TcpListener::bind(&bind_target).await?;
    let local_addr = listener.local_addr()?;
    let app = Router::new()
        .nest("/deploy", deploy_routes)
        .with_state(service);

    info!(%local_addr, "server started");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("server stopped");

    Ok(())
}

async fn shutdown_signal() {
    signal::ctrl_c().await.unwrap();
}
