pub mod archive;
mod auth;
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
    middleware,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    log::init_tracing();

    let config = config::MainConfig::load()?;
    let bind_target = format!("{}:{}", config.server.addr, config.server.port);

    let service = Arc::new(DeployService::default());
    let deploy_routes = deploy_router().route_layer(middleware::from_fn_with_state(
        service.clone(),
        auth::authorize_request,
    ));

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
