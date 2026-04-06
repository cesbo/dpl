use std::{
    io,
    sync::Arc,
};

use axum::{
    Json,
    body::Body,
    extract::{
        Path,
        State,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::TryStreamExt;
use tokio_util::io::StreamReader;

use super::{
    DeployError,
    DeployService,
};

pub async fn deploy_handler(
    State(service): State<Arc<DeployService>>,
    Path(name): Path<String>,
    body: Body,
) -> Result<impl IntoResponse, DeployError> {
    let stream = body.into_data_stream().map_err(io::Error::other);
    let reader = StreamReader::new(stream);

    let version = service.deploy(&name, reader).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "name": &name, "version": version })),
    ))
}
