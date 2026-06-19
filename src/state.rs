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

#[derive(Debug)]
pub struct UndeployOutcome {
    pub kind: Option<String>,
    pub active_version: Option<u32>,
    pub changed: bool,
}

#[derive(Default, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    #[default]
    Idle,
    Building,
    /// Built and active, awaiting startup verification (deploy health-checks).
    Check,
    Ready,
    Failed,
}

/// Deploy phase for DeployError.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStage {
    /// Loading/validating config, resolving secrets, acquiring the lock,
    /// bumping the version, staging inputs, rendering build artifacts.
    Prepare,
    /// Building the container image from the staged sources.
    Build,
    /// Exporting files, creating or restoring databases,
    /// writing rendered config into volumes.
    Install,
    /// Waiting for the deployed unit to become healthy (its port/ping check).
    Startup,
}

impl fmt::Display for DeployStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            DeployStage::Prepare => "prepare",
            DeployStage::Build => "build",
            DeployStage::Install => "install",
            DeployStage::Startup => "startup",
        };
        f.write_str(name)
    }
}

/// Where and why the latest build failed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployFailure {
    /// Deploy stage the attempt failed in.
    pub stage: DeployStage,

    /// Human-readable cause (the failing step plus its source chain).
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeployState {
    /// Timestamp of the most recent deploy attempt event.
    pub updated_at: DateTime<Utc>,

    /// Type of the unit for the latest deploy attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Currently running version
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<u32>,

    /// Increments on each deploy attempt
    pub last_version: u32,

    /// Version for last attempt
    pub last_status: DeployStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<DeployFailure>,

    /// True once this unit was handed off to `dpl serve` to run a container.
    #[serde(default)]
    pub supervised: bool,

    /// db-server units whose container must be up before serve spawns this
    /// unit's `dpl start`. Snapshotted from the app's `${db:...}` refs at the
    /// deploy hand-off; empty for every non-app unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start_after: Vec<UnitName>,

    #[serde(skip)]
    path: PathBuf,
}

impl DeployState {
    /// Acquire the unit-level busy lock and load its state.
    pub fn acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<(DeployLockGuard, DeployState), DeployStateError> {
        let guard = DeployLockGuard::lock(ctx, name)?;
        let state = DeployState::load(ctx, name)?;
        if state.last_status == DeployStatus::Building {
            return Err(DeployStateError::Busy);
        }

        Ok((guard, state))
    }

    pub fn load(ctx: &MainContext, name: &UnitName) -> Result<Self, DeployStateError> {
        let path = ctx.deploy_state_path(name);

        let content = match read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(DeployState {
                    updated_at: Utc::now(),
                    kind: None,
                    active_version: None,
                    last_version: 0,
                    last_status: Default::default(),
                    failure: None,
                    supervised: false,
                    start_after: Vec::new(),
                    path,
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

    /// Every unit that has a deploy state file.
    pub fn list(ctx: &MainContext) -> Vec<(UnitName, DeployState)> {
        let Ok(entries) = std::fs::read_dir(ctx.state_dir()) else {
            return Vec::new();
        };

        let mut units: Vec<(UnitName, DeployState)> = entries
            .flatten()
            .filter_map(|entry| {
                let file_type = entry.file_type().ok()?;
                if !file_type.is_dir() {
                    return None;
                }
                let file_name = entry.file_name();
                let file_name = file_name.to_str()?;
                let unit_name = UnitName::new(file_name).ok()?;
                if !ctx.deploy_state_path(&unit_name).exists() {
                    return None;
                }
                let state = DeployState::load(ctx, &unit_name).ok()?;
                Some((unit_name, state))
            })
            .collect();

        units.sort_by(|a, b| a.0.cmp(&b.0));
        units
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

    /// Begin a new deploy attempt: record its unit kind, bump the version,
    /// mark it `Building`, and clear the previous error.
    pub fn begin_deploy(&mut self, kind: impl Into<String>) -> Result<u32, DeployStateError> {
        let next_version = self
            .last_version
            .checked_add(1)
            .ok_or(DeployStateError::VersionOverflow)?;
        self.kind = Some(kind.into());
        self.last_version = next_version;
        self.last_status = DeployStatus::Building;
        self.updated_at = Utc::now();
        self.failure = None;
        self.save()?;

        Ok(next_version)
    }

    /// Marks the latest deploy attempt failed.
    pub fn set_failed(&mut self, stage: DeployStage, message: String) {
        self.last_status = DeployStatus::Failed;
        self.updated_at = Utc::now();
        self.failure = Some(DeployFailure {
            stage,
            error: message,
        });
        let _ = self.save();
    }

    /// Marks the build active but not yet verified.
    pub fn set_check(&mut self) {
        self.active_version = Some(self.last_version);
        self.last_status = DeployStatus::Check;
        self.supervised = true;
        self.updated_at = Utc::now();
        self.failure = None;
        let _ = self.save();
    }

    /// Record the db-server units this unit must wait for before serve spawns
    /// its `dpl start`. Called just before `set_check` on every app deploy, so
    /// the edges always reflect the version being handed off.
    pub fn set_start_after(&mut self, deps: Vec<UnitName>) {
        self.start_after = deps;
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

    /// Remove the active deployment from service. The unit can become live again
    /// only through a later deploy, which will set `supervised` at hand-off.
    pub fn undeploy(&mut self) -> Result<UndeployOutcome, DeployStateError> {
        let kind = self.kind.clone();
        let active_version = self.active_version;
        let changed = self.active_version.is_some()
            || self.supervised
            || !self.start_after.is_empty()
            || self.last_status != DeployStatus::Idle
            || self.failure.is_some();

        if changed {
            self.active_version = None;
            self.last_status = DeployStatus::Idle;
            self.supervised = false;
            self.start_after = Vec::new();
            self.failure = None;
            self.updated_at = Utc::now();
            self.save()?;
        }

        Ok(UndeployOutcome {
            kind,
            active_version,
            changed,
        })
    }
}

/// Holds an OS-level exclusive `flock` on `state/{unit}/deploy.lock`.
pub struct DeployLockGuard {
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl DeployLockGuard {
    fn lock(ctx: &MainContext, name: &UnitName) -> Result<Self, DeployStateError> {
        let path = ctx.deploy_lock_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(DeployStateError::Lock)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(DeployStateError::Lock)?;

        match file.try_lock_exclusive() {
            Ok(true) => Ok(DeployLockGuard { file, path }),
            Ok(false) => Err(DeployStateError::Busy),
            Err(err) => Err(DeployStateError::Lock(err)),
        }
    }

    /// Try to take the unit's deploy lock without blocking.
    /// Returns `None` if a deploy currently holds it.
    pub fn try_acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<Option<Self>, DeployStateError> {
        match Self::lock(ctx, name) {
            Ok(guard) => Ok(Some(guard)),
            Err(DeployStateError::Busy) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

impl Drop for DeployLockGuard {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            crate::log::warn(format!(
                "remove deploy lock file {}: {err}",
                self.path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_at(dir: &std::path::Path) -> DeployState {
        DeployState {
            updated_at: Utc::now(),
            kind: None,
            active_version: None,
            last_version: 0,
            last_status: Default::default(),
            failure: None,
            supervised: false,
            start_after: Vec::new(),
            path: dir.join(".deploy.state"),
        }
    }

    #[test]
    fn set_failed_records_stage_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.set_failed(
            DeployStage::Startup,
            "waiting for app: container exited with code 1 (ran 2s)".to_owned(),
        );
        assert_eq!(state.last_status, DeployStatus::Failed);
        let failure = state.failure.as_ref().unwrap();
        assert_eq!(failure.stage, DeployStage::Startup);
        assert_eq!(
            failure.error,
            "waiting for app: container exited with code 1 (ran 2s)"
        );
    }

    #[test]
    fn begin_deploy_persists_unit_kind() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());

        state.begin_deploy("http-server").unwrap();

        let content = std::fs::read_to_string(dir.path().join(".deploy.state")).unwrap();
        assert!(
            content.contains("\"kind\": \"http-server\""),
            "state file did not persist kind:\n{content}"
        );
    }

    #[test]
    fn undeploy_clears_active_runtime_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.begin_deploy("app").unwrap();
        state.set_start_after(vec![UnitName::new("pg").unwrap()]);
        state.set_check();
        state.set_ready();

        let outcome = state.undeploy().unwrap();

        assert_eq!(outcome.kind.as_deref(), Some("app"));
        assert_eq!(outcome.active_version, Some(1));
        assert!(outcome.changed);
        assert_eq!(state.kind.as_deref(), Some("app"));
        assert_eq!(state.active_version, None);
        assert_eq!(state.last_status, DeployStatus::Idle);
        assert!(!state.supervised);
        assert!(state.start_after.is_empty());
        assert!(state.failure.is_none());
    }

    #[test]
    fn set_start_after_persists_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        let pg = UnitName::new("pg").unwrap();
        let path = dir.path().join(".deploy.state");

        state.set_start_after(vec![pg.clone()]);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"start_after\""),
            "edges not persisted:\n{content}"
        );
        let reloaded: DeployState = serde_json::from_str(&content).unwrap();
        assert_eq!(reloaded.start_after, vec![pg]);

        // Empty edges are omitted (skip_serializing_if) and default back to empty.
        state.set_start_after(Vec::new());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("start_after"),
            "empty edges should be omitted:\n{content}"
        );
        let reloaded: DeployState = serde_json::from_str(&content).unwrap();
        assert!(reloaded.start_after.is_empty());
    }
}
