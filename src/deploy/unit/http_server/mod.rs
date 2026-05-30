mod artifacts;
mod model;

use std::path::Path;

pub use self::model::HttpServerConfig;
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        state::DeployState,
    },
    log,
    podman::{
        NGINX_WWW_VOLUME,
        ensure_volume,
        health,
        volume_mountpoint,
    },
    systemd,
};

const HTTP_PORT: u16 = 80;

#[derive(Debug)]
pub struct HttpServerUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: HttpServerConfig,
}

impl<'a> HttpServerUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: HttpServerConfig) -> Self {
        Self { ctx, name, config }
    }

    /// Per-instance conf volume name (`dpl--<name>-conf`). Holds the global
    /// `00-dpl.conf` plus every dependent domain's `<domain>.conf`.
    pub fn conf_volume(&self) -> String {
        format!("{}-conf", self.name.scoped_unit_name())
    }

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        self.install_inner(Path::new(systemd::SYSTEMD_DIR))?;
        state.set_ready();
        Ok(())
    }

    /// Make this http-server reflect on-disk config:
    ///   - if its systemd service is active, ask nginx to reload (fast path);
    ///   - otherwise run a full deploy under the unit's own DeployState lock,
    ///     so a dependent unit (e.g. a domain) can trigger the chain.
    pub fn reload_or_deploy(self) -> Result<(), DeployError> {
        let service_name = format!("{}.service", self.name.scoped_unit_name());

        if systemd::is_active(&service_name) {
            let _phase = log::phase(format!("reloading http-server '{}'", self.name));
            return systemd::reload_service(&service_name).map_err(|e| {
                DeployError::step_install(format!("reload service '{service_name}'"), e)
            });
        }

        let (_guard, mut state) = DeployState::acquire(self.ctx, self.name).map_err(|e| {
            DeployError::step_prepare(format!("acquire http-server '{}'", self.name), e)
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

    /// Render this unit's `00-dpl.conf` into its conf volume, render the
    /// systemd service, then (re)start the container — restart forces
    /// `podman run --replace --rm` to recreate it.
    fn install_inner(&self, systemd_dir: &Path) -> Result<(), DeployError> {
        let container_name = self.name.scoped_unit_name();
        let service_name = format!("{container_name}.service");

        let conf_volume = self.conf_volume();
        let conf_dir = ensure_volume(&conf_volume)
            .and_then(|_| volume_mountpoint(&conf_volume))
            .map_err(|e| {
                DeployError::step_install(format!("resolve volume '{conf_volume}' mountpoint"), e)
            })?;

        artifacts::write_global_config(&conf_dir)
            .map_err(|e| DeployError::step_install("write global config for http-server", e))?;

        ensure_volume(NGINX_WWW_VOLUME).map_err(|e| {
            DeployError::step_install(format!("get volume '{NGINX_WWW_VOLUME}'"), e)
        })?;

        artifacts::create_service_file(systemd_dir, self)
            .map_err(|e| DeployError::step_install("create service file for http-server", e))?;

        systemd::reload().map_err(|e| DeployError::step_install("reload systemd", e))?;

        {
            let _phase = log::phase(format!("starting http-server '{}'", self.name));
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

        let phase_name = format!("waiting for http-server '{}'", self.name);
        let _phase = log::phase(&phase_name);
        if let Err(err) = health::check(self.name, HTTP_PORT) {
            tracing::error!("{err}");
            return Err(DeployError::step_runtime(phase_name, err));
        }

        Ok(())
    }
}
