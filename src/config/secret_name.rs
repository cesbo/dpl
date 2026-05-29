use std::{
    fmt,
    path::{
        Path,
        PathBuf,
    },
};

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};
use thiserror::Error;

use crate::{
    MainContext,
    config::ResourceName,
};

const SECRETS_DIR: &str = ".secrets";
const SECRET_FILE_EXT: &str = "json";

#[derive(Error, Debug)]
#[error("invalid secret name '{0}'")]
pub struct SecretNameError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

impl SecretName {
    pub fn new(value: impl Into<String>) -> Result<Self, SecretNameError> {
        let name = value.into();
        if !Self::is_valid(&name) {
            Err(SecretNameError(name))
        } else {
            Ok(Self(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(name: &str) -> bool {
        !name.is_empty() && name.split('/').all(ResourceName::is_valid)
    }

    /// `{base}/.secrets/{group_components}/{last}.json`
    pub fn file_path(&self, ctx: &MainContext) -> PathBuf {
        self.file_path_in(&ctx.base().join(SECRETS_DIR))
    }

    /// Build the secret file path relative to a pre-resolved secrets dir
    /// (e.g. `{base}/.secrets`). Internal helper for `MasterKey`, which
    /// stores its `secrets_dir` directly and has no full `MainContext`.
    pub(crate) fn file_path_in(&self, secrets_dir: &Path) -> PathBuf {
        let mut path = secrets_dir.to_path_buf();
        let mut segments = self.0.split('/');
        let last = segments.next_back().expect("validated non-empty");
        for segment in segments {
            path.push(segment);
        }
        path.push(format!("{last}.{SECRET_FILE_EXT}"));
        path
    }
}

impl std::str::FromStr for SecretName {
    type Err = SecretNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        SecretName::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for SecretName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_name_accepts_valid() {
        assert!(SecretName::new("foo").is_ok());
        assert!(SecretName::new("foo/bar").is_ok());
        assert!(SecretName::new("a-b-c").is_ok());
        assert!(SecretName::new("db/prod-password").is_ok());
        assert!(SecretName::new("a/b/c").is_ok());
    }

    #[test]
    fn secret_name_rejects_invalid() {
        assert!(SecretName::new("").is_err());
        assert!(SecretName::new("/foo").is_err());
        assert!(SecretName::new("foo/").is_err());
        assert!(SecretName::new("foo//bar").is_err());
        assert!(SecretName::new("foo/../bar").is_err());
        assert!(SecretName::new("Foo").is_err());
        assert!(SecretName::new("foo_bar").is_err());
        assert!(SecretName::new(" foo").is_err());
    }

    #[test]
    fn secret_name_serde_roundtrip() {
        let name = SecretName::new("db/prod-password").unwrap();
        let yaml = serde_yaml::to_string(&name).unwrap();
        assert_eq!(yaml.trim(), "db/prod-password");
        let parsed: SecretName = serde_yaml::from_str("db/prod-password").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn secret_name_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<SecretName>("Bad/Name").unwrap_err();
        assert!(err.to_string().contains("invalid secret name"));
    }

    #[test]
    fn file_path_in_flat() {
        let name = SecretName::new("foo").unwrap();
        assert_eq!(
            name.file_path_in(Path::new("/tmp/secrets")),
            PathBuf::from("/tmp/secrets/foo.json")
        );
    }

    #[test]
    fn file_path_in_nested() {
        let name = SecretName::new("a/b/c").unwrap();
        assert_eq!(
            name.file_path_in(Path::new("/tmp/secrets")),
            PathBuf::from("/tmp/secrets/a/b/c.json")
        );
    }
}
