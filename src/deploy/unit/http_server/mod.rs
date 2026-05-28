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

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        self.install_inner(Path::new(systemd::SYSTEMD_DIR))?;
        state.set_ready();
        Ok(())
    }

    /// Render this unit's `00-dpl.conf` into its conf volume, render the
    /// systemd service, then (re)start the container — restart forces
    /// `podman run --replace --rm` to recreate it.
    fn install_inner(&self, systemd_dir: &Path) -> Result<(), DeployError> {
        let container_name = self.name.scoped_unit_name();
        let conf_volume = format!("{container_name}-conf");
        let service_name = format!("{container_name}.service");

        ensure_volume(&conf_volume)
            .map_err(|e| DeployError::unit(format!("get volume '{conf_volume}'"), e))?;

        let conf_dir = volume_mountpoint(&conf_volume).map_err(|e| {
            DeployError::unit(format!("resolve volume '{conf_volume}' mountpoint"), e)
        })?;

        artifacts::write_global_config(&conf_dir)?;

        ensure_volume(NGINX_WWW_VOLUME)
            .map_err(|e| DeployError::unit(format!("get volume '{NGINX_WWW_VOLUME}'"), e))?;

        artifacts::create_service_file(systemd_dir, &self.name, &self.config)?;

        systemd::reload().map_err(|e| DeployError::unit("reload systemd", e))?;

        {
            let _phase = log::phase(format!("starting {container_name}"));
            if systemd::is_active(&service_name) {
                systemd::restart_service(&service_name).map_err(|e| {
                    DeployError::unit(format!("restart service '{service_name}'"), e)
                })?;
            } else {
                systemd::enable_service(&service_name).map_err(|e| {
                    DeployError::unit(format!("enable service '{service_name}'"), e)
                })?;
            }
        }

        let _phase = log::phase(format!("{container_name} health check"));
        health::check(&container_name, HTTP_PORT)
            .map_err(|e| DeployError::unit(format!("{container_name} health check"), e))?;

        Ok(())
    }
}
