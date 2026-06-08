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
    Local,
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
    /// The named timer isn't registered in the unit's timer state - unknown,
    /// disabled, or not yet reconciled.
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
    /// Registered with `dpl serve`, awaiting its first run.
    Idle,
    /// A run is in progress.
    Running,
    /// The last run succeeded.
    Success { duration_ms: u64 },
    /// The last run failed.
    Failed { duration_ms: u64, error: String },
    /// The schedule has no upcoming occurrence.
    Invalid { error: String },
}

/// The most recent record of one of a unit's timers.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct TimerState {
    #[serde(flatten)]
    pub outcome: TimerOutcome,

    /// Cron schedule this timer fires on.
    pub schedule: Cron,

    /// When the timer should next fire (UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<DateTime<Utc>>,

    /// Anchor for the next-occurrence computation: the start of the most recent
    /// run, or the seeded registration time for an `Idle` timer that never ran.
    pub last_run_at: DateTime<Utc>,

    /// When the timer last completed successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,

    /// Consecutive failed runs since the last success; 0 while healthy.
    #[serde(default, skip_serializing_if = "crate::config::is_default")]
    pub consecutive_failures: u32,
}

impl TimerState {
    /// Next occurrence of `schedule` strictly after `after`.
    /// The cron fields in the host's local time and returned in UTC.
    /// `None` when the schedule has no upcoming occurrence (an impossible date).
    pub fn next_occurrence(schedule: &Cron, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let after_local = after.with_timezone(&Local);
        schedule
            .find_next_occurrence(&after_local, false)
            .ok()
            .map(|next| next.with_timezone(&Utc))
    }

    /// A timer registered but not yet run.
    pub fn idle(
        schedule: Cron,
        last_run_at: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
    ) -> Self {
        TimerState {
            outcome: TimerOutcome::Idle,
            schedule,
            next_run,
            last_run_at,
            last_success_at: None,
            consecutive_failures: 0,
        }
    }

    /// Begin a run starting at `last_run_at`. The schedule and failure streak
    /// carry forward from the prior record (`self`); the run's own
    /// `last_run_at`/`next_run` come from the caller.
    pub fn running(&self, last_run_at: DateTime<Utc>, next_run: Option<DateTime<Utc>>) -> Self {
        TimerState {
            outcome: TimerOutcome::Running,
            schedule: self.schedule.clone(),
            next_run,
            last_run_at,
            last_success_at: self.last_success_at,
            consecutive_failures: self.consecutive_failures,
        }
    }

    /// Complete the in-progress run successfully. Schedule, run start and
    /// next_run are taken from `self` (the `Running` record).
    pub fn success(&self, duration: Duration) -> Self {
        TimerState {
            outcome: TimerOutcome::Success {
                duration_ms: duration.as_millis() as u64,
            },
            schedule: self.schedule.clone(),
            next_run: self.next_run,
            last_run_at: self.last_run_at,
            last_success_at: Some(self.last_run_at),
            consecutive_failures: 0,
        }
    }

    /// Complete the in-progress run with a failure, advancing the streak.
    /// Schedule, run start, next_run and the carried `last_success_at`/streak
    /// all come from `self` (the `Running` record).
    pub fn failed(&self, duration: Duration, error: impl Into<String>) -> Self {
        TimerState {
            outcome: TimerOutcome::Failed {
                duration_ms: duration.as_millis() as u64,
                error: error.into(),
            },
            schedule: self.schedule.clone(),
            next_run: self.next_run,
            last_run_at: self.last_run_at,
            last_success_at: self.last_success_at,
            consecutive_failures: self.consecutive_failures.saturating_add(1),
        }
    }

    /// A timer whose schedule has no upcoming occurrence.
    pub fn invalid(schedule: Cron, last_run_at: DateTime<Utc>, error: impl Into<String>) -> Self {
        TimerState {
            outcome: TimerOutcome::Invalid {
                error: error.into(),
            },
            schedule,
            next_run: None,
            last_run_at,
            last_success_at: None,
            consecutive_failures: 0,
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

    /// Reconcile persisted timer state with configured timers.
    pub fn reconcile(&mut self, configured: &[(String, Cron)], anchor: DateTime<Utc>) {
        let names: BTreeSet<&str> = configured.iter().map(|(name, _)| name.as_str()).collect();
        self.timers.retain(|name, _| names.contains(name.as_str()));

        for (name, schedule) in configured {
            // No upcoming occurrence: park as invalid (replacing any prior
            // record). Visible in inspect, but next_run is None so it never fires.
            let Some(next) = TimerState::next_occurrence(schedule, anchor) else {
                let error = "schedule has no upcoming occurrence";
                crate::log::warn(format!("timer '{name}': {error}"));
                self.timers.insert(
                    name.clone(),
                    TimerState::invalid(schedule.clone(), anchor, error),
                );
                continue;
            };

            match self.timers.get_mut(name) {
                None => {
                    self.timers.insert(
                        name.clone(),
                        TimerState::idle(schedule.clone(), anchor, Some(next)),
                    );
                }

                Some(state) if matches!(state.outcome, TimerOutcome::Invalid { .. }) => {
                    *state = TimerState::idle(schedule.clone(), anchor, Some(next));
                }

                Some(state) if state.schedule != *schedule => {
                    state.schedule = schedule.clone();
                    state.next_run = Some(next);
                }

                Some(_) => {}
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
            crate::log::warn(format!(
                "remove timer lock file {}: {err}",
                self.path.display()
            ));
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

    /// The next_run a constructor should store for a schedulable timer. Asserted
    /// against rather than a hardcoded instant, since the value is timezone-dependent.
    fn next(expr: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        TimerState::next_occurrence(&cron(expr), after)
    }

    /// The `Running` pivot for `name`'s next run at `at`, mirroring run_timer:
    /// carry the prior record forward, or seed an idle baseline if absent. Call
    /// `.success(..)`/`.failed(..)` on the result to record the run's outcome.
    fn attempt(state: &TimersState, name: &str, expr: &str, at: DateTime<Utc>) -> TimerState {
        state
            .timers
            .get(name)
            .cloned()
            .unwrap_or_else(|| TimerState::idle(cron(expr), at, next(expr, at)))
            .running(at, next(expr, at))
    }

    #[test]
    fn record_timer_run_persists_last_run_per_timer() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let cleanup =
            attempt(&state, "cleanup", "0 3 * * *", t0).success(Duration::from_millis(1840));
        state.set_timer_state("cleanup", cleanup);
        let sync =
            attempt(&state, "sync", "0 * * * *", t0).failed(Duration::from_secs(2), "exit code 1");
        state.set_timer_state("sync", sync);

        // A second run of the same timer replaces the first.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        let cleanup =
            attempt(&state, "cleanup", "0 3 * * *", t1).success(Duration::from_millis(990));
        state.set_timer_state("cleanup", cleanup);

        assert_eq!(state.timers.len(), 2);
        let cleanup = &state.timers["cleanup"];
        assert_eq!(cleanup.outcome, TimerOutcome::Success { duration_ms: 990 });
        assert_eq!(cleanup.last_run_at, t1);
        assert_eq!(cleanup.last_success_at, Some(t1));
        assert_eq!(cleanup.next_run, next("0 3 * * *", t1));
        assert!(cleanup.next_run > Some(t1));
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

        let running = attempt(&state, "backup", "0 * * * *", t);
        state.set_timer_state("backup", running.clone());
        let run = &state.timers["backup"];
        assert_eq!(run.outcome, TimerOutcome::Running);

        // A `Running` record with no prior history serializes without
        // `duration_ms` or `consecutive_failures` (both belong elsewhere/omitted).
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"status\":\"running\""), "{json}");
        assert!(!json.contains("duration_ms"), "{json}");
        assert!(!json.contains("consecutive_failures"), "{json}");

        // Completing the run overwrites the `Running` record in place.
        state.set_timer_state("backup", running.success(Duration::from_millis(500)));
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

        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t0).success(Duration::from_millis(10));
        state.set_timer_state("job", run);

        // First failure: last success is preserved, counter goes to 1.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t1).failed(Duration::from_millis(20), "boom");
        state.set_timer_state("job", run);
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.consecutive_failures, 1);

        // Second failure: counter climbs, last success still preserved.
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_120, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t2).failed(Duration::from_millis(20), "boom");
        state.set_timer_state("job", run);
        let job = &state.timers["job"];
        assert_eq!(job.last_success_at, Some(t0));
        assert_eq!(job.consecutive_failures, 2);

        // Success resets the counter and advances the last-success marker.
        let t3 = DateTime::<Utc>::from_timestamp(1_700_000_240, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t3).success(Duration::from_millis(10));
        state.set_timer_state("job", run);
        let job = &state.timers["job"];
        assert_eq!(job.consecutive_failures, 0);
        assert_eq!(job.last_success_at, Some(t3));
    }

    #[test]
    fn idle_seeds_anchor_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());
        let anchor = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        state.set_timer_state(
            "backup",
            TimerState::idle(cron("0 2 * * *"), anchor, next("0 2 * * *", anchor)),
        );
        let idle = &state.timers["backup"];
        assert_eq!(idle.outcome, TimerOutcome::Idle);
        // The anchor is stored as last_run_at even though the timer never ran.
        assert_eq!(idle.last_run_at, anchor);
        assert_eq!(idle.schedule, cron("0 2 * * *"));
        // next_run is the first occurrence after the registration anchor.
        assert_eq!(idle.next_run, next("0 2 * * *", anchor));
        assert!(idle.next_run > Some(anchor));
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
        let run = attempt(&state, "job", "0 * * * *", t0).failed(Duration::from_millis(5), "boom");
        state.set_timer_state("job", run);

        // The next run is left in `Running` (as a crash would), carrying the streak.
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_060, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t1);
        state.set_timer_state("job", run);
        let job = &state.timers["job"];
        assert_eq!(job.outcome, TimerOutcome::Running);
        assert_eq!(job.consecutive_failures, 1);

        // The next failure, reading the carried `Running` record, climbs to 2.
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_120, 0).unwrap();
        let run = attempt(&state, "job", "0 * * * *", t2).failed(Duration::from_millis(5), "boom");
        state.set_timer_state("job", run);
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
        state.set_timer_state(
            "keep",
            TimerState::idle(cron("0 * * * *"), t0, next("0 * * * *", t0)),
        );
        let change = attempt(&state, "change", "0 3 * * *", t0).success(Duration::from_millis(5));
        state.set_timer_state("change", change);
        state.set_timer_state(
            "gone",
            TimerState::idle(cron("0 0 * * *"), t0, next("0 0 * * *", t0)),
        );

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

        // "keep" is untouched: same anchor, still idle, same schedule, and its
        // next_run is left at the originally-seeded value (not refreshed).
        let keep = &state.timers["keep"];
        assert_eq!(keep.outcome, TimerOutcome::Idle);
        assert_eq!(keep.last_run_at, t0);
        assert_eq!(keep.schedule, cron("0 * * * *"));
        assert_eq!(keep.next_run, next("0 * * * *", t0));

        // "change" picks up the new schedule and a next_run recomputed from it,
        // but keeps its run history.
        let change = &state.timers["change"];
        assert_eq!(change.schedule, cron("30 4 * * *"));
        assert_eq!(change.outcome, TimerOutcome::Success { duration_ms: 5 });
        assert_eq!(change.last_run_at, t0);
        assert_eq!(change.next_run, next("30 4 * * *", anchor));

        // "new" is seeded as idle, anchored at the reconcile time, with its
        // first next_run computed from that anchor.
        let new = &state.timers["new"];
        assert_eq!(new.outcome, TimerOutcome::Idle);
        assert_eq!(new.last_run_at, anchor);
        assert_eq!(new.schedule, cron("15 * * * *"));
        assert_eq!(new.next_run, next("15 * * * *", anchor));
    }

    #[test]
    fn reconcile_parks_timer_with_undeterminable_next_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());

        // Feb 30 never occurs, so find_next_occurrence can't resolve a next run.
        let ghost = cron("0 0 30 2 *");
        assert!(
            TimerState::next_occurrence(&ghost, Utc::now()).is_none(),
            "expected an impossible schedule to have no next occurrence",
        );

        let anchor = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let configured = vec![
            ("ok".to_string(), cron("0 * * * *")),
            ("ghost".to_string(), ghost),
        ];
        state.reconcile(&configured, anchor);

        // Both stay registered: the dead one is parked as Invalid with no
        // next_run, so it shows in `dpl inspect` but never fires.
        let names: Vec<&str> = state.timers.keys().map(String::as_str).collect();
        assert_eq!(names, ["ghost", "ok"]);

        let ghost = &state.timers["ghost"];
        assert!(matches!(ghost.outcome, TimerOutcome::Invalid { .. }));
        assert!(ghost.next_run.is_none());

        let ok = &state.timers["ok"];
        assert_eq!(ok.outcome, TimerOutcome::Idle);
        assert_eq!(ok.next_run, next("0 * * * *", anchor));
    }

    #[test]
    fn reconcile_parks_then_recovers_on_schedule_edit() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = timers_at(dir.path());
        let t0 = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();

        // A healthy timer with run history.
        let job = attempt(&state, "job", "0 * * * *", t0).success(Duration::from_millis(5));
        state.set_timer_state("job", job);

        // Editing it to an impossible schedule parks it: visible, but unscheduled.
        let a1 = DateTime::<Utc>::from_timestamp(1_700_000_500, 0).unwrap();
        state.reconcile(&[("job".to_string(), cron("0 0 30 2 *"))], a1);
        let job = &state.timers["job"];
        assert!(matches!(job.outcome, TimerOutcome::Invalid { .. }));
        assert!(job.next_run.is_none());

        // Editing back to a valid schedule re-seeds it as idle and schedulable.
        let a2 = DateTime::<Utc>::from_timestamp(1_700_001_000, 0).unwrap();
        state.reconcile(&[("job".to_string(), cron("15 * * * *"))], a2);
        let job = &state.timers["job"];
        assert_eq!(job.outcome, TimerOutcome::Idle);
        assert_eq!(job.next_run, next("15 * * * *", a2));
    }
}
