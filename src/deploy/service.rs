use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
    },
};

use tokio::{
    fs,
    io::AsyncRead,
    sync::Mutex as AsyncMutex,
};
use tracing::{
    error,
    info,
};

use super::{
    DeployEntity,
    DeployError,
    DeployState,
    DeployStatus,
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

        let entity_dir = crate::config::ENV.base_dir.join(name);
        let entity = DeployEntity::load(name, &entity_dir).await?;

        let name = name.to_owned();

        let mut state = DeployState::load(&entity_dir)?;
        if state.status == DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }
        let version = state.bump_version()?;
        state.status = DeployStatus::Building;
        state.last_error = None;
        state.save(&entity_dir)?;

        info!(entity = %name, version = %version, "deploy started");

        match entity {
            DeployEntity::App(app) => {
                if let Err(err) = app.prepare(version, archive).await {
                    error!(entity = %name, error = %err, "prepare app deploy");
                    state.status = DeployStatus::Failed;
                    state.last_error = Some(err.to_string());
                    let _ = state.save(&entity_dir);
                    return Err(err);
                }

                tokio::task::spawn_blocking(move || {
                    if let Err(err) = app.build(version) {
                        error!(entity = %name, error = %err, "build app image");
                        state.status = DeployStatus::Failed;
                        state.last_error = Some(err.to_string());
                        let _ = state.save(&entity_dir);
                        return;
                    }

                    // TODO: run

                    info!(entity = %name, version = %version, "deploy completed");
                });
            }
        }

        Ok(version)
    }

    pub async fn state(&self, name: &str) -> Result<DeployState, DeployError> {
        let dir = crate::config::ENV.base_dir.join(name);

        let config_path = dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::EntityNotFound);
        }

        let state = DeployState::load(&dir)?;
        Ok(state)
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
