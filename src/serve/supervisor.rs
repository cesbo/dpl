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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartPolicy {
    Never,
    Always,
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
    /// Whether this active version has already been started/adopted while in
    /// `Check`. Reset on redeploy; ignored once the unit is `Ready`.
    start_attempted: bool,
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
            start_attempted: false,
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

    fn note_exit_without_restart(&mut self) {
        self.child = None;
        self.started_at = None;
        self.backoff_until = None;
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    RESTART_BACKOFF_BASE
        .saturating_mul(1 << exp)
        .min(RESTART_BACKOFF_CAP)
}

/// Launches and supervises long-running unit containers for `dpl serve`.
/// It spawns `dpl start <unit>` as a child, watches it, and restarts it
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
            match status {
                DeployStatus::Failed => {
                    if let Some(mut m) = self.managed.remove(name) {
                        stop_child(&mut m);
                    }
                }
                DeployStatus::Building | DeployStatus::Idle => {
                    if let Some(m) = self.managed.get_mut(name) {
                        reap(m);
                    }
                }
                DeployStatus::Check => {
                    let unit = self
                        .managed
                        .entry(name.clone())
                        .or_insert_with(|| ManagedUnit::new(name.clone(), *active));
                    drive_start_once(ctx, &exe, unit, *active);
                }
                DeployStatus::Ready => {
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

    /// SIGCHLD handler: Reaps children and arms backoff for `Ready` units.
    /// Respawns are deferred to the next reconcile to avoid SIGCHLD loops.
    pub fn reap_exited(&mut self, ctx: &MainContext) -> Option<Instant> {
        let now = Instant::now();
        for m in self.managed.values_mut() {
            let restart_policy = restart_policy(ctx, m);
            poll_child(m, now, restart_policy);
        }
        next_wake(&self.managed)
    }

    /// Stop every supervised container and reap its child. Called once when
    /// `dpl serve` shuts down.
    pub fn shutdown(&mut self) {
        // Stop unconditionally, including adopted containers (child == None);
        // stop_and_remove is idempotent, so a stopped unit is a no-op.
        for m in self.managed.values() {
            if let Err(err) = podman::stop_and_remove(&m.name) {
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

fn reset_for_active_version(m: &mut ManagedUnit, active: Option<u32>) {
    if m.active_version != active {
        stop_child(m);
        m.active_version = active;
        m.backoff_until = None;
        m.consecutive_failures = 0;
        m.start_attempted = false;
    }
}

/// Start a `Check` unit's container once for the deploy health-check. If it
/// exits while still in `Check`, do not schedule a restart; the deploy attempt
/// will fail and move the state to `Failed`.
fn drive_start_once(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, active: Option<u32>) {
    reset_for_active_version(m, active);

    let now = Instant::now();
    if m.child.is_some() {
        poll_child(m, now, RestartPolicy::Never);
    } else if podman::is_running(&m.name) {
        // Adopted across a serve restart while the deploy health-check is still
        // running. Count it as the one startup attempt for this version.
        m.start_attempted = true;
    } else if !m.start_attempted {
        m.start_attempted = true;
        spawn_once(ctx, exe, m);
    }
}

/// Ensure a `Ready` unit's container is running: reap a dead child and
/// schedule a backoff restart, or spawn one once the backoff elapses. A
/// redeploy (new active version) re-adopts from scratch.
fn drive(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, active: Option<u32>) {
    reset_for_active_version(m, active);

    let now = Instant::now();
    if m.child.is_some() {
        poll_child(m, now, RestartPolicy::Always);
    } else if !podman::is_running(&m.name) {
        // No child and nothing running: spawn once the backoff (if any) elapses.
        // A container that IS running without a child is one a prior serve
        // started (adopted across a serve restart); leave it until it dies.
        if m.backoff_until.is_none_or(|until| now >= until) {
            spawn(ctx, exe, m);
        }
    }
}

/// Reap a supervised child if it has exited. Depending on the restart policy,
/// either arm its backoff or leave it stopped for the deploy health-check to
/// fail. No-op if it is still alive or absent.
fn poll_child(m: &mut ManagedUnit, now: Instant, restart: RestartPolicy) {
    let Some(child) = m.child.as_mut() else {
        return;
    };
    match child.try_wait() {
        Ok(Some(_)) => match restart {
            RestartPolicy::Always => {
                log::warn(format!(
                    "supervisor: '{}' exited; restarting",
                    m.name.as_str()
                ));
                m.note_crash(now);
            }
            RestartPolicy::Never => {
                log::warn(format!(
                    "supervisor: '{}' exited during startup check",
                    m.name.as_str()
                ));
                m.note_exit_without_restart();
            }
        },
        Ok(None) => {}
        Err(err) => {
            log::warn(format!("supervisor: wait '{}': {err}", m.name.as_str()));
            match restart {
                RestartPolicy::Always => m.note_crash(now),
                RestartPolicy::Never => m.note_exit_without_restart(),
            }
        }
    }
}

fn restart_policy(ctx: &MainContext, m: &ManagedUnit) -> RestartPolicy {
    let Ok(state) = DeployState::load(ctx, &m.name) else {
        return RestartPolicy::Never;
    };

    if state.supervised
        && state.last_status == DeployStatus::Ready
        && state.active_version == m.active_version
    {
        RestartPolicy::Always
    } else {
        RestartPolicy::Never
    }
}

/// Spawn `dpl start <unit>` once for the startup check. Spawn failure is left
/// for the deploy health-check to report; it must not arm a restart loop.
fn spawn_once(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(child) => {
            m.child = Some(child);
            m.started_at = Some(Instant::now());
            m.backoff_until = None;
        }
        Err(err) => {
            log::warn(format!("supervisor: start '{}': {err}", m.name.as_str()));
            m.child = None;
            m.started_at = None;
            m.backoff_until = None;
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

    fn spawn_exiting_child() -> Child {
        Command::new("sh").arg("-c").arg("exit 7").spawn().unwrap()
    }

    fn reap_until_child_cleared(supervisor: &mut Supervisor, ctx: &MainContext, name: &UnitName) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            supervisor.reap_exited(ctx);
            if supervisor
                .managed
                .get(name)
                .is_none_or(|m| m.child.is_none())
            {
                return;
            }
            assert!(Instant::now() < deadline, "child did not exit in time");
            thread::sleep(Duration::from_millis(10));
        }
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
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
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

    /// Mark a unit as deployed through the serve hand-off (`set_check`), so its
    /// state file carries the `supervised` flag.
    fn deploy_supervised(ctx: &MainContext, name: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_check();
    }

    /// Mark a unit deployed without a serve hand-off (`set_ready` only), as db
    /// and domain units do - no `supervised` flag, no container.
    fn deploy_plain(ctx: &MainContext, name: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.begin_deploy("domain").unwrap();
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
        state.begin_deploy("app").unwrap();
        // Building: handed off not yet, so not supervised.
        assert!(!state.supervised);

        state.set_check();
        assert!(state.supervised);
        assert_eq!(state.last_status, DeployStatus::Check);

        // Ready keeps the flag set; a later redeploy (Building/Failed) too.
        state.set_ready();
        assert!(state.supervised);
        state.set_failed(crate::state::DeployStage::Build, "boom".into());
        assert!(state.supervised);
    }

    #[test]
    fn child_exit_during_check_does_not_arm_restart() {
        let (_dir, ctx) = ctx();
        let name = UnitName::new("app-live").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_check();

        let mut unit = ManagedUnit::new(name.clone(), state.active_version);
        unit.child = Some(spawn_exiting_child());
        unit.started_at = Some(Instant::now());

        let mut supervisor = Supervisor {
            managed: HashMap::from([(name.clone(), unit)]),
            self_exe: None,
        };

        reap_until_child_cleared(&mut supervisor, &ctx, &name);

        let unit = supervisor.managed.get(&name).unwrap();
        assert!(unit.backoff_until.is_none());
        assert_eq!(unit.consecutive_failures, 0);
    }

    #[test]
    fn child_exit_after_ready_arms_restart() {
        let (_dir, ctx) = ctx();
        let name = UnitName::new("app-live").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_check();
        state.set_ready();

        let mut unit = ManagedUnit::new(name.clone(), state.active_version);
        unit.child = Some(spawn_exiting_child());
        unit.started_at = Some(Instant::now());

        let mut supervisor = Supervisor {
            managed: HashMap::from([(name.clone(), unit)]),
            self_exe: None,
        };

        reap_until_child_cleared(&mut supervisor, &ctx, &name);

        let unit = supervisor.managed.get(&name).unwrap();
        assert!(unit.backoff_until.is_some());
        assert_eq!(unit.consecutive_failures, 1);
    }
}
