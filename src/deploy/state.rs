use std::{
    fs,
    io,
    path::Path,
};

use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

const STATE_FILE_NAME: &str = "state.yaml";

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    #[default]
    Idle,
    Building,
    Ready,
    Failed,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    pub version: u32,
    pub status: DeployStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    /// Currently running version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<u32>,
    /// Version for last attempt
    pub latest_build: BuildResult,
}

#[derive(Debug, Error)]
pub enum DeployStateError {
    #[error("read state file")]
    Read(#[source] io::Error),
    #[error("write state file")]
    Write(#[source] io::Error),
    #[error("unit version overflow")]
    VersionOverflow,
    #[error("unit has no active version")]
    NoActiveVersion,
}

impl DeployState {
    pub fn load(unit_dir: &Path) -> Result<Self, DeployStateError> {
        let path = unit_dir.join(STATE_FILE_NAME);

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState::default());
            }
            Err(err) => return Err(DeployStateError::Read(err)),
        };

        serde_yaml::from_str(&content)
            .map_err(|err| DeployStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err)))
    }

    pub fn save(&self, unit_dir: &Path) -> Result<(), DeployStateError> {
        let path = unit_dir.join(STATE_FILE_NAME);
        let content = serde_yaml::to_string(self).map_err(|err| {
            DeployStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;
        fs::write(path, content).map_err(DeployStateError::Write)
    }

    /// Checked version addition.
    /// Sets the latest build status to `Building` and clears previous error.
    pub fn bump_version(&mut self) -> Result<u32, DeployStateError> {
        let next = self
            .latest_build
            .version
            .checked_add(1)
            .ok_or(DeployStateError::VersionOverflow)?;
        self.latest_build.version = next;
        self.latest_build.status = DeployStatus::Building;
        self.latest_build.error = None;
        Ok(next)
    }

    pub fn set_error<T: ToString>(&mut self, error: T) {
        self.latest_build.status = DeployStatus::Failed;
        self.latest_build.error = Some(error.to_string());
    }

    pub fn set_ready(&mut self) {
        self.latest_build.status = DeployStatus::Ready;
        self.latest_build.error = None;
    }
}
