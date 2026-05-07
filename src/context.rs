use std::{
    io,
    path::Path,
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

#[derive(Default, Debug)]
pub struct MainContext {
    pub config: MainConfig,
    pub master_key: Option<MasterKey>,
}

impl MainContext {
    pub fn load(path: &Path) -> Result<Self, ContextError> {
        let config = MainConfig::load(path)?;
        let master_key = match MasterKey::load(&config.base) {
            Ok(v) => Some(v),
            Err(SecretError::LoadKey(err)) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        Ok(MainContext { config, master_key })
    }

    pub fn base(&self) -> &Path {
        &self.config.base
    }

    pub fn resolve_secret(&self, name: &str) -> Result<String, SecretError> {
        let Some(master_key) = &self.master_key else {
            return Err(SecretError::KeyNotFound);
        };

        master_key.decrypt_from_file(name)
    }

    pub fn secret_exists(&self, name: &str) -> bool {
        crate::secret::secret_exists(&self.config.base, name)
    }
}
