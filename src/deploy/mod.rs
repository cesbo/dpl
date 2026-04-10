#![allow(dead_code)]

mod app_entity;
mod entity;
mod error;
mod guard;
mod handlers;
mod state;

use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
        atomic::AtomicBool,
    },
};

use app_entity::AppEntity;
use entity::EntityConfig;
use error::DeployError;
use guard::BusyGuard;
pub use handlers::router as deploy_router;
use state::DeployState;
use tokio::{
    fs,
    io::AsyncRead,
};

#[derive(Default)]
pub struct DeployService {
    locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DeployService {
    pub async fn deploy<R>(&self, name: &str, archive: R) -> Result<DeployState, DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let _guard = self.entity_lock(name)?;

        let entity_dir = crate::config().base.join(name);
        let entity = EntityConfig::load(&entity_dir)?;

        match entity {
            EntityConfig::App(config) => {
                AppEntity::new(name, &entity_dir, &config)
                    .deploy(archive)
                    .await
            }
        }
    }

    pub async fn state(&self, name: &str) -> Result<DeployState, DeployError> {
        let dir = crate::config().base.join(name);

        let config_path = dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::EntityNotFound);
        }

        let state = DeployState::load(&dir)?;
        Ok(state)
    }

    fn entity_lock(&self, name: &str) -> Result<BusyGuard, DeployError> {
        let busy = self
            .locks
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();

        BusyGuard::lock(busy)
    }
}
