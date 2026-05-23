use std::{
    fs::{
        self,
        OpenOptions,
        create_dir_all,
        remove_file,
    },
    io::{
        self,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
};

use aes_gcm::{
    Aes256Gcm,
    KeyInit,
    Nonce,
    aead::{
        Aead,
        Payload,
    },
};
use base64::{
    Engine as _,
    engine::general_purpose::STANDARD as B64,
};
use chrono::{
    DateTime,
    Utc,
};
use rand::RngCore;
use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

use crate::config::SecretName;

const KEY_NAME: &str = "master.key";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const FILE_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq)]
pub struct MasterKey {
    secrets_dir: PathBuf,
    key: [u8; KEY_LEN],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SecretFile {
    pub version: u32,
    pub metadata: SecretMetadata,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret '{name}' not found")]
    NotFound { name: String },

    #[error("secret '{name}' already exists")]
    AlreadyExists { name: String },

    #[error("secret '{name}' is not valid UTF-8")]
    InvalidData { name: String },

    #[error("master key not found")]
    KeyNotFound,

    #[error("load master key")]
    LoadKey(#[source] io::Error),

    #[error("save master key")]
    SaveKey(#[source] io::Error),

    #[error("decrypt secret '{name}'")]
    Decrypt { name: String },

    #[error("encrypt secret '{name}'")]
    Encrypt { name: String },

    #[error("read secret '{name}'")]
    ReadSecret {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("write secret '{name}'")]
    WriteSecret {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("serialize secret '{name}'")]
    Serialize {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("deserialize secret '{name}'")]
    Deserialize {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("unsupported secret file version {version} for '{name}'")]
    UnsupportedVersion { name: String, version: u32 },
}

pub fn get_secrets_dir(base: &Path) -> PathBuf {
    base.join(".secrets")
}

/// List all secret names under `base` (e.g. `foo`, `group/bar`), sorted.
/// Returns an empty vec when the secrets dir does not exist.
pub fn list_secrets(base: &Path) -> io::Result<Vec<SecretName>> {
    let secrets_dir = get_secrets_dir(base);
    let mut out = Vec::new();
    walk_secrets(&secrets_dir, &secrets_dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_secrets(root: &Path, dir: &Path, out: &mut Vec<SecretName>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            walk_secrets(root, &path, out)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };

        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };

        let mut display = PathBuf::new();
        if let Some(parent) = rel.parent() {
            display.push(parent);
        }
        display.push(stem);
        let Ok(name) = SecretName::new(display.to_string_lossy().into_owned()) else {
            continue;
        };
        out.push(name);
    }

    Ok(())
}

pub fn check(base: &Path, name: &SecretName) -> Result<(), SecretError> {
    let secrets_dir = get_secrets_dir(base);
    let path = name.file_path_in(&secrets_dir);
    match fs::metadata(path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(SecretError::NotFound {
            name: name.to_string(),
        }),
        Err(source) => Err(SecretError::ReadSecret {
            name: name.to_string(),
            source,
        }),
    }
}

pub fn remove(base: &Path, name: &SecretName) -> Result<(), SecretError> {
    let secrets_dir = get_secrets_dir(base);
    let path = name.file_path_in(&secrets_dir);
    match remove_file(&path) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(SecretError::NotFound {
            name: name.to_string(),
        }),
        Err(source) => Err(SecretError::WriteSecret {
            name: name.to_string(),
            source,
        }),
    }
}

fn metadata_aad(metadata: &SecretMetadata) -> Vec<u8> {
    format!("created_at={}", metadata.created_at.to_rfc3339()).into_bytes()
}

impl MasterKey {
    pub fn generate(base: &Path) -> Self {
        let mut key = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut key);
        Self {
            secrets_dir: get_secrets_dir(base),
            key,
        }
    }

    pub fn load(base: &Path) -> Result<Self, SecretError> {
        let secrets_dir = get_secrets_dir(base);
        let path = secrets_dir.join(KEY_NAME);
        let bytes = match fs::read(&path) {
            Ok(v) => v,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(SecretError::KeyNotFound);
            }
            Err(err) => return Err(SecretError::LoadKey(err)),
        };
        if bytes.len() != KEY_LEN {
            return Err(SecretError::LoadKey(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid key length",
            )));
        }

        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&bytes);
        Ok(Self { secrets_dir, key })
    }

    pub fn save(&self) -> Result<(), SecretError> {
        create_dir_all(&self.secrets_dir).map_err(SecretError::SaveKey)?;

        let path = self.secrets_dir.join(KEY_NAME);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(SecretError::SaveKey)?;

        file.write_all(&self.key).map_err(SecretError::SaveKey)?;
        file.sync_all().map_err(SecretError::SaveKey)?;

        Ok(())
    }

    pub fn encrypt_to_file(&self, name: &SecretName, text: &str) -> Result<(), SecretError> {
        let path = name.file_path_in(&self.secrets_dir);
        let metadata = SecretMetadata {
            created_at: Utc::now(),
        };
        let secret_file = self.encrypt(name.as_str(), &metadata, text)?;
        let json = serde_json::to_string_pretty(&secret_file).map_err(|source| {
            SecretError::Serialize {
                name: name.to_string(),
                source,
            }
        })?;

        if let Some(parent) = path.parent() {
            create_dir_all(parent).map_err(|source| SecretError::WriteSecret {
                name: name.to_string(),
                source,
            })?;
        }

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    SecretError::AlreadyExists {
                        name: name.to_string(),
                    }
                } else {
                    SecretError::WriteSecret {
                        name: name.to_string(),
                        source,
                    }
                }
            })?;

        file.write_all(json.as_bytes())
            .map_err(|source| SecretError::WriteSecret {
                name: name.to_string(),
                source,
            })?;

        file.sync_all().map_err(|source| SecretError::WriteSecret {
            name: name.to_string(),
            source,
        })?;

        Ok(())
    }

    fn encrypt(
        &self,
        name: &str,
        metadata: &SecretMetadata,
        text: &str,
    ) -> Result<SecretFile, SecretError> {
        let cipher = Aes256Gcm::new(&self.key.into());

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let aad = metadata_aad(metadata);

        let payload = Payload {
            msg: text.as_bytes(),
            aad: &aad,
        };

        let ciphertext = cipher
            .encrypt(nonce, payload)
            .map_err(|_| SecretError::Encrypt {
                name: name.to_string(),
            })?;

        Ok(SecretFile {
            version: FILE_VERSION,
            metadata: metadata.clone(),
            nonce: B64.encode(nonce_bytes),
            ciphertext: B64.encode(ciphertext),
        })
    }

    pub fn decrypt_from_file(&self, name: &SecretName) -> Result<String, SecretError> {
        let path = name.file_path_in(&self.secrets_dir);
        let content = fs::read_to_string(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                SecretError::NotFound {
                    name: name.to_string(),
                }
            } else {
                SecretError::ReadSecret {
                    name: name.to_string(),
                    source,
                }
            }
        })?;

        let file: SecretFile =
            serde_json::from_str(&content).map_err(|source| SecretError::Deserialize {
                name: name.to_string(),
                source,
            })?;

        self.decrypt(name.as_str(), &file)
    }

    fn decrypt(&self, name: &str, file: &SecretFile) -> Result<String, SecretError> {
        if file.version != FILE_VERSION {
            return Err(SecretError::UnsupportedVersion {
                name: name.to_string(),
                version: file.version,
            });
        }

        let nonce_bytes = B64.decode(&file.nonce).map_err(|_| SecretError::Decrypt {
            name: name.to_string(),
        })?;

        if nonce_bytes.len() != NONCE_LEN {
            return Err(SecretError::Decrypt {
                name: name.to_string(),
            });
        }

        let ciphertext = B64
            .decode(&file.ciphertext)
            .map_err(|_| SecretError::Decrypt {
                name: name.to_string(),
            })?;

        let cipher = Aes256Gcm::new(&self.key.into());
        let nonce = Nonce::from_slice(&nonce_bytes);

        let aad = metadata_aad(&file.metadata);

        let payload = Payload {
            msg: &ciphertext,
            aad: &aad,
        };

        let plain = cipher
            .decrypt(nonce, payload)
            .map_err(|_| SecretError::Decrypt { name: name.into() })?;

        String::from_utf8(plain).map_err(|_| SecretError::InvalidData {
            name: name.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn sample_metadata() -> SecretMetadata {
        SecretMetadata {
            created_at: DateTime::parse_from_rfc3339("2026-05-09T12:34:56Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn round_trip() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let metadata = sample_metadata();
        let file = key.encrypt("name", &metadata, "value").unwrap();
        let plain = key.decrypt("name", &file).unwrap();
        assert_eq!(&plain, "value");
        assert_eq!(file.version, 1);
    }

    #[test]
    fn aad_binding() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let metadata = sample_metadata();
        let mut file = key.encrypt("name", &metadata, "value").unwrap();
        file.metadata.created_at = DateTime::parse_from_rfc3339("2026-05-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(key.decrypt("name", &file).is_err());
    }

    #[test]
    fn tamper_detected() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let metadata = sample_metadata();
        let mut file = key.encrypt("name", &metadata, "value").unwrap();
        let mut bytes = B64.decode(&file.ciphertext).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        file.ciphertext = B64.encode(bytes);
        assert!(key.decrypt("name", &file).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = MasterKey::generate(Path::new("/tmp"));
        let key2 = MasterKey::generate(Path::new("/tmp"));
        let metadata = sample_metadata();
        let file = key1.encrypt("name", &metadata, "value").unwrap();
        assert!(key2.decrypt("name", &file).is_err());
    }

    #[test]
    fn unsupported_version_rejected() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let metadata = sample_metadata();
        let mut file = key.encrypt("name", &metadata, "value").unwrap();
        file.version = 2;
        assert!(matches!(
            key.decrypt("name", &file),
            Err(SecretError::UnsupportedVersion { version: 2, .. })
        ));
    }

    #[test]
    fn file_round_trip() {
        let tmp = tempdir().unwrap();
        let base = tmp.path();
        let key = MasterKey::generate(base);
        let name = SecretName::new("group/foo").unwrap();
        key.encrypt_to_file(&name, "secret-value").unwrap();

        let path = name.file_path_in(&get_secrets_dir(base));
        assert!(path.extension().is_some_and(|ext| ext == "json"));
        assert!(path.exists());

        let plain = key.decrypt_from_file(&name).unwrap();
        assert_eq!(plain, "secret-value");
    }

    #[test]
    fn file_json_format() {
        let tmp = tempdir().unwrap();
        let base = tmp.path();
        let key = MasterKey::generate(base);
        let name = SecretName::new("foo").unwrap();
        key.encrypt_to_file(&name, "v").unwrap();

        let path = name.file_path_in(&get_secrets_dir(base));
        let content = fs::read_to_string(&path).unwrap();
        let file: SecretFile = serde_json::from_str(&content).unwrap();
        assert_eq!(file.version, 1);
        assert!(!file.nonce.is_empty());
        assert!(!file.ciphertext.is_empty());
    }

    #[test]
    fn write_master_key_refuses_overwrite() {
        let tmp = tempdir().unwrap();
        let key = MasterKey::generate(tmp.path());
        key.save().unwrap();
        let err = key.save().unwrap_err();
        assert!(
            matches!(err, SecretError::SaveKey(ref err) if err.kind() == io::ErrorKind::AlreadyExists)
        );
    }

    #[test]
    fn load_master_key_round_trip() {
        let tmp = tempdir().unwrap();
        let base = tmp.path();
        let key = MasterKey::generate(base);

        key.save().unwrap();
        let loaded = MasterKey::load(base).unwrap();
        assert_eq!(loaded, key);
    }

    #[test]
    fn load_master_key_missing() {
        let tmp = tempdir().unwrap();
        let err = MasterKey::load(tmp.path()).unwrap_err();
        assert!(matches!(err, SecretError::KeyNotFound));
    }

    #[test]
    fn load_master_key_malformed() {
        let tmp = tempdir().unwrap();
        let base = tmp.path();

        let secrets_dir = get_secrets_dir(base);
        fs::create_dir_all(&secrets_dir).unwrap();

        let master_key_path = secrets_dir.join(KEY_NAME);
        fs::write(&master_key_path, b"too short").unwrap();

        let err = MasterKey::load(base).unwrap_err();
        assert!(
            matches!(err, SecretError::LoadKey(ref err) if err.kind() == io::ErrorKind::InvalidData)
        );
    }
}
