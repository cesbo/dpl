use std::{
    io,
    path::PathBuf,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("invalid configuration {path}: {info}")]
    Invalid { path: PathBuf, info: String },
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("create artifacts directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("render template {name}: {source}")]
    Render {
        name: String,
        #[source]
        source: minijinja::Error,
    },
    #[error("write artifact {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
