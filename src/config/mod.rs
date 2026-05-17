mod args;
mod error;
mod node;

use std::{
    fs,
    io,
    path::Path,
};

use kdl::{
    KdlNode,
    KdlValue,
};
use serde::{
    Serialize,
    de::DeserializeOwned,
};

pub use self::{
    error::*,
    node::*,
};
#[allow(unused_imports)] // call-site migration in the next PR will use these
pub(crate) use self::args::*;

/// Parse the single positional string argument of a wrapper node, e.g. the
/// `"run-tasks"` in `timer "run-tasks" { ... }`. Rejects zero or multiple
/// entries, named entries, and non-string values. Unlike
/// `parse_string_child`, this does not reject child blocks — wrapper nodes
/// typically carry their body as children.
pub(crate) fn parse_string_arg(node: &KdlNode) -> Result<&str, FieldError> {
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
