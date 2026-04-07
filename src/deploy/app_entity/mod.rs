mod artifacts;
mod model;
mod port;

use std::{
    io,
    path::{
        Path,
        PathBuf,
    },
    sync::Mutex,
};

use artifacts::ArtifactsContext;
use model::AppConfig;
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

use crate::{
    deploy::{
        DeployError,
        DeployStatus,
        load_entity_config,
        write_deploy_status,
    },
    error::ConfigError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub dir: PathBuf,
    pub name: String,
    pub config: AppConfig,
}

impl AppEntity {
    fn get_deploy_dir(&self, version: u32) -> PathBuf {
        let deploy_dir_name = format!("deploy_{version}");
        self.dir.join(deploy_dir_name)
    }

    pub async fn load(name: &str, dir: PathBuf) -> Result<Self, ConfigError> {
        let name = name.to_owned();
        let config: AppConfig = load_entity_config(&dir).await?;

        Ok(AppEntity { dir, name, config })
    }

    pub async fn prepare<R>(&self, archive: R) -> Result<u32, DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let status = crate::deploy::read_deploy_status(&self.dir)
            .await
            .map_err(DeployError::StatusError)?;

        if status == crate::deploy::DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }

        let version = crate::deploy::reserve_entity_version(&self.dir)
            .await
            .map_err(DeployError::VersionError)?;

        let deploy_dir = self.get_deploy_dir(version);

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        write_deploy_status(&self.dir, DeployStatus::Building)
            .await
            .map_err(DeployError::StatusError)?;

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

        let port = port::get_port(&self.dir)
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

        Ok(version)
    }

    pub fn deploy(self, version: u32) {
        info!(entity = %self.name, version, "starting background deploy");

        tokio::task::spawn_blocking(move || {
            let deploy_dir = self.get_deploy_dir(version);
            let log_path = deploy_dir.join("log").join("build.log");

            let log_file = std::fs::OpenOptions::new()
                .append(true)
                .write(true)
                .open(&log_path)
                .unwrap();

            let subscriber = build_log_subscriber(log_file);
            tracing::subscriber::with_default(subscriber, || {
                do_deploy(&self.name, &deploy_dir, version)
            });
        });
    }
}

fn build_log_subscriber(file: std::fs::File) -> impl tracing::Subscriber {
    tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .finish()
}

fn do_deploy(name: &str, deploy_dir: &Path, _version: u32) {
    info!("build started for {name}");

    let archive_path = deploy_dir.join("app.tar.gz");
    let app_dir = deploy_dir.join("app");

    if let Err(err) = crate::archive::extract(&archive_path, &app_dir) {
        error!("failed to extract archive: {err}");
        return;
    }

    info!("archive extracted");
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
