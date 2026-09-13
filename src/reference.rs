use std::fmt;

use thiserror::Error;

use crate::{
    config::ConfigError,
    secret::SecretError,
};

/// A reference-resolution failure: a single [`ReferenceErrorKind`] plus the trail of
/// [`Location`] breadcrumbs that led to it.
#[derive(Debug)]
pub struct ReferenceError {
    pub kind: ReferenceErrorKind,
    /// Breadcrumbs leading to the failure, innermost-first (the order `.at(..)`
    /// pushes them). Rendered outermost-first by [`Display`](fmt::Display).
    pub trail: Vec<Location>,
}

#[derive(Debug, Error)]
pub enum ReferenceErrorKind {
    #[error("unit '{name}' not found")]
    UnknownUnit { name: String },

    #[error("unit '{unit}' is not a {expected}")]
    WrongUnitType {
        unit: String,
        expected: &'static str,
    },

    #[error("unknown export '{key}'")]
    UnknownExport { key: String },

    #[error("export '{key}' needs 'runtime.port', which is not set")]
    MissingPort { key: String },

    #[error("unit '{name}' has no active deployment")]
    NotDeployed { name: String },

    #[error("load config for unit '{name}'")]
    LoadConfig {
        name: String,
        #[source]
        source: ConfigError,
    },

    #[error(transparent)]
    Secret(#[from] SecretError),
}

impl ReferenceError {
    pub fn at(mut self, location: Location) -> Self {
        self.trail.push(location);
        self
    }

    pub fn unknown_export(key: impl Into<String>) -> Self {
        ReferenceErrorKind::UnknownExport { key: key.into() }.into()
    }

    pub fn missing_port(key: impl Into<String>) -> Self {
        ReferenceErrorKind::MissingPort { key: key.into() }.into()
    }

    pub fn not_deployed(name: impl Into<String>) -> Self {
        ReferenceErrorKind::NotDeployed { name: name.into() }.into()
    }

    pub fn wrong_unit_type(unit: impl Into<String>, expected: &'static str) -> Self {
        ReferenceErrorKind::WrongUnitType {
            unit: unit.into(),
            expected,
        }
        .into()
    }
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Invalid reference:")?;

        // Render the breadcrumb trail (outermost-first) as an arrow path
        if !self.trail.is_empty() {
            for location in self.trail.iter().rev() {
                if console::colors_enabled_stderr() {
                    write!(f, "{} ", console::style("→ ").red())?;
                }
                writeln!(f, "{location}")?;
            }
        }

        if console::colors_enabled_stderr() {
            write!(f, "{} ", console::style("✗ ").red())?;
        }
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for ReferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // The kind's own message is already embedded by `Display`; surface only
        // the source *behind* the kind (e.g. `LoadConfig`'s `ConfigError`).
        self.kind.source()
    }
}

impl From<ReferenceErrorKind> for ReferenceError {
    fn from(kind: ReferenceErrorKind) -> Self {
        ReferenceError {
            kind,
            trail: Vec::new(),
        }
    }
}

impl From<SecretError> for ReferenceError {
    fn from(err: SecretError) -> Self {
        ReferenceErrorKind::from(err).into()
    }
}

impl From<ConfigError> for ReferenceError {
    fn from(err: ConfigError) -> Self {
        let kind = match &err {
            ConfigError::NotFound { name } => {
                ReferenceErrorKind::UnknownUnit { name: name.clone() }
            }
            ConfigError::Read { name, .. } => ReferenceErrorKind::LoadConfig {
                name: name.clone(),
                source: err,
            },
            ConfigError::Parse { name, .. } => ReferenceErrorKind::LoadConfig {
                name: name.clone(),
                source: err,
            },
        };
        kind.into()
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
