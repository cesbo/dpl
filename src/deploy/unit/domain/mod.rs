mod artifacts;
mod host_name;
mod model;
mod proxy;
mod route_location;

use std::{
    fs,
    path::Path,
};

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use super::app;
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        http_server::{
            HttpServerConfig,
            HttpServerUnit,
        },
    },
    log,
    state::DeployState,
};

/// Filename of a domain's vhost config inside its http-server's conf dir.
fn config_name(name: &UnitName) -> String {
    format!("{name}.conf")
}

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

        let conf_dir = self.ctx.http_conf_dir(&self.config.server);
        self.write_config(&conf_dir, resolved.as_ref())?;

        // Copy each app this domain serves files from into the server's www dir,
        // so nginx finds them at the `/var/www/<app>_<version>` root we rendered.
        let www_dir = self.ctx.http_www_dir(&self.config.server);
        for serve_app in self.config.exported_apps() {
            app::export_to_www(self.ctx, &serve_app, &www_dir)?;
        }

        HttpServerUnit::new(self.ctx, &self.config.server, server_config).reload_or_deploy()
    }

    fn write_config(
        &self,
        conf_dir: &Path,
        proxy: Option<&proxy::ResolvedProxy>,
    ) -> Result<(), DeployError> {
        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            config: &self.config,
            proxy,
        };

        let content = artifacts
            .render()
            .map_err(|e| DeployError::step_install("render domain config", e))?;

        // create_dir_all covers a domain deploying before its server's first deploy.
        fs::create_dir_all(conf_dir)
            .map_err(|e| DeployError::step_install("create http-server conf dir", e))?;

        let conf_name = config_name(self.name);
        let conf_path = conf_dir.join(conf_name);
        fs::write(conf_path, content)
            .map_err(|e| DeployError::step_install("write domain config", e))?;

        Ok(())
    }

    /// Removes domain's `<name>.conf` from http-server and reload nginx if running.
    pub fn undeploy(ctx: &MainContext, name: &UnitName) {
        let conf_name = config_name(name);
        for (server, state) in DeployState::list(ctx) {
            if state.kind.as_deref() != Some(HttpServerConfig::KIND) {
                continue;
            }

            let conf_file = ctx.http_conf_dir(&server).join(&conf_name);
            match fs::remove_file(&conf_file) {
                Ok(()) => {
                    if crate::podman::is_running(&server)
                        && let Err(err) = HttpServerUnit::reload(&server)
                    {
                        log::warn(format!("reload http-server '{server}': {err}"));
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => log::warn(format!(
                    "remove domain config '{}': {err}",
                    conf_file.display()
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn test_ctx(base: &TempDir) -> MainContext {
        MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        }
    }

    fn domain(yaml: &str) -> DomainConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn write_config_drops_file_into_conf_dir() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let name = UnitName::new("site").unwrap();
        let config = domain("server: web\nhosts:\n  - example.com\nroutes: []\n");
        let unit = DomainUnit::new(&ctx, &name, config);

        let conf_dir = base.path().join("conf.d");
        unit.write_config(&conf_dir, None).unwrap();

        let content = fs::read_to_string(conf_dir.join("site.conf")).unwrap();
        assert!(content.contains("example.com"), "{content}");
    }

    #[test]
    fn undeploy_removes_the_domain_conf() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let server = UnitName::new("web").unwrap();

        // The scan drives off persisted http-server deploy state, not config.
        let (_guard, mut state) = DeployState::acquire(&ctx, &server).unwrap();
        state.begin_deploy(HttpServerConfig::KIND).unwrap();
        state.set_ready();

        let conf_dir = ctx.http_conf_dir(&server);
        fs::create_dir_all(&conf_dir).unwrap();
        let conf_file = conf_dir.join("site.conf");
        fs::write(&conf_file, "server {}\n").unwrap();

        DomainUnit::undeploy(&ctx, &UnitName::new("site").unwrap());

        assert!(!conf_file.exists());
    }

    #[test]
    fn undeploy_leaves_files_under_non_http_units() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let other = UnitName::new("web").unwrap();

        // Same name, but deployed as something other than an http-server.
        let (_guard, mut state) = DeployState::acquire(&ctx, &other).unwrap();
        state.begin_deploy("app").unwrap();
        state.set_ready();

        let conf_dir = ctx.http_conf_dir(&other);
        fs::create_dir_all(&conf_dir).unwrap();
        let conf_file = conf_dir.join("site.conf");
        fs::write(&conf_file, "server {}\n").unwrap();

        DomainUnit::undeploy(&ctx, &UnitName::new("site").unwrap());

        assert!(conf_file.exists());
    }
}
