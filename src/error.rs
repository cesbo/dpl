use std::fmt;

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
