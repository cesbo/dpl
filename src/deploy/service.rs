use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
    },
};

use tokio::sync::Mutex as AsyncMutex;

use super::{
    DeployEntity,
    DeployError,
};

#[derive(Default)]
pub struct DeployService {
    locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl DeployService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn prepare(&self, name: &str) -> Result<(), DeployError> {
        let lock = self.entity_lock(name);
        let _guard = lock.lock().await;

        let entity = DeployEntity::load(name).await?;
        match entity {
            DeployEntity::App(app) => {
                app.prepare().await?;
            }
        }

        Ok(())
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
