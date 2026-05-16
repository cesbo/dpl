use std::{
    fs,
    io,
    path::Path,
};

use kdl::{
    KdlNode,
    KdlValue,
};
use miette::SourceSpan;
use serde::{
    Serialize,
    de::DeserializeOwned,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FieldError {
    #[error("expected exactly one positional value")]
    EntryCount { span: SourceSpan },

    #[error("named entries are not allowed")]
    NamedEntry { span: SourceSpan },

    #[error("child blocks are not allowed")]
    HasChildren { span: SourceSpan },

    #[error("expected {expected}")]
    InvalidType {
        expected: &'static str,
        span: SourceSpan,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigNodeError {
    #[error("config node must not have arguments or properties")]
    WrapperHasArgs { span: SourceSpan },

    #[error("unknown field '{name}'")]
    UnknownField { name: String, span: SourceSpan },

    #[error("duplicate field '{name}'")]
    DuplicateField { name: String, span: SourceSpan },

    #[error("missing required field '{name}'")]
    MissingField {
        name: &'static str,
        span: SourceSpan,
    },

    #[error("invalid field '{name}': {source}")]
    InvalidField {
        name: String,
        span: SourceSpan,
        #[source]
        source: FieldError,
    },

    #[error("unknown value '{value}' for field '{field}'")]
    UnknownVariant {
        field: &'static str,
        value: String,
        span: SourceSpan,
    },
}

/// Parse a child node shaped like `name "value"` and return the borrowed
/// string. Rejects child blocks, named entries, arity ≠ 1, and non-string
/// values.
pub(crate) fn parse_string_child(node: &KdlNode) -> Result<&str, FieldError> {
    if node.children().is_some() {
        return Err(FieldError::HasChildren { span: node.span() });
    }

    let entries = node.entries();
    for entry in entries {
        if entry.name().is_some() {
            return Err(FieldError::NamedEntry { span: entry.span() });
        }
    }

    let [entry] = entries else {
        return Err(FieldError::EntryCount { span: node.span() });
    };

    match entry.value() {
        KdlValue::String(s) => Ok(s.as_str()),
        _ => Err(FieldError::InvalidType {
            expected: "string",
            span: entry.span(),
        }),
    }
}

/// Convert a single child node (e.g. `engine "postgresql"`) into a typed
/// field value. Implementors decide which `ConfigNodeError` shape is the
/// most precise — `FieldError`-wrapping shapes for scalar mismatches,
/// `UnknownVariant` for enums, etc.
pub(crate) trait FromConfigNode: Sized {
    fn from_config_node(node: &KdlNode, name: &'static str) -> Result<Self, ConfigNodeError>;
}

impl FromConfigNode for String {
    fn from_config_node(node: &KdlNode, name: &'static str) -> Result<Self, ConfigNodeError> {
        parse_string_child(node)
            .map(str::to_owned)
            .map_err(|source| ConfigNodeError::InvalidField {
                name: name.to_owned(),
                span: node.span(),
                source,
            })
    }
}

/// Assign `child` into `target`, dispatching to `T::from_config_node`.
/// Rejects re-assignment with `DuplicateField`.
pub(crate) fn set_field<T: FromConfigNode>(
    target: &mut Option<T>,
    child: &KdlNode,
    name: &'static str,
) -> Result<(), ConfigNodeError> {
    if target.is_some() {
        return Err(ConfigNodeError::DuplicateField {
            name: name.to_owned(),
            span: child.span(),
        });
    }

    *target = Some(T::from_config_node(child, name)?);
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("read config")]
    Read(#[source] io::Error),

    #[error("write config")]
    Write(#[source] io::Error),

    #[error("parse config")]
    Parse(#[source] serde_yaml::Error),

    #[error("serialize config")]
    Serialize(#[source] serde_yaml::Error),

    #[error("invalid config: {0}")]
    Invalid(String),
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read(err) if err.kind() == io::ErrorKind::NotFound)
    }
}

pub trait ValidateConfig {
    /// Config required by the default
    fn default_config() -> Option<Self>
    where
        Self: Sized,
    {
        None
    }

    fn validate_config(&self) -> Result<(), String> {
        Ok(())
    }
}

pub fn load_config<T>(path: impl AsRef<Path>) -> Result<T, ConfigError>
where
    T: DeserializeOwned + ValidateConfig,
{
    let content = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound
                && let Some(v) = T::default_config()
            {
                return Ok(v);
            }

            return Err(ConfigError::Read(err));
        }
    };

    let config: T = serde_yaml::from_str(&content).map_err(ConfigError::Parse)?;
    config.validate_config().map_err(ConfigError::Invalid)?;

    Ok(config)
}

pub fn save_config<T>(path: impl AsRef<Path>, config: &T) -> Result<(), ConfigError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(config).map_err(ConfigError::Serialize)?;
    fs::write(path, yaml).map_err(ConfigError::Write)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, Default)]
    #[serde(default)]
    struct TestConfig {
        value: i32,
    }

    impl ValidateConfig for TestConfig {
        fn default_config() -> Option<Self> {
            Some(Self { value: 42 })
        }

        fn validate_config(&self) -> Result<(), String> {
            if self.value < 0 {
                Err(format!("value must be non-negative, got {}", self.value))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn load_config_uses_default_when_file_missing() {
        let missing = Path::new("/nonexistent.yaml");
        let config: TestConfig = load_config(missing).expect("should fall back to default");
        assert_eq!(config.value, 42);
    }

    #[test]
    fn load_config_reports_invalid_when_validation_fails() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"value: -1\n").unwrap();

        if let Err(ConfigError::Invalid(info)) = load_config::<TestConfig>(file.path()) {
            assert!(info.contains("non-negative"), "unexpected info: {info}");
        } else {
            panic!("unexpected result");
        }
    }
}
