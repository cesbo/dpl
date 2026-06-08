use std::{
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

const PING_TIMEOUT: Duration = Duration::from_secs(60);
const PING_INTERVAL: Duration = Duration::from_millis(800);

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

        state.set_check();
        crate::serve::notify(self.ctx);

        log::phase(format!("waiting for db-server '{}'", self.name));
        let deadline = Instant::now() + PING_TIMEOUT;
        loop {
            if self
                .config
                .engine
                .ping(self.name, &root_password, None)
                .is_ok()
            {
                state.set_ready();
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(DeployError::step_startup(
                    format!("waiting for db-server '{}'", self.name),
                    std::io::Error::other("timeout"),
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
        let password = self
            .ctx
            .resolve_secret(&self.config.secret)
            .map_err(|e| RunError::new(format!("resolve secret '{}'", &self.config.secret), e))?;

        let container = self.name.scoped_unit_name();
        let mut cmd = crate::podman::PodmanRun::new(&container)
            .map_err(|e| RunError::new(format!("prepare podman to run '{}'", self.name), e))?;

        let engine = self.config.engine;
        cmd.env(engine.password_env(), password);

        cmd.volume(format!("{container}-data"), engine.data_path());

        cmd.run_foreground(self.config.image(), &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }
}
