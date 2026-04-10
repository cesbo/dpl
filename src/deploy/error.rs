use std::io;

use axum::{
    Json,
    http::StatusCode,
    response::{
        IntoResponse,
        Response,
    },
};
use thiserror::Error;

use super::state::DeployStateError;
use crate::{
    archive::ArchiveError,
    error::ConfigError,
};

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("{0}")]
    EntityConfig(#[from] ConfigError),
    #[error("entity not found")]
    EntityNotFound,
    #[error("{0}")]
    Status(#[from] DeployStateError),
    #[error("invalid entity name")]
    InvalidEntityName,
    #[error("entity busy")]
    EntityBusy,
    #[error("save artifacts: {0}")]
    ArtifactError(#[from] crate::error::ArtifactError),
    #[error("extract archive: {0}")]
    ArchiveError(#[from] ArchiveError),
    #[error("{info}: {source}")]
    EntityError {
        info: String,
        #[source]
        source: io::Error,
    },
}

impl IntoResponse for DeployError {
    fn into_response(self) -> Response {
        let status = match &self {
            DeployError::EntityNotFound => StatusCode::NOT_FOUND,
            DeployError::EntityBusy => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = serde_json::json!({ "error": self.to_string() });
        (status, Json(body)).into_response()
    }
}
