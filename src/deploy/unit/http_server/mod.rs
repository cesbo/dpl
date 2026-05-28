mod artifacts;
mod model;

use std::path::Path;

pub use self::model::HttpServerConfig;
use crate::{
    MainContext,
    config::ResourceName,
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
    pub name: ResourceName,
    pub config: HttpServerConfig,
}

impl<'a> HttpServerUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &ResourceName, config: HttpServerConfig) -> Self {
        Self {
            ctx,
            name: name.clone(),
            config,
        }
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
            let _phase = log::phase(format!("reloading http-server '{}'", &self.name));
            return systemd::reload_service(&service_name)
                .map_err(|e| DeployError::unit(format!("reload service '{service_name}'"), e));
        }

        let unit_dir = self.name.unit_dir(self.ctx);
        let (_guard, mut state) = DeployState::acquire(&unit_dir)
            .map_err(|e| DeployError::unit(format!("acquire http-server '{}'", &self.name), e))?;
        state.bump_version().map_err(DeployError::from)?;

        match self.deploy(&mut state) {
            Ok(()) => Ok(()),
            Err(err) => {
                state.set_error();
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
                DeployError::unit(format!("resolve volume '{conf_volume}' mountpoint"), e)
            })?;

        artifacts::write_global_config(&conf_dir)?;

        ensure_volume(NGINX_WWW_VOLUME)
            .map_err(|e| DeployError::unit(format!("get volume '{NGINX_WWW_VOLUME}'"), e))?;

        artifacts::create_service_file(systemd_dir, self)?;

        systemd::reload().map_err(|e| DeployError::unit("reload systemd", e))?;

        {
            let _phase = log::phase(format!("starting http-server '{}'", &self.name));
            if systemd::is_active(&service_name) {
                systemd::restart_service(&service_name).map_err(|e| {
                    DeployError::unit(format!("restart service for '{}'", &self.name), e)
                })?;
            } else {
                systemd::enable_service(&service_name).map_err(|e| {
                    DeployError::unit(format!("enable service for '{}'", &self.name), e)
                })?;
            }
        }

        let _phase = log::phase(format!("{container_name} health check"));
        health::check(&self.name, HTTP_PORT)
            .map_err(|e| DeployError::unit(format!("{container_name} health check"), e))?;

        Ok(())
    }
}
