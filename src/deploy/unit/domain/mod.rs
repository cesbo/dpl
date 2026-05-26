mod artifacts;
mod host_name;
mod model;
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
    podman::{
        NGINX_CONF_VOLUME,
        NGINX_WWW_VOLUME,
        ensure_volume,
        volume_mountpoint,
    },
    systemd,
};

const NGINX_SERVICE: &str = "dpl-nginx.service";

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
        ensure_volume(NGINX_CONF_VOLUME).map_err(|source| DeployError::UnitError {
            info: format!("get nginx volume '{NGINX_CONF_VOLUME}'"),
            source,
        })?;

        let conf_dir =
            volume_mountpoint(NGINX_CONF_VOLUME).map_err(|source| DeployError::UnitError {
                info: format!("resolve nginx volume '{NGINX_CONF_VOLUME}' mountpoint"),
                source,
            })?;

        self.write_config(&conf_dir)?;

        // If nginx is already running, just reload its config; otherwise
        // (first deploy, or a stopped/crashed service) (re)create and start it.
        if systemd::is_active(NGINX_SERVICE) {
            systemd::reload_service(NGINX_SERVICE).map_err(|source| DeployError::UnitError {
                info: format!("reload service '{NGINX_SERVICE}'"),
                source,
            })?;
        } else {
            self.install_service(&conf_dir, Path::new(systemd::SYSTEMD_DIR))?;
        }

        Ok(())
    }

    /// Create the singleton `dpl-nginx` service: ensure the static-export
    /// volume exists, drop the global `00-dpl.conf`, render the service unit,
    /// then `daemon-reload` and `enable --now`.
    fn install_service(&self, conf_dir: &Path, systemd_dir: &Path) -> Result<(), DeployError> {
        ensure_volume(NGINX_WWW_VOLUME).map_err(|source| DeployError::UnitError {
            info: format!("get nginx volume '{NGINX_WWW_VOLUME}'"),
            source,
        })?;

        artifacts::write_global_config(conf_dir)?;
        artifacts::create_nginx_service(systemd_dir)?;

        systemd::reload().map_err(|source| DeployError::UnitError {
            info: "reload systemd".to_string(),
            source,
        })?;
        crate::spinner::with_spinner("starting nginx", |_| systemd::enable_service(NGINX_SERVICE))
            .map_err(|source| DeployError::UnitError {
                info: format!("enable service '{NGINX_SERVICE}'"),
                source,
            })?;

        Ok(())
    }

    fn write_config(&self, conf_dir: &Path) -> Result<(), DeployError> {
        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
        };
        artifacts.save(conf_dir)?;

        Ok(())
    }
}
