use std::io;

use thiserror::Error;

use super::{
    artifacts::ArtifactError,
    state::DeployStateError,
    unit::UnitConfigError,
};
use crate::archive::ArchiveError;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error(transparent)]
    Unit(#[from] UnitConfigError),

    #[error(transparent)]
    Status(#[from] DeployStateError),

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
