use std::{
    io,
    sync::Arc,
};

use axum::{
    Router,
    body::Body,
    extract::{
        Path,
        Query,
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
use serde::Deserialize;
use tokio_util::io::StreamReader;

use super::{
    DeployError,
    DeployService,
    entity::validate_name,
};

pub fn router() -> Router<Arc<DeployService>> {
    Router::new()
        .route("/{name}", post(deploy_handler))
        .route("/{name}/state", get(state_handler))
        .route("/{name}/log", get(log_handler))
}

async fn state_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, DeployError> {
    if !validate_name(&name) {
        return Err(DeployError::InvalidEntityName);
    }

    let state = service.state(&name).await?;
    Ok(state)
}

async fn deploy_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
    body: Body,
) -> Result<impl IntoResponse, DeployError> {
    if !validate_name(&name) {
        return Err(DeployError::InvalidEntityName);
    }

    let stream = body.into_data_stream().map_err(io::Error::other);
    let reader = StreamReader::new(stream);

    let state = service.deploy(&name, reader).await?;
    Ok((StatusCode::ACCEPTED, state))
}

#[derive(Deserialize)]
struct LogQuery {
    offset: Option<u64>,
}

async fn log_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<impl IntoResponse, DeployError> {
    if !validate_name(&name) {
        return Err(DeployError::InvalidEntityName);
    }

    let offset = query.offset.unwrap_or(0);
    let (data, total_size) = service.build_log(&name, offset).await?;

    Ok((
        [
            ("x-offset", total_size.to_string()),
            ("content-type", "application/octet-stream".to_string()),
        ],
        data,
    ))
}
