use std::{
    fs::{
        File,
        OpenOptions,
        read_to_string,
    },
    io::{
        self,
        Write,
    },
    path::Path,
};

use fs4::fs_std::FileExt;
use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

const LOCK_FILE_NAME: &str = ".deploy.lock";
const STATE_FILE_NAME: &str = "state.json";

#[derive(Debug, Error)]
pub enum DeployStateError {
    #[error("lock unit")]
    Lock(#[source] io::Error),

    #[error("unit busy")]
    Busy,

    #[error("read state file")]
    Read(#[source] io::Error),

    #[error("write state file")]
    Write(#[source] io::Error),

    #[error("unit version overflow")]
    VersionOverflow,

    #[error("unit has no active version")]
    NoActiveVersion,
}

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
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    /// Currently running version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<u32>,
    /// Version for last attempt
    pub latest_build: BuildResult,
}

impl DeployState {
    /// Acquire the unit-level busy lock and load its state.
    pub fn acquire(unit_dir: &Path) -> Result<(DeployStateGuard, DeployState), DeployStateError> {
        let guard = DeployStateGuard::lock(unit_dir)?;
        let state = DeployState::load(unit_dir)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployStateError::Busy);
        }

        Ok((guard, state))
    }

    pub fn load(unit_dir: &Path) -> Result<Self, DeployStateError> {
        let path = unit_dir.join(STATE_FILE_NAME);

        let content = match read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState::default());
            }
            Err(err) => return Err(DeployStateError::Read(err)),
        };

        serde_json::from_str(&content)
            .map_err(|err| DeployStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err)))
    }

    pub fn save(&self, unit_dir: &Path) -> Result<(), DeployStateError> {
        let path = unit_dir.join(STATE_FILE_NAME);
        let content = serde_json::to_string_pretty(self).map_err(|err| {
            DeployStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        let mut tmp = tempfile::NamedTempFile::new_in(unit_dir).map_err(DeployStateError::Write)?;
        tmp.write_all(content.as_bytes())
            .map_err(DeployStateError::Write)?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(DeployStateError::Write)?;
        tmp.persist(&path)
            .map_err(|err| DeployStateError::Write(err.error))?;
        Ok(())
    }

    pub fn get_active_version(unit_dir: &Path) -> Result<u32, DeployStateError> {
        let state = Self::load(unit_dir)?;
        state
            .active_version
            .ok_or(DeployStateError::NoActiveVersion)
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
        Ok(next)
    }

    pub fn set_error(&mut self) {
        self.latest_build.status = DeployStatus::Failed;
    }

    pub fn set_ready(&mut self) {
        self.latest_build.status = DeployStatus::Ready;
    }
}

/// Holds an OS-level exclusive `flock` on `{unit_dir}/.deploy.lock` for the
/// lifetime of the value. The kernel releases the lock when the file
/// descriptor is closed, including on process crash.
pub struct DeployStateGuard(File);

impl DeployStateGuard {
    fn lock(unit_dir: &Path) -> Result<Self, DeployStateError> {
        let path = unit_dir.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(DeployStateError::Lock)?;

        match file.try_lock_exclusive() {
            Ok(true) => Ok(DeployStateGuard(file)),
            Ok(false) => Err(DeployStateError::Busy),
            Err(err) => Err(DeployStateError::Lock(err)),
        }
    }
}
