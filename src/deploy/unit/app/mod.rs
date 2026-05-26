mod artifacts;
mod health;
mod inspect;
mod model;
mod podman;
mod systemd;

use std::{
    fs,
    io::{
        self,
        Read,
    },
    path::{
        Path,
        PathBuf,
    },
};

use podman::PodmanContext;
use systemd::SystemdContext;
use tempfile::TempDir;
use tracing::{
    debug,
    error,
    info,
    warn,
};

use self::artifacts::ArtifactsContext;
pub use self::model::AppConfig;
use super::{
    UnitConfig,
    domain::{
        DomainConfig,
        DomainUnit,
    },
    list_units,
};
use crate::{
    MainContext,
    config::ResourceName,
    deploy::{
        DeployError,
        state::DeployState,
    },
    log::{
        DeployLog,
        PHASE_TARGET,
    },
};

#[derive(Debug)]
pub struct AppUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a ResourceName,
    pub unit_dir: PathBuf,
    pub config: AppConfig,
}

impl<'a> AppUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a ResourceName, config: AppConfig) -> Self {
        let unit_dir = name.unit_dir(ctx);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    fn prepare<R: Read>(&self, version: u32, archive: R) -> Result<TempDir, DeployError> {
        info!(target: PHASE_TARGET, "preparing");

        let temp_dir =
            tempfile::tempdir_in(&self.unit_dir).map_err(|source| DeployError::UnitError {
                info: "failed to create temporary build directory".to_string(),
                source,
            })?;
        let deploy_dir = temp_dir.path();

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path).map_err(|source| DeployError::UnitError {
            info: "failed to save archive".to_string(),
            source,
        })?;

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: self.name.as_str(),
            config: &self.config,
            version,
        };
        artifacts.save(deploy_dir)?;

        Ok(temp_dir)
    }

    /// Run the full deploy synchronously: bump version, extract archive,
    /// render artifacts, build image, install service.
    pub fn deploy<R: Read>(self, mut state: DeployState, archive: R) -> Result<(), DeployError> {
        let version = state.bump_version()?;

        let build_log_path = self.build_log_path(version);
        let log = match DeployLog::open(&build_log_path, self.name.as_str(), version) {
            Ok(log) => log,
            Err(source) => {
                state.set_error();
                return Err(DeployError::UnitError {
                    info: "open deploy log".to_string(),
                    source,
                });
            }
        };

        // Route the `tracing` macros below (and on the podman worker threads)
        // to this deploy's subscriber for the rest of the function. From here on
        // failures are rendered by `finish_err` and reported via the build log,
        // so they collapse to `DeployError::Reported` instead of bubbling up.
        let _default = log.set_default();

        let temp_dir = match self.prepare(version, archive) {
            Ok(temp_dir) => temp_dir,
            Err(err) => return Err(self.fail(&log, &mut state, err)),
        };

        match self.deploy_worker(version, temp_dir.path(), &mut state) {
            Ok(()) => {
                state.set_ready();

                self.redeploy_dependent_domains();
                log.finish_ok();

                Ok(())
            }
            Err(err) => Err(self.fail(&log, &mut state, err)),
        }
    }

    /// Mark the deploy failed: record the cause in the build log (file only),
    /// print the one-line summary, and collapse to [`DeployError::Reported`].
    fn fail(&self, log: &DeployLog, state: &mut DeployState, err: DeployError) -> DeployError {
        state.set_error();
        debug!("deploy failed: {:#}", anyhow::Error::new(err));
        log.finish_err();
        DeployError::Reported
    }

    fn deploy_worker(
        &self,
        version: u32,
        deploy_dir: &Path,
        state: &mut DeployState,
    ) -> Result<(), DeployError> {
        self.build_inner(deploy_dir, version)?;

        if let Some(active_version) = state.take_active_version() {
            info!(target: PHASE_TARGET, "uninstalling v{active_version}");
            self.uninstall_inner(active_version);
        }

        if let Err(err) = self.install_inner(deploy_dir, version) {
            self.uninstall_inner(version);
            return Err(err);
        }

        Ok(())
    }

    /// Domain units whose routes reference this app via `${<app>:export|url}`.
    fn dependent_domains(&self) -> Vec<(ResourceName, DomainConfig)> {
        list_units(self.ctx, |c| matches!(c, UnitConfig::Domain(_)))
            .into_iter()
            .filter_map(|(name, config)| match config {
                UnitConfig::Domain(d) if d.unit_deps().contains(self.name) => Some((name, d)),
                _ => None,
            })
            .collect()
    }

    /// Re-render and reload every domain that depends on this app.
    /// Should be called after state saved.
    fn redeploy_dependent_domains(&self) {
        let domains = self.dependent_domains();
        if domains.is_empty() {
            return;
        }

        info!(target: PHASE_TARGET, "updating dependent domains");
        for (name, config) in domains {
            let unit_dir = name.unit_dir(self.ctx);
            let (_guard, state) = match DeployState::acquire(&unit_dir) {
                Ok(v) => v,
                Err(err) => {
                    error!("skip domain '{name}': {err}");
                    continue;
                }
            };

            let domain = DomainUnit::new(self.ctx, name.as_str(), config);
            if let Err(err) = domain.deploy(state) {
                error!(
                    "domain '{name}' redeploy failed: {:#}",
                    anyhow::Error::new(err)
                );
            }
        }
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        info!(target: PHASE_TARGET, "extracting archive");

        let archive_path = deploy_dir.join("app.tar.gz");
        let app_dir = deploy_dir.join("app");

        crate::archive::extract(&archive_path, &app_dir)?;

        info!(target: PHASE_TARGET, "building image");
        PodmanContext::new(self.name.as_str(), version)
            .build(deploy_dir)
            .map_err(|source| DeployError::UnitError {
                info: "failed to build image".to_string(),
                source,
            })?;

        Ok(())
    }

    fn install_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        if !self.config.exports.is_empty() {
            info!(target: PHASE_TARGET, "exporting files");
            PodmanContext::new(self.name.as_str(), version)
                .export(&self.config.exports)
                .map_err(|source| DeployError::UnitError {
                    info: "export static files".to_string(),
                    source,
                })?;
        }

        let systemd_ctx = SystemdContext::new(self.name.as_str());

        info!(target: PHASE_TARGET, "installing service");
        systemd_ctx
            .install_app(deploy_dir)
            .map_err(|source| DeployError::UnitError {
                info: "install app service".to_string(),
                source,
            })?;

        info!(target: PHASE_TARGET, "health check");
        health::check(self.name.as_str(), self.config.port).map_err(|source| {
            DeployError::UnitError {
                info: "health check".to_string(),
                source,
            }
        })?;
        systemd_ctx
            .set_restart_value("always")
            .map_err(|source| DeployError::UnitError {
                info: "set restart policy to 'always'".to_string(),
                source,
            })?;

        info!(target: PHASE_TARGET, "installing timers");
        systemd_ctx.install_timers(deploy_dir);

        Ok(())
    }

    fn uninstall_inner(&self, version: u32) {
        let systemd_ctx = SystemdContext::new(self.name.as_str());
        systemd_ctx.uninstall_timers();
        systemd_ctx.uninstall_app();

        let podman_ctx = PodmanContext::new(self.name.as_str(), version);
        podman_ctx.remove_exports();
        podman_ctx.remove();

        self.remove_build_log(version);
    }

    fn build_log_path(&self, version: u32) -> PathBuf {
        self.unit_dir
            .join("log")
            .join(format!("build-{version}.log"))
    }

    fn remove_build_log(&self, version: u32) {
        let path = self.build_log_path(version);
        match fs::remove_file(&path) {
            Ok(()) => debug!("removed build log {}", path.display()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => warn!("failed to remove build log {}: {err}", path.display()),
        }
    }
}

fn save_archive<R: Read>(archive: R, dst: &Path) -> io::Result<()> {
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst)?;
    io::copy(&mut reader, &mut archive_file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn write_unit(base: &Path, name: &str, config: &str) {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), config).unwrap();
    }

    #[test]
    fn dependent_domains_selects_only_referencing_domains() {
        let base = TempDir::new().unwrap();

        // Serves this app's static export — should be selected.
        write_unit(
            base.path(),
            "site",
            "type: domain\nhosts: [\"example.com\"]\nroutes:\n  - location: /\n    kind: serve_files\n    root: \"${web:export}\"\n",
        );
        // References a different app — should be skipped.
        write_unit(
            base.path(),
            "other-site",
            "type: domain\nhosts: [\"other.com\"]\nroutes:\n  - location: /\n    kind: reverse_proxy\n    target: \"${api:url}\"\n",
        );
        // A non-domain unit — must not match the domain predicate.
        write_unit(
            base.path(),
            "web-db",
            "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
        );

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config: AppConfig =
            serde_yaml::from_str("image: alpine\nport: 8080\nbuilds: []\nruntime:\n  cmd: ./run\n")
                .unwrap();
        let unit_name = ResourceName::new("web").unwrap();
        let app = AppUnit::new(&ctx, &unit_name, config);

        let domains = app.dependent_domains();
        let names: Vec<&str> = domains.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["site"]);
    }
}
