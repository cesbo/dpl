use std::{
    fmt,
    fs::{
        File,
        OpenOptions,
        read_to_string,
    },
    io::{
        self,
        Read,
        Seek,
        SeekFrom,
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

    #[error("unit busy: another process holds the deploy lock{}", holder(*.pid))]
    Busy { pid: Option<u32> },

    #[error("read state file")]
    Read(#[source] io::Error),

    #[error("write state file")]
    Write(#[source] io::Error),

    #[error("unit version overflow")]
    VersionOverflow,

    #[error("unit has no active version")]
    NoActiveVersion,
}

/// Render the holder pid for the `Busy` message.
fn holder(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!(" (pid {pid})"),
        None => String::new(),
    }
}

/// Who holds a unit's deploy lock right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockHolder {
    /// Nobody holds it - or it could not be probed at all.
    Free,
    /// Held by a live process; the pid when the lock file carries one.
    Held(Option<u32>),
}

#[derive(Debug)]
pub struct UndeployOutcome {
    pub kind: Option<String>,
    pub active_version: Option<u32>,
    pub candidate_version: Option<u32>,
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

    /// The retained version a rollback deploy was made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_version: Option<u32>,

    /// The version started next to the active one by a parallel deploy,
    /// awaiting its switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_version: Option<u32>,

    #[serde(skip)]
    path: PathBuf,
}

impl DeployState {
    /// Acquire the unit-level busy lock and load its state.
    ///
    /// A `Building` status read *under* the lock has no live deployer - every
    /// writer of `Building` holds this flock for its whole deploy - so it is
    /// recorded as a failed attempt instead of latching the unit as busy
    /// forever after a killed deploy or a reboot.
    pub fn acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<(DeployLockGuard, DeployState), DeployStateError> {
        let guard = DeployLockGuard::lock(ctx, name)?;
        let mut state = DeployState::load(ctx, name)?;

        if state.last_status == DeployStatus::Building {
            crate::log::warn(format!(
                "unit '{name}': deploy of version {} never finished (last update {}) - \
                 marking it failed",
                state.last_version, state.updated_at
            ));
            state.set_failed(DeployStage::Prepare, "deploy interrupted".to_string());
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
                    source_version: None,
                    candidate_version: None,
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
        self.source_version = None;
        self.candidate_version = None;
        self.save()?;

        Ok(next_version)
    }

    /// Begin a rollback attempt: a normal deploy attempt that also records the
    /// retained version it is made from.
    pub fn begin_rollback(
        &mut self,
        kind: impl Into<String>,
        source: u32,
    ) -> Result<u32, DeployStateError> {
        let next_version = self.begin_deploy(kind)?;
        self.source_version = Some(source);
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

    /// Hand the build off next to the active version: a parallel deploy's
    /// `set_check` that leaves `active_version` alone until the switch.
    pub fn set_candidate(&mut self) {
        self.candidate_version = Some(self.last_version);
        self.last_status = DeployStatus::Check;
        self.supervised = true;
        self.updated_at = Utc::now();
        self.failure = None;
        let _ = self.save();
    }

    /// Switch: the candidate becomes active. Returns the previous active.
    pub fn promote_candidate(&mut self) -> Option<u32> {
        let previous = self.active_version;
        self.active_version = self.candidate_version.take();
        self.updated_at = Utc::now();
        let _ = self.save();
        previous
    }

    /// Undo a switch: `active` is back in service, the candidate is dropped.
    pub fn revert_candidate(&mut self, active: Option<u32>) {
        self.active_version = active;
        self.candidate_version = None;
        self.updated_at = Utc::now();
        let _ = self.save();
    }

    /// Forget a candidate that was discarded before its switch.
    pub fn clear_candidate(&mut self) {
        self.candidate_version = None;
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
        let candidate_version = self.candidate_version;
        let changed = self.active_version.is_some()
            || self.candidate_version.is_some()
            || self.supervised
            || !self.start_after.is_empty()
            || self.last_status != DeployStatus::Idle
            || self.failure.is_some();

        if changed {
            self.active_version = None;
            self.candidate_version = None;
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
            candidate_version,
            changed,
        })
    }
}

/// Holds an OS-level exclusive `flock` on `state/{unit}/deploy.lock` and, while
/// held, records the holder's pid in it. The flock is released when the file
/// handle drops - including when the process dies, however it dies.
///
/// The lock file itself is never unlinked: unlink-while-locked lets a waiter
/// wake up holding a deleted inode while a newcomer locks a fresh one, and the
/// pid must stay readable by whoever gets `Busy`.
pub struct DeployLockGuard {
    #[allow(dead_code)]
    file: File,
}

impl DeployLockGuard {
    fn lock(ctx: &MainContext, name: &UnitName) -> Result<Self, DeployStateError> {
        Self::open(ctx, name, true)
    }

    /// `write_pid` records this process as the holder; a bare probe passes
    /// `false` so it never rewrites a lock it is only inspecting.
    fn open(ctx: &MainContext, name: &UnitName, write_pid: bool) -> Result<Self, DeployStateError> {
        let path = ctx.deploy_lock_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(DeployStateError::Lock)?;
        }

        // Never truncate before the flock attempt: it would wipe the holder pid
        // of a lock somebody else currently holds.
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(DeployStateError::Lock)?;

        match file.try_lock_exclusive() {
            Ok(true) => {
                if write_pid {
                    write_holder(&mut file);
                }
                Ok(DeployLockGuard { file })
            }
            Ok(false) => Err(DeployStateError::Busy {
                pid: read_holder(&mut file),
            }),
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
            Err(DeployStateError::Busy { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Who holds the unit's deploy lock, for reports. There is no way to peek at
    /// an `flock`, so this takes and immediately drops the lock: never call it
    /// while this process already holds it, and only when a report actually
    /// needs the answer, since for that instant a deploy would see `Busy`.
    pub fn holder(ctx: &MainContext, name: &UnitName) -> LockHolder {
        match Self::open(ctx, name, false) {
            Ok(_released) => LockHolder::Free,
            Err(DeployStateError::Busy { pid }) => LockHolder::Held(pid),
            Err(_) => LockHolder::Free,
        }
    }
}

/// Record this process as the lock holder. Best-effort: losing the pid costs a
/// detail in one error message, never the lock itself.
fn write_holder(file: &mut File) {
    let pid = std::process::id().to_string();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.write_all(pid.as_bytes());
    let _ = file.set_len(pid.len() as u64);
    let _ = file.flush();
}

/// The pid recorded in a lock file, if it holds a readable one.
fn read_holder(file: &mut File) -> Option<u32> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut content = String::new();
    file.read_to_string(&mut content).ok()?;
    content.trim().parse().ok()
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
            source_version: None,
            candidate_version: None,
            path: dir.join(".deploy.state"),
        }
    }

    /// v1 ready and live, v2 handed off next to it as a candidate.
    fn state_with_candidate(dir: &std::path::Path) -> DeployState {
        let mut state = state_at(dir);
        state.begin_deploy("app").unwrap();
        state.set_ready();
        state.begin_deploy("app").unwrap();
        state.set_candidate();
        state
    }

    fn ctx_at(dir: &std::path::Path) -> MainContext {
        MainContext {
            base: dir.to_path_buf(),
            master_key: None,
        }
    }

    #[test]
    fn busy_names_the_holder_pid() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        let _held = DeployLockGuard::lock(&ctx, &name).unwrap();
        let Err(err) = DeployState::acquire(&ctx, &name) else {
            panic!("expected Busy while the lock is held");
        };

        let DeployStateError::Busy { pid } = &err else {
            panic!("expected Busy, got {err:?}");
        };
        let pid = *pid;
        assert_eq!(pid, Some(std::process::id()));
        assert!(
            err.to_string().contains(&std::process::id().to_string()),
            "{err}"
        );
    }

    #[test]
    fn holder_reports_free_and_held() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        assert_eq!(DeployLockGuard::holder(&ctx, &name), LockHolder::Free);

        let held = DeployLockGuard::lock(&ctx, &name).unwrap();
        assert_eq!(
            DeployLockGuard::holder(&ctx, &name),
            LockHolder::Held(Some(std::process::id()))
        );

        drop(held);
        assert_eq!(DeployLockGuard::holder(&ctx, &name), LockHolder::Free);
    }

    #[test]
    fn lock_file_survives_guard_drop() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        drop(DeployLockGuard::lock(&ctx, &name).unwrap());

        // One stable inode per path: never unlinked while anyone could hold it.
        assert!(ctx.deploy_lock_path(&name).exists());
        assert!(DeployLockGuard::lock(&ctx, &name).is_ok());
    }

    #[test]
    fn acquire_recovers_a_stale_building_status() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        // v1 live, then a v2 deploy that bumped the version and died mid-build.
        {
            let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
            state.begin_deploy("app").unwrap();
            state.set_ready();
            state.begin_deploy("app").unwrap();
            assert_eq!(state.last_status, DeployStatus::Building);
            assert_eq!(state.last_version, 2);
        }

        let (_guard, state) = DeployState::acquire(&ctx, &name).unwrap();
        assert_eq!(state.last_status, DeployStatus::Failed);
        let failure = state.failure.as_ref().unwrap();
        assert_eq!(failure.stage, DeployStage::Prepare);
        assert_eq!(failure.error, "deploy interrupted");
        assert_eq!(state.last_version, 2);
        // The live version stays claimed, so a later undeploy can still clean it.
        assert_eq!(state.active_version, Some(1));

        // The recovery is persisted, not just returned.
        let reloaded = DeployState::load(&ctx, &name).unwrap();
        assert_eq!(reloaded.last_status, DeployStatus::Failed);
    }

    #[test]
    fn acquire_is_busy_only_while_the_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        let held = DeployLockGuard::lock(&ctx, &name).unwrap();
        assert!(matches!(
            DeployState::acquire(&ctx, &name),
            Err(DeployStateError::Busy { .. })
        ));

        drop(held);
        assert!(DeployState::acquire(&ctx, &name).is_ok());
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
    fn source_version_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.begin_rollback("app", 3).unwrap();

        let reloaded = DeployState::load(&ctx, &name).unwrap();
        assert_eq!(reloaded.source_version, Some(3));
        assert_eq!(reloaded.last_version, 1);
    }

    #[test]
    fn begin_rollback_records_the_source_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());

        let version = state.begin_rollback("app", 6).unwrap();

        assert_eq!(version, 1);
        assert_eq!(state.last_version, 1);
        assert_eq!(state.last_status, DeployStatus::Building);
        assert_eq!(state.source_version, Some(6));
        let content = std::fs::read_to_string(dir.path().join(".deploy.state")).unwrap();
        assert!(
            content.contains("\"source_version\": 6"),
            "source not persisted:\n{content}"
        );
    }

    #[test]
    fn begin_deploy_clears_the_source_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        state.begin_rollback("app", 6).unwrap();

        state.begin_deploy("app").unwrap();

        assert_eq!(state.source_version, None);
        // Absent (skip_serializing_if) rather than `null`.
        let content = std::fs::read_to_string(dir.path().join(".deploy.state")).unwrap();
        assert!(
            !content.contains("source_version"),
            "cleared source should be omitted:\n{content}"
        );
    }

    #[test]
    fn set_candidate_keeps_the_active_version() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_with_candidate(dir.path());

        assert_eq!(state.active_version, Some(1));
        assert_eq!(state.candidate_version, Some(2));
        assert_eq!(state.last_status, DeployStatus::Check);
        assert!(state.supervised);
        assert!(state.failure.is_none());
    }

    #[test]
    fn promote_candidate_returns_the_old_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_with_candidate(dir.path());

        assert_eq!(state.promote_candidate(), Some(1));

        assert_eq!(state.active_version, Some(2));
        assert_eq!(state.candidate_version, None);
        // The switch does not decide readiness.
        assert_eq!(state.last_status, DeployStatus::Check);
    }

    #[test]
    fn revert_candidate_restores_the_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_with_candidate(dir.path());
        let previous = state.promote_candidate();

        state.revert_candidate(previous);

        assert_eq!(state.active_version, Some(1));
        assert_eq!(state.candidate_version, None);
    }

    #[test]
    fn begin_deploy_clears_the_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_with_candidate(dir.path());

        state.begin_deploy("app").unwrap();

        assert_eq!(state.candidate_version, None);
        assert_eq!(state.active_version, Some(1));
    }

    #[test]
    fn undeploy_clears_and_reports_the_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_with_candidate(dir.path());

        let outcome = state.undeploy().unwrap();

        assert_eq!(outcome.active_version, Some(1));
        assert_eq!(outcome.candidate_version, Some(2));
        assert!(outcome.changed);
        assert_eq!(state.candidate_version, None);
        assert_eq!(state.active_version, None);
    }

    #[test]
    fn candidate_version_loads_absent_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let name = UnitName::new("web").unwrap();

        // A deploy.json from before the field existed.
        let path = ctx.deploy_state_path(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"active_version":1,"last_version":1,"last_status":"ready","updated_at":"2026-05-31T07:00:00.000000Z"}"#,
        )
        .unwrap();
        let state = DeployState::load(&ctx, &name).unwrap();
        assert_eq!(state.candidate_version, None);

        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_candidate();

        let reloaded = DeployState::load(&ctx, &name).unwrap();
        assert_eq!(reloaded.candidate_version, Some(2));
        assert_eq!(reloaded.active_version, Some(1));

        state.clear_candidate();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("candidate_version"),
            "cleared candidate should be omitted:\n{content}"
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
