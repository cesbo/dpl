use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("{token}: {reason}")]
    ResolveRef { token: String, reason: String },
}
