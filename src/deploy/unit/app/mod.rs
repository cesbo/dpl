mod artifacts;
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
    path::Path,
};

use podman::PodmanContext;
use systemd::SystemdContext;
use tempfile::TempDir;
use tracing::{
    debug,
    error,
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
    config::UnitName,
    deploy::{
        DeployError,
        state::DeployState,
    },
    log,
    podman::health,
};

#[derive(Debug)]
pub struct AppUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: AppConfig,
}

impl<'a> AppUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: AppConfig) -> Self {
        Self { ctx, name, config }
    }

    fn prepare<R: Read>(&self, version: u32, archive: R) -> Result<TempDir, DeployError> {
        let _phase = log::phase("preparing");

        let temp_dir = tempfile::tempdir_in(self.ctx.unit_dir(self.name))
            .map_err(|e| DeployError::unit("create temporary directory", e))?;
        let deploy_dir = temp_dir.path();

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path)
            .map_err(|e| DeployError::unit("save app archive to the temporary directory", e))?;

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: self.name,
            config: &self.config,
            version,
        };
        artifacts.save(deploy_dir)?;

        Ok(temp_dir)
    }

    /// Run the install phase: extract archive, render artifacts,
    /// build image, install service.
    pub fn deploy<R: Read>(
        self,
        state: &mut DeployState,
        version: u32,
        archive: R,
    ) -> Result<(), DeployError> {
        let temp_dir = self.prepare(version, archive)?;
        self.deploy_worker(version, temp_dir.path(), state)?;

        state.set_ready();
        self.redeploy_dependent_domains();

        Ok(())
    }

    fn deploy_worker(
        &self,
        version: u32,
        deploy_dir: &Path,
        state: &mut DeployState,
    ) -> Result<(), DeployError> {
        self.build_inner(deploy_dir, version)?;

        if let Some(active_version) = state.take_active_version() {
            let _phase = log::phase(format_args!("uninstalling v{active_version}"));
            self.uninstall_inner(active_version);
        }

        if let Err(err) = self.install_inner(deploy_dir, version) {
            debug!("deploy failed, removing {} version {}", self.name, version);
            self.uninstall_inner(version);
            return Err(err);
        }

        Ok(())
    }

    /// Domain units whose routes reference this app via `${<app>:export|url}`.
    fn dependent_domains(&self) -> Vec<(UnitName, DomainConfig)> {
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

        let _phase = log::phase("updating dependent domains");
        for (name, config) in domains {
            let (_guard, mut state) = match DeployState::acquire(self.ctx, self.name) {
                Ok(v) => v,
                Err(err) => {
                    error!("skip domain '{name}': {err}");
                    continue;
                }
            };

            if let Err(err) = state.bump_version() {
                error!("skip domain '{name}': {err}");
                continue;
            }

            let domain = DomainUnit::new(self.ctx, &name, config);
            if let Err(err) = domain.deploy(&mut state) {
                state.set_error(None);
                error!(
                    "domain '{name}' redeploy failed: {:#}",
                    anyhow::Error::new(err)
                );
            }
        }
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        {
            let _phase = log::phase("extracting app archive");
            let archive_path = deploy_dir.join("app.tar.gz");
            let app_dir = deploy_dir.join("app");
            crate::archive::extract(&archive_path, &app_dir)
                .map_err(|e| DeployError::unit("extract app archive", e))?;
        }

        let _phase = log::phase("building app image");
        PodmanContext::new(self.name, version)
            .build(deploy_dir)
            .map_err(|e| DeployError::unit("build app image", e))?;

        Ok(())
    }

    fn install_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        if !self.config.exports.is_empty() {
            let _phase = log::phase("exporting files");
            PodmanContext::new(self.name, version)
                .export(&self.config.exports)
                .map_err(|e| DeployError::unit("export files", e))?;
        }

        let systemd_ctx = SystemdContext::new(self.name);

        {
            let _phase = log::phase("installing app service");
            systemd_ctx
                .install_app(deploy_dir)
                .map_err(|e| DeployError::unit("install app service", e))?;
        }

        {
            let phase_name = "waiting for app".to_string();
            let _phase = log::phase(&phase_name);
            health::check(self.name, self.config.port)
                .map_err(|e| DeployError::unit(phase_name, e))?;

            systemd_ctx
                .set_restart_value("always")
                .map_err(|e| DeployError::unit("set restart policy to 'always'", e))?;
        }

        let _phase = log::phase("installing timers");
        systemd_ctx.install_timers(deploy_dir);

        Ok(())
    }

    fn uninstall_inner(&self, version: u32) {
        let systemd_ctx = SystemdContext::new(self.name);
        systemd_ctx.uninstall_timers();
        systemd_ctx.uninstall_app();

        let podman_ctx = PodmanContext::new(self.name, version);
        podman_ctx.remove_exports();
        podman_ctx.remove();
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
            "type: domain\nserver: nginx\nhosts: [\"example.com\"]\nroutes:\n  - location: /\n    kind: serve_files\n    root: \"${web:export}\"\n",
        );
        // References a different app — should be skipped.
        write_unit(
            base.path(),
            "other-site",
            "type: domain\nserver: nginx\nhosts: [\"other.com\"]\nroutes:\n  - location: /\n    kind: reverse_proxy\n    target: \"${api:url}\"\n",
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
        let unit_name = UnitName::new("web").unwrap();
        let app = AppUnit::new(&ctx, &unit_name, config);

        let domains = app.dependent_domains();
        let names: Vec<&str> = domains.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["site"]);
    }
}
