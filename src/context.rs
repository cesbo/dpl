use std::path::{
    Path,
    PathBuf,
};

use thiserror::Error;

use crate::secret::{
    MasterKey,
    SecretError,
};

#[derive(Debug, Error)]
pub enum ContextError {
    #[error(transparent)]
    Secret(#[from] SecretError),
}

#[derive(Debug)]
pub struct MainContext {
    pub base: PathBuf,
    pub master_key: Option<MasterKey>,
}

impl Default for MainContext {
    fn default() -> Self {
        MainContext {
            base: PathBuf::from("/opt/dpl"),
            master_key: None,
        }
    }
}

impl MainContext {
    pub fn load(base: &Path) -> Result<Self, ContextError> {
        let master_key = match MasterKey::load(base) {
            Ok(v) => Some(v),
            Err(SecretError::KeyNotFound) => None,
            Err(err) => return Err(err.into()),
        };
        Ok(MainContext {
            base: base.to_path_buf(),
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
