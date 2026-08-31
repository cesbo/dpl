use std::{
    collections::BTreeMap,
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

use super::model::DbServerConfig;
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        RunError,
    },
    log,
    podman::{
        image_exists,
        pull_image,
    },
    state::DeployState,
};

/// Total budget for a db-server readiness wait.
const PING_TIMEOUT: Duration = Duration::from_secs(60);
const PING_INTERVAL: Duration = Duration::from_millis(800);

/// Cap on a single `SELECT 1` through `podman exec`. A starting engine refuses
/// in milliseconds; a wedged one must not eat the whole [`PING_TIMEOUT`].
pub const PING_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Budget for one probe: the per-call cap, or whatever is left of the deadline.
/// Never zero, so the last attempt before the deadline still runs.
pub fn probe_budget(now: Instant, deadline: Instant, cap: Duration) -> Duration {
    cap.min(deadline.saturating_duration_since(now))
        .max(Duration::from_millis(1))
}

#[derive(Debug)]
pub struct DbServerUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: DbServerConfig,
}

impl<'a> DbServerUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: DbServerConfig) -> Self {
        Self { ctx, name, config }
    }

    /// Hand the container off to `dpl serve` and wait until the
    /// engine accepts connections.
    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        let root_password = self.ctx.resolve_secret(&self.config.secret).map_err(|e| {
            DeployError::step_prepare(format!("resolve secret '{}'", &self.config.secret), e)
        })?;

        let image = self.config.image();
        if !image_exists(&image) {
            log::phase("downloading db-server image");
            pull_image(&image)
                .map_err(|e| DeployError::step_install("download db-server image", e))?;
        }

        // Snapshot the password into this version's runtime env.
        let version = state.last_version;
        let env = BTreeMap::from([(
            self.config.engine.password_env().to_string(),
            root_password.clone(),
        )]);
        crate::podman::env::save(self.ctx, self.name, version, &env)
            .map_err(|e| DeployError::step_install("save runtime env", e))?;

        // Remove old runtime env
        if let Some(old) = state.active_version {
            let _ = crate::podman::env::remove(self.ctx, self.name, old);
        }

        state.set_check();

        let container = self.name.scoped_unit_name();
        crate::serve::notify_or_warn(self.ctx, &container);

        let phase_name = format!("waiting for db-server '{}'", self.name);
        log::phase(format!(
            "{phase_name} (container {container}): the engine accepts the root login              (probe: podman exec 'SELECT 1') - up to {}",
            log::fmt_duration(PING_TIMEOUT),
        ));

        let started = Instant::now();
        let deadline = started + PING_TIMEOUT;
        loop {
            let now = Instant::now();
            let last = match self.config.engine.ping(
                self.name,
                &root_password,
                None,
                probe_budget(now, deadline, PING_PROBE_TIMEOUT),
            ) {
                Ok(()) => {
                    state.set_ready();
                    return Ok(());
                }
                Err(err) => err,
            };

            if Instant::now() >= deadline {
                // Report why the engine refused, not a bare "timeout".
                return Err(DeployError::step_startup(
                    phase_name,
                    std::io::Error::other(format!(
                        "waited {}: {last}",
                        log::fmt_duration(started.elapsed())
                    )),
                ));
            }

            sleep(PING_INTERVAL);
        }
    }

    /// Bring the db-server up if it isn't already running.
    /// When the container is already running this is a no-op.
    pub fn reload_or_deploy(self) -> Result<(), DeployError> {
        if crate::podman::is_running(self.name) {
            return Ok(());
        }

        let (_guard, mut state) = DeployState::acquire(self.ctx, self.name).map_err(|e| {
            DeployError::step_prepare(format!("acquire db-server '{}'", self.name), e)
        })?;

        state
            .begin_deploy(DbServerConfig::KIND)
            .map_err(|e| DeployError::step_prepare("begin deploy", e))?;

        match self.deploy(&mut state) {
            Ok(()) => Ok(()),
            Err(err) => {
                // No deploy console of its own here; the primary unit's state records the stage.
                if let Some((stage, message)) = err.failure() {
                    state.set_failed(stage, message);
                }
                Err(err)
            }
        }
    }

    pub fn inspect(&self) {
        crate::podman::inspect::print_container_state(self.name);
    }

    /// Run the db-server container in the foreground.
    pub fn start(&self) -> Result<(), RunError> {
        let version = {
            let state = DeployState::load(self.ctx, self.name)
                .map_err(|e| RunError::new(format!("load state for '{}'", self.name), e))?;
            state.active_version.unwrap_or(state.last_version)
        };

        if version == 0 {
            return Err(RunError::new(
                format!("db-server '{}' has not been deployed", self.name),
                std::io::Error::other("no deployed version"),
            ));
        }

        let container = self.name.scoped_unit_name();
        let mut cmd = crate::podman::PodmanRun::new(&container)
            .map_err(|e| RunError::new(format!("prepare podman to run '{}'", self.name), e))?;

        let env = crate::podman::env::load(self.ctx, self.name, version)
            .map_err(|e| RunError::new("load runtime env", e))?;
        for (env_name, value) in env {
            cmd.env(&env_name, value);
        }

        let engine = self.config.engine;
        cmd.volume(&container, engine.data_path(), &[]);

        cmd.run_foreground(self.config.image(), &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }

    /// Tear down a deployed version: stop the container and drop its env file.
    pub fn undeploy(ctx: &MainContext, name: &UnitName, version: u32) {
        if let Err(err) = crate::podman::stop_and_remove(name) {
            log::warn(format!("stop container '{name}': {err}"));
        }
        if let Err(err) = crate::podman::env::remove(ctx, name, version) {
            log::warn(format!(
                "remove runtime env file for '{name} v{version}': {err}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_budget_is_capped_far_from_the_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(60);
        assert_eq!(
            probe_budget(now, deadline, PING_PROBE_TIMEOUT),
            PING_PROBE_TIMEOUT
        );
    }

    #[test]
    fn probe_budget_shrinks_near_the_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(2);
        assert_eq!(
            probe_budget(now, deadline, PING_PROBE_TIMEOUT),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn probe_budget_is_never_zero() {
        // A deadline already past must still allow one bounded attempt.
        let now = Instant::now();
        let deadline = now - Duration::from_secs(1);
        assert_eq!(
            probe_budget(now, deadline, PING_PROBE_TIMEOUT),
            Duration::from_millis(1)
        );
    }
}
