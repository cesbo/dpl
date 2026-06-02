use std::{
    collections::BTreeMap,
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
    time::Duration,
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
    Startup,
    /// Bringing the unit's container up (`dpl start`, the service `ExecStart`):
    /// prerequisites, db-dependency waits, and the foreground `podman run`.
    Start,
    /// Tearing the unit's container down (`dpl stop`, the service `ExecStop`).
    Stop,
    /// Running one of a unit's timers (`dpl timer`, the timer service's
    /// `ExecStart`): `podman exec` of the timer script in the live container.
    Timer,
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
            Stage::Timer => "timer",
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

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildResult {
    pub version: u32,
    pub status: DeployStatus,
    pub updated_at: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<BuildFailure>,
}

/// Outcome of a single timer run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerStatus {
    /// The script is still executing (set before `podman exec`, overwritten
    /// with the result once it returns). A stale `Running` (left by a crashed
    /// `dpl timer`) is cosmetic only and self-heals on the timer's next run.
    Running,
    Success,
    Failed,
}

/// The most recent run of one of a unit's timers (`dpl timer`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerState {
    /// When the timer started.
    pub started_at: DateTime<Utc>,
    pub status: TimerStatus,
    /// Duration of the last timer run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Failure cause, set only when `status` is `Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TimerState {
    pub fn running(started_at: DateTime<Utc>) -> Self {
        TimerState {
            started_at,
            status: TimerStatus::Running,
            duration_ms: None,
            error: None,
        }
    }

    pub fn success(started_at: DateTime<Utc>, duration: Duration) -> Self {
        TimerState {
            started_at,
            status: TimerStatus::Success,
            duration_ms: Some(duration.as_millis() as u64),
            error: None,
        }
    }

    pub fn failed(started_at: DateTime<Utc>, duration: Duration, error: impl Into<String>) -> Self {
        TimerState {
            started_at,
            status: TimerStatus::Failed,
            duration_ms: Some(duration.as_millis() as u64),
            error: Some(error.into()),
        }
    }
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

    /// Most recent run of each timer, keyed by timer name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timers: BTreeMap<String, TimerState>,
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
                    timers: BTreeMap::new(),
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
        self.latest_build.failure = None;
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

            self.latest_build.failure = Some(BuildFailure {
                stage: *stage,
                error: message,
            });
        } else {
            self.latest_build.failure = None;
        }

        let _ = self.save();
    }

    /// Sets build status to ready, sets build version as active version
    pub fn set_ready(&mut self) {
        self.active_version = Some(self.latest_build.version);
        self.latest_build.status = DeployStatus::Ready;
        self.latest_build.updated_at = Utc::now();
        self.latest_build.failure = None;
        let _ = self.save();
    }

    /// Records the latest run of a timer, replacing any previous record for it.
    pub fn set_timer_state(&mut self, timer: &str, run: TimerState) {
        self.timers.insert(timer.to_string(), run);
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
            timers: BTreeMap::new(),
        }
    }

    #[test]
    fn set_error_records_stage_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        let err = DeployError::step_startup(
            "waiting for app",
            io::Error::other("container exited with code 1 (ran 2s)"),
        );
        state.set_error(&err);
        assert_eq!(state.latest_build.status, DeployStatus::Failed);
        let failure = state.latest_build.failure.as_ref().unwrap();
        assert_eq!(failure.stage, Stage::Startup);
        assert_eq!(
            failure.error,
            "waiting for app: container exited with code 1 (ran 2s)"
        );
    }

    #[test]
    fn record_timer_run_persists_last_run_per_timer() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state(
            "cleanup",
            TimerState::success(t0, Duration::from_millis(1840)),
        );
        state.set_timer_state(
            "sync",
            TimerState::failed(t0, Duration::from_secs(2), "exit code 1"),
        );

        // A second run of the same timer replaces the first.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        state.set_timer_state(
            "cleanup",
            TimerState::success(t1, Duration::from_millis(990)),
        );

        assert_eq!(state.timers.len(), 2);
        let cleanup = &state.timers["cleanup"];
        assert_eq!(cleanup.status, TimerStatus::Success);
        assert_eq!(cleanup.started_at, t1);
        assert_eq!(cleanup.duration_ms, Some(990));
        assert!(cleanup.error.is_none());
        assert_eq!(state.timers["sync"].status, TimerStatus::Failed);

        // record_timer_run's save() wrote `.state.json`; reload it from disk and
        // confirm the timer map round-trips (incl. snake_case status strings).
        let on_disk = read_to_string(dir.path().join(".state.json")).unwrap();
        assert!(on_disk.contains("\"status\": \"success\""), "{on_disk}");
        assert!(on_disk.contains("\"status\": \"failed\""), "{on_disk}");
        let reloaded: DeployState = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(reloaded.timers, state.timers);
    }

    #[test]
    fn running_timer_run_omits_duration() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state_at(dir.path());
        let t = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        state.set_timer_state("backup", TimerState::running(t));
        let run = &state.timers["backup"];
        assert_eq!(run.status, TimerStatus::Running);
        assert!(run.duration_ms.is_none());

        // A `Running` record serializes without a `duration_ms` field.
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"status\":\"running\""), "{json}");
        assert!(!json.contains("duration_ms"), "{json}");

        // Completing the run overwrites the `Running` record in place.
        state.set_timer_state("backup", TimerState::success(t, Duration::from_millis(500)));
        assert_eq!(state.timers.len(), 1);
        assert_eq!(state.timers["backup"].status, TimerStatus::Success);
        assert_eq!(state.timers["backup"].duration_ms, Some(500));
    }

    #[test]
    fn empty_timers_map_is_omitted_from_json() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path());
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("timers"), "{json}");
    }
}
