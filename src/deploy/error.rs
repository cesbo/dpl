use std::io;

use thiserror::Error;

use super::{
    artifacts::ArtifactError,
    state::DeployStateError,
};
use crate::{
    archive::ArchiveError,
    config::ConfigError,
    context::ContextError,
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
