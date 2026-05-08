use std::{
    io,
    path::{
        Path,
        PathBuf,
    },
};

use thiserror::Error;

use crate::{
    config::ConfigError,
    model::MainConfig,
    secret::{
        MasterKey,
        SecretError,
    },
};

#[derive(Debug, Error)]
pub enum ContextError {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Secret(#[from] SecretError),
}

#[derive(Debug)]
pub struct MainContext {
    pub base: PathBuf,
    pub config: MainConfig,
    pub master_key: Option<MasterKey>,
}

impl Default for MainContext {
    fn default() -> Self {
        MainContext {
            base: PathBuf::from("/opt/dpl"),
            config: MainConfig::default(),
            master_key: None,
        }
    }
}

impl MainContext {
    pub fn load(base: &Path) -> Result<Self, ContextError> {
        let config = MainConfig::load(&base.join("config.yaml"))?;
        let master_key = match MasterKey::load(base) {
            Ok(v) => Some(v),
            Err(SecretError::LoadKey(err)) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        Ok(MainContext {
            base: base.to_path_buf(),
            config,
            master_key,
        })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn resolve_secret(&self, name: &str) -> Result<String, SecretError> {
        let Some(master_key) = &self.master_key else {
            return Err(SecretError::KeyNotFound);
        };

        master_key.decrypt_from_file(name)
    }

    pub fn secret_exists(&self, name: &str) -> bool {
        crate::secret::secret_exists(&self.base, name)
    }
}
