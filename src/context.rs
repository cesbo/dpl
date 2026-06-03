use std::path::{
    Path,
    PathBuf,
};

use thiserror::Error;

use crate::{
    config::{
        SecretName,
        UnitName,
    },
    secret::{
        MasterKey,
        SecretError,
    },
};

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("resolve base directory '{}'", path.display())]
    Base {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

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
        let base = std::path::absolute(base).map_err(|source| ContextError::Base {
            path: base.to_path_buf(),
            source,
        })?;

        let master_key = match MasterKey::load(&base) {
            Ok(v) => Some(v),
            Err(SecretError::KeyNotFound) => None,
            Err(err) => return Err(err.into()),
        };

        Ok(MainContext { base, master_key })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn unit_dir(&self, unit: &UnitName) -> PathBuf {
        self.base.join(unit.as_str())
    }

    pub fn config_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join("config.yaml")
    }

    pub fn lock_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join(".deploy.lock")
    }

    pub fn state_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join(".deploy.state")
    }

    pub fn timers_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join(".timers.state")
    }

    pub fn timers_lock_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join(".timers.lock")
    }

    pub fn build_log_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_dir(unit).join("build.log")
    }

    pub fn resolve_secret(&self, name: &SecretName) -> Result<String, SecretError> {
        let Some(master_key) = &self.master_key else {
            return Err(SecretError::KeyNotFound);
        };

        master_key.decrypt_from_file(name)
    }

    pub fn check_secret(&self, name: &SecretName) -> Result<(), SecretError> {
        crate::secret::check(&self.base, name)
    }
}
