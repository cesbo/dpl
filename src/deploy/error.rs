use thiserror::Error;

use crate::state::DeployStage;

#[derive(Debug, Error)]
pub enum DeployError {
    /// A deploy step failed. `stage` says which phase it happened in (drives the
    /// log hint and the `dpl inspect` readout), `info` names the specific step.
    #[error("{info}")]
    Step {
        stage: DeployStage,
        info: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A deploy failed: the cause is in the build log (when one was written) and
    /// `DeployConsole::finish_err` already printed a one-line summary. Carries no
    /// detail so the top level can exit non-zero without repeating anything.
    #[error("deploy failed")]
    Reported,
}

impl DeployError {
    pub fn step_prepare(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: DeployStage::Prepare,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_build(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: DeployStage::Build,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_install(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: DeployStage::Install,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_startup(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: DeployStage::Startup,
            info: info.into(),
            source: source.into(),
        }
    }

    /// Stage and flattened cause (the failing step plus its source chain,
    /// joined by `": "`) for a `Step` error. `None` for `Reported`, which
    /// carries no detail.
    pub fn failure(&self) -> Option<(DeployStage, String)> {
        let DeployError::Step {
            stage,
            info,
            source,
        } = self
        else {
            return None;
        };

        let mut messages = vec![info.to_owned()];
        let mut source: &(dyn std::error::Error + 'static) = source.as_ref();
        loop {
            messages.push(source.to_string());
            match source.source() {
                Some(next) => source = next,
                None => break,
            }
        }

        Some((*stage, messages.join(": ")))
    }
}

/// A container-lifecycle failure from internal runtime commands like
/// `dpl start`. Unlike
/// [`DeployError`], these never feed `DeployState` (no version, no build log,
/// no recorded stage), so the error only carries the failing step and its cause
/// for the CLI to print.
#[derive(Debug, Error)]
#[error("{info}")]
pub struct RunError {
    info: String,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl RunError {
    pub fn new(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        RunError {
            info: info.into(),
            source: source.into(),
        }
    }
}
