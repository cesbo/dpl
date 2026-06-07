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

/// Delay before a crashed container is restarted.
const RESTART_BACKOFF: Duration = Duration::from_secs(5);
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
    /// When set, don't respawn until this instant (post-crash backoff).
    backoff_until: Option<Instant>,
    /// active_version observed when adopted; a redeploy bumps it and re-adopts.
    active_version: Option<u32>,
}

impl ManagedUnit {
    fn new(name: UnitName, active_version: Option<u32>) -> Self {
        ManagedUnit {
            name,
            child: None,
            backoff_until: None,
            active_version,
        }
    }
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
    /// runtime unit per its deploy status.
    pub fn reconcile(&mut self, ctx: &MainContext) {
        let Some(exe) = self.self_exe.clone() else {
            return;
        };

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

/// Ensure a `Check`/`Ready` unit's container is running: reap a dead child and
/// schedule a backoff restart, or spawn one once the backoff elapses. A
/// redeploy (new active version) re-adopts from scratch.
fn drive(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit, active: Option<u32>) {
    if m.active_version != active {
        stop_child(m);
        m.active_version = active;
        m.backoff_until = None;
    }

    let now = Instant::now();
    if let Some(child) = m.child.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => {
                log::warn(format!(
                    "supervisor: '{}' exited; restarting",
                    m.name.as_str()
                ));
                m.child = None;
                m.backoff_until = Some(now + RESTART_BACKOFF);
            }
            Ok(None) => {}
            Err(err) => {
                log::warn(format!("supervisor: wait '{}': {err}", m.name.as_str()));
                m.child = None;
                m.backoff_until = Some(now + RESTART_BACKOFF);
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

/// Spawn `dpl start <unit>`; back off on spawn failure.
fn spawn(ctx: &MainContext, exe: &Path, m: &mut ManagedUnit) {
    match spawn_child(exe, ctx.base(), &m.name) {
        Ok(child) => {
            m.child = Some(child);
            m.backoff_until = None;
        }
        Err(err) => {
            log::warn(format!("supervisor: start '{}': {err}", m.name.as_str()));
            m.backoff_until = Some(Instant::now() + RESTART_BACKOFF);
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
