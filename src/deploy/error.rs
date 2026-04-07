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

use super::EntityType;
use crate::error::ConfigError;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("config error: {0}")]
    Config(#[from] ConfigError),
    #[error("entity not found")]
    EntityNotFound,
    #[error("version error: {0}")]
    VersionError(io::Error),
    #[error("status error: {0}")]
    StatusError(io::Error),
    #[error("unexpected entity type, got {0}")]
    InvalidEntityType(EntityType),
    #[error("entity busy")]
    EntityBusy,
    #[error("failed to save artifacts: {0}")]
    ArtifactError(#[from] crate::error::ArtifactError),
    #[error("failed to extract archive: {0}")]
    ArchiveError(#[from] crate::error::ArchiveError),
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
