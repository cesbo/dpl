mod artifacts;
mod host_name;
mod model;
mod proxy;
mod route_location;

use std::path::Path;

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use super::http_server::HttpServerUnit;
use crate::{
    MainContext,
    config::UnitName,
    deploy::DeployError,
    log,
    podman::{
        ensure_volume,
        volume_mountpoint,
    },
    state::DeployState,
};

#[derive(Debug)]
pub struct DomainUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: DomainConfig,
}

impl<'a> DomainUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: DomainConfig) -> Self {
        Self { ctx, name, config }
    }

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        self.install_inner()?;
        state.set_ready();
        Ok(())
    }

    fn install_inner(&self) -> Result<(), DeployError> {
        let server_config = self.config.resolve_server(self.ctx).map_err(|e| {
            DeployError::step_prepare(format!("resolve http-server '{}'", self.config.server), e)
        })?;
        let server_unit = HttpServerUnit::new(self.ctx, &self.config.server, server_config);

        let conf_volume = server_unit.conf_volume();
        ensure_volume(&conf_volume)
            .map_err(|e| DeployError::step_install(format!("get volume '{conf_volume}'"), e))?;

        let conf_dir = volume_mountpoint(&conf_volume).map_err(|e| {
            DeployError::step_install(format!("resolve volume '{conf_volume}' mountpoint"), e)
        })?;

        // Resolve the proxy's trusted-IP allowlist.
        let resolved = match &self.config.proxy {
            Some(cfg) => {
                log::phase("resolving proxy IP ranges");
                Some(
                    proxy::resolve(cfg)
                        .map_err(|e| DeployError::step_install("resolve proxy IP ranges", e))?,
                )
            }
            None => None,
        };

        self.write_config(&conf_dir, resolved.as_ref())?;

        server_unit.reload_or_deploy()
    }

    fn write_config(
        &self,
        conf_dir: &Path,
        proxy: Option<&proxy::ResolvedProxy>,
    ) -> Result<(), DeployError> {
        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: self.name,
            config: &self.config,
            proxy,
        };
        artifacts
            .save(conf_dir)
            .map_err(|e| DeployError::step_install("render domain config", e))?;

        Ok(())
    }
}
