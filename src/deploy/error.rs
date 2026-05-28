use thiserror::Error;

use super::{
    artifacts::ArtifactError,
    state::DeployStateError,
};

#[derive(Debug, Error)]
pub enum DeployError {
    #[error(transparent)]
    State(#[from] DeployStateError),

    #[error("save artifacts")]
    Artifact(#[from] ArtifactError),

    #[error("{info}")]
    UnitError {
        info: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A deploy failed after its build log was opened: the cause is in that log
    /// and `DeployLog::finish_err` already printed a one-line summary. Carries
    /// no detail so the top level can exit non-zero without repeating anything.
    #[error("deploy failed")]
    Reported,
}

impl DeployError {
    /// Wrap any unit-level failure with a human-readable `info` summary.
    pub fn unit(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::UnitError {
            info: info.into(),
            source: source.into(),
        }
    }
}
