use std::{
    env,
    error::Error,
    fmt,
    fs,
    io,
    path::PathBuf,
    sync::LazyLock,
};

use serde::Deserialize;

pub const DEFAULT_BASE_DIR: &str = "/opt/dpl";
pub const DEFAULT_SERVER_ADDR: &str = "0.0.0.0";
pub const DEFAULT_SERVER_PORT: u16 = 3000;

pub static ENV: LazyLock<EnvConfig> = LazyLock::new(|| {
    let base_dir = env::var_os("DPL_BASE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_BASE_DIR));
    EnvConfig { base_dir }
});

pub struct EnvConfig {
    pub base_dir: PathBuf,
}

#[derive(Default, Debug, Deserialize)]
pub struct MainConfig {
    pub server: ServerConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub addr: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: DEFAULT_SERVER_ADDR.to_string(),
            port: DEFAULT_SERVER_PORT,
        }
    }
}

impl MainConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = ENV.base_dir.join("config.yaml");

        match fs::read_to_string(&path) {
            Ok(contents) => serde_yaml::from_str(&contents)
                .map_err(|source| ConfigError::Parse { path, source }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(MainConfig::default()),
            Err(source) => Err(ConfigError::Read { path, source }),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read config {}: {}", path.display(), source)
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse config {}: {}", path.display(), source)
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}
