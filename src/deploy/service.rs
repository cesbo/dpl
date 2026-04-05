use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
    },
};

use tokio::{
    io::AsyncRead,
    sync::Mutex as AsyncMutex,
};

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
                // TODO: call AppEntity::deploy() to run build + restart in background
                Ok(version)
            }
        }
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
