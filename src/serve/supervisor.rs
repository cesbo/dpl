use std::{
    collections::{
        HashMap,
        HashSet,
    },
    env,
    io,
    os::unix::process::CommandExt,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Stdio,
    },
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

/// A container that ran at least this long before dying is treated as a fresh
/// transient crash, not part of a loop: the backoff resets to the base.
const STABLE_RUN: Duration = Duration::from_secs(30);

/// Grace after spawning `dpl start` before a not-running reading counts as a
/// death: `dpl start` does foreground work (db wait, image pull/launch), so the
/// container can take many seconds to appear.
const STARTUP_GRACE: Duration = Duration::from_secs(120);

/// While a unit is held back by its `start_after` gate, re-poll this soon so the
/// gate opens as its db-servers come up, instead of waiting a full poll cycle.
const GATE_RECHECK: Duration = Duration::from_secs(1);

/// Cap the doubling exponent so the `1 << n` shift can't overflow; the result is
/// clamped to [`RESTART_BACKOFF_CAP`] long before this bites.
const MAX_BACKOFF_SHIFT: u32 = 16;

/// Cap on the one container probe reconcile makes per unit. A reconcile pass
/// runs on serve's main loop, so a wedged podman must cost this much and no
/// more - otherwise one unit stalls supervision and timers for every unit.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartPolicy {
    Never,
    Always,
}

/// One supervised container across ticks.
struct ManagedUnit {
    name: UnitName,
    /// The container is expected to be (or to come) up.
    running: bool,
    /// When the current run was spawned/adopted.
    started_at: Option<Instant>,
    /// Whether podman ever reported this run as up.
    observed_up: bool,
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
            running: false,
            started_at: None,
            observed_up: false,
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
        self.running = false;
        self.started_at = None;
        self.observed_up = false;
        self.backoff_until = Some(now + backoff_delay(self.consecutive_failures));
    }

    fn note_exit_without_restart(&mut self) {
        self.running = false;
        self.started_at = None;
        self.observed_up = false;
        self.backoff_until = None;
    }

    fn is_death(&self, now: Instant) -> bool {
        if self.observed_up {
            return true;
        }

        self.started_at
            .is_none_or(|started| now.duration_since(started) >= STARTUP_GRACE)
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
    RESTART_BACKOFF_BASE
        .saturating_mul(1 << exp)
        .min(RESTART_BACKOFF_CAP)
}

/// Launches and supervises long-running unit containers for `dpl serve`: spawns
/// `dpl start <unit>` detached, watches via `podman`, restarts on death.
/// Reconciled per serve tick, on SIGHUP, and on a podman events death signal.
///
/// The managed set and each unit's `start_after` gate come only from deploy
/// state files; an app's `dpl start` is held until its db-server containers are
/// up. Shutdown leaves containers running so serve can restart without downtime.
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

    /// One reconcile pass: drop containers no longer wanted from tracking, then
    /// act on each runtime unit per its deploy status.
    /// Returns the earliest pending respawn deadline so the caller can wake
    /// exactly then instead of polling blindly.
    pub fn reconcile(&mut self, ctx: &MainContext) -> Option<Instant> {
        let exe = self.self_exe.clone()?;

        // Every unit handed off to serve (the `supervised` flag), straight from
        // deploy state - the configs are never read.
        let desired = desired_units(ctx);
        let desired_names: HashSet<&UnitName> = desired.iter().map(|(name, ..)| name).collect();

        // Stop tracking units no longer supervised.
        self.managed.retain(|name, _| desired_names.contains(name));

        // One podman read per unit, shared by each unit's own gate check.
        // `None` means podman did not answer in time: no decision this pass.
        let running: HashMap<&UnitName, Option<bool>> = desired
            .iter()
            .map(|(name, ..)| {
                let state = match podman::is_running_within(name, PROBE_TIMEOUT) {
                    Ok(up) => Some(up),
                    Err(err) => {
                        log::warn(format!("probe container '{name}': {err}"));
                        None
                    }
                };
                (name, state)
            })
            .collect();

        // True if a unit that wants to spawn (Check/Ready) is held back by its
        // gate; only then do we re-poll early so the gate can advance.
        let mut gated = false;
        for (name, status, active, start_after) in &desired {
            // Unknown state: skip this unit entirely rather than read it as
            // down, which would spawn a second `dpl start`. The next ordinary
            // poll retries - no early recheck, so a wedged podman is not probed
            // in a tight loop.
            let Some(is_up) = running.get(name).copied().flatten() else {
                continue;
            };
            let open = can_spawn(start_after, &running);
            match status {
                DeployStatus::Failed => {
                    self.managed.remove(name);
                }
                DeployStatus::Building | DeployStatus::Idle => {
                    if let Some(m) = self.managed.get_mut(name) {
                        observe(m, is_up);
                    }
                }
                DeployStatus::Check => {
                    gated |= !open && !is_up;
                    let unit = self
                        .managed
                        .entry(name.clone())
                        .or_insert_with(|| ManagedUnit::new(name.clone(), *active));
                    drive_start_once(ctx, &exe, unit, *active, is_up, open);
                }
                DeployStatus::Ready => {
                    gated |= !open && !is_up;
                    let unit = self
                        .managed
                        .entry(name.clone())
                        .or_insert_with(|| ManagedUnit::new(name.clone(), *active));
                    drive(ctx, &exe, unit, *active, is_up, open);
                }
            }
        }

        let wake = next_wake(&self.managed);
        if gated {
            let soon = Instant::now() + GATE_RECHECK;
            Some(wake.map_or(soon, |at| at.min(soon)))
        } else {
            wake
        }
    }

    /// Stop watching every supervised unit.
    pub fn shutdown(&mut self) {
        self.managed.clear();
    }
}

/// Earliest pending respawn: the soonest backoff among managed units that are
/// not currently running (those awaiting a respawn). `None` = nothing pending.
fn next_wake(managed: &HashMap<UnitName, ManagedUnit>) -> Option<Instant> {
    managed
        .values()
        .filter(|m| !m.running)
        .filter_map(|m| m.backoff_until)
        .min()
}

/// Supervised units to reconcile, straight from deploy state. The fourth field
/// is the unit's `start_after` gate (db-server names it must see up first).
fn desired_units(ctx: &MainContext) -> Vec<(UnitName, DeployStatus, Option<u32>, Vec<UnitName>)> {
    DeployState::list(ctx)
        .into_iter()
        .filter(|(_, state)| state.supervised)
        .map(|(name, state)| {
            (
                name,
                state.last_status,
                state.active_version,
                state.start_after,
            )
        })
        .collect()
}

/// Whether a unit's start gate is open: every db-server in `start_after` is up.
/// A dep absent from `running` (e.g. unsupervised or undeployed) is queried
/// directly, so a vanished dependency keeps the gate shut - and so does a dep
/// whose probe did not answer, since a shut gate is the conservative direction.
fn can_spawn(start_after: &[UnitName], running: &HashMap<&UnitName, Option<bool>>) -> bool {
    start_after.iter().all(|dep| match running.get(dep) {
        Some(state) => *state == Some(true),
        None => podman::is_running(dep),
    })
}

fn reset_for_active_version(m: &mut ManagedUnit, active: Option<u32>) {
    if m.active_version != active {
        m.running = false;
        m.started_at = None;
        m.observed_up = false;
        m.active_version = active;
        m.backoff_until = None;
        m.consecutive_failures = 0;
        m.start_attempted = false;
    }
}

/// Start a `Check` unit's container once for the deploy health-check. If it
/// exits while still in `Check`, do not schedule a restart.
/// The deploy attempt will fail and move the state to `Failed`.
fn drive_start_once(
    ctx: &MainContext,
    exe: &Path,
    m: &mut ManagedUnit,
    active: Option<u32>,
    is_up: bool,
    can_spawn: bool,
) {
    reset_for_active_version(m, active);

    if is_up {
        m.running = true;
        m.observed_up = true;
        if m.started_at.is_none() {
            m.started_at = Some(Instant::now());
        }
        m.start_attempted = true;
    } else if m.running && m.is_death(Instant::now()) {
        observe_death(m, RestartPolicy::Never);
    } else if can_spawn && !m.running && !m.start_attempted {
        m.start_attempted = true;
        spawn_once(ctx, exe, m);
    }
}

/// Ensure a `Ready` unit's container is running: on a death arm a backoff
/// restart, or spawn once the backoff elapses.
fn drive(
    ctx: &MainContext,
    exe: &Path,
    m: &mut ManagedUnit,
    active: Option<u32>,
    is_up: bool,
    can_spawn: bool,
) {
    reset_for_active_version(m, active);

    let now = Instant::now();
    if is_up {
        m.running = true;
        m.observed_up = true;
        if m.started_at.is_none() {
            m.started_at = Some(now);
        }
        m.backoff_until = None;
    } else if m.running && m.is_death(now) {
        observe_death(m, RestartPolicy::Always);
    } else if can_spawn && !m.running && m.backoff_until.is_none_or(|until| now >= until) {
        spawn(ctx, exe, m);
    }
}

/// Note a running->not-running transition and apply the restart policy.
fn observe_death(m: &mut ManagedUnit, restart: RestartPolicy) {
    match restart {
        RestartPolicy::Always => {
            log::warn(format!(
                "supervisor: '{}' exited; restarting",
                m.name.as_str()
            ));
            m.note_crash(Instant::now());
        }
        RestartPolicy::Never => {
            log::warn(format!(
                "supervisor: '{}' exited during startup check",
                m.name.as_str()
            ));
            m.note_exit_without_restart();
        }
    }
}

/// Reconcile tracking for a unit no longer running (`Building`/`Idle`): forget a
/// run that has ended so a later `Ready` transition starts clean.
fn observe(m: &mut ManagedUnit, is_up: bool) {
    if m.running && !is_up {
        m.running = false;
        m.started_at = None;
        m.observed_up = false;
    }
}

/// Spawn `dpl start <unit>` once for the startup check. Spawn failure is left
/// for the deploy health-check to report; it must not arm a restart loop.
fn spawn_once(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(()) => {
            m.running = true;
            m.started_at = Some(Instant::now());
            m.backoff_until = None;
        }
        Err(err) => {
            log::warn(format!("supervisor: start '{}': {err}", m.name.as_str()));
            m.running = false;
            m.started_at = None;
            m.backoff_until = None;
        }
    }
}

/// Spawn `dpl start <unit>`; back off (exponentially) on spawn failure.
fn spawn(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    let now = Instant::now();
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(()) => {
            m.running = true;
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

/// Spawn `dpl start <unit>` detached, in its own process group so a console
/// signal to serve does not reach it.
fn spawn_child(exe: &Path, base: &Path, name: &UnitName) -> io::Result<()> {
    let mut cmd;

    if is_running_under_systemd() {
        let unit = format!("dpl-container-{name}.scope");
        cmd = Command::new("systemd-run");
        cmd.args(["--scope", "--unit", &unit, "--quiet"]);
        cmd.arg(exe);
    } else {
        cmd = Command::new(exe);
    }

    cmd.args(["start", name.as_str()])
        .env("DPL_BASE", base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

fn is_running_under_systemd() -> bool {
    env::var_os("INVOCATION_ID").is_some()
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
    fn next_wake_ignores_running_units() {
        let now = Instant::now();
        let (name, mut m) = managed_with("up", Some(now + Duration::from_secs(5)));
        m.running = true;
        let managed = HashMap::from([(name, m)]);
        assert!(next_wake(&managed).is_none());
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
        assert!(!m.running);
        assert!(m.started_at.is_none());
    }

    #[test]
    fn death_during_check_does_not_arm_restart() {
        let name = UnitName::new("app").unwrap();
        let mut m = ManagedUnit::new(name, Some(3));
        m.running = true;
        m.started_at = Some(Instant::now());

        observe_death(&mut m, RestartPolicy::Never);

        assert!(!m.running);
        assert!(m.backoff_until.is_none());
        assert_eq!(m.consecutive_failures, 0);
    }

    #[test]
    fn death_after_ready_arms_restart() {
        let name = UnitName::new("app").unwrap();
        let mut m = ManagedUnit::new(name, Some(3));
        m.running = true;
        m.started_at = Some(Instant::now());

        observe_death(&mut m, RestartPolicy::Always);

        assert!(!m.running);
        assert!(m.backoff_until.is_some());
        assert_eq!(m.consecutive_failures, 1);
    }

    #[test]
    fn launching_run_inside_grace_is_not_a_death() {
        // Spawned but never seen up, still inside the startup grace: a
        // not-running reading must not count as a death (foreground `dpl start`
        // may not have launched the container yet).
        let name = UnitName::new("app").unwrap();
        let now = Instant::now();
        let mut m = ManagedUnit::new(name, Some(1));
        m.running = true;
        m.started_at = Some(now);
        assert!(!m.is_death(now));
    }

    #[test]
    fn launching_run_past_grace_is_a_death() {
        // Never seen up but the startup grace elapsed: treat as a death so a
        // wedged start does not hang forever.
        let name = UnitName::new("app").unwrap();
        let now = Instant::now();
        let mut m = ManagedUnit::new(name, Some(1));
        m.running = true;
        m.started_at = Some(now - (STARTUP_GRACE + Duration::from_secs(1)));
        assert!(m.is_death(now));
    }

    #[test]
    fn once_observed_up_a_drop_is_a_death_immediately() {
        // After podman has reported the run up, any later not-running reading is
        // a death regardless of the grace window.
        let name = UnitName::new("app").unwrap();
        let now = Instant::now();
        let mut m = ManagedUnit::new(name, Some(1));
        m.running = true;
        m.observed_up = true;
        m.started_at = Some(now);
        assert!(m.is_death(now));
    }

    #[test]
    fn shutdown_leaves_containers_running() {
        // shutdown() must only drop tracking; it must never call podman stop.
        // Build a supervisor tracking a unit, then assert shutdown only clears
        // the tracked map (a stop would touch podman, which is absent in tests).
        let name = UnitName::new("app").unwrap();
        let mut m = ManagedUnit::new(name.clone(), Some(1));
        m.running = true;

        let mut supervisor = Supervisor {
            managed: HashMap::from([(name, m)]),
            self_exe: None,
        };

        supervisor.shutdown();
        assert!(supervisor.managed.is_empty());
    }

    /// Mark a unit as deployed through the serve hand-off (`set_check`), so its
    /// state file carries the `supervised` flag.
    fn deploy_supervised(ctx: &MainContext, name: &str) {
        deploy_supervised_as(ctx, name, "app");
    }

    fn deploy_supervised_as(ctx: &MainContext, name: &str, kind: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.begin_deploy(kind).unwrap();
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
    fn desired_units_lists_supervised_in_name_order() {
        let (_dir, ctx) = ctx();

        // desired_units carries no kind ordering - it is just DeployState::list
        // order. Start gating happens per-unit in reconcile via `start_after`.
        deploy_supervised_as(&ctx, "nginx", "http-server");
        deploy_supervised_as(&ctx, "web", "app");
        deploy_supervised_as(&ctx, "pg", "db-server");

        let names: Vec<UnitName> = desired_units(&ctx)
            .into_iter()
            .map(|(name, ..)| name)
            .collect();

        assert_eq!(
            names,
            vec![
                UnitName::new("nginx").unwrap(),
                UnitName::new("pg").unwrap(),
                UnitName::new("web").unwrap(),
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
    fn reconcile_drops_unsupervised_without_stopping() {
        // A unit dropped from the supervised set is forgotten by reconcile but
        // never stopped (no podman call). With self_exe None reconcile is a
        // no-op early return, so assert the tracked map directly via retain.
        let (_dir, ctx) = ctx();
        let name = UnitName::new("gone").unwrap();
        let mut m = ManagedUnit::new(name.clone(), Some(1));
        m.running = true;

        let mut supervisor = Supervisor {
            managed: HashMap::from([(name.clone(), m)]),
            self_exe: None,
        };

        // Nothing supervised -> reconcile would forget it. self_exe None makes
        // reconcile early-return, so emulate the retain step it performs.
        let desired = supervised_set(&ctx);
        let desired_names: HashSet<&UnitName> = desired.iter().collect();
        supervisor.managed.retain(|n, _| desired_names.contains(n));
        assert!(supervisor.managed.is_empty());
    }

    #[test]
    fn gate_open_only_when_every_dep_is_up() {
        let pg = UnitName::new("pg").unwrap();
        let cache = UnitName::new("cache").unwrap();

        // No deps -> always open.
        assert!(can_spawn(&[], &HashMap::new()));

        // Single dep up -> open; down -> shut.
        assert!(can_spawn(
            std::slice::from_ref(&pg),
            &HashMap::from([(&pg, Some(true))])
        ));
        assert!(!can_spawn(
            std::slice::from_ref(&pg),
            &HashMap::from([(&pg, Some(false))])
        ));

        // Two deps: gate opens only when both are up.
        let both_up = HashMap::from([(&pg, Some(true)), (&cache, Some(true))]);
        let one_down = HashMap::from([(&pg, Some(true)), (&cache, Some(false))]);
        assert!(can_spawn(&[pg.clone(), cache.clone()], &both_up));
        assert!(!can_spawn(&[pg.clone(), cache.clone()], &one_down));
    }

    #[test]
    fn gate_is_shut_when_a_dep_probe_did_not_answer() {
        // An unanswered probe must not read as "up": holding the gate shut is
        // the direction that cannot start an app before its database.
        let pg = UnitName::new("pg").unwrap();
        let unknown = HashMap::from([(&pg, None)]);
        assert!(!can_spawn(std::slice::from_ref(&pg), &unknown));
    }

    #[test]
    fn gate_ignores_unrelated_running_units() {
        // An unrelated db-server being up must not open an app's gate: control
        // is scoped to the app's own dependencies.
        let pg = UnitName::new("pg").unwrap();
        let unrelated = UnitName::new("other-pg").unwrap();
        let running = HashMap::from([(&pg, Some(false)), (&unrelated, Some(true))]);
        assert!(!can_spawn(std::slice::from_ref(&pg), &running));
    }

    #[test]
    fn gated_ready_unit_is_not_spawned_but_still_observed() {
        // can_spawn=false holds back the spawn, but adoption (is_up) and death
        // detection stay unconditional. exe is never touched while gated.
        let (_dir, ctx) = ctx();
        let exe = std::path::Path::new("/nonexistent/dpl");
        let name = UnitName::new("web").unwrap();
        let mut m = ManagedUnit::new(name, Some(1));

        drive(&ctx, exe, &mut m, Some(1), false, false);
        assert!(!m.running);
        assert!(m.started_at.is_none());
        assert!(m.backoff_until.is_none());
        assert_eq!(m.consecutive_failures, 0);

        // db-server came up and the app launched: adopt it even though the gate
        // value is still false this tick.
        drive(&ctx, exe, &mut m, Some(1), true, false);
        assert!(m.running);
        assert!(m.observed_up);
    }

    #[test]
    fn gated_check_unit_is_not_started() {
        let (_dir, ctx) = ctx();
        let exe = std::path::Path::new("/nonexistent/dpl");
        let name = UnitName::new("web").unwrap();
        let mut m = ManagedUnit::new(name, Some(1));

        drive_start_once(&ctx, exe, &mut m, Some(1), false, false);
        assert!(!m.running);
        assert!(!m.start_attempted);
    }

    #[test]
    fn desired_units_carries_start_after() {
        let (_dir, ctx) = ctx();
        let app = UnitName::new("web").unwrap();
        let pg = UnitName::new("pg").unwrap();

        let (_guard, mut state) = DeployState::acquire(&ctx, &app).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_start_after(vec![pg.clone()]);
        state.set_check();
        drop((_guard, state));

        let found = desired_units(&ctx);
        let entry = found.iter().find(|(n, ..)| n == &app).unwrap();
        assert_eq!(entry.3, vec![pg]);
    }
}
