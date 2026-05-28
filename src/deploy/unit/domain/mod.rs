mod artifacts;
mod host_name;
mod model;
mod proxy;
mod route_location;

use std::path::{
    Path,
    PathBuf,
};

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
        NGINX_CONF_VOLUME,
        NGINX_WWW_VOLUME,
        ensure_volume,
        health,
        volume_mountpoint,
    },
    systemd,
};

const NGINX_CONTAINER: &str = "dpl-nginx";
const NGINX_SERVICE: &str = "dpl-nginx.service";
const NGINX_PORT: u16 = 80;

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

    /// Render this domain's config into the `dpl-nginx-conf` volume, then make
    /// sure the singleton `dpl-nginx` service is running: create it on first
    /// use (also dropping the global `00-dpl.conf`), or reload nginx if it is
    /// already installed.
    fn install_inner(&self) -> Result<(), DeployError> {
        ensure_volume(NGINX_CONF_VOLUME)
            .map_err(|e| DeployError::unit(format!("get nginx volume '{NGINX_CONF_VOLUME}'"), e))?;

        let conf_dir = volume_mountpoint(NGINX_CONF_VOLUME).map_err(|e| {
            DeployError::unit(
                format!("resolve nginx volume '{NGINX_CONF_VOLUME}' mountpoint"),
                e,
            )
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

        // If nginx is already running, just reload its config; otherwise
        // (first deploy, or a stopped/crashed service) (re)create and start it.
        if systemd::is_active(NGINX_SERVICE) {
            systemd::reload_service(NGINX_SERVICE)
                .map_err(|e| DeployError::unit(format!("reload service '{NGINX_SERVICE}'"), e))?;
        } else {
            self.install_service(&conf_dir, Path::new(systemd::SYSTEMD_DIR))?;
        }

        Ok(())
    }

    /// Create the singleton `dpl-nginx` service: ensure the static-export
    /// volume exists, drop the global `00-dpl.conf`, render the service unit,
    /// then `daemon-reload` and `enable --now`.
    fn install_service(&self, conf_dir: &Path, systemd_dir: &Path) -> Result<(), DeployError> {
        ensure_volume(NGINX_WWW_VOLUME)
            .map_err(|e| DeployError::unit(format!("get nginx volume '{NGINX_WWW_VOLUME}'"), e))?;

        artifacts::write_global_config(conf_dir)?;
        artifacts::create_nginx_service(systemd_dir)?;

        systemd::reload().map_err(|e| DeployError::unit("reload systemd", e))?;
        {
            let _phase = log::phase("starting nginx");
            systemd::enable_service(NGINX_SERVICE)
                .map_err(|e| DeployError::unit(format!("enable service '{NGINX_SERVICE}'"), e))?;
        }

        let _phase = log::phase("nginx health check");
        health::check(NGINX_CONTAINER, NGINX_PORT)
            .map_err(|e| DeployError::unit("nginx health check", e))?;

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
