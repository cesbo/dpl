use std::io;

use thiserror::Error;

use super::{
    artifacts::ArtifactError,
    state::DeployStateError,
};
use crate::archive::ArchiveError;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error(transparent)]
    Status(#[from] DeployStateError),

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
