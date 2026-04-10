use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
        atomic::AtomicBool,
    },
};

use tokio::{
    fs,
    io::AsyncRead,
};

use super::{
    DeployEntity,
    DeployError,
    DeployState,
    guard::BusyGuard,
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
        let entity = DeployEntity::load(name, &entity_dir)?;

        let state = match entity {
            DeployEntity::App(app) => app.deploy(archive).await?,
        };

        Ok(state)
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
