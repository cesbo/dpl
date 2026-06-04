mod artifacts;
mod cron;
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
    process::Command,
    time::{
        Duration,
        Instant,
    },
};

use chrono::Utc;
use tempfile::TempDir;
use tracing::{
    debug,
    error,
    info,
};

pub use self::model::AppConfig;
use self::{
    artifacts::ArtifactsContext,
    cron::CronContext,
    podman::PodmanContext,
    systemd::SystemdContext,
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
    timers::{
        TimerError,
        TimerState,
        TimersState,
    },
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
        let _phase = log::phase("preparing");

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
            name: self.name,
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
                if let Some((stage, message)) = err.failure() {
                    state.set_failed(stage, message);
                }
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
                .map_err(|e| DeployError::step_build("extract app archive", e))?;
        }

        let _phase = log::phase("building app image");
        PodmanContext::new(self.name, version)
            .build(deploy_dir)
            .map_err(|e| DeployError::step_build("build app image", e))?;

        Ok(())
    }

    fn install_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        if !self.config.exports.is_empty() {
            let _phase = log::phase("exporting files");
            PodmanContext::new(self.name, version)
                .export(&self.config.exports)
                .map_err(|e| DeployError::step_install("export files", e))?;
        }

        // A static build-and-export unit has no runtime: nothing to install.
        // The build + export above is the whole deploy.
        let Some(runtime) = &self.config.runtime else {
            return Ok(());
        };

        let systemd_ctx = SystemdContext::new(self.name);

        {
            let _phase = log::phase("installing app service");
            systemd_ctx
                .install_app(deploy_dir)
                .map_err(|e| DeployError::step_install("install app service", e))?;
        }

        {
            let phase_name = "waiting for app".to_string();
            let _phase = log::phase(&phase_name);
            if let Err(err) = crate::podman::health::check(self.name, runtime.port) {
                error!("{err}");
                return Err(DeployError::step_startup(phase_name, err));
            }

            systemd_ctx
                .set_restart_value("always")
                .map_err(|e| DeployError::step_install("set restart policy to 'always'", e))?;
        }

        let _phase = log::phase("installing timers");
        CronContext::new(self.name)
            .install(deploy_dir)
            .map_err(|e| DeployError::step_install("install timers", e))?;

        Ok(())
    }

    fn uninstall_inner(&self, version: u32) {
        CronContext::new(self.name).uninstall();

        SystemdContext::new(self.name).uninstall_app();

        let podman_ctx = PodmanContext::new(self.name, version);
        podman_ctx.remove_exports();
        podman_ctx.remove();
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

        // `exec_run` only returns when the exec itself fails.
        let err = cmd.exec(format!("localhost/{}:{}", self.name, version));
        Err(RunError::new("exec podman run", err))
    }

    /// Stop and remove the app container.
    pub fn stop(&self) -> Result<(), RunError> {
        if self.config.runtime.is_none() {
            return Err(RunError::new(
                format!(
                    "app '{}' is a static export unit with no container",
                    self.name
                ),
                io::Error::other("nothing to stop"),
            ));
        }

        crate::podman::stop_and_remove(self.name)
            .map_err(|e| RunError::new(format!("stop container '{}'", self.name), e))
    }

    /// Run one of the unit's timers once.
    pub fn run_timer(&self, timers: &mut TimersState, timer_name: &str) -> Result<(), TimerError> {
        let timer = self
            .config
            .timers
            .iter()
            .find(|t| t.name == timer_name && !t.disabled)
            .ok_or_else(|| TimerError::Unknown(timer_name.to_string()))?;

        // Carry-forward fields (last success, consecutive failures) are derived
        // from the timer's prior record; capture it once before overwriting.
        let prev = timers.timers.get(timer_name).cloned();
        let started_at = Utc::now();

        if !crate::podman::is_running(self.name) {
            let error = format!("container for '{}' not running", self.name);
            info!("{error}; skipping timer '{timer_name}'");
            timers.set_timer_state(
                timer_name,
                TimerState::failed(prev.as_ref(), started_at, Duration::default(), error),
            );
            return Ok(());
        }

        let container = self.name.scoped_unit_name();
        let command = format!("timer--{}", timer.name);

        timers.set_timer_state(timer_name, TimerState::running(prev.as_ref(), started_at));

        let clock = Instant::now();
        let status = Command::new("podman")
            .args(["exec", &container, "/bin/sh", "/opt/dpl/run.sh", &command])
            .status();
        let elapsed = clock.elapsed();

        match status {
            Ok(status) if status.success() => {
                timers.set_timer_state(timer_name, TimerState::success(started_at, elapsed));
                Ok(())
            }
            Ok(status) => {
                let cause = format!("timer script exited with {status}");
                timers.set_timer_state(
                    timer_name,
                    TimerState::failed(prev.as_ref(), started_at, elapsed, &cause),
                );
                Err(TimerError::Run {
                    name: timer_name.to_string(),
                    source: io::Error::other(cause),
                })
            }
            Err(err) => {
                let err = crate::podman::podman_spawn_error(err);
                timers.set_timer_state(
                    timer_name,
                    TimerState::failed(prev.as_ref(), started_at, elapsed, err.to_string()),
                );
                Err(TimerError::Run {
                    name: timer_name.to_string(),
                    source: err,
                })
            }
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
