use std::{
    io,
    path::PathBuf,
};

use thiserror::Error;

use crate::deploy::EntityType;

#[derive(Debug, Error)]
pub enum AppEntityError {
    #[error("config error: {0}")]
    ConfigError(#[from] crate::error::ConfigError),
    #[error("port error: {0}")]
    PortError(io::Error),
    #[error("version error: {0}")]
    VersionError(io::Error),
    #[error("status error: {0}")]
    StatusError(io::Error),
    #[error("expected app entity type, got {0}")]
    InvalidEntityType(EntityType),
    #[error("app build in progress")]
    BuildInProgress,
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("template render error: {0}")]
    Render(#[from] minijinja::Error),
    #[error("failed to write artifact {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
