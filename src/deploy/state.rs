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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    Idle,
    Building,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    pub version: u32,
    pub status: DeployStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Error)]
pub enum DeployStateError {
    #[error("read state file: {0}")]
    Read(io::Error),
    #[error("write state file: {0}")]
    Write(io::Error),
    #[error("entity version overflow")]
    VersionOverflow,
}

impl DeployState {
    pub fn load(dir: &Path) -> Result<Self, DeployStateError> {
        let path = dir.join(STATE_FILE_NAME);

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState {
                    version: 0,
                    status: DeployStatus::Idle,
                    last_error: None,
                });
            }
            Err(err) => return Err(DeployStateError::Read(err)),
        };

        serde_yaml::from_str(&content)
            .map_err(|err| DeployStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err)))
    }

    pub fn save(&self, dir: &Path) -> Result<(), DeployStateError> {
        let path = dir.join(STATE_FILE_NAME);
        let content = serde_yaml::to_string(self).map_err(|err| {
            DeployStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;
        fs::write(path, content).map_err(DeployStateError::Write)
    }

    pub fn bump_version(&mut self) -> Result<u32, DeployStateError> {
        let next = self
            .version
            .checked_add(1)
            .ok_or(DeployStateError::VersionOverflow)?;
        self.version = next;
        Ok(next)
    }
}
