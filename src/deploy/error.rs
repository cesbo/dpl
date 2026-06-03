use thiserror::Error;

use super::state::Stage;

#[derive(Debug, Error)]
pub enum DeployError {
    /// A deploy step failed. `stage` says which phase it happened in (drives the
    /// log hint and the `dpl inspect` readout), `info` names the specific step.
    #[error("{info}")]
    Step {
        stage: Stage,
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
    pub fn step_prepare(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Prepare,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_build(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Build,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_install(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Install,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_startup(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Startup,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_start(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Start,
            info: info.into(),
            source: source.into(),
        }
    }

    pub fn step_stop(
        info: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        DeployError::Step {
            stage: Stage::Stop,
            info: info.into(),
            source: source.into(),
        }
    }

    /// Stage and flattened cause (the failing step plus its source chain,
    /// joined by `": "`) for a `Step` error. `None` for `Reported`, which
    /// carries no detail.
    pub fn failure(&self) -> Option<(Stage, String)> {
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
