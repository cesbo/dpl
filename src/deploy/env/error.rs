use thiserror::Error;

use crate::secret::SecretError;

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("secret")]
    Secret(#[from] SecretError),

    #[error("secret '{name}' does not exist")]
    MissingSecret { name: String },
}
