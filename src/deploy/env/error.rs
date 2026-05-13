use thiserror::Error;

use crate::secret::SecretError;

#[derive(Debug, Error)]
pub enum EnvError {
    #[error(transparent)]
    Secret(#[from] SecretError),

    #[error("{token}: {reason}")]
    ResolveRef { token: String, reason: String },
}
