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
    deploy::app::{
        is_app_kind,
        probe_names,
    },
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

/// Cap on each container probe reconcile makes per slot (one, or two for an
/// app whose versioned name is not up; a probe error ends the search). A
/// reconcile pass runs on serve's main loop, so a wedged podman must cost this
/// much and no more - otherwise one unit stalls supervision and timers for
/// every unit.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RestartPolicy {
    Never,
    Always,
}

/// How reconcile acts on a slot, from its unit's deploy status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Policy {
    /// Keep the container up: restart with backoff on death.
    Restart,
    /// Start once for the deploy health-check; a death is the deploy's to
    /// report, never restarted.
    StartOnce,
    /// Watch only: forget a run that ended.
    Observe,
}

/// Tracking key: one slot per supervised container, so an app can have two
/// (active + candidate) and a redeploy (new version) is a fresh slot.
type SlotKey = (UnitName, Option<u32>);

/// One supervised container as deploy state describes it.
struct Slot {
    name: UnitName,
    /// The deploy-state version this slot runs; `dpl start` gets it for an
    /// app only (see [`ManagedUnit::start_version`]).
    version: Option<u32>,
    policy: Policy,
    /// Names its container may run under, most specific first.
    containers: Vec<String>,
    /// db-server units that must be up before its `dpl start` is spawned.
    start_after: Vec<UnitName>,
    is_app: bool,
}

impl Slot {
    fn key(&self) -> SlotKey {
        (self.name.clone(), self.version)
    }
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
    /// The slot's version (part of its key): a redeploy is a fresh slot.
    version: Option<u32>,
    /// Whether this slot has already been started/adopted while in `Check`.
    /// Ignored once the unit is `Ready`.
    start_attempted: bool,
    /// An app: `dpl start` gets the version, so it runs the versioned
    /// container. Refreshed from deploy state every pass.
    is_app: bool,
}

impl ManagedUnit {
    fn new(name: UnitName, version: Option<u32>) -> Self {
        ManagedUnit {
            name,
            running: false,
            started_at: None,
            observed_up: false,
            backoff_until: None,
            consecutive_failures: 0,
            version,
            start_attempted: false,
            is_app: false,
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

    /// The version argv for `dpl start`: the slot's one for an app (it names
    /// the versioned container), nothing for every other kind.
    fn start_version(&self) -> Option<u32> {
        self.version.filter(|_| self.is_app)
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
    managed: HashMap<SlotKey, ManagedUnit>,
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

    /// One reconcile pass: drop slots no longer wanted from tracking, then act
    /// on each slot per its policy.
    /// Returns the earliest pending respawn deadline so the caller can wake
    /// exactly then instead of polling blindly.
    pub fn reconcile(&mut self, ctx: &MainContext) -> Option<Instant> {
        let exe = self.self_exe.clone()?;

        // Every container handed off to serve (the `supervised` flag), straight
        // from deploy state - the configs are never read.
        let desired = desired_slots(ctx);
        let keys: HashSet<SlotKey> = desired.iter().map(Slot::key).collect();

        // Stop tracking slots no longer wanted - before any probe or spawn, so
        // a version a failed deploy (or a switch) retired is never respawned
        // from a pending backoff.
        self.managed.retain(|key, _| keys.contains(key));

        // One podman read per slot. `None` means podman did not answer in
        // time: no decision for that slot this pass.
        let probed: Vec<Option<bool>> = desired
            .iter()
            .map(|slot| match probe_up(&slot.containers) {
                Ok(up) => Some(up),
                Err(err) => {
                    let what = slot_label(&slot.name, slot.version, slot.is_app);
                    log::warn(format!("probe container {what}: {err}"));
                    None
                }
            })
            .collect();

        // Per unit, for the `start_after` gates: up if any of its slots is.
        let mut running: HashMap<&UnitName, Option<bool>> = HashMap::new();
        for (slot, up) in desired.iter().zip(&probed) {
            let unit = running.entry(&slot.name).or_insert(Some(false));
            *unit = match (*unit, *up) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (None, _) | (_, None) => None,
                _ => Some(false),
            };
        }

        // True if a slot that wants to spawn (Check/Ready) is held back by its
        // gate; only then do we re-poll early so the gate can advance.
        let mut gated = false;
        for (slot, up) in desired.iter().zip(&probed) {
            // Unknown state: skip this slot entirely rather than read it as
            // down, which would spawn a second `dpl start`. The next ordinary
            // poll retries - no early recheck, so a wedged podman is not probed
            // in a tight loop.
            let Some(is_up) = *up else {
                continue;
            };
            let key = slot.key();
            match slot.policy {
                Policy::Observe => {
                    if let Some(m) = self.managed.get_mut(&key) {
                        observe(m, is_up);
                    }
                }
                Policy::StartOnce | Policy::Restart => {
                    let open = can_spawn(&slot.start_after, &running);
                    gated |= !open && !is_up;
                    let m = self
                        .managed
                        .entry(key)
                        .or_insert_with(|| ManagedUnit::new(slot.name.clone(), slot.version));
                    m.is_app = slot.is_app;
                    if slot.policy == Policy::StartOnce {
                        drive_start_once(ctx, &exe, m, is_up, open);
                    } else {
                        drive(ctx, &exe, m, is_up, open);
                    }
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

/// Earliest pending respawn: the soonest backoff among managed slots that are
/// not currently running (those awaiting a respawn). `None` = nothing pending.
fn next_wake(managed: &HashMap<SlotKey, ManagedUnit>) -> Option<Instant> {
    managed
        .values()
        .filter(|m| !m.running)
        .filter_map(|m| m.backoff_until)
        .min()
}

/// Supervised slots to reconcile, straight from deploy state: per unit its
/// active container and, during a parallel deploy, the candidate.
///
/// | status                               | active slot | candidate slot            |
/// |--------------------------------------|-------------|---------------------------|
/// | Ready                                | Restart     | none (a stray is ignored) |
/// | Check, candidate Some                | Restart     | StartOnce                 |
/// | Check, candidate None                | StartOnce   | none (the replace flow)   |
/// | Failed, active != Some(last_version) | Restart     | none                      |
/// | Failed, active == Some(last_version) | none        | none                      |
/// | Building / Idle                      | Observe     | none                      |
///
/// The active slot exists only while `active_version` is Some (Check/Ready
/// always set it; a replace deploy clears it between stopping the old version
/// and the hand-off, so the unit has no slot there); its key carries that
/// version, so a redeploy is a fresh slot. A candidate slot exists for an app
/// only.
fn desired_slots(ctx: &MainContext) -> Vec<Slot> {
    let mut slots = Vec::new();
    for (name, state) in DeployState::list(ctx) {
        if !state.supervised {
            continue;
        }
        let is_app = is_app_kind(state.kind.as_deref());
        let candidate = state.candidate_version.filter(|_| is_app);
        let (active_policy, candidate_policy) = match state.last_status {
            DeployStatus::Ready => (Some(Policy::Restart), None),
            DeployStatus::Check if candidate.is_some() => {
                (Some(Policy::Restart), Some(Policy::StartOnce))
            }
            DeployStatus::Check => (Some(Policy::StartOnce), None),
            DeployStatus::Failed if state.active_version == Some(state.last_version) => {
                (None, None)
            }
            DeployStatus::Failed => (Some(Policy::Restart), None),
            DeployStatus::Building | DeployStatus::Idle => (Some(Policy::Observe), None),
        };
        if let (Some(policy), Some(active)) = (active_policy, state.active_version) {
            slots.push(Slot {
                name: name.clone(),
                version: Some(active),
                policy,
                containers: probe_names(&name, state.kind.as_deref(), Some(active)),
                start_after: state.start_after.clone(),
                is_app,
            });
        }
        if let (Some(policy), Some(candidate)) = (candidate_policy, candidate) {
            slots.push(Slot {
                name: name.clone(),
                version: Some(candidate),
                policy,
                containers: vec![name.container_name(candidate)],
                start_after: state.start_after,
                is_app,
            });
        }
    }
    slots
}

/// Whether a slot's container is up under any of its names.
/// `Err` means podman did not answer - the caller must not read that as down.
/// The first probe to fail ends the search, so a wedged podman costs one
/// [`PROBE_TIMEOUT`], not one per name.
fn probe_up(containers: &[String]) -> io::Result<bool> {
    for container in containers {
        if podman::is_running_within(container, PROBE_TIMEOUT)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a unit's start gate is open: every db-server in `start_after` is up.
/// A dep absent from `running` (unsupervised, undeployed, or supervised with
/// no slot to probe) is queried directly within [`PROBE_TIMEOUT`], so a
/// vanished dependency keeps the gate shut - and so does a dep whose probe did
/// not answer, since a shut gate is the conservative direction.
fn can_spawn(start_after: &[UnitName], running: &HashMap<&UnitName, Option<bool>>) -> bool {
    start_after.iter().all(|dep| match running.get(dep) {
        Some(state) => *state == Some(true),
        None => podman::is_running_within(&dep.scoped_unit_name(), PROBE_TIMEOUT).unwrap_or(false),
    })
}

/// Start a `Check` slot's container once for the deploy health-check. If it
/// exits while still in `Check`, do not schedule a restart.
/// The deploy attempt will fail and move the state to `Failed`.
fn drive_start_once(
    ctx: &MainContext,
    exe: &Path,
    m: &mut ManagedUnit,
    is_up: bool,
    can_spawn: bool,
) {
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

/// Ensure a `Ready` slot's container is running: on a death arm a backoff
/// restart, or spawn once the backoff elapses.
fn drive(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, is_up: bool, can_spawn: bool) {
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
    let what = slot_label(&m.name, m.version, m.is_app);
    match restart {
        RestartPolicy::Always => {
            log::warn(format!("supervisor: {what} exited; restarting"));
            m.note_crash(Instant::now());
        }
        RestartPolicy::Never => {
            log::warn(format!("supervisor: {what} exited during startup check"));
            m.note_exit_without_restart();
        }
    }
}

/// A slot in log lines: `'web' v7` for an app (two of its versions may run
/// side by side), the bare `'pg-main'` for every other kind.
fn slot_label(name: &UnitName, version: Option<u32>, is_app: bool) -> String {
    match version {
        Some(version) if is_app => format!("'{name}' v{version}"),
        _ => format!("'{name}'"),
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
    match spawn_child(exe, ctx.base(), &m.name, m.start_version()) {
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
    match spawn_child(exe, ctx.base(), &m.name, m.start_version()) {
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

/// Spawn `dpl start <unit> [version]` detached, in its own process group so a
/// console signal to serve does not reach it.
fn spawn_child(exe: &Path, base: &Path, name: &UnitName, version: Option<u32>) -> io::Result<()> {
    let mut cmd;

    if is_running_under_systemd() {
        // No fixed `--unit`: the scope lives as long as the attached `dpl start`,
        // so a fixed name is still taken while the previous run is stopping (and
        // by a sibling version), and systemd-run then fails silently.
        let description = match version {
            Some(version) => format!("dpl start {name} {version}"),
            None => format!("dpl start {name}"),
        };
        cmd = Command::new("systemd-run");
        cmd.args(["--scope", "--description", &description, "--quiet"]);
        cmd.arg(exe);
    } else {
        cmd = Command::new(exe);
    }

    cmd.args(["start", name.as_str()]);
    if let Some(version) = version {
        cmd.arg(version.to_string());
    }
    cmd.env("DPL_BASE", base)
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

    fn managed_with(name: &str, backoff_until: Option<Instant>) -> (SlotKey, ManagedUnit) {
        let name = UnitName::new(name).unwrap();
        let mut m = ManagedUnit::new(name.clone(), None);
        m.backoff_until = backoff_until;
        ((name, None), m)
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
        let (key, mut m) = managed_with("up", Some(now + Duration::from_secs(5)));
        m.running = true;
        let managed = HashMap::from([(key, m)]);
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
            managed: HashMap::from([((name, Some(1)), m)]),
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
    fn desired_slots_lists_supervised_in_name_order() {
        let (_dir, ctx) = ctx();

        // desired_slots carries no kind ordering - it is just DeployState::list
        // order. Start gating happens per-unit in reconcile via `start_after`.
        deploy_supervised_as(&ctx, "nginx", "http-server");
        deploy_supervised_as(&ctx, "web", "app");
        deploy_supervised_as(&ctx, "pg", "db-server");

        let names: Vec<UnitName> = desired_slots(&ctx)
            .into_iter()
            .map(|slot| slot.name)
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
            managed: HashMap::from([((name.clone(), Some(1)), m)]),
            self_exe: None,
        };

        // Nothing supervised -> reconcile would forget it. self_exe None makes
        // reconcile early-return, so emulate the retain step it performs.
        let desired = supervised_set(&ctx);
        let desired_names: HashSet<&UnitName> = desired.iter().collect();
        supervisor
            .managed
            .retain(|(n, _), _| desired_names.contains(n));
        assert!(supervisor.managed.is_empty());
    }

    #[test]
    fn reconcile_drops_retired_slot_despite_pending_backoff() {
        // A slot absent from deploy state (its version was retired by a
        // failed deploy or a switch) is dropped before anything else, backoff
        // or not - serve must never resurrect it. No supervised unit on disk,
        // so reconcile neither probes nor spawns.
        let (_dir, ctx) = ctx();
        let name = UnitName::new("web").unwrap();
        let mut m = ManagedUnit::new(name.clone(), Some(1));
        m.backoff_until = Some(Instant::now() + Duration::from_secs(30));

        let mut supervisor = Supervisor {
            managed: HashMap::from([((name, Some(1)), m)]),
            self_exe: Some(PathBuf::from("/nonexistent/dpl")),
        };

        assert!(supervisor.reconcile(&ctx).is_none());
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

        drive(&ctx, exe, &mut m, false, false);
        assert!(!m.running);
        assert!(m.started_at.is_none());
        assert!(m.backoff_until.is_none());
        assert_eq!(m.consecutive_failures, 0);

        // db-server came up and the app launched: adopt it even though the gate
        // value is still false this tick.
        drive(&ctx, exe, &mut m, true, false);
        assert!(m.running);
        assert!(m.observed_up);
    }

    #[test]
    fn gated_check_unit_is_not_started() {
        let (_dir, ctx) = ctx();
        let exe = std::path::Path::new("/nonexistent/dpl");
        let name = UnitName::new("web").unwrap();
        let mut m = ManagedUnit::new(name, Some(1));

        drive_start_once(&ctx, exe, &mut m, false, false);
        assert!(!m.running);
        assert!(!m.start_attempted);
    }

    /// The slots of one unit, in `desired_slots` order.
    fn slots_of(ctx: &MainContext, unit: &str) -> Vec<Slot> {
        let name = UnitName::new(unit).unwrap();
        desired_slots(ctx)
            .into_iter()
            .filter(|slot| slot.name == name)
            .collect()
    }

    #[test]
    fn desired_slots_carries_is_app_and_probe_names() {
        let (_dir, ctx) = ctx();
        deploy_supervised_as(&ctx, "web", "app");
        deploy_supervised_as(&ctx, "pg", "db-server");

        let web = slots_of(&ctx, "web");
        assert_eq!(web.len(), 1);
        assert!(web[0].is_app);
        assert_eq!(web[0].containers, vec!["dpl--web--1", "dpl--web"]);

        let pg = slots_of(&ctx, "pg");
        assert_eq!(pg.len(), 1);
        assert!(!pg[0].is_app);
        assert_eq!(pg[0].containers, vec!["dpl--pg"]);
    }

    #[test]
    fn redeploy_is_a_fresh_slot_for_every_kind() {
        // The key carries the active version for every kind, so a redeploy
        // drops the old slot (and its `start_attempted`) and starts clean.
        let (_dir, ctx) = ctx();
        let pg = UnitName::new("pg").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &pg).unwrap();
        state.begin_deploy("db-server").unwrap();
        state.set_check();
        state.set_ready();
        assert_eq!(slots_of(&ctx, "pg")[0].key(), (pg.clone(), Some(1)));

        state.begin_deploy("db-server").unwrap();
        state.set_check();
        let slots = slots_of(&ctx, "pg");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key(), (pg, Some(2)));
        assert_eq!(slots[0].policy, Policy::StartOnce);
    }

    #[test]
    fn start_version_only_for_apps() {
        let name = UnitName::new("web").unwrap();
        let mut m = ManagedUnit::new(name, Some(3));
        assert_eq!(m.start_version(), None);
        m.is_app = true;
        assert_eq!(m.start_version(), Some(3));
        m.version = None;
        assert_eq!(m.start_version(), None);
    }

    #[test]
    fn desired_slots_carries_start_after() {
        let (_dir, ctx) = ctx();
        let app = UnitName::new("web").unwrap();
        let pg = UnitName::new("pg").unwrap();

        let (_guard, mut state) = DeployState::acquire(&ctx, &app).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_start_after(vec![pg.clone()]);
        state.set_check();
        drop((_guard, state));

        let slots = slots_of(&ctx, "web");
        assert_eq!(slots[0].start_after, vec![pg]);
    }

    #[test]
    fn check_with_candidate_yields_active_restart_and_candidate_start_once() {
        // v1 live, v2 handed off next to it: the active keeps its restart
        // supervision while the candidate gets the one-shot startup check.
        let (_dir, ctx) = ctx();
        let web = UnitName::new("web").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &web).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_ready();
        state.begin_deploy("app").unwrap();
        state.set_candidate();
        drop((_guard, state));

        let slots = slots_of(&ctx, "web");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].key(), (web.clone(), Some(1)));
        assert_eq!(slots[0].policy, Policy::Restart);
        assert_eq!(slots[0].containers, vec!["dpl--web--1", "dpl--web"]);
        assert_eq!(slots[1].key(), (web, Some(2)));
        assert_eq!(slots[1].policy, Policy::StartOnce);
        assert_eq!(slots[1].containers, vec!["dpl--web--2"]);
        assert!(slots[1].is_app);
    }

    #[test]
    fn failed_with_live_active_yields_one_restart_slot() {
        // A build failure leaves the previous version running: keep it
        // supervised.
        let (_dir, ctx) = ctx();
        let web = UnitName::new("web").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &web).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_check();
        state.set_ready();
        state.begin_deploy("app").unwrap();
        state.set_failed(crate::state::DeployStage::Build, "boom".into());
        drop((_guard, state));

        let slots = slots_of(&ctx, "web");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key(), (web, Some(1)));
        assert_eq!(slots[0].policy, Policy::Restart);
    }

    #[test]
    fn failed_at_active_version_yields_no_slot() {
        // The failed version is the active one (handed off, then unready):
        // nothing to keep up until the next deploy.
        let (_dir, ctx) = ctx();
        let web = UnitName::new("web").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &web).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_check();
        state.set_failed(crate::state::DeployStage::Startup, "boom".into());
        drop((_guard, state));

        assert!(slots_of(&ctx, "web").is_empty());
    }

    #[test]
    fn ready_ignores_a_stray_candidate() {
        let (_dir, ctx) = ctx();
        let web = UnitName::new("web").unwrap();
        let (_guard, mut state) = DeployState::acquire(&ctx, &web).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_ready();
        state.begin_deploy("app").unwrap();
        state.set_candidate();
        state.set_ready();
        assert_eq!(state.candidate_version, Some(2));
        drop((_guard, state));

        let slots = slots_of(&ctx, "web");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].key(), (web, Some(2)));
        assert_eq!(slots[0].policy, Policy::Restart);
    }
}
