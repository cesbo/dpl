mod artifacts;
mod health;
mod model;
mod podman;
mod port;
mod systemd;

use std::{
    io,
    path::{
        Path,
        PathBuf,
    },
};

use podman::PodmanContext;
use systemd::SystemdContext;
use tokio::{
    fs,
    io::{
        AsyncRead,
        AsyncWriteExt,
    },
    task::JoinHandle,
};

use self::artifacts::ArtifactsContext;
pub use self::model::AppConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        guard::BusyGuard,
        state::{
            DeployState,
            DeployStatus,
        },
    },
    error::format_error_chain,
    log::DeployLog,
};

#[derive(Debug)]
pub struct AppUnit {
    pub ctx: MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: AppConfig,
}

impl AppUnit {
    pub fn new(ctx: MainContext, name: impl Into<String>, config: AppConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    /// Acquire the busy lock, bump version, persist state, then prepare the
    /// deploy workspace (extract archive, allocate port, render artifacts).
    /// Returns the new state (status `Building`) and the deploy directory path.
    pub async fn prepare_deploy<R>(
        &self,
        archive: R,
    ) -> Result<(DeployState, PathBuf), DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let _guard = BusyGuard::lock(&self.unit_dir)?;

        let mut state = DeployState::load(&self.unit_dir)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployError::UnitBusy);
        }

        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        if let Err(err) = self.prepare(version, archive).await {
            let chain = format_error_chain(&err);
            eprintln!("prepare failed: {chain}");
            state.set_error(format!("prepare app deploy failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
        Ok((state, deploy_dir))
    }

    /// Spawn the blocking worker that runs build → install → uninstall-old.
    /// Consumes `self`; the returned handle drives the deploy to completion
    /// and finalizes the log file.
    pub fn start_worker(
        self,
        deploy_dir: PathBuf,
        state: DeployState,
        log: DeployLog,
    ) -> JoinHandle<()> {
        tokio::task::spawn_blocking(move || {
            self.deploy_worker(&deploy_dir, state, &log);
        })
    }

    async fn prepare<R>(&self, version: u32, archive: R) -> Result<(), DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        let log_dir = deploy_dir.join("log");
        fs::create_dir(&log_dir)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "failed to create log directory".to_string(),
                source,
            })?;

        let build_log = log_dir.join("build.log");
        fs::File::create(&build_log)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "failed to create build.log".to_string(),
                source,
            })?;

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "failed to save archive".to_string(),
                source,
            })?;

        let port =
            port::get_port(&self.unit_dir)
                .await
                .map_err(|source| DeployError::UnitError {
                    info: "failed to get port".to_string(),
                    source,
                })?;

        let artifacts = ArtifactsContext {
            ctx: &self.ctx,
            name: &self.name,
            config: &self.config,
            version,
            port,
        };
        artifacts.save(&deploy_dir).await?;

        Ok(())
    }

    fn deploy_worker(&self, deploy_dir: &Path, mut state: DeployState, log: &DeployLog) {
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
            ctx.export(deploy_dir, &self.config.exports, log)
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
        podman_ctx.remove(log);
    }
}

async fn save_archive<R>(archive: R, dst: &Path) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send,
{
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst).await?;
    tokio::io::copy(&mut reader, &mut archive_file).await?;
    archive_file.flush().await?;
    Ok(())
}
