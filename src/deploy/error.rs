use std::io;

use thiserror::Error;

use super::EntityType;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("config error: {0}")]
    Config(#[from] crate::error::ConfigError),
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
    #[error("{info}: {source}")]
    EntityError {
        info: String,
        #[source]
        source: io::Error,
    },
}
