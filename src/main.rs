pub mod archive;
mod config;
mod deploy;
pub mod error;
mod log;

use std::{
    error::Error,
    sync::Arc,
};

use axum::{
    Router,
    routing::{
        get,
        post,
    },
};
use tokio::{
    net::TcpListener,
    signal,
};
use tracing::info;

use deploy::{
    DeployService,
    deploy_handler,
    status_handler,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    log::init_tracing();

    let config = config::MainConfig::load()?;
    let bind_target = format!("{}:{}", config.server.addr, config.server.port);

    let service = Arc::new(DeployService::new());

    let listener = TcpListener::bind(&bind_target).await?;
    let local_addr = listener.local_addr()?;
    let app = Router::new()
        .route("/hello", get(hello_world))
        .route("/deploy/{name}", post(deploy_handler))
        .route("/deploy/{name}/status", get(status_handler))
        .with_state(service);

    info!(%local_addr, "server started");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("server stopped");

    Ok(())
}

async fn hello_world() -> &'static str {
    "hello world"
}

async fn shutdown_signal() {
    signal::ctrl_c().await.unwrap();
}
