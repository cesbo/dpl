use std::{
    io,
    path::PathBuf,
};

use minijinja::Environment;
use serde::Serialize;
use thiserror::Error;

pub fn render<S>(env: &Environment, name: &str, ctx: S) -> Result<String, ArtifactError>
where
    S: Serialize,
{
    let template = env
        .get_template(name)
        .map_err(|source| ArtifactError::Render {
            name: name.into(),
            source,
        })?;

    template
        .render(ctx)
        .map_err(|source| ArtifactError::Render {
            name: name.into(),
            source,
        })
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
