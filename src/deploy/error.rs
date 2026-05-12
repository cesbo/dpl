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
    State(#[from] DeployStateError),

    #[error("save artifacts")]
    Artifact(#[from] ArtifactError),

    #[error("extract archive")]
    Archive(#[from] ArchiveError),

    #[error("{info}")]
    UnitError {
        info: String,
        #[source]
        source: io::Error,
    },
}
