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
    deploy::{
        UnitConfig,
        list_units,
    },
    log,
    podman,
    state::{
        DeployLockGuard,
        DeployState,
        DeployStatus,
    },
    systemd,
};

/// Delay before a crashed container is restarted.
const RESTART_BACKOFF: Duration = Duration::from_secs(5);
/// Upper bound on waiting for a child to exit during shutdown before SIGKILL.
const STOP_REAP_TIMEOUT: Duration = Duration::from_secs(30);

/// How a unit's container should be restarted after it exits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestartPolicy {
    /// app-with-runtime: only keep restarting once it has been healthy; an app
    /// that fails its initial readiness check is left down (today's behavior).
    HealthGated { port: u16 },
    /// db-server / http-server: always restart with backoff (`Restart=always`).
    Always,
}

/// Lifecycle of one supervised container across ticks.
#[derive(Debug)]
enum Phase {
    /// Child spawned; readiness not yet confirmed.
    Starting,
    /// Confirmed up: app passed health, or an always-unit's container runs.
    Ready,
    /// Child exited; respawn once `now >= until`.
    Backoff { until: Instant },
    /// app failed its initial readiness check: terminal, no auto-restart.
    Failed,
}

struct ManagedUnit {
    name: UnitName,
    policy: RestartPolicy,
    /// The `dpl start <unit>` child, while one is running.
    child: Option<Child>,
    phase: Phase,
    /// Healthy at least once during this serve run (gates app restarts).
    ever_ready: bool,
    /// active_version observed when adopted; a redeploy bumps it and re-adopts.
    active_version: Option<u32>,
}

impl ManagedUnit {
    fn new(name: UnitName, policy: RestartPolicy, active_version: Option<u32>) -> Self {
        ManagedUnit {
            name,
            policy,
            child: None,
            // Spawn on the first drive this tick.
            phase: Phase::Backoff {
                until: Instant::now(),
            },
            ever_ready: false,
            active_version,
        }
    }
}

/// Launches and supervises long-running unit containers for the `dpl serve`
/// daemon: it spawns `dpl start <unit>` as a child, watches it, and restarts it
/// per [`RestartPolicy`]. Reconciled once per serve tick.
///
/// Inert until the systemd cutover: a unit is only adopted when its per-unit
/// `dpl--<name>.service` is NOT active, and deploy still enables those services,
/// so nothing is adopted yet. Removing a unit's service hands it to serve.
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

    /// One reconcile pass: stop containers no longer wanted, then for each
    /// adoptable unit spawn/health-check/restart its child as its phase dictates.
    pub fn reconcile(&mut self, ctx: &MainContext) {
        let Some(exe) = self.self_exe.clone() else {
            return;
        };

        let desired = collect_desired(ctx);
        let desired_names: HashSet<UnitName> = desired.iter().map(|(n, _)| n.clone()).collect();

        // Drop containers we manage that are no longer deployed/configured.
        self.managed.retain(|name, m| {
            if desired_names.contains(name) {
                return true;
            }
            stop_child(m);
            false
        });

        for (name, policy) in desired {
            let service = format!("{}.service", name.scoped_unit_name());

            // Inert guard: systemd still owns this unit, leave it alone.
            if systemd::is_active(&service) {
                if let Some(mut m) = self.managed.remove(&name) {
                    discard_child(&mut m);
                }
                continue;
            }

            if !deploy_ready(ctx, &name) {
                if let Some(mut m) = self.managed.remove(&name) {
                    stop_child(&mut m);
                }
                continue;
            }

            // Guard the spawn/health window against a concurrent deploy; skip
            // this unit until the next tick if a deploy holds the lock.
            let _guard = match DeployLockGuard::try_acquire(ctx, &name) {
                Ok(Some(guard)) => guard,
                Ok(None) => continue,
                Err(err) => {
                    log::warn(format!(
                        "supervisor: deploy lock '{}': {err}",
                        name.as_str()
                    ));
                    continue;
                }
            };

            let active = deploy_active_version(ctx, &name);
            // A redeploy (new active version) supersedes any prior state,
            // including a terminal Failed: drop and re-adopt fresh.
            if self
                .managed
                .get(&name)
                .is_some_and(|m| m.active_version != active)
                && let Some(mut old) = self.managed.remove(&name)
            {
                stop_child(&mut old);
            }

            let unit = self
                .managed
                .entry(name.clone())
                .or_insert_with(|| ManagedUnit::new(name, policy, active));

            drive_unit(ctx, &exe, unit);
        }
    }

    /// Stop every supervised container and reap its child. Called once when the
    /// daemon is shutting down; serve otherwise just dropped the loop and
    /// orphaned the children.
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
                reap(child, deadline);
            }
        }
        self.managed.clear();
    }
}

/// Advance one unit's phase: spawn after backoff, confirm readiness while
/// starting, restart after a healthy container exits. Holds the deploy lock.
fn drive_unit(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    let now = Instant::now();

    match m.phase {
        Phase::Failed => {}

        Phase::Backoff { until } => {
            if now >= until {
                spawn(ctx, exe, m, now);
            }
        }

        Phase::Starting => {
            let Some(child) = m.child.as_mut() else {
                m.phase = Phase::Backoff { until: now };
                return;
            };
            match child.try_wait() {
                Ok(Some(_)) => {
                    m.child = None;
                    m.phase = on_child_exit(&m.policy, m.ever_ready, now);
                    if matches!(m.phase, Phase::Failed) {
                        log::error(format!(
                            "supervisor: '{}' failed initial startup; not restarting",
                            m.name.as_str()
                        ));
                    }
                }
                Ok(None) => confirm_ready(m),
                Err(err) => {
                    log::warn(format!("supervisor: wait '{}': {err}", m.name.as_str()));
                    m.child = None;
                    m.phase = Phase::Backoff {
                        until: now + RESTART_BACKOFF,
                    };
                }
            }
        }

        Phase::Ready => {
            let Some(child) = m.child.as_mut() else {
                m.phase = Phase::Backoff { until: now };
                return;
            };
            match child.try_wait() {
                Ok(Some(_)) => {
                    log::warn(format!(
                        "supervisor: '{}' exited; restarting",
                        m.name.as_str()
                    ));
                    m.child = None;
                    m.phase = Phase::Backoff {
                        until: now + RESTART_BACKOFF,
                    };
                }
                Ok(None) => {}
                Err(err) => {
                    log::warn(format!("supervisor: wait '{}': {err}", m.name.as_str()));
                    m.child = None;
                    m.phase = Phase::Backoff {
                        until: now + RESTART_BACKOFF,
                    };
                }
            }
        }
    }
}

/// While starting, once the container is up, gate on readiness. The health
/// check blocks (up to ~24s, fast-failing on exit); acceptable as a one-time
/// per-unit startup cost on a single host with few units.
fn confirm_ready(m: &mut ManagedUnit) {
    if !podman::is_running(&m.name) {
        return;
    }
    match m.policy {
        RestartPolicy::Always => {
            m.phase = Phase::Ready;
            m.ever_ready = true;
        }
        RestartPolicy::HealthGated { port } => match podman::health::check(&m.name, port) {
            Ok(()) => {
                m.phase = Phase::Ready;
                m.ever_ready = true;
            }
            Err(err) => {
                log::error(format!("supervisor: '{}' health: {err}", m.name.as_str()));
                stop_child(m);
                m.phase = Phase::Failed;
            }
        },
    }
}

/// Spawn `dpl start <unit>` and enter `Starting`; back off on spawn failure.
fn spawn(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, now: Instant) {
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(child) => {
            m.child = Some(child);
            m.phase = Phase::Starting;
        }
        Err(err) => {
            log::warn(format!("supervisor: start '{}': {err}", m.name.as_str()));
            m.phase = Phase::Backoff {
                until: now + RESTART_BACKOFF,
            };
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

/// Phase after a child exits: a never-healthy app is terminal, everything else
/// backs off and restarts.
fn on_child_exit(policy: &RestartPolicy, ever_ready: bool, now: Instant) -> Phase {
    match policy {
        RestartPolicy::HealthGated { .. } if !ever_ready => Phase::Failed,
        _ => Phase::Backoff {
            until: now + RESTART_BACKOFF,
        },
    }
}

/// Stop the unit's container and reap its child (used when a unit is dropped or
/// found unhealthy). `podman stop` makes the child's `supervise()` loop exit.
fn stop_child(m: &mut ManagedUnit) {
    if let Err(err) = podman::stop_and_remove(&m.name) {
        log::warn(format!("supervisor: stop '{}': {err}", m.name.as_str()));
    }
    if let Some(mut child) = m.child.take() {
        reap(&mut child, Instant::now() + STOP_REAP_TIMEOUT);
    }
}

/// Drop our handle on a child without touching its container (used for the
/// inert systemd-owned case, where stopping would fight systemd).
fn discard_child(m: &mut ManagedUnit) {
    if let Some(mut child) = m.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Wait for a child to exit, SIGKILL it once `deadline` passes.
fn reap(child: &mut Child, deadline: Instant) {
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

/// Runtime units serve can supervise, paired with their restart policy.
fn collect_desired(ctx: &MainContext) -> Vec<(UnitName, RestartPolicy)> {
    list_units(ctx, |cfg| policy_for(cfg).is_some())
        .into_iter()
        .filter_map(|(name, cfg)| policy_for(&cfg).map(|policy| (name, policy)))
        .collect()
}

/// Restart policy for a unit, or `None` if it has no long-running container
/// (static app, db, domain).
fn policy_for(cfg: &UnitConfig) -> Option<RestartPolicy> {
    match cfg {
        UnitConfig::App(app) => app
            .runtime
            .as_ref()
            .map(|runtime| RestartPolicy::HealthGated { port: runtime.port }),
        UnitConfig::DbServer(_) | UnitConfig::HttpServer(_) => Some(RestartPolicy::Always),
        UnitConfig::Db(_) | UnitConfig::Domain(_) => None,
    }
}

/// `true` when the unit is deployed and runnable (`Ready` with an active version).
fn deploy_ready(ctx: &MainContext, name: &UnitName) -> bool {
    DeployState::load(ctx, name)
        .is_ok_and(|s| s.last_status == DeployStatus::Ready && s.active_version.is_some())
}

fn deploy_active_version(ctx: &MainContext, name: &UnitName) -> Option<u32> {
    DeployState::load(ctx, name)
        .ok()
        .and_then(|s| s.active_version)
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

    #[test]
    fn collect_desired_selects_runtime_units_with_policies() {
        let (_dir, ctx) = ctx();
        ctx.write_test_unit(
            "app-live",
            "type: app\nimage: alpine\nbuilds: []\nruntime:\n  port: 8080\n  cmd: ./run\n",
        );
        ctx.write_test_unit("app-static", "type: app\nimage: alpine\nbuilds: []\n");
        ctx.write_test_unit(
            "dbx",
            "type: db\nserver: pg\nuser: app1\nsecret: app1-pass\n",
        );
        ctx.write_test_unit("httpx", "type: http-server\n");
        ctx.write_test_unit(
            "pg",
            "type: db-server\nengine: postgresql\nversion: \"18\"\nsecret: pg-pass\n",
        );
        ctx.write_test_unit(
            "site-com",
            "type: domain\nserver: nginx\nhosts:\n  - example.com\n",
        );

        // Sorted by name; static app, db and domain are excluded.
        let desired = collect_desired(&ctx);
        assert_eq!(
            desired,
            vec![
                (
                    UnitName::new("app-live").unwrap(),
                    RestartPolicy::HealthGated { port: 8080 }
                ),
                (UnitName::new("httpx").unwrap(), RestartPolicy::Always),
                (UnitName::new("pg").unwrap(), RestartPolicy::Always),
            ]
        );
    }

    #[test]
    fn health_gated_app_is_terminal_until_first_ready() {
        let now = Instant::now();
        let policy = RestartPolicy::HealthGated { port: 8080 };

        // Never healthy -> Failed (no restart loop on a broken app).
        assert!(matches!(on_child_exit(&policy, false, now), Phase::Failed));
        // Healthy once -> keep restarting with backoff.
        assert!(matches!(
            on_child_exit(&policy, true, now),
            Phase::Backoff { .. }
        ));
    }

    #[test]
    fn always_policy_restarts_regardless_of_readiness() {
        let now = Instant::now();
        assert!(matches!(
            on_child_exit(&RestartPolicy::Always, false, now),
            Phase::Backoff { .. }
        ));
        assert!(matches!(
            on_child_exit(&RestartPolicy::Always, true, now),
            Phase::Backoff { .. }
        ));
    }

    #[test]
    fn deploy_ready_requires_ready_status_and_active_version() {
        let (_dir, ctx) = ctx();
        let name = UnitName::new("app-live").unwrap();
        ctx.write_test_unit(
            "app-live",
            "type: app\nimage: alpine\nbuilds: []\nruntime:\n  port: 8080\n  cmd: ./run\n",
        );

        // No deploy state yet.
        assert!(!deploy_ready(&ctx, &name));

        // Building, then ready.
        let (_guard, mut state) = DeployState::acquire(&ctx, &name).unwrap();
        state.bump_version().unwrap();
        assert!(!deploy_ready(&ctx, &name));
        state.set_ready();
        assert!(deploy_ready(&ctx, &name));
        assert_eq!(deploy_active_version(&ctx, &name), Some(1));
    }
}
