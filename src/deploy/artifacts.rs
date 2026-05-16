use std::io;

use minijinja::Environment;
use serde::Serialize;
use thiserror::Error;

pub fn render_template<S>(env: &Environment, name: &str, ctx: S) -> Result<String, ArtifactError>
where
    S: Serialize,
{
    env.get_template(name)?
        .render(ctx)
        .map_err(ArtifactError::Render)
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("create artifacts directory")]
    CreateDir(#[source] io::Error),

    #[error("render template")]
    Render(#[from] minijinja::Error),

    #[error("write artifact")]
    Write(#[source] io::Error),

    #[error("resolve dpl binary path")]
    CurrentExe(#[source] io::Error),

    #[error("resolve env")]
    Env(#[from] crate::error::RefError),

    #[error("resolve template data")]
    Resolve(#[source] Box<dyn std::error::Error + Send + Sync>),
}
