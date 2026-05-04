#![allow(dead_code)]

mod app;
mod artifacts;
mod domain;
mod env;
mod error;
mod guard;
mod handlers;
mod state;
mod unit;

use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        Mutex,
        atomic::AtomicBool,
    },
};

use app::AppUnit;
use env::{
    EnvError,
    EnvList,
};
pub use error::DeployError;
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
pub use unit::UnitConfig;

#[derive(Default)]
pub struct DeployService {
    locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DeployService {
    fn unit_lock(&self, name: &str) -> Result<BusyGuard, DeployError> {
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
        let _guard = self.unit_lock(name)?;

        let base = crate::config().base.as_path();
        let unit = UnitConfig::load(base, name)?;

        match unit {
            UnitConfig::App(config) => {
                if let Err(err) = config.validate_references(base) {
                    return Err(DeployError::UnitError {
                        info: "app references".into(),
                        source: io::Error::other(err),
                    });
                }
                AppUnit::new(base, name, &config).deploy(archive).await
            }
            UnitConfig::Domain(_) => Err(DeployError::UnitNotAllowed),
        }
    }

    pub async fn state(&self, name: &str) -> Result<DeployState, DeployError> {
        let dir = crate::config().base.join(name);

        let config_path = dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::UnitNotFound);
        }

        let state = DeployState::load(&dir)?;
        Ok(state)
    }

    pub async fn build_log(&self, name: &str, offset: u64) -> Result<(Vec<u8>, u64), DeployError> {
        let unit_dir = crate::config().base.join(name);

        let config_path = unit_dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::UnitNotFound);
        }

        let state = DeployState::load(&unit_dir)?;

        let log_path = unit_dir
            .join(format!("deploy_{}", state.latest_build.version))
            .join("log")
            .join("build.log");

        let mut file =
            fs::File::open(&log_path)
                .await
                .map_err(|source| DeployError::UnitError {
                    info: "read build log".into(),
                    source,
                })?;

        let metadata = file
            .metadata()
            .await
            .map_err(|source| DeployError::UnitError {
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
                .map_err(|source| DeployError::UnitError {
                    info: "seek build log".to_string(),
                    source,
                })?;
        }

        file.read_to_end(&mut buf)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "read build log".to_string(),
                source,
            })?;

        Ok((buf, total_size))
    }
}
