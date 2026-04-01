use thiserror::Error;

use super::app_entity::AppEntityError;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("config error: {0}")]
    Config(#[from] crate::error::ConfigError),
    #[error("app entity error: {0}")]
    AppEntity(#[from] AppEntityError),
}
