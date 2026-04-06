mod artifacts;
mod model;
mod port;

use std::{
    io,
    path::{
        Path,
        PathBuf,
    },
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

use crate::{
    deploy::{
        DeployError,
        load_entity_config,
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

        let deploy_dir_name = format!("deploy_{}", version);
        let deploy_dir = self.dir.join(deploy_dir_name);

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create deploy directory".to_string(),
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
