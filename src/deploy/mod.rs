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
    io::{
        AsyncRead,
        AsyncReadExt,
        AsyncSeekExt,
    },
};

#[derive(Default)]
pub struct DeployService {
    locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DeployService {
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

    pub async fn build_log(&self, name: &str, offset: u64) -> Result<(Vec<u8>, u64), DeployError> {
        let entity_dir = crate::config().base.join(name);

        let config_path = entity_dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::EntityNotFound);
        }

        let state = DeployState::load(&entity_dir)?;

        let log_path = entity_dir
            .join(format!("deploy_{}", state.version))
            .join("log")
            .join("build.log");

        let mut file =
            fs::File::open(&log_path)
                .await
                .map_err(|source| DeployError::EntityError {
                    info: "read build log".into(),
                    source,
                })?;

        let metadata = file
            .metadata()
            .await
            .map_err(|source| DeployError::EntityError {
                info: "read build log metadata".into(),
                source,
            })?;
        let total_size = metadata.len();

        let mut buf = Vec::new();

        if offset >= total_size {
            return Ok((buf, total_size));
        }

        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|source| DeployError::EntityError {
                    info: "seek build log".to_string(),
                    source,
                })?;
        }

        file.read_to_end(&mut buf)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "read build log".to_string(),
                source,
            })?;

        Ok((buf, total_size))
    }
}
