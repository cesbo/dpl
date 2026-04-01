use std::io;

use thiserror::Error;

use super::{
    EntityType,
    app_entity::AppEntityError,
};

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("config error: {0}")]
    Config(#[from] crate::error::ConfigError),
    #[error("version error: {0}")]
    VersionError(io::Error),
    #[error("status error: {0}")]
    StatusError(io::Error),
    #[error("expected app entity type, got {0}")]
    InvalidEntityType(EntityType),
    #[error("entity busy")]
    EntityBusy,
    #[error("failed to create deploy directory: {0}")]
    DeployDirectoryError(io::Error),
    #[error("app entity error: {0}")]
    AppEntity(#[from] AppEntityError),
}
