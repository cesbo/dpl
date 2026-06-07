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

const CONF_DIR: &str = "conf";
const STATE_DIR: &str = "state";
const LOG_DIR: &str = "log";
const BACKUP_DIR: &str = "backup";

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

    /// `{base}/conf` - unit config files, one `{unit}.yaml` per unit.
    pub fn conf_dir(&self) -> PathBuf {
        self.base.join(CONF_DIR)
    }

    /// `{base}/state` - deploy/timer locks and state files.
    /// State dir will be created by DeployLockGuard or TimerLockGuard
    pub fn state_dir(&self) -> PathBuf {
        self.base.join(STATE_DIR)
    }

    /// `{base}/state/scheduler.pid` - PID of the running `dpl serve`.
    /// The daemon holds an exclusive lock on this file.
    pub fn scheduler_pid_path(&self) -> PathBuf {
        self.state_dir().join("scheduler.pid")
    }

    /// `{base}/log` - per-unit build logs.
    pub fn log_dir(&self) -> PathBuf {
        self.base.join(LOG_DIR)
    }

    /// `{base}/backup` - database dumps written before a destructive drop.
    pub fn backup_dir(&self) -> PathBuf {
        self.base.join(BACKUP_DIR)
    }

    pub fn config_path(&self, unit: &UnitName) -> PathBuf {
        self.conf_dir().join(format!("{}.yaml", unit.as_str()))
    }

    pub fn deploy_lock_path(&self, unit: &UnitName) -> PathBuf {
        self.state_dir()
            .join(format!("{}--deploy.lock", unit.as_str()))
    }

    pub fn deploy_state_path(&self, unit: &UnitName) -> PathBuf {
        self.state_dir()
            .join(format!("{}--deploy.json", unit.as_str()))
    }

    pub fn timers_state_path(&self, unit: &UnitName) -> PathBuf {
        self.state_dir()
            .join(format!("{}--timers.json", unit.as_str()))
    }

    pub fn timers_lock_path(&self, unit: &UnitName) -> PathBuf {
        self.state_dir()
            .join(format!("{}--timers.lock", unit.as_str()))
    }

    /// `{base}/log/{unit}.build.log` - unit's last build.
    /// Captured podman output in CRI format.
    /// Build for app unit, restore for db unit.
    pub fn build_log_path(&self, unit: &UnitName) -> PathBuf {
        self.log_dir().join(format!("{}.build.log", unit.as_str()))
    }

    /// `{base}/log/{unit}.timers.log` - unit's timers.
    /// Captured podman output in CRI format
    pub fn timers_log_path(&self, unit: &UnitName) -> PathBuf {
        self.log_dir().join(format!("{}.timers.log", unit.as_str()))
    }

    /// `{base}/log/{unit}.runtime.log` - unit's runtime container.
    /// stdout/stderr captured by `dpl start` in CRI format.
    pub fn runtime_log_path(&self, unit: &UnitName) -> PathBuf {
        self.log_dir()
            .join(format!("{}.runtime.log", unit.as_str()))
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

#[cfg(test)]
impl MainContext {
    /// Write a unit config to `conf/{name}.yaml`, creating parent dirs.
    /// Routes through `config_path` so tests can't drift from the real layout.
    pub(crate) fn write_test_unit(&self, name: &str, yaml: &str) {
        let path = self.config_path(&UnitName::new(name).unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, yaml).unwrap();
    }
}
