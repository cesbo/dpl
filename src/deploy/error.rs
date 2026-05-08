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

use super::{
    artifacts::ArtifactError,
    state::DeployStateError,
};
use crate::{
    archive::ArchiveError,
    config::ConfigError,
    context::ContextError,
    error::format_error_chain,
};

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("load main context")]
    MainContext(#[from] ContextError),

    #[error(transparent)]
    UnitConfig(#[from] ConfigError),

    #[error("unit not found")]
    UnitNotFound,

    #[error(transparent)]
    Status(#[from] DeployStateError),

    #[error("invalid unit name")]
    InvalidUnitName,

    #[error("unit busy")]
    UnitBusy,

    #[error("not allowed")]
    UnitNotAllowed,

    #[error("save artifacts")]
    ArtifactError(#[from] ArtifactError),

    #[error("extract archive")]
    ArchiveError(#[from] ArchiveError),

    #[error("{info}")]
    UnitError {
        info: String,
        #[source]
        source: io::Error,
    },
}

impl IntoResponse for DeployError {
    fn into_response(self) -> Response {
        let status = match &self {
            DeployError::UnitNotFound => StatusCode::NOT_FOUND,
            DeployError::UnitBusy => StatusCode::CONFLICT,
            DeployError::UnitNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = serde_json::json!({ "error": format_error_chain(&self) });
        (status, Json(body)).into_response()
    }
}
