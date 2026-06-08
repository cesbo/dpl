mod artifacts;
mod model;
mod podman;

use std::{
    fs,
    io::{
        self,
        Read,
    },
    path::Path,
    time::Duration,
};

use chrono::Utc;
use croner::Cron;
use tempfile::TempDir;

pub use self::model::AppConfig;
use self::{
    artifacts::ArtifactsContext,
    podman::PodmanContext,
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        RunError,
        UnitConfig,
        unit::{
            domain::{
                DomainConfig,
                DomainUnit,
            },
            list_units,
        },
    },
    log,
    state::DeployState,
    timers::TimersState,
};

/// How long `dpl start` waits for each database dependency before giving up.
const DB_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

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

    fn prepare<R: Read>(&self, archive: R) -> Result<TempDir, DeployError> {
        log::phase("preparing");

        let state_dir = self.ctx.state_dir();
        let temp_dir = tempfile::tempdir_in(&state_dir)
            .map_err(|e| DeployError::step_prepare("create temporary directory", e))?;
        let deploy_dir = temp_dir.path();

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path).map_err(|e| {
            DeployError::step_prepare("save app archive to the temporary directory", e)
        })?;

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            config: &self.config,
        };

        artifacts
            .save(deploy_dir)
            .map_err(|e| DeployError::step_prepare("render artifacts", e))?;

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
        let temp_dir = self.prepare(archive)?;
        self.deploy_worker(version, temp_dir.path(), state)?;

        self.start_via_serve(state)?;
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
            log::phase(format_args!("uninstalling v{active_version}"));
            Self::undeploy(self.name, active_version);
        }

        if let Err(err) = self.install_inner(deploy_dir, version) {
            Self::undeploy(self.name, version);
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

        log::phase("updating dependent domains");
        for (name, config) in domains {
            let (_guard, mut state) = match DeployState::acquire(self.ctx, &name) {
                Ok(v) => v,
                Err(err) => {
                    log::error(format!("skip domain '{name}': {err}"));
                    continue;
                }
            };

            if let Err(err) = state.begin_deploy(DomainConfig::KIND) {
                log::error(format!("skip domain '{name}': {err}"));
                continue;
            }

            let domain = DomainUnit::new(self.ctx, &name, config);
            if let Err(err) = domain.deploy(&mut state) {
                if let Some((stage, message)) = err.failure() {
                    state.set_failed(stage, message);
                }
                log::error(format!(
                    "domain '{name}' redeploy failed: {:#}",
                    anyhow::Error::new(err)
                ));
            }
        }
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        {
            log::phase("extracting app archive");
            let archive_path = deploy_dir.join("app.tar.gz");
            let app_dir = deploy_dir.join("app");
            crate::archive::extract(&archive_path, &app_dir)
                .map_err(|e| DeployError::step_build("extract app archive", e))?;
        }

        log::phase("building app image");
        PodmanContext::new(self.name, version)
            .build(deploy_dir, &self.ctx.build_log_path(self.name))
            .map_err(|e| DeployError::step_build("build app image", e))?;

        Ok(())
    }

    fn install_inner(&self, _deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        if !self.config.exports.is_empty() {
            log::phase("exporting files");
            PodmanContext::new(self.name, version)
                .export(&self.config.exports)
                .map_err(|e| DeployError::step_install("export files", e))?;
        }

        // A static build-and-export unit has no runtime: nothing else to install.
        // The build + export above is the whole deploy. Starting the container is
        // `dpl serve` starts the container (see `start_via_serve`).
        if self.config.runtime.is_none() {
            return Ok(());
        }

        log::phase("registering timers");
        self.register_timers();

        Ok(())
    }

    /// Hand the container off to `dpl serve` and wait for it to come
    /// up. Marks the build `Check` (active, unverified), nudges serve to start
    /// it, then runs the readiness check and flips to `Ready`. A static unit has
    /// no container and is `Ready` immediately.
    fn start_via_serve(&self, state: &mut DeployState) -> Result<(), DeployError> {
        let Some(runtime) = &self.config.runtime else {
            state.set_ready();
            return Ok(());
        };

        state.set_check();
        crate::serve::notify(self.ctx);

        let phase_name = format!("waiting for app '{}'", self.name);
        log::phase(&phase_name);
        if let Err(err) = crate::podman::health::check(self.name, runtime.port) {
            return Err(DeployError::step_startup(phase_name, err));
        }

        state.set_ready();
        Ok(())
    }

    /// Reconcile persisted timer state against the configured timers.
    /// Best-effort: a failure here must not fail an otherwise-successful deploy.
    fn register_timers(&self) {
        let configured: Vec<(String, Cron)> = self
            .config
            .timers
            .iter()
            .filter(|t| !t.disabled)
            .map(|t| (t.name.clone(), t.schedule.clone()))
            .collect();

        let (_lock, mut timers) = match TimersState::acquire(self.ctx, self.name) {
            Ok(acquired) => acquired,
            Err(err) => {
                log::warn(format!("register timers for '{}': {err}", self.name));
                return;
            }
        };

        timers.reconcile(&configured, Utc::now());
    }

    pub fn undeploy(name: &UnitName, version: u32) {
        // Stop the running container (serve was supervising it) before dropping
        // its image; serve sees the unit leave `Ready` and won't restart it.
        if let Err(err) = crate::podman::stop_and_remove(name) {
            log::warn(format!("stop container '{name}': {err}"));
        }

        remove_version_artifacts(name, version);
    }

    pub fn inspect(&self) -> Result<(), DeployError> {
        // A static build-and-export unit never runs a container.
        if self.config.runtime.is_none() {
            log::print_field(
                "Container",
                format!("{} static export (no container)", log::success_mark()),
            );
            return Ok(());
        }

        crate::podman::inspect::print_container_state(self.name);

        Ok(())
    }

    /// Run the app container in the foreground.
    pub fn start(&self) -> Result<(), RunError> {
        if self.config.runtime.is_none() {
            return Err(RunError::new(
                format!("app '{}' is a static unit with no container", self.name),
                io::Error::other("nothing to start"),
            ));
        }

        let version = {
            let state = DeployState::load(self.ctx, self.name)
                .map_err(|e| RunError::new(format!("load state for '{}'", self.name), e))?;
            state.active_version.unwrap_or(state.last_version)
        };

        if version == 0 {
            return Err(RunError::new(
                format!("app '{}' has not been deployed", self.name),
                io::Error::other("no deployed version"),
            ));
        }

        let container = self.name.scoped_unit_name();
        let mut cmd = crate::podman::PodmanRun::new(&container)
            .map_err(|e| RunError::new(format!("prepare podman to run '{}'", self.name), e))?;

        let databases = self
            .config
            .database_deps(self.ctx)
            .map_err(|e| RunError::new("resolve database dependencies", e))?;

        for db in databases {
            crate::deploy::unit::db::wait_until_ready(self.ctx, &db, DB_WAIT_TIMEOUT)
                .map_err(|e| RunError::new(format!("wait for database '{db}'"), e))?;
        }

        for volume in &self.config.volumes {
            cmd.volume(&volume.source, &volume.path);
        }

        let image = format!("localhost/{}:{}", self.name, version);
        cmd.run_foreground(image, &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }
}

pub(crate) fn remove_version_artifacts(name: &UnitName, version: u32) {
    let podman_ctx = PodmanContext::new(name, version);
    podman_ctx.remove_exports();
    podman_ctx.remove();
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
        MainContext {
            base: base.to_path_buf(),
            master_key: None,
        }
        .write_test_unit(name, config);
    }

    #[test]
    fn dependent_domains_selects_only_referencing_domains() {
        let base = TempDir::new().unwrap();

        // Serves this app's static export - should be selected.
        write_unit(
            base.path(),
            "site",
            "type: domain\nserver: nginx\nhosts: [\"example.com\"]\nroutes:\n  - location: /\n    kind: serve_files\n    root: \"${web:export}\"\n",
        );
        // References a different app - should be skipped.
        write_unit(
            base.path(),
            "other-site",
            "type: domain\nserver: nginx\nhosts: [\"other.com\"]\nroutes:\n  - location: /\n    kind: reverse_proxy\n    target: \"${api:url}\"\n",
        );
        // A non-domain unit - must not match the domain predicate.
        write_unit(
            base.path(),
            "web-db",
            "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
        );

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nbuilds: []\nruntime:\n  port: 8080\n  cmd: ./run\n",
        )
        .unwrap();
        let unit_name = UnitName::new("web").unwrap();
        let app = AppUnit::new(&ctx, &unit_name, config);

        let domains = app.dependent_domains();
        let names: Vec<&str> = domains.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["site"]);
    }
}
