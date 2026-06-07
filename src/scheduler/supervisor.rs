use std::{
    collections::{
        HashMap,
        HashSet,
    },
    env,
    io,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Child,
        Command,
        Stdio,
    },
    thread,
    time::{
        Duration,
        Instant,
    },
};

use crate::{
    MainContext,
    config::UnitName,
    log,
    podman,
    state::{
        DeployState,
        DeployStatus,
    },
};

/// First delay before a crashed container is restarted.
/// Doubles per consecutive crash up to [`RESTART_BACKOFF_CAP`].
const RESTART_BACKOFF_BASE: Duration = Duration::from_secs(2);

/// Ceiling on the exponential restart backoff.
const RESTART_BACKOFF_CAP: Duration = Duration::from_secs(60);

/// A child that ran at least this long before dying is treated as a fresh
/// transient crash, not part of a loop: the backoff resets to the base.
const STABLE_RUN: Duration = Duration::from_secs(30);

/// Cap the doubling exponent so the `1 << n` shift can't overflow; the result is
/// clamped to [`RESTART_BACKOFF_CAP`] long before this bites.
const MAX_BACKOFF_SHIFT: u32 = 16;

/// Upper bound on waiting for a child to exit during shutdown before SIGKILL.
const STOP_REAP_TIMEOUT: Duration = Duration::from_secs(30);

/// What the supervisor should do with a unit's container this tick, derived
/// from its deploy status.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// `Check`/`Ready`: keep the container running, restart it if it dies.
    Ensure,

    /// `Building`/`Idle`: a deploy is mid-flight or the unit isn't live - leave
    /// a running container alone (don't start, don't stop), just reap an exit.
    Passive,

    /// `Failed`/no state: stop and forget the container.
    Stop,
}

fn action_for(status: DeployStatus) -> Action {
    match status {
        DeployStatus::Check | DeployStatus::Ready => Action::Ensure,
        DeployStatus::Building | DeployStatus::Idle => Action::Passive,
        DeployStatus::Failed => Action::Stop,
    }
}

/// One supervised container across ticks.
struct ManagedUnit {
    name: UnitName,
    /// The `dpl start <unit>` child, while one is running.
    child: Option<Child>,
    /// When the current child was spawned; lets a death decide whether the run
    /// was stable enough to reset the backoff.
    started_at: Option<Instant>,
    /// When set, don't respawn until this instant (post-crash backoff).
    backoff_until: Option<Instant>,
    /// Consecutive crashes without a stable run; drives the exponential backoff.
    consecutive_failures: u32,
    /// active_version observed when adopted; a redeploy bumps it and re-adopts.
    active_version: Option<u32>,
}

impl ManagedUnit {
    fn new(name: UnitName, active_version: Option<u32>) -> Self {
        ManagedUnit {
            name,
            child: None,
            started_at: None,
            backoff_until: None,
            consecutive_failures: 0,
            active_version,
        }
    }

    /// Record a crash and arm the next (exponential) backoff.
    fn note_crash(&mut self, now: Instant) {
        let stable = self
            .started_at
            .is_some_and(|started| now.duration_since(started) >= STABLE_RUN);
        self.consecutive_failures = if stable {
            1
        } else {
            self.consecutive_failures + 1
        };
        self.child = None;
        self.started_at = None;
        self.backoff_until = Some(now + backoff_delay(self.consecutive_failures));
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    RESTART_BACKOFF_BASE
        .saturating_mul(1 << exp)
        .min(RESTART_BACKOFF_CAP)
}

/// Launches and supervises long-running unit containers for the `dpl serve`
/// daemon. It spawns `dpl start <unit>` as a child, watches it, and restarts it
/// when it dies. Reconciled once per serve tick (and on SIGHUP).
///
/// The managed set comes entirely from deploy state files.
pub struct Supervisor {
    managed: HashMap<UnitName, ManagedUnit>,
    /// Cached `dpl` binary path; `None` disables spawning (non-fatal).
    self_exe: Option<PathBuf>,
}

impl Supervisor {
    pub fn new() -> Self {
        let self_exe = match env::current_exe() {
            Ok(path) => Some(path),
            Err(err) => {
                log::warn(format!("supervisor: resolve own binary path: {err}"));
                None
            }
        };
        Supervisor {
            managed: HashMap::new(),
            self_exe,
        }
    }

    /// One reconcile pass: stop containers no longer wanted, then act on each
    /// runtime unit per its deploy status. Returns the earliest pending respawn
    /// deadline so the caller can wake exactly then instead of polling blindly.
    pub fn reconcile(&mut self, ctx: &MainContext) -> Option<Instant> {
        let exe = self.self_exe.clone()?;

        // Every unit handed off to serve (the `supervised` flag), straight from
        // deploy state - the configs are never read.
        let desired: Vec<(UnitName, DeployStatus, Option<u32>)> = DeployState::list(ctx)
            .into_iter()
            .filter(|(_, state)| state.supervised)
            .map(|(name, state)| (name, state.last_status, state.active_version))
            .collect();
        let desired_names: HashSet<&UnitName> = desired.iter().map(|(name, ..)| name).collect();

        // Drop containers we manage that are no longer in the supervised set.
        self.managed.retain(|name, m| {
            if desired_names.contains(name) {
                return true;
            }
            stop_child(m);
            false
        });

        for (name, status, active) in &desired {
            match action_for(*status) {
                Action::Stop => {
                    if let Some(mut m) = self.managed.remove(name) {
                        stop_child(&mut m);
                    }
                }
                Action::Passive => {
                    if let Some(m) = self.managed.get_mut(name) {
                        reap(m);
                    }
                }
                Action::Ensure => {
                    let unit = self
                        .managed
                        .entry(name.clone())
                        .or_insert_with(|| ManagedUnit::new(name.clone(), *active));
                    drive(ctx, &exe, unit, *active);
                }
            }
        }

        next_wake(&self.managed)
    }

    /// Stop every supervised container and reap its child. Called once when the
    /// daemon shuts down.
    pub fn shutdown(&mut self) {
        for m in self.managed.values() {
            if m.child.is_some()
                && let Err(err) = podman::stop_and_remove(&m.name)
            {
                log::warn(format!("supervisor: stop '{}': {err}", m.name.as_str()));
            }
        }

        let deadline = Instant::now() + STOP_REAP_TIMEOUT;
        for m in self.managed.values_mut() {
            if let Some(child) = m.child.as_mut() {
                reap_until(child, deadline);
            }
        }
        self.managed.clear();
    }
}

/// Earliest pending respawn: the soonest backoff among managed units with no live
/// child (those awaiting a respawn). `None` = nothing pending. After a reconcile
/// pass any surviving `backoff_until` is in the future, since `drive` spawns once
/// `now >= backoff_until`.
fn next_wake(managed: &HashMap<UnitName, ManagedUnit>) -> Option<Instant> {
    managed
        .values()
        .filter(|m| m.child.is_none())
        .filter_map(|m| m.backoff_until)
        .min()
}

/// Ensure a `Check`/`Ready` unit's container is running: reap a dead child and
/// schedule a backoff restart, or spawn one once the backoff elapses. A
/// redeploy (new active version) re-adopts from scratch.
fn drive(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, active: Option<u32>) {
    if m.active_version != active {
        stop_child(m);
        m.active_version = active;
        m.backoff_until = None;
        m.consecutive_failures = 0;
    }

    let now = Instant::now();
    if let Some(child) = m.child.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => {
                log::warn(format!(
                    "supervisor: '{}' exited; restarting",
                    m.name.as_str()
                ));
                m.note_crash(now);
            }
            Ok(None) => {}
            Err(err) => {
                log::warn(format!("supervisor: wait '{}': {err}", m.name.as_str()));
                m.note_crash(now);
            }
        }
    } else if !podman::is_running(&m.name) {
        // No child and nothing running: spawn once the backoff (if any) elapses.
        // A container that IS running without a child is one a prior serve
        // started (adopted across a serve restart); leave it until it dies.
        if m.backoff_until.is_none_or(|until| now >= until) {
            spawn(ctx, exe, m);
        }
    }
}

/// Spawn `dpl start <unit>`; back off (exponentially) on spawn failure.
fn spawn(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    let now = Instant::now();
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(child) => {
            m.child = Some(child);
            m.started_at = Some(now);
            m.backoff_until = None;
        }
        Err(err) => {
            log::warn(format!("supervisor: start '{}': {err}", m.name.as_str()));
            m.consecutive_failures += 1;
            m.backoff_until = Some(now + backoff_delay(m.consecutive_failures));
        }
    }
}

/// `dpl --base <base> start <unit>` as a child. It writes its own runtime.log
/// via CriLog, so its std streams are dropped here (nothing drains them).
fn spawn_child(exe: &Path, base: &Path, name: &UnitName) -> io::Result<Child> {
    Command::new(exe)
        .arg("--base")
        .arg(base)
        .arg("start")
        .arg(name.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// Reap an exited child without restarting (used while a unit is `Passive`).
fn reap(m: &mut ManagedUnit) {
    if let Some(child) = m.child.as_mut()
        && matches!(child.try_wait(), Ok(Some(_)))
    {
        m.child = None;
    }
}

/// Stop the unit's container and reap its child. `podman stop` makes the child's
/// `supervise()` loop exit.
fn stop_child(m: &mut ManagedUnit) {
    if let Err(err) = podman::stop_and_remove(&m.name) {
        log::warn(format!("supervisor: stop '{}': {err}", m.name.as_str()));
    }
    if let Some(mut child) = m.child.take() {
        reap_until(&mut child, Instant::now() + STOP_REAP_TIMEOUT);
    }
}

/// Wait for a child to exit, SIGKILL it once `deadline` passes.
fn reap_until(child: &mut Child, deadline: Instant) {
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn ctx() -> (TempDir, MainContext) {
        let dir = TempDir::new().unwrap();
        let ctx = MainContext {
            base: dir.path().to_path_buf(),
            master_key: None,
        };
        (dir, ctx)
    }

    fn managed_with(name: &str, backoff_until: Option<Instant>) -> (UnitName, ManagedUnit) {
        let name = UnitName::new(name).unwrap();
        let mut m = ManagedUnit::new(name.clone(), None);
        m.backoff_until = backoff_until;
        (name, m)
    }

    #[test]
    fn next_wake_returns_soonest_pending_backoff() {
        let now = Instant::now();
        let t0 = now + Duration::from_secs(5);
        let t1 = now + Duration::from_secs(30);

        let mut managed = HashMap::new();
        managed.extend([
            managed_with("late", Some(t1)),
            managed_with("soon", Some(t0)),
            managed_with("none", None),
        ]);

        assert_eq!(next_wake(&managed), Some(t0));
    }

    #[test]
    fn next_wake_empty_is_none() {
        assert!(next_wake(&HashMap::new()).is_none());
    }

    #[test]
    fn backoff_delay_doubles_then_caps() {
        assert_eq!(backoff_delay(1), RESTART_BACKOFF_BASE);
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(3), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(8));
        // Clamped at the cap, and the huge-exponent path stays clamped (no shift
        // overflow).
        assert_eq!(backoff_delay(20), RESTART_BACKOFF_CAP);
        assert_eq!(backoff_delay(u32::MAX), RESTART_BACKOFF_CAP);
    }

    #[test]
    fn crash_escalates_then_resets_after_stable_run() {
        let name = UnitName::new("app").unwrap();
        let now = Instant::now();
        let mut m = ManagedUnit::new(name, None);

        // A fast crash (ran < STABLE_RUN) escalates the streak.
        m.started_at = Some(now - Duration::from_secs(1));
        m.note_crash(now);
        assert_eq!(m.consecutive_failures, 1);
        assert_eq!(m.backoff_until, Some(now + backoff_delay(1)));

        m.started_at = Some(now - Duration::from_secs(1));
        m.note_crash(now);
        assert_eq!(m.consecutive_failures, 2);
        assert_eq!(m.backoff_until, Some(now + backoff_delay(2)));

        // A crash after a stable run resets the streak to a single failure.
        m.started_at = Some(now - (STABLE_RUN + Duration::from_secs(1)));
        m.note_crash(now);
        assert_eq!(m.consecutive_failures, 1);
        assert_eq!(m.backoff_until, Some(now + backoff_delay(1)));
        assert!(m.child.is_none());
        assert!(m.started_at.is_none());
    }

    #[test]
    fn action_follows_deploy_status() {
        assert_eq!(action_for(DeployStatus::Check), Action::Ensure);
        assert_eq!(action_for(DeployStatus::Ready), Action::Ensure);
        assert_eq!(action_for(DeployStatus::Building), Action::Passive);
        assert_eq!(action_for(DeployStatus::Idle), Action::Passive);
        assert_eq!(action_for(DeployStatus::Failed), Action::Stop);
    }

    /// Mark a unit as deployed through the serve hand-off (`set_check`), so its
    /// state file carries the `supervised` flag.
    fn deploy_supervised(ctx: &MainContext, name: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.bump_version().unwrap();
        state.set_check();
    }

    /// Mark a unit deployed without a serve hand-off (`set_ready` only), as db
    /// and domain units do - no `supervised` flag, no container.
    fn deploy_plain(ctx: &MainContext, name: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.bump_version().unwrap();
        state.set_ready();
    }

    fn supervised_set(ctx: &MainContext) -> Vec<UnitName> {
        DeployState::list(ctx)
            .into_iter()
            .filter(|(_, state)| state.supervised)
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn supervised_set_comes_from_state_not_config() {
        let (_dir, ctx) = ctx();

        // Container units hand off via set_check; db/domain only set_ready.
        deploy_supervised(&ctx, "app-live");
        deploy_supervised(&ctx, "pg");
        deploy_plain(&ctx, "dbx");
        deploy_plain(&ctx, "site-com");
        // Configured but never deployed -> no state file -> not supervised.
        ctx.write_test_unit("app-static", "type: app\nimage: alpine\nbuilds: []\n");

        assert_eq!(
            supervised_set(&ctx),
            vec![
                UnitName::new("app-live").unwrap(),
                UnitName::new("pg").unwrap(),
            ]
        );
    }

    #[test]
    fn supervised_flag_is_sticky_across_status_transitions() {
        let (_dir, ctx) = ctx();
        let name = UnitName::new("app-live").unwrap();

        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.bump_version().unwrap();
        // Building: handed off not yet, so not supervised.
        assert!(!state.supervised);
        assert_eq!(action_for(state.last_status), Action::Passive);

        state.set_check();
        assert!(state.supervised);
        assert_eq!(state.last_status, DeployStatus::Check);
        assert_eq!(action_for(state.last_status), Action::Ensure);

        // Ready keeps the flag set; a later redeploy (Building/Failed) too.
        state.set_ready();
        assert!(state.supervised);
        assert_eq!(action_for(state.last_status), Action::Ensure);
        state.set_failed(crate::state::DeployStage::Build, "boom".into());
        assert!(state.supervised);
        assert_eq!(action_for(state.last_status), Action::Stop);
    }
}
