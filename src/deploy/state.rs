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
    path::PathBuf,
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

    /// Deploy phase active when the last attempt failed (e.g. `building app
    /// image`, `waiting for app`). Set only on failure; tells later analysis
    /// where to look — build-time phases point at `{unit_dir}/build.log`, the
    /// health-check phase at `/var/log/podman/{scoped_unit_name}.log`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
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
        self.latest_build.phase = None;
        self.save()?;

        Ok(next)
    }

    /// Marks the latest build failed, recording the phase it failed at (the
    /// open [`crate::log::phase`] when available, else `None`).
    pub fn set_error(&mut self, phase: Option<String>) {
        self.latest_build.status = DeployStatus::Failed;
        self.latest_build.phase = phase;
        let _ = self.save();
    }

    /// Sets build status to ready, sets build version as active version
    pub fn set_ready(&mut self) {
        self.latest_build.status = DeployStatus::Ready;
        self.latest_build.phase = None;
        self.active_version = Some(self.latest_build.version);
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
    fn set_error_records_phase() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.set_error(Some("building app image".to_string()));
        assert_eq!(state.latest_build.status, DeployStatus::Failed);
        assert_eq!(
            state.latest_build.phase.as_deref(),
            Some("building app image")
        );
    }

    #[test]
    fn bump_version_clears_phase() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.set_error(Some("waiting for app".to_string()));
        assert_eq!(state.bump_version().unwrap(), 1);
        assert_eq!(state.latest_build.status, DeployStatus::Building);
        assert_eq!(state.latest_build.phase, None);
    }

    #[test]
    fn set_ready_clears_phase() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.set_error(Some("waiting for app".to_string()));
        state.set_ready();
        assert_eq!(state.latest_build.status, DeployStatus::Ready);
        assert_eq!(state.latest_build.phase, None);
        assert_eq!(state.active_version, Some(state.latest_build.version));
    }

    #[test]
    fn phase_serde_roundtrips_and_is_omitted_when_none() {
        let failed = BuildResult {
            version: 2,
            status: DeployStatus::Failed,
            phase: Some("building app image".to_string()),
        };
        let json = serde_json::to_string(&failed).unwrap();
        assert!(json.contains(r#""phase":"building app image""#), "{json}");
        assert_eq!(serde_json::from_str::<BuildResult>(&json).unwrap(), failed);

        // Omitted from the JSON when there is no failure.
        let ready = BuildResult {
            version: 3,
            status: DeployStatus::Ready,
            phase: None,
        };
        let json = serde_json::to_string(&ready).unwrap();
        assert!(!json.contains("phase"), "{json}");

        // Backward-compat: legacy state files without the field load as None.
        let legacy: BuildResult =
            serde_json::from_str(r#"{"version":3,"status":"ready"}"#).unwrap();
        assert_eq!(legacy.phase, None);
    }
}
