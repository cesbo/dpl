use std::fmt;

use miette::Diagnostic;
use thiserror::Error;

use crate::{
    config::ConfigError,
    secret::SecretError,
};

pub fn format_error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

#[derive(Debug, Error)]
pub enum RefError {
    #[error("unit '{name}' not found")]
    UnknownUnit { name: String },

    #[error("unit '{unit}' is not a {expected}")]
    WrongUnitType {
        unit: String,
        expected: &'static str,
    },

    #[error("unknown export '{key}'")]
    UnknownExport { key: String },

    #[error(transparent)]
    LoadConfig(Box<ConfigError>),

    #[error("{reason}")]
    Export { reason: String },

    #[error(transparent)]
    Secret(#[from] SecretError),

    #[error("at {location}")]
    At {
        location: Location,
        #[source]
        inner: Box<RefError>,
    },
}

impl RefError {
    pub fn at(self, location: Location) -> Self {
        RefError::At {
            location,
            inner: Box::new(self),
        }
    }
}

impl Diagnostic for RefError {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            RefError::LoadConfig(inner) => inner.code(),
            _ => None,
        }
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            RefError::LoadConfig(inner) => inner.help(),
            _ => None,
        }
    }

    fn url<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        match self {
            RefError::LoadConfig(inner) => inner.url(),
            _ => None,
        }
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        match self {
            RefError::LoadConfig(inner) => inner.source_code(),
            // Forward the source-code carried by inner errors so miette can
            // resolve spans coming from the innermost NodeError against the
            // right NamedSource (the referenced unit's config file). Without
            // this, the chain visits causes with parent_src=None and snippets
            // never render past the top frame.
            RefError::At { inner, .. } => inner.source_code(),
            _ => None,
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        match self {
            RefError::LoadConfig(inner) => inner.labels(),
            _ => None,
        }
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        match self {
            // Fully transparent: forward into ConfigError's chain so we don't
            // re-emit ConfigError's message at this level (RefError::LoadConfig
            // already shares its Display via #[error(transparent)]).
            RefError::LoadConfig(inner) => inner.diagnostic_source(),
            RefError::At { inner, .. } => Some(inner.as_ref() as &dyn Diagnostic),
            RefError::UnknownUnit { .. }
            | RefError::WrongUnitType { .. }
            | RefError::UnknownExport { .. }
            | RefError::Export { .. }
            | RefError::Secret(_) => None,
        }
    }
}

impl From<ConfigError> for RefError {
    fn from(err: ConfigError) -> Self {
        match err {
            ConfigError::NotFound { name } => RefError::UnknownUnit { name },
            other => RefError::LoadConfig(Box::new(other)),
        }
    }
}

#[derive(Debug)]
pub enum Location {
    Unit { name: String },
    Field { path: String },
    Token { raw: String },
}

impl Location {
    pub fn unit(name: impl Into<String>) -> Self {
        Self::Unit { name: name.into() }
    }

    pub fn field(path: impl Into<String>) -> Self {
        Self::Field { path: path.into() }
    }

    pub fn token(raw: impl Into<String>) -> Self {
        Self::Token { raw: raw.into() }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit { name } => write!(f, "unit '{name}'"),
            Self::Field { path } => write!(f, "field '{path}'"),
            Self::Token { raw } => write!(f, "token '{raw}'"),
        }
    }
}
