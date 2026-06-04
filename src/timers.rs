use std::{
    collections::BTreeMap,
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
};

#[derive(Debug, Error)]
pub enum TimerStateError {
    #[error("lock timers")]
    Lock(#[source] io::Error),

    #[error("read timers file")]
    Read(#[source] io::Error),

    #[error("write timers file")]
    Write(#[source] io::Error),
}

#[derive(Debug, Error)]
pub enum TimerError {
    /// The named timer doesn't exist on the unit, or is disabled.
    #[error("unknown or disabled timer '{0}'")]
    Unknown(String),

    /// The timer's command failed to spawn or exited non-zero.
    #[error("timer '{name}' failed")]
    Run {
        name: String,
        #[source]
        source: io::Error,
    },
}

/// Outcome of a single timer run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerStatus {
    Running,
    Success,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerFailure {
    /// Number of consecutive failed runs since the last success.
    pub count: u32,
    /// Failure cause, set only when `status` is `Failed`.
    pub error: String,
}

/// The most recent run of one of a unit's timers (`dpl timer`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimerState {
    pub status: TimerStatus,

    /// Timestamp of the most recent timer event.
    pub last_run_at: DateTime<Utc>,

    /// When the timer last completed successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,

    /// Duration of the last timer run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<TimerFailure>,
}

impl TimerState {
    pub fn running(prev: Option<&TimerState>, last_run_at: DateTime<Utc>) -> Self {
        TimerState {
            status: TimerStatus::Running,
            last_run_at,
            last_success_at: prev.and_then(|p| p.last_success_at),
            duration_ms: None,
            failure: prev.and_then(|p| p.failure.clone()),
        }
    }

    pub fn success(last_run_at: DateTime<Utc>, duration: Duration) -> Self {
        TimerState {
            status: TimerStatus::Success,
            last_run_at,
            last_success_at: Some(last_run_at),
            duration_ms: Some(duration.as_millis() as u64),
            failure: None,
        }
    }

    pub fn failed(
        prev: Option<&TimerState>,
        last_run_at: DateTime<Utc>,
        duration: Duration,
        error: impl Into<String>,
    ) -> Self {
        TimerState {
            status: TimerStatus::Failed,
            last_run_at,
            last_success_at: prev.and_then(|p| p.last_success_at),
            duration_ms: Some(duration.as_millis() as u64),
            failure: Some(TimerFailure {
                count: prev
                    .and_then(|p| p.failure.as_ref())
                    .map_or(0, |f| f.count)
                    .saturating_add(1),
                error: error.into(),
            }),
        }
    }
}

/// On-disk record of each timer's most recent run, stored separately from the
/// deploy state so timer runs and deploys never clobber each other's writes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimersState {
    /// Most recent run of each timer, keyed by timer name.
    pub timers: BTreeMap<String, TimerState>,

    #[serde(skip)]
    path: PathBuf,
}

impl TimersState {
    /// Acquire the per-unit timer lock (blocking) and load the timer state.
    pub fn acquire(
        ctx: &MainContext,
        name: &UnitName,
    ) -> Result<(TimerLockGuard, TimersState), TimerStateError> {
        let guard = TimerLockGuard::lock(ctx, name)?;
        let state = TimersState::load(ctx, name)?;

        Ok((guard, state))
    }

    pub fn load(ctx: &MainContext, name: &UnitName) -> Result<Self, TimerStateError> {
        let path = ctx.timers_state_path(name);

        let content = match read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(TimersState {
                    path,
                    timers: BTreeMap::new(),
                });
            }
            Err(err) => return Err(TimerStateError::Read(err)),
        };

        let mut state: TimersState = serde_json::from_str(&content).map_err(|err| {
            TimerStateError::Read(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        state.path = path;

        Ok(state)
    }

    fn save(&self) -> Result<(), TimerStateError> {
        let content = serde_json::to_string_pretty(self).map_err(|err| {
            TimerStateError::Write(io::Error::new(io::ErrorKind::InvalidData, err))
        })?;

        let mut tmp = match self.path.parent() {
            Some(parent) => tempfile::NamedTempFile::new_in(parent),
            None => tempfile::NamedTempFile::new(),
        }
        .map_err(TimerStateError::Write)?;

        tmp.write_all(content.as_bytes())
            .map_err(TimerStateError::Write)?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(TimerStateError::Write)?;
        tmp.persist(&self.path)
            .map_err(|err| TimerStateError::Write(err.error))?;
        Ok(())
    }

    /// Records the latest run of a timer, replacing any previous record for it.
    pub fn set_timer_state(&mut self, timer: &str, run: TimerState) {
        self.timers.insert(timer.to_string(), run);
        let _ = self.save();
    }
}

/// Holds an OS-level exclusive `flock` on `state/{unit}--timers.lock`.
pub struct TimerLockGuard {
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl TimerLockGuard {
    fn lock(ctx: &MainContext, name: &UnitName) -> Result<Self, TimerStateError> {
        let path = ctx.timers_lock_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(TimerStateError::Lock)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(TimerStateError::Lock)?;

        // Block until the lock is ours: a concurrent `dpl timer` for the same
        // unit waits here rather than skipping.
        file.lock_exclusive().map_err(TimerStateError::Lock)?;

        Ok(TimerLockGuard { file, path })
    }
}

impl Drop for TimerLockGuard {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("remove timer lock file {}: {err}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timers_at(dir: &std::path::Path) -> TimersState {
        TimersState {
            path: dir.join(".timers.state"),
            timers: BTreeMap::new(),
        }
    }

    #[test]
    fn record_timer_run_persists_last_run_per_timer() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state(
            "cleanup",
            TimerState::success(t0, Duration::from_millis(1840)),
        );
        state.set_timer_state(
            "sync",
            TimerState::failed(None, t0, Duration::from_secs(2), "exit code 1"),
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
        assert_eq!(cleanup.last_run_at, t1);
        assert_eq!(cleanup.last_success_at, Some(t1));
        assert_eq!(cleanup.duration_ms, Some(990));
        assert!(cleanup.failure.is_none());

        let sync = &state.timers["sync"];
        assert_eq!(sync.status, TimerStatus::Failed);
        assert_eq!(sync.failure.as_ref().map_or(0, |f| f.count), 1);
        assert!(sync.last_success_at.is_none());

        // set_timer_state's save() wrote `.timers.state`; reload it from disk and
        // confirm the timer map round-trips (incl. snake_case status strings).
        let on_disk = read_to_string(dir.path().join(".timers.state")).unwrap();
        assert!(on_disk.contains("\"status\": \"success\""), "{on_disk}");
        assert!(on_disk.contains("\"status\": \"failed\""), "{on_disk}");
        // Flat shape: timer names sit at the top level, with no enclosing
        // `timers` wrapper object (`#[serde(transparent)]`).
        assert!(on_disk.contains("\"cleanup\""), "{on_disk}");
        assert!(!on_disk.contains("\"timers\""), "{on_disk}");
        let reloaded: TimersState = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(reloaded.timers, state.timers);
    }

    #[test]
    fn running_timer_run_omits_duration() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());
        let t = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        state.set_timer_state("backup", TimerState::running(None, t));
        let run = &state.timers["backup"];
        assert_eq!(run.status, TimerStatus::Running);
        assert!(run.duration_ms.is_none());

        // A `Running` record with no prior history serializes without
        // `duration_ms` or `consecutive_failures` (both omitted when empty).
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"status\":\"running\""), "{json}");
        assert!(!json.contains("duration_ms"), "{json}");
        assert!(!json.contains("consecutive_failures"), "{json}");

        // Completing the run overwrites the `Running` record in place.
        state.set_timer_state("backup", TimerState::success(t, Duration::from_millis(500)));
        assert_eq!(state.timers.len(), 1);
        assert_eq!(state.timers["backup"].status, TimerStatus::Success);
        assert_eq!(state.timers["backup"].duration_ms, Some(500));
    }

    #[test]
    fn timer_carries_last_success_and_counts_consecutive_failures() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let prev = |s: &TimersState| s.timers.get("job").cloned();

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state("job", TimerState::success(t0, Duration::from_millis(10)));

        // First failure: last success is preserved, counter goes to 1.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        let p = prev(&state);
        state.set_timer_state(
            "job",
            TimerState::failed(p.as_ref(), t1, Duration::from_millis(20), "boom"),
        );
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.failure.as_ref().map_or(0, |f| f.count), 1);

        // Second failure: counter climbs, last success still preserved.
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_120, 0).unwrap();
        let p = prev(&state);
        state.set_timer_state(
            "job",
            TimerState::failed(p.as_ref(), t2, Duration::from_millis(20), "boom"),
        );
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.failure.as_ref().map_or(0, |f| f.count), 2);

        // Success resets the counter and advances the last-success marker.
        let t3 = DateTime::<Utc>::from_timestamp(1_700_000_240, 0).unwrap();
        state.set_timer_state("job", TimerState::success(t3, Duration::from_millis(10)));
        let job = &state.timers["job"];
        assert!(job.failure.is_none());
        assert_eq!(job.last_success_at, Some(t3));
    }

    #[test]
    fn empty_timers_map_serializes_as_bare_object() {
        let dir = tempfile::tempdir().unwrap();
        let state = timers_at(dir.path());
        // Transparent struct over an empty map -> a bare `{}`, no `timers` key.
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "{}", "{json}");
    }
}
