use std::{
    collections::{
        BTreeMap,
        BTreeSet,
    },
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
use croner::Cron;
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

/// A timer's outcome, carrying the data specific to that outcome.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TimerOutcome {
    /// Registered with the scheduler, awaiting its first run.
    Idle,
    /// A run is in progress.
    Running,
    /// The last run succeeded.
    Success { duration_ms: u64 },
    /// The last run failed.
    Failed { duration_ms: u64, error: String },
}

/// The most recent record of one of a unit's timers.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct TimerState {
    #[serde(flatten)]
    pub outcome: TimerOutcome,

    /// Cron schedule this timer fires on.
    pub schedule: Cron,

    /// Anchor for the next-occurrence computation: the start of the most recent
    /// run, or the seeded registration time for an `Idle` timer that never ran.
    pub last_run_at: DateTime<Utc>,

    /// When the timer last completed successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,

    /// Consecutive failed runs since the last success; 0 while healthy.
    pub consecutive_failures: u32,
}

impl TimerState {
    /// A timer registered but not yet run.
    pub fn idle(schedule: Cron, anchor: DateTime<Utc>) -> Self {
        TimerState {
            outcome: TimerOutcome::Idle,
            schedule,
            last_run_at: anchor,
            last_success_at: None,
            consecutive_failures: 0,
        }
    }

    pub fn running(schedule: Cron, prev: Option<&TimerState>, last_run_at: DateTime<Utc>) -> Self {
        TimerState {
            outcome: TimerOutcome::Running,
            schedule,
            last_run_at,
            last_success_at: prev.and_then(|p| p.last_success_at),
            consecutive_failures: prev.map_or(0, |p| p.consecutive_failures),
        }
    }

    pub fn success(schedule: Cron, last_run_at: DateTime<Utc>, duration: Duration) -> Self {
        TimerState {
            outcome: TimerOutcome::Success {
                duration_ms: duration.as_millis() as u64,
            },
            schedule,
            last_run_at,
            last_success_at: Some(last_run_at),
            consecutive_failures: 0,
        }
    }

    pub fn failed(
        schedule: Cron,
        prev: Option<&TimerState>,
        last_run_at: DateTime<Utc>,
        duration: Duration,
        error: impl Into<String>,
    ) -> Self {
        TimerState {
            outcome: TimerOutcome::Failed {
                duration_ms: duration.as_millis() as u64,
                error: error.into(),
            },
            schedule,
            last_run_at,
            last_success_at: prev.and_then(|p| p.last_success_at),
            consecutive_failures: prev.map_or(0, |p| p.consecutive_failures).saturating_add(1),
        }
    }
}

/// On-disk record of each timer's most recent run, stored separately from the
/// deploy state so timer runs and deploys never clobber each other's writes.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Debug))]
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

    /// Reconcile persisted state against a unit's configured timers in one
    /// write: add, update changed schedules, drop removed. `anchor` seeds
    /// newly added timers.
    pub fn reconcile(&mut self, configured: &[(String, Cron)], anchor: DateTime<Utc>) {
        let names: BTreeSet<&str> = configured.iter().map(|(name, _)| name.as_str()).collect();
        self.timers.retain(|name, _| names.contains(name.as_str()));

        for (name, schedule) in configured {
            match self.timers.get_mut(name) {
                Some(state) => {
                    if state.schedule != *schedule {
                        state.schedule = schedule.clone();
                    }
                }
                None => {
                    let timer = TimerState::idle(schedule.clone(), anchor);
                    self.timers.insert(name.clone(), timer);
                }
            }
        }

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

    fn cron(expr: &str) -> Cron {
        expr.parse().unwrap()
    }

    #[test]
    fn record_timer_run_persists_last_run_per_timer() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state(
            "cleanup",
            TimerState::success(cron("0 3 * * *"), t0, Duration::from_millis(1840)),
        );
        state.set_timer_state(
            "sync",
            TimerState::failed(
                cron("0 * * * *"),
                None,
                t0,
                Duration::from_secs(2),
                "exit code 1",
            ),
        );

        // A second run of the same timer replaces the first.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        state.set_timer_state(
            "cleanup",
            TimerState::success(cron("0 3 * * *"), t1, Duration::from_millis(990)),
        );

        assert_eq!(state.timers.len(), 2);
        let cleanup = &state.timers["cleanup"];
        assert_eq!(cleanup.outcome, TimerOutcome::Success { duration_ms: 990 });
        assert_eq!(cleanup.last_run_at, t1);
        assert_eq!(cleanup.last_success_at, Some(t1));
        assert_eq!(cleanup.consecutive_failures, 0);

        let sync = &state.timers["sync"];
        assert!(matches!(sync.outcome, TimerOutcome::Failed { .. }));
        assert_eq!(sync.consecutive_failures, 1);
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

        state.set_timer_state("backup", TimerState::running(cron("0 * * * *"), None, t));
        let run = &state.timers["backup"];
        assert_eq!(run.outcome, TimerOutcome::Running);

        // A `Running` record with no prior history serializes without
        // `duration_ms` or `consecutive_failures` (both belong elsewhere/omitted).
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"status\":\"running\""), "{json}");
        assert!(!json.contains("duration_ms"), "{json}");
        assert!(!json.contains("consecutive_failures"), "{json}");

        // Completing the run overwrites the `Running` record in place.
        state.set_timer_state(
            "backup",
            TimerState::success(cron("0 * * * *"), t, Duration::from_millis(500)),
        );
        assert_eq!(state.timers.len(), 1);
        assert_eq!(
            state.timers["backup"].outcome,
            TimerOutcome::Success { duration_ms: 500 }
        );
    }

    #[test]
    fn timer_carries_last_success_and_counts_consecutive_failures() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let prev = |s: &TimersState| s.timers.get("job").cloned();

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state(
            "job",
            TimerState::success(cron("0 * * * *"), t0, Duration::from_millis(10)),
        );

        // First failure: last success is preserved, counter goes to 1.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        let p = prev(&state);
        state.set_timer_state(
            "job",
            TimerState::failed(
                cron("0 * * * *"),
                p.as_ref(),
                t1,
                Duration::from_millis(20),
                "boom",
            ),
        );
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.consecutive_failures, 1);

        // Second failure: counter climbs, last success still preserved.
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_120, 0).unwrap();
        let p = prev(&state);
        state.set_timer_state(
            "job",
            TimerState::failed(
                cron("0 * * * *"),
                p.as_ref(),
                t2,
                Duration::from_millis(20),
                "boom",
            ),
        );
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.consecutive_failures, 2);

        // Success resets the counter and advances the last-success marker.
        let t3 = DateTime::<Utc>::from_timestamp(1_700_000_240, 0).unwrap();
        state.set_timer_state(
            "job",
            TimerState::success(cron("0 * * * *"), t3, Duration::from_millis(10)),
        );
        let job = &state.timers["job"];
        assert_eq!(job.consecutive_failures, 0);
        assert_eq!(job.last_success_at, Some(t3));
    }

    #[test]
    fn idle_seeds_anchor_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());
        let anchor = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        state.set_timer_state("backup", TimerState::idle(cron("0 2 * * *"), anchor));
        let idle = &state.timers["backup"];
        assert_eq!(idle.outcome, TimerOutcome::Idle);
        // The anchor is stored as last_run_at even though the timer never ran.
        assert_eq!(idle.last_run_at, anchor);
        assert_eq!(idle.schedule, cron("0 2 * * *"));
        assert!(idle.last_success_at.is_none());
        assert_eq!(idle.consecutive_failures, 0);

        let on_disk = read_to_string(dir.path().join(".timers.state")).unwrap();
        assert!(on_disk.contains("\"status\": \"idle\""), "{on_disk}");
        let reloaded: TimersState = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(reloaded.timers, state.timers);
    }

    #[test]
    fn running_carries_failure_streak_across_a_crash() {
        // A run killed mid-flight leaves a `Running` record on disk. The next
        // run reads it as `prev`, so the streak must survive the transition
        // through `Running` rather than resetting.
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        state.set_timer_state(
            "job",
            TimerState::failed(
                cron("0 * * * *"),
                None,
                t0,
                Duration::from_millis(5),
                "boom",
            ),
        );

        let prev = state.timers.get("job").cloned();
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        state.set_timer_state(
            "job",
            TimerState::running(cron("0 * * * *"), prev.as_ref(), t1),
        );
        let job = &state.timers["job"];
        assert_eq!(job.outcome, TimerOutcome::Running);
        assert_eq!(job.consecutive_failures, 1);

        // The next failure, reading the carried `Running` record, climbs to 2.
        let prev = state.timers.get("job").cloned();
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_120, 0).unwrap();
        state.set_timer_state(
            "job",
            TimerState::failed(
                cron("0 * * * *"),
                prev.as_ref(),
                t2,
                Duration::from_millis(5),
                "boom",
            ),
        );
        assert_eq!(state.timers["job"].consecutive_failures, 2);
    }

    #[test]
    fn empty_timers_map_serializes_as_bare_object() {
        let dir = tempfile::tempdir().unwrap();
        let state = timers_at(dir.path());
        // Transparent struct over an empty map -> a bare `{}`, no `timers` key.
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "{}", "{json}");
    }

    #[test]
    fn reconcile_adds_updates_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        // Pre-existing state: an idle timer, one with run history, and one that
        // the new config no longer mentions.
        state.set_timer_state("keep", TimerState::idle(cron("0 * * * *"), t0));
        state.set_timer_state(
            "change",
            TimerState::success(cron("0 3 * * *"), t0, Duration::from_millis(5)),
        );
        state.set_timer_state("gone", TimerState::idle(cron("0 0 * * *"), t0));

        let anchor = DateTime::<Utc>::from_timestamp(1_700_000_500, 0).unwrap();
        let configured = vec![
            ("keep".to_string(), cron("0 * * * *")),    // unchanged
            ("change".to_string(), cron("30 4 * * *")), // schedule edited
            ("new".to_string(), cron("15 * * * *")),    // newly added
        ];
        state.reconcile(&configured, anchor);

        // "gone" is dropped; the map is exactly the configured set.
        let names: Vec<&str> = state.timers.keys().map(String::as_str).collect();
        assert_eq!(names, ["change", "keep", "new"]);

        // "keep" is untouched: same anchor, still idle, same schedule.
        let keep = &state.timers["keep"];
        assert_eq!(keep.outcome, TimerOutcome::Idle);
        assert_eq!(keep.last_run_at, t0);
        assert_eq!(keep.schedule, cron("0 * * * *"));

        // "change" picks up the new schedule but keeps its run history.
        let change = &state.timers["change"];
        assert_eq!(change.schedule, cron("30 4 * * *"));
        assert_eq!(change.outcome, TimerOutcome::Success { duration_ms: 5 });
        assert_eq!(change.last_run_at, t0);

        // "new" is seeded as idle, anchored at the reconcile time.
        let new = &state.timers["new"];
        assert_eq!(new.outcome, TimerOutcome::Idle);
        assert_eq!(new.last_run_at, anchor);
        assert_eq!(new.schedule, cron("15 * * * *"));
    }
}
