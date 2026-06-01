use std::{
    path::Path,
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

use super::{
    artifacts,
    model::DbServerConfig,
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        state::DeployState,
    },
    log,
    systemd,
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

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        self.install_inner(Path::new(systemd::SYSTEMD_DIR))?;
        state.set_ready();
        Ok(())
    }

    /// Bring the db-server up if it isn't already running.
    /// When the systemd service is active this is a no-op.
    pub fn reload_or_deploy(self) -> Result<(), DeployError> {
        let service_name = format!("{}.service", self.name.scoped_unit_name());

        if systemd::is_active(&service_name) {
            return Ok(());
        }

        let (_guard, mut state) = DeployState::acquire(self.ctx, self.name).map_err(|e| {
            DeployError::step_prepare(format!("acquire db-server '{}'", self.name), e)
        })?;

        state
            .bump_version()
            .map_err(|e| DeployError::step_prepare("bump version", e))?;

        match self.deploy(&mut state) {
            Ok(()) => Ok(()),
            Err(err) => {
                // No DeployLog handle here; the primary unit's state records the stage.
                state.set_error(&err);
                Err(err)
            }
        }
    }

    fn install_inner(&self, systemd_dir: &Path) -> Result<(), DeployError> {
        let root_password = self.ctx.resolve_secret(&self.config.secret).map_err(|e| {
            DeployError::step_prepare(format!("resolve secret '{}'", &self.config.secret), e)
        })?;

        artifacts::create_service_file(
            systemd_dir,
            self.name,
            self.config.engine,
            &self.config.image(),
            &root_password,
        )
        .map_err(|e| DeployError::step_install("render db-server service", e))?;

        systemd::reload().map_err(|e| DeployError::step_install("reload systemd", e))?;

        let service_name = format!("{}.service", self.name.scoped_unit_name());

        {
            let _phase = log::phase(format!("starting db-server '{}'", self.name));
            if systemd::is_active(&service_name) {
                systemd::restart_service(&service_name).map_err(|e| {
                    DeployError::step_install(format!("restart service for '{}'", self.name), e)
                })?;
            } else {
                systemd::enable_service(&service_name).map_err(|e| {
                    DeployError::step_install(format!("enable service for '{}'", self.name), e)
                })?;
            }
        }

        let _phase = log::phase(format!("waiting for db-server '{}'", self.name));
        let deadline = Instant::now() + PING_TIMEOUT;
        loop {
            if self
                .config
                .engine
                .ping(self.name, &root_password, None)
                .is_ok()
            {
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

    pub fn inspect(&self) {
        crate::podman::inspect::print_container_state(self.name);
    }
}
