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
    deploy::DeployError,
};

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
    Runtime,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Stage::Prepare => "prepare",
            Stage::Build => "build",
            Stage::Install => "install",
            Stage::Runtime => "runtime",
        };
        f.write_str(name)
    }
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    pub version: u32,
    pub status: DeployStatus,

    /// When the lataest build last changed status.
    pub updated_at: DateTime<Utc>,

    /// Deploy stage the last attempt failed in. Set only on failure; selects
    /// the relevant log and reads back in `dpl inspect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,

    /// Human-readable cause of the last failure (the failing step plus its
    /// source chain). Set only on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    #[serde(skip)]
    path: PathBuf,

    /// Currently running version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<u32>,

    /// Version for last attempt
    pub latest_build: BuildResult,
}

impl DeployState {
    /// Acquire the unit-level busy lock and load its state.
    pub fn acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<(DeployStateGuard, DeployState), DeployStateError> {
        let guard = DeployStateGuard::lock(ctx, name)?;
        let state = DeployState::load(ctx, name)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployStateError::Busy);
        }

        Ok((guard, state))
    }

    pub fn load(ctx: &MainContext, name: &UnitName) -> Result<Self, DeployStateError> {
        let path = ctx.state_path(name);

        let content = match read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState {
                    path,
                    active_version: None,
                    latest_build: BuildResult::default(),
                });
            }
            Err(err) => return Err(DeployStateError::Read(err)),
        };

        let mut state: DeployState = serde_json::from_str(&content).map_err(|err| {
            DeployStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        state.path = path;

        Ok(state)
    }

    fn save(&self) -> Result<(), DeployStateError> {
        let content = serde_json::to_string_pretty(self).map_err(|err| {
            DeployStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        let mut tmp = match self.path.parent() {
            Some(parent) => tempfile::NamedTempFile::new_in(parent),
            None => tempfile::NamedTempFile::new(),
        }
        .map_err(DeployStateError::Write)?;

        tmp.write_all(content.as_bytes())
            .map_err(DeployStateError::Write)?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(DeployStateError::Write)?;
        tmp.persist(&self.path)
            .map_err(|err| DeployStateError::Write(err.error))?;
        Ok(())
    }

    /// Returns currently running version
    pub fn get_active_version(ctx: &MainContext, name: &UnitName) -> Result<u32, DeployStateError> {
        let state = Self::load(ctx, name)?;
        state
            .active_version
            .ok_or(DeployStateError::NoActiveVersion)
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
    pub fn bump_version(&mut self) -> Result<u32, DeployStateError> {
        let next = self
            .latest_build
            .version
            .checked_add(1)
            .ok_or(DeployStateError::VersionOverflow)?;
        self.latest_build.version = next;
        self.latest_build.status = DeployStatus::Building;
        self.latest_build.updated_at = Utc::now();
        self.latest_build.stage = None;
        self.latest_build.error = None;
        self.save()?;

        Ok(next)
    }

    /// Marks the latest build failed, recording the [`Stage`] it failed in and
    /// the human-readable `error` cause for later inspection.
    pub fn set_error(&mut self, error: &DeployError) {
        self.latest_build.status = DeployStatus::Failed;
        self.latest_build.updated_at = Utc::now();

        if let DeployError::Step {
            stage,
            info,
            source,
        } = error
        {
            let mut messages = Vec::new();
            messages.push(info.to_owned());

            let mut source: &(dyn std::error::Error + 'static) = source.as_ref();
            loop {
                messages.push(source.to_string());
                match source.source() {
                    Some(next) => source = next,
                    None => break,
                }
            }
            let message = messages.join(": ");

            self.latest_build.stage = Some(*stage);
            self.latest_build.error = Some(message);
        } else {
            self.latest_build.stage = None;
            self.latest_build.error = None;
        }

        let _ = self.save();
    }

    /// Sets build status to ready, sets build version as active version
    pub fn set_ready(&mut self) {
        self.active_version = Some(self.latest_build.version);
        self.latest_build.status = DeployStatus::Ready;
        self.latest_build.updated_at = Utc::now();
        self.latest_build.stage = None;
        self.latest_build.error = None;
        let _ = self.save();
    }
}

/// Holds an OS-level exclusive `flock` on `{unit_dir}/.deploy.lock` for the
/// lifetime of the value; the kernel releases it when the fd closes, including
/// on crash. The lock file is unlinked on drop (carrying the usual flock-unlink
/// race, acceptable since `dpl` deploys are serialized on a single host).
pub struct DeployStateGuard {
    // Held for the flock; the lock is released when this is dropped.
    file: File,
    path: PathBuf,
}

impl DeployStateGuard {
    fn lock(ctx: &MainContext, name: &UnitName) -> Result<Self, DeployStateError> {
        let path = ctx.lock_path(name);
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(DeployStateError::Lock)?;

        match file.try_lock_exclusive() {
            Ok(true) => Ok(DeployStateGuard { file, path }),
            Ok(false) => Err(DeployStateError::Busy),
            Err(err) => Err(DeployStateError::Lock(err)),
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
            path: dir.join(".state.json"),
            active_version: None,
            latest_build: BuildResult::default(),
        }
    }

    #[test]
    fn set_error_records_stage_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        let err = DeployError::step_runtime(
            "waiting for app",
            io::Error::other("container exited with code 1 (ran 2s)"),
        );
        state.set_error(&err);
        assert_eq!(state.latest_build.status, DeployStatus::Failed);
        assert_eq!(state.latest_build.stage, Some(Stage::Runtime));
        assert_eq!(
            state.latest_build.error.as_deref(),
            Some("waiting for app: container exited with code 1 (ran 2s)")
        );
    }
}
