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
};
use tracing::{
    error,
    info,
};

use self::artifacts::ArtifactsContext;
pub use self::model::AppConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::{
            DeployState,
            DeployStatus,
        },
    },
    error::format_error_chain,
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

    pub async fn deploy<R>(self, archive: R) -> Result<DeployState, DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut state = DeployState::load(&self.unit_dir)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployError::UnitBusy);
        }

        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        info!(unit = %self.name, version = %version, "deploy started");

        if let Err(err) = self.prepare(version, archive).await {
            let chain = format_error_chain(&err);
            error!(unit = %self.name, error = %chain, "prepare app deploy");
            state.set_error(format!("prepare app deploy failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        let result = state.clone();

        tokio::task::spawn_blocking(move || {
            let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
            let log_path = deploy_dir.join("log").join("build.log");
            let subscriber = crate::log::init_tracing_log(&log_path).unwrap();
            tracing::subscriber::with_default(subscriber, || {
                self.deploy_worker(&deploy_dir, state);
            });
        });

        Ok(result)
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

    fn deploy_worker(&self, deploy_dir: &Path, mut state: DeployState) {
        let version = state.latest_build.version;

        if let Err(err) = self.build_inner(deploy_dir, version) {
            let chain = format_error_chain(&err);
            error!(%version, error = %chain, "failed to build app image");
            state.set_error(format!("failed to build app image: {chain}"));
            let _ = state.save(&self.unit_dir);
            return;
        }

        info!(%version, "app image build completed");

        // Uninstall active version
        if let Some(active_version) = state.active_version {
            info!(version = %active_version, "uninstalling active version");
            self.uninstall_inner(active_version);
        }

        // Install new version
        if let Err(err) = self.install_inner(version) {
            let chain = format_error_chain(&err);
            error!(%version, error = %chain, "failed to install app");
            self.uninstall_inner(version);
            state.active_version = None;
            state.set_error(format!("failed to install app: {chain}"));
            let _ = state.save(&self.unit_dir);
            return;
        }

        state.active_version = Some(version);
        state.set_ready();
        let _ = state.save(&self.unit_dir);

        info!(%version, "app deploy completed");
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        info!("build started for {}", self.name);

        let archive_path = deploy_dir.join("app.tar.gz");
        let app_dir = deploy_dir.join("app");

        crate::archive::extract(&archive_path, &app_dir)?;
        info!("archive extracted");

        let ctx = PodmanContext::new(&self.name, version);

        ctx.build(deploy_dir)
            .map_err(|source| DeployError::UnitError {
                info: "failed to build image".to_string(),
                source,
            })?;

        ctx.export(deploy_dir, &self.config.exports)
            .map_err(|source| DeployError::UnitError {
                info: "failed to export static files".to_string(),
                source,
            })?;

        Ok(())
    }

    fn install_inner(&self, version: u32) -> io::Result<()> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));

        let systemd_ctx = SystemdContext::new(&self.name);
        systemd_ctx.install_app(&deploy_dir)?;

        info!(%version, "checking app health");
        health::check(&self.name, self.config.port)?;
        systemd_ctx.set_restart_value("always")?;

        systemd_ctx.install_timers(&deploy_dir);

        Ok(())
    }

    fn uninstall_inner(&self, version: u32) {
        let systemd_ctx = SystemdContext::new(&self.name);
        systemd_ctx.uninstall_timers();
        systemd_ctx.uninstall_app();

        let podman_ctx = PodmanContext::new(&self.name, version);
        podman_ctx.remove();
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
