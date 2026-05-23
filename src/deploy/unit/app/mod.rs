mod artifacts;
mod health;
mod model;
mod podman;
mod port;
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

use self::artifacts::ArtifactsContext;
pub use self::model::AppConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::DeployState,
    },
    error::format_error_chain,
    log::DeployLog,
};

#[derive(Debug)]
pub struct AppUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: AppConfig,
}

impl<'a> AppUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: impl Into<String>, config: AppConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    /// Run the full deploy synchronously: bump version, extract archive,
    /// render artifacts, build image, install service. The caller holds the
    /// busy lock and provides freshly loaded state.
    pub fn deploy<R: Read>(
        self,
        mut state: DeployState,
        archive: R,
    ) -> Result<(DeployState, DeployLog), DeployError> {
        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        if let Err(err) = self.prepare(version, archive) {
            let chain = format_error_chain(&err);
            eprintln!("prepare failed: {chain}");
            state.set_error(format!("prepare app deploy failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
        let log_path = deploy_dir.join("log").join("build.log");

        let log = match DeployLog::open(&log_path, &self.name, version) {
            Ok(log) => log,
            Err(source) => {
                state.set_error(format!("open deploy log: {source}"));
                let _ = state.save(&self.unit_dir);
                return Err(DeployError::UnitError {
                    info: "open deploy log".to_string(),
                    source,
                });
            }
        };

        self.deploy_worker(&deploy_dir, &mut state, &log);
        Ok((state, log))
    }

    fn prepare<R: Read>(&self, version: u32, archive: R) -> Result<(), DeployError> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));

        fs::create_dir(&deploy_dir).map_err(|source| DeployError::UnitError {
            info: "failed to create deploy directory".to_string(),
            source,
        })?;

        let log_dir = deploy_dir.join("log");
        fs::create_dir(&log_dir).map_err(|source| DeployError::UnitError {
            info: "failed to create log directory".to_string(),
            source,
        })?;

        let build_log = log_dir.join("build.log");
        fs::File::create(&build_log).map_err(|source| DeployError::UnitError {
            info: "failed to create build.log".to_string(),
            source,
        })?;

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path).map_err(|source| DeployError::UnitError {
            info: "failed to save archive".to_string(),
            source,
        })?;

        let port = port::get_port(&self.unit_dir).map_err(|source| DeployError::UnitError {
            info: "failed to get port".to_string(),
            source,
        })?;

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
            version,
            port,
        };
        artifacts.save(&deploy_dir)?;

        Ok(())
    }

    fn deploy_worker(&self, deploy_dir: &Path, state: &mut DeployState, log: &DeployLog) {
        let version = state.latest_build.version;

        if let Err(err) = self.build_inner(deploy_dir, version, log) {
            let chain = format_error_chain(&err);
            log.error(&format!("failed to build app image: {chain}"));
            state.set_error(format!("failed to build app image: {chain}"));
            let _ = state.save(&self.unit_dir);
            log.finish_err(&chain);
            return;
        }

        if let Some(active_version) = state.active_version {
            log.phase(&format!("uninstalling v{active_version}"));
            self.uninstall_inner(active_version, log);
        }

        if let Err(err) = self.install_inner(version, log) {
            let chain = format_error_chain(&err);
            log.error(&format!("failed to install app: {chain}"));
            self.uninstall_inner(version, log);
            state.active_version = None;
            state.set_error(format!("failed to install app: {chain}"));
            let _ = state.save(&self.unit_dir);
            log.finish_err(&chain);
            return;
        }

        state.active_version = Some(version);
        state.set_ready();
        let _ = state.save(&self.unit_dir);

        log.finish_ok();
    }

    fn build_inner(
        &self,
        deploy_dir: &Path,
        version: u32,
        log: &DeployLog,
    ) -> Result<(), DeployError> {
        log.phase("extracting archive");

        let archive_path = deploy_dir.join("app.tar.gz");
        let app_dir = deploy_dir.join("app");

        crate::archive::extract(&archive_path, &app_dir)?;

        let ctx = PodmanContext::new(&self.name, version);

        log.phase("building image");
        ctx.build(deploy_dir, log)
            .map_err(|source| DeployError::UnitError {
                info: "failed to build image".to_string(),
                source,
            })?;

        if !self.config.exports.is_empty() {
            log.phase("exporting files");
            ctx.export(&self.config.exports, log)
                .map_err(|source| DeployError::UnitError {
                    info: "failed to export static files".to_string(),
                    source,
                })?;
        }

        Ok(())
    }

    fn install_inner(&self, version: u32, log: &DeployLog) -> io::Result<()> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));

        let systemd_ctx = SystemdContext::new(&self.name);

        log.phase("installing service");
        systemd_ctx.install_app(&deploy_dir, log)?;

        log.phase("health check");
        health::check(&self.name, self.config.port)?;
        systemd_ctx.set_restart_value("always")?;

        log.phase("installing timers");
        systemd_ctx.install_timers(&deploy_dir, log);

        Ok(())
    }

    fn uninstall_inner(&self, version: u32, log: &DeployLog) {
        let systemd_ctx = SystemdContext::new(&self.name);
        systemd_ctx.uninstall_timers(log);
        systemd_ctx.uninstall_app(log);

        let podman_ctx = PodmanContext::new(&self.name, version);
        podman_ctx.remove_exports(log);
        podman_ctx.remove(log);
    }
}

fn save_archive<R: Read>(archive: R, dst: &Path) -> io::Result<()> {
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst)?;
    io::copy(&mut reader, &mut archive_file)?;
    Ok(())
}
