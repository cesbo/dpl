use std::{
    io,
    path::PathBuf,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("template render error: {0}")]
    Render(#[from] minijinja::Error),
    #[error("failed to write artifact {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
