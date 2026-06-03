use std::{
    fmt,
    fs::{
        File,
        OpenOptions,
        read_to_string,
    },
    io::{
        self,
        Write,
    },
    path::PathBuf,
};

use chrono::{
    DateTime,
    Utc,
};
use fs4::fs_std::FileExt;
use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

use crate::{
    MainContext,
    config::UnitName,
};

#[derive(Debug, Error)]
pub enum UnitStateError {
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

/// Deploy phase for DeployError.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Loading/validating config, resolving secrets, acquiring the lock,
    /// bumping the version, staging inputs, rendering build artifacts.
    Prepare,
    /// Building the container image from the staged sources.
    Build,
    /// Installing/starting systemd services, exporting files, creating or
    /// restoring databases, writing rendered config into volumes.
    Install,
    /// Waiting for the deployed unit to become healthy (its port/ping check).
    Startup,
    /// Bringing the unit's container up (`dpl start`, the service `ExecStart`):
    /// prerequisites, db-dependency waits, and the foreground `podman run`.
    Start,
    /// Tearing the unit's container down (`dpl stop`, the service `ExecStop`).
    Stop,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Stage::Prepare => "prepare",
            Stage::Build => "build",
            Stage::Install => "install",
            Stage::Startup => "startup",
            Stage::Start => "start",
            Stage::Stop => "stop",
        };
        f.write_str(name)
    }
}

/// Where and why the latest build failed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildFailure {
    /// Deploy stage the attempt failed in.
    pub stage: Stage,

    /// Human-readable cause (the failing step plus its source chain).
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    #[serde(skip)]
    path: PathBuf,

    /// Timestamp of the most recent deploy attempt event.
    pub updated_at: DateTime<Utc>,

    /// Currently running version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<u32>,

    /// Increments on each deploy attempt
    pub last_version: u32,

    /// Version for last attempt
    pub last_status: DeployStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<BuildFailure>,
}

impl DeployState {
    /// Acquire the unit-level busy lock and load its state.
    pub fn acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<(DeployStateGuard, DeployState), UnitStateError> {
        let guard = DeployStateGuard::lock(ctx, name)?;
        let state = DeployState::load(ctx, name)?;
        if state.last_status == DeployStatus::Building {
            return Err(UnitStateError::Busy);
        }

        Ok((guard, state))
    }

    pub fn load(ctx: &MainContext, name: &UnitName) -> Result<Self, UnitStateError> {
        let path = ctx.state_path(name);

        let content = match read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState {
                    path,
                    updated_at: Utc::now(),
                    active_version: None,
                    last_version: 0,
                    last_status: Default::default(),
                    failure: None,
                });
            }
            Err(err) => return Err(UnitStateError::Read(err)),
        };

        let mut state: DeployState = serde_json::from_str(&content)
            .map_err(|err| UnitStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err)))?;

        state.path = path;

        Ok(state)
    }

    fn save(&self) -> Result<(), UnitStateError> {
        let content = serde_json::to_string_pretty(self).map_err(|err| {
            UnitStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        let mut tmp = match self.path.parent() {
            Some(parent) => tempfile::NamedTempFile::new_in(parent),
            None => tempfile::NamedTempFile::new(),
        }
        .map_err(UnitStateError::Write)?;

        tmp.write_all(content.as_bytes())
            .map_err(UnitStateError::Write)?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(UnitStateError::Write)?;
        tmp.persist(&self.path)
            .map_err(|err| UnitStateError::Write(err.error))?;
        Ok(())
    }

    /// Returns currently running version
    pub fn get_active_version(ctx: &MainContext, name: &UnitName) -> Result<u32, UnitStateError> {
        let state = Self::load(ctx, name)?;
        state.active_version.ok_or(UnitStateError::NoActiveVersion)
    }

    /// Returns currently running version before uninstall
    pub fn take_active_version(&mut self) -> Option<u32> {
        let result = self.active_version.take();
        if result.is_some() {
            let _ = self.save();
        }
        result
    }

    /// Checked version addition.
    /// Sets the latest build status to `Building` and clears previous error.
    pub fn bump_version(&mut self) -> Result<u32, UnitStateError> {
        let next_version = self
            .last_version
            .checked_add(1)
            .ok_or(UnitStateError::VersionOverflow)?;
        self.last_version = next_version;
        self.last_status = DeployStatus::Building;
        self.updated_at = Utc::now();
        self.failure = None;
        self.save()?;

        Ok(next_version)
    }

    /// Marks the latest build failed, recording the [`Stage`] it failed in and
    /// the human-readable `message` cause for later inspection.
    pub fn set_failed(&mut self, stage: Stage, message: String) {
        self.last_status = DeployStatus::Failed;
        self.updated_at = Utc::now();
        self.failure = Some(BuildFailure {
            stage,
            error: message,
        });
        let _ = self.save();
    }

    /// Sets build status to ready, sets build version as active version
    pub fn set_ready(&mut self) {
        self.active_version = Some(self.last_version);
        self.last_status = DeployStatus::Ready;
        self.updated_at = Utc::now();
        self.failure = None;
        let _ = self.save();
    }
}

/// Holds an OS-level exclusive `flock` on `{unit_dir}/.deploy.lock` for the
/// lifetime of the value; the kernel releases it when the fd closes, including
/// on crash. The lock file is unlinked on drop (carrying the usual flock-unlink
/// race, acceptable since `dpl` deploys are serialized on a single host).
pub struct DeployStateGuard {
    // Held for the flock; the lock is released when this is dropped.
    // Never read directly, just keep the fd and lock alive.
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl DeployStateGuard {
    fn lock(ctx: &MainContext, name: &UnitName) -> Result<Self, UnitStateError> {
        let path = ctx.lock_path(name);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(UnitStateError::Lock)?;

        match file.try_lock_exclusive() {
            Ok(true) => Ok(DeployStateGuard { file, path }),
            Ok(false) => Err(UnitStateError::Busy),
            Err(err) => Err(UnitStateError::Lock(err)),
        }
    }

    /// Try to take the unit's deploy lock without blocking.
    /// Returns `None` if a deploy currently holds it.
    pub fn try_acquire(ctx: &MainContext, name: &UnitName) -> Result<Option<Self>, UnitStateError> {
        match Self::lock(ctx, name) {
            Ok(guard) => Ok(Some(guard)),
            Err(UnitStateError::Busy) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

impl Drop for DeployStateGuard {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("remove deploy lock file {}: {err}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_at(dir: &std::path::Path) -> DeployState {
        DeployState {
            path: dir.join(".deploy.state"),
            updated_at: Utc::now(),
            active_version: None,
            last_version: 0,
            last_status: Default::default(),
            failure: None,
        }
    }

    #[test]
    fn set_failed_records_stage_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.set_failed(
            Stage::Startup,
            "waiting for app: container exited with code 1 (ran 2s)".to_owned(),
        );
        assert_eq!(state.last_status, DeployStatus::Failed);
        let failure = state.failure.as_ref().unwrap();
        assert_eq!(failure.stage, Stage::Startup);
        assert_eq!(
            failure.error,
            "waiting for app: container exited with code 1 (ran 2s)"
        );
    }
}
