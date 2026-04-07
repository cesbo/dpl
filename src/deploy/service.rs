use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
    },
};

use serde::Serialize;
use tokio::{
    fs,
    io::AsyncRead,
    sync::Mutex as AsyncMutex,
};

use super::{
    DeployEntity,
    DeployError,
    DeployStatus,
    get_entity_version,
    read_deploy_status,
};

#[derive(Default)]
pub struct DeployService {
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl DeployService {
    pub async fn deploy<R>(&self, name: &str, archive: R) -> Result<u32, DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let lock = self.entity_lock(name);
        let _guard = lock.lock().await;

        let entity = DeployEntity::load(name).await?;
        match entity {
            DeployEntity::App(app) => {
                let version = app.prepare(archive).await?;
                app.deploy(version);
                Ok(version)
            }
        }
    }

    pub async fn status(&self, name: &str) -> Result<EntityStatus, DeployError> {
        let dir = crate::config::ENV.base_dir.join(name);

        let config_path = dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::EntityNotFound);
        }

        let status = read_deploy_status(&dir)
            .await
            .map_err(DeployError::StatusError)?;

        let version = get_entity_version(&dir)
            .await
            .map_err(DeployError::VersionError)?;

        Ok(EntityStatus { status, version })
    }

    fn entity_lock(&self, name: &str) -> Arc<AsyncMutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

#[derive(Debug, Serialize)]
pub struct EntityStatus {
    pub status: DeployStatus,
    pub version: u32,
}
