#![allow(dead_code)]

mod artifacts;
mod env;
mod error;
mod guard;
mod handlers;
mod state;
mod unit;

use std::{
    collections::HashMap,
    io,
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        Mutex,
        atomic::AtomicBool,
    },
};

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
use unit::app::AppUnit;

use crate::MainContext;

pub struct DeployService {
    base: PathBuf,
    locks: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DeployService {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        DeployService {
            base: base.into(),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

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
        let ctx = MainContext::load(&self.base)?;
        let _guard = self.unit_lock(name)?;

        let unit = UnitConfig::load(&ctx, name)?;

        match unit {
            UnitConfig::App(config) => {
                if let Err(err) = config.validate_references(&ctx) {
                    return Err(DeployError::UnitError {
                        info: "app references".into(),
                        source: io::Error::other(err),
                    });
                }
                AppUnit::new(ctx, name, config).deploy(archive).await
            }
            UnitConfig::Domain(_) => Err(DeployError::UnitNotAllowed),
        }
    }

    pub async fn state(&self, name: &str) -> Result<DeployState, DeployError> {
        let ctx = MainContext::load(&self.base)?;
        let unit_dir = ctx.base().join(name);

        let config_path = unit_dir.join("config.yaml");
        if fs::metadata(&config_path).await.is_err() {
            return Err(DeployError::UnitNotFound);
        }

        let state = DeployState::load(&unit_dir)?;
        Ok(state)
    }

    pub async fn build_log(&self, name: &str, offset: u64) -> Result<(Vec<u8>, u64), DeployError> {
        let ctx = MainContext::load(&self.base)?;
        let unit_dir = ctx.base().join(name);

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
