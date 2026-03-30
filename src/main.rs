mod config;
mod log;

use std::error::Error;

use axum::{
    Router,
    routing::get,
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

    let listener = TcpListener::bind(&bind_target).await?;
    let local_addr = listener.local_addr()?;
    let app = Router::new().route("/hello", get(hello_world));

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
