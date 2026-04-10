use std::{
    io,
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    extract::{
        Path,
        State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{
        get,
        post,
    },
};
use futures_util::TryStreamExt;
use tokio_util::io::StreamReader;

use super::{
    DeployError,
    DeployService,
};

pub fn router() -> Router<Arc<DeployService>> {
    Router::new()
        .route("/{name}", post(deploy_handler))
        .route("/{name}/state", get(state_handler))
}

async fn state_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, DeployError> {
    let state = service.state(&name).await?;
    Ok(state)
}

async fn deploy_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
    body: Body,
) -> Result<impl IntoResponse, DeployError> {
    let stream = body.into_data_stream().map_err(io::Error::other);
    let reader = StreamReader::new(stream);

    let state = service.deploy(&name, reader).await?;
    Ok((StatusCode::ACCEPTED, state))
}
