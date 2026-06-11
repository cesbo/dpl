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

    /// `{base}/conf` - unit config files, one `{unit}.yaml` per unit.
    pub fn conf_dir(&self) -> PathBuf {
        self.base.join("conf")
    }

    /// `{base}/state` - per-unit state subdirs plus `serve.pid`.
    pub fn state_dir(&self) -> PathBuf {
        self.base.join("state")
    }

    /// `{base}/state/{unit}` - the unit's state, lock, and log files.
    /// Created on demand by DeployLockGuard or TimerLockGuard.
    pub fn unit_state_dir(&self, unit: &UnitName) -> PathBuf {
        self.state_dir().join(unit.as_str())
    }

    /// `{base}/state/{unit}/log` - the unit's log files.
    /// Created on demand by CriLog::open.
    pub fn unit_log_dir(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("log")
    }

    /// `{base}/state/serve.pid` - PID of the running `dpl serve`.
    /// The serve process holds an exclusive lock on this file.
    pub fn serve_pid_path(&self) -> PathBuf {
        self.state_dir().join("serve.pid")
    }

    /// `{base}/backup` - database dumps written before a destructive drop.
    pub fn backup_dir(&self) -> PathBuf {
        self.base.join("backup")
    }

    pub fn config_path(&self, unit: &UnitName) -> PathBuf {
        self.conf_dir().join(format!("{}.yaml", unit.as_str()))
    }

    pub fn deploy_lock_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("deploy.lock")
    }

    pub fn deploy_state_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("deploy.json")
    }

    pub fn timers_state_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("timers.json")
    }

    pub fn timers_lock_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("timers.lock")
    }

    /// `{base}/state/{unit}/log/build.log` - unit's last build.
    /// Captured podman output in CRI format.
    /// Build for app unit, restore for db unit.
    pub fn build_log_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_log_dir(unit).join("build.log")
    }

    /// `{base}/state/{unit}/log/timers.log` - unit's timers.
    /// Captured podman output in CRI format
    pub fn timers_log_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_log_dir(unit).join("timers.log")
    }

    /// `{base}/state/{unit}/log/runtime.log` - unit's runtime container.
    /// stdout/stderr captured by `dpl start` in CRI format.
    pub fn runtime_log_path(&self, unit: &UnitName) -> PathBuf {
        self.unit_log_dir(unit).join("runtime.log")
    }

    /// `{base}/state/{unit}/conf` - nginx conf.d source for an http-server,
    /// bind-mounted read-only into the container. Holds `00-dpl.conf` plus each
    /// dependent domain's `<domain>.conf`.
    pub fn http_conf_dir(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("conf")
    }

    /// `{base}/state/{unit}/www` - static-export root for an http-server,
    /// bind-mounted read-only into nginx at `/var/www`. Holds each served app's
    /// `<app>_<version>/...` tree, copied in at domain deploy.
    pub fn http_www_dir(&self, unit: &UnitName) -> PathBuf {
        self.unit_state_dir(unit).join("www")
    }

    /// `{base}/state/{unit}/.env-v{version}.json` - encrypted runtime env
    /// for a deployed version, written at deploy and read by `dpl start`.
    pub fn runtime_env_path(&self, unit: &UnitName, version: u32) -> PathBuf {
        self.unit_state_dir(unit)
            .join(format!(".env-v{version}.json"))
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
