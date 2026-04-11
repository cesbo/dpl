mod artifacts;
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

use artifacts::ArtifactsContext;
pub use model::AppConfig;
use podman::PodmanContext;
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

use crate::deploy::{
    DeployError,
    state::{
        DeployState,
        DeployStatus,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub name: String,
    pub entity_dir: PathBuf,
    pub config: AppConfig,
}

impl AppEntity {
    pub fn new(name: &str, entity_dir: &Path, config: &AppConfig) -> Self {
        Self {
            name: name.into(),
            entity_dir: entity_dir.into(),
            config: config.clone(),
        }
    }

    pub async fn deploy<R>(self, archive: R) -> Result<DeployState, DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut state = DeployState::load(&self.entity_dir)?;
        if state.status == DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }

        let version = state.bump_version()?;

        info!(entity = %self.name, version = %version, "deploy started");

        state.status = DeployStatus::Building;
        state.last_error = None;
        state.save(&self.entity_dir)?;

        if let Err(err) = self.prepare(version, archive).await {
            error!(entity = %self.name, error = %err, "prepare app deploy");
            state.status = DeployStatus::Failed;
            state.last_error = Some(err.to_string());
            let _ = state.save(&self.entity_dir);
            return Err(err);
        }

        let result = state.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(err) = self.build_worker(version) {
                error!(entity = %self.name, error = %err, "build app image");
                state.status = DeployStatus::Failed;
                state.last_error = Some(err.to_string());
                let _ = state.save(&self.entity_dir);
                return;
            }

            // TODO: run

            state.status = DeployStatus::Ready;
            state.last_error = None;
            let _ = state.save(&self.entity_dir);

            info!(entity = %self.name, version = %version, "deploy completed");
        });

        Ok(result)
    }

    async fn prepare<R>(&self, version: u32, archive: R) -> Result<(), DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let deploy_dir = self.entity_dir.join(format!("deploy_{version}"));

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        let log_dir = deploy_dir.join("log");
        fs::create_dir(&log_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create log directory".to_string(),
                source,
            })?;

        let build_log = log_dir.join("build.log");
        fs::File::create(&build_log)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create build.log".to_string(),
                source,
            })?;

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to save archive".to_string(),
                source,
            })?;

        let port =
            port::get_port(&self.entity_dir)
                .await
                .map_err(|source| DeployError::EntityError {
                    info: "failed to get port".to_string(),
                    source,
                })?;

        let artifacts = ArtifactsContext {
            name: &self.name,
            config: &self.config,
            version,
            port,
        };
        artifacts.save(&deploy_dir).await?;

        Ok(())
    }

    fn build_worker(&self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.entity_dir.join(format!("deploy_{version}"));
        let log_path = deploy_dir.join("log").join("build.log");

        let subscriber = crate::log::init_tracing_log(&log_path).unwrap();
        tracing::subscriber::with_default(subscriber, || {
            match self.build_inner(&deploy_dir, version) {
                Ok(_) => {
                    info!("app image built successfully");
                    Ok(())
                }
                Err(err) => {
                    error!(error = %err, "app image build failed");
                    Err(err)
                }
            }
        })
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        info!("build started for {}", self.name);

        let archive_path = deploy_dir.join("app.tar.gz");
        let app_dir = deploy_dir.join("app");

        crate::archive::extract(&archive_path, &app_dir)?;
        info!("archive extracted");

        let ctx = PodmanContext::new(&self.name, deploy_dir, version);

        ctx.build().map_err(|source| DeployError::EntityError {
            info: "failed to build image".to_string(),
            source,
        })?;

        ctx.export(&self.config.exports)
            .map_err(|source| DeployError::EntityError {
                info: "failed to export static files".to_string(),
                source,
            })?;

        Ok(())
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
