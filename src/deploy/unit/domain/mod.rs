mod artifacts;
mod host_name;
mod model;
mod proxy;
mod route_location;

use std::path::{
    Path,
    PathBuf,
};

use tracing::warn;

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::DeployState,
    },
    log,
    podman::{
        ensure_volume,
        volume_mountpoint,
    },
    systemd,
};

#[derive(Debug)]
pub struct DomainUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: DomainConfig,
}

impl<'a> DomainUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: impl Into<String>, config: DomainConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    pub fn deploy(self, mut state: DeployState) -> Result<(), DeployError> {
        let _ = state.bump_version()?;
        let _ = state.take_active_version();

        if let Err(err) = self.install_inner() {
            state.set_error();
            Err(err)
        } else {
            state.set_ready();
            Ok(())
        }
    }

    fn install_inner(&self) -> Result<(), DeployError> {
        let server_container = self.config.server.scoped_unit_name();
        let conf_volume = format!("{server_container}-conf");
        let service_name = format!("{server_container}.service");

        ensure_volume(&conf_volume)
            .map_err(|e| DeployError::unit(format!("get volume '{conf_volume}'"), e))?;

        let conf_dir = volume_mountpoint(&conf_volume).map_err(|e| {
            DeployError::unit(format!("resolve volume '{conf_volume}' mountpoint"), e)
        })?;

        // Resolve the proxy's trusted-IP allowlist.
        let resolved = match &self.config.proxy {
            Some(cfg) => {
                let _phase = log::phase("resolving proxy IP ranges");
                Some(
                    proxy::resolve(cfg)
                        .map_err(|e| DeployError::unit("resolve proxy IP ranges", e))?,
                )
            }
            None => None,
        };

        self.write_config(&conf_dir, resolved.as_ref())?;

        if systemd::is_active(&service_name) {
            let _phase = log::phase(format!("reloading {server_container}"));
            systemd::reload_service(&service_name)
                .map_err(|e| DeployError::unit(format!("reload service '{service_name}'"), e))?;
        } else {
            warn!(
                "http-server '{}' is not running; config written but nginx not reloaded",
                self.config.server,
            );
        }

        Ok(())
    }

    fn write_config(
        &self,
        conf_dir: &Path,
        proxy: Option<&proxy::ResolvedProxy>,
    ) -> Result<(), DeployError> {
        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
            proxy,
        };
        artifacts.save(conf_dir)?;

        Ok(())
    }
}
