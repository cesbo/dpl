use std::{
    fs,
    io,
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
use rand::RngCore;
use thiserror::Error;

const KEY_NAME: &str = "master.key";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub struct MasterKey {
    secrets_dir: PathBuf,
    key: [u8; KEY_LEN],
}

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("invalid secret name")]
    InvalidName,

    #[error("invalid utf-8")]
    InvalidData,

    #[error("load master key")]
    LoadKey(#[source] io::Error),

    #[error("save master key")]
    SaveKey(#[source] io::Error),

    #[error("decrypt secret '{name}'")]
    Decrypt { name: String },

    #[error("encrypt secret '{name}'")]
    Encrypt { name: String },

    #[error("read secret")]
    ReadSecret(#[source] io::Error),

    #[error("write secret")]
    WriteSecret(#[source] io::Error),
}

pub fn get_secrets_dir(base: &Path) -> PathBuf {
    base.join(".secrets")
}

fn get_secret_path(secrets_dir: &Path, name: &str) -> PathBuf {
    let mut dir = secrets_dir.to_path_buf();
    let parts = name.split('/').collect::<Vec<&str>>();
    let (last, rest) = parts.split_last().unwrap();
    for item in rest {
        dir = dir.join(item);
    }
    dir.join(format!("{last}.bin"))
}

pub fn secret_exists(base: &Path, name: &str) -> bool {
    let secrets_dir = get_secrets_dir(base);
    get_secret_path(&secrets_dir, name)
        .try_exists()
        .unwrap_or(false)
}

pub fn secret_rm(base: &Path, name: &str) -> io::Result<()> {
    let secrets_dir = get_secrets_dir(base);
    let path = get_secret_path(&secrets_dir, name);
    fs::remove_file(&path)
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
        let bytes = fs::read(&path).map_err(SecretError::LoadKey)?;
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
        use std::io::Write;

        fs::create_dir_all(&self.secrets_dir).map_err(SecretError::SaveKey)?;

        let path = self.secrets_dir.join(KEY_NAME);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(SecretError::SaveKey)?;

        file.write_all(&self.key).map_err(SecretError::SaveKey)?;
        file.sync_all().map_err(SecretError::SaveKey)?;

        Ok(())
    }

    pub fn encrypt_to_file(&self, name: &str, text: &str) -> Result<(), SecretError> {
        let path = get_secret_path(&self.secrets_dir, name);
        let blob = self.encrypt(name, text)?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(SecretError::WriteSecret)?;
        }
        fs::write(&path, blob).map_err(SecretError::WriteSecret)?;

        Ok(())
    }

    pub fn encrypt(&self, name: &str, text: &str) -> Result<Vec<u8>, SecretError> {
        let cipher = Aes256Gcm::new(&self.key.into());

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let payload = Payload {
            msg: text.as_bytes(),
            aad: name.as_bytes(),
        };

        let ciphertext = cipher
            .encrypt(nonce, payload)
            .map_err(|_| SecretError::Encrypt { name: name.into() })?;

        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ciphertext);
        Ok(blob)
    }

    pub fn decrypt_from_file(&self, name: &str) -> Result<String, SecretError> {
        let path = get_secret_path(&self.secrets_dir, name);
        let blob = std::fs::read(&path).map_err(SecretError::ReadSecret)?;
        self.decrypt(name, &blob)
    }

    pub fn decrypt(&self, name: &str, blob: &[u8]) -> Result<String, SecretError> {
        if blob.len() < NONCE_LEN + TAG_LEN {
            return Err(SecretError::Decrypt { name: name.into() });
        }

        let cipher = Aes256Gcm::new(&self.key.into());
        let (nonce_bytes, ciphertext) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);

        let payload = Payload {
            msg: ciphertext,
            aad: name.as_bytes(),
        };

        let blob = cipher
            .decrypt(nonce, payload)
            .map_err(|_| SecretError::Decrypt { name: name.into() })?;

        String::from_utf8(blob).map_err(|_| SecretError::InvalidData)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn round_trip() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let blob = key.encrypt("name", "value").unwrap();
        let plain = key.decrypt("name", &blob).unwrap();
        assert_eq!(&plain, "value");
    }

    #[test]
    fn aad_binding() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let blob = key.encrypt("alpha", "value").unwrap();
        assert!(key.decrypt("beta", &blob).is_err());
    }

    #[test]
    fn tamper_detected() {
        let key = MasterKey::generate(Path::new("/tmp"));
        let mut blob = key.encrypt("name", "value").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(key.decrypt("name", &blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = MasterKey::generate(Path::new("/tmp"));
        let key2 = MasterKey::generate(Path::new("/tmp"));
        let blob = key1.encrypt("name", "value").unwrap();
        assert!(key2.decrypt("name", &blob).is_err());
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
        assert!(
            matches!(err, SecretError::LoadKey(ref err) if err.kind() == std::io::ErrorKind::NotFound)
        );
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
