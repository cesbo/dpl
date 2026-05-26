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

    /// A deploy failed after its build log was opened: the cause is in that log
    /// and `DeployLog::finish_err` already printed a one-line summary. Carries
    /// no detail so the top level can exit non-zero without repeating anything.
    #[error("deploy failed")]
    Reported,
}
