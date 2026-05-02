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

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_NAME: &str = "secrets.key";

#[derive(Debug, PartialEq, Eq)]
pub struct MasterKey([u8; KEY_LEN]);

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("invalid secret name")]
    InvalidName,

    #[error("invalid utf-8")]
    InvalidData,

    #[error("master key: {0}")]
    KeyIo(io::Error),

    #[error("master key is malformed")]
    KeyMalformed,

    #[error("decrypt secret '{name}'")]
    Decrypt { name: String },

    #[error("encrypt secret '{name}'")]
    Encrypt { name: String },

    #[error("read secret: {0}")]
    ReadSecret(io::Error),
}

pub fn get_secrets_dir(base: &Path) -> PathBuf {
    base.join("secrets")
}

pub fn get_secret_path(base: &Path, name: &str) -> PathBuf {
    let mut dir = get_secrets_dir(base);
    let parts = name.split('/').collect::<Vec<&str>>();
    let (last, rest) = parts.split_last().unwrap();
    for item in rest {
        dir = dir.join(item);
    }
    dir.join(format!("{last}.bin"))
}

pub fn generate_master_key() -> MasterKey {
    let mut key = [0u8; KEY_LEN];
    rand::thread_rng().fill_bytes(&mut key);
    MasterKey(key)
}

pub fn load_master_key(base: &Path) -> Result<MasterKey, SecretError> {
    let path = base.join(KEY_NAME);
    let bytes = fs::read(path).map_err(SecretError::KeyIo)?;
    if bytes.len() != KEY_LEN {
        return Err(SecretError::KeyMalformed);
    }

    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(MasterKey(key))
}

pub fn write_master_key(base: &Path, key: &MasterKey) -> Result<(), SecretError> {
    use std::io::Write;

    let path = base.join(KEY_NAME);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(SecretError::KeyIo)?;

    file.write_all(&key.0).map_err(SecretError::KeyIo)?;
    file.sync_all().map_err(SecretError::KeyIo)?;

    Ok(())
}

pub fn encrypt(name: &str, key: &MasterKey, plaintext: &[u8]) -> Result<Vec<u8>, SecretError> {
    let cipher = Aes256Gcm::new(&key.0.into());

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let payload = Payload {
        msg: plaintext,
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

pub fn decrypt_from_file(base: &Path, name: &str, key: &MasterKey) -> Result<String, SecretError> {
    let path = get_secret_path(base, name);
    let blob = std::fs::read(&path).map_err(SecretError::ReadSecret)?;
    decrypt(name, key, &blob)
}

pub fn decrypt(name: &str, key: &MasterKey, blob: &[u8]) -> Result<String, SecretError> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(SecretError::Decrypt { name: name.into() });
    }

    let cipher = Aes256Gcm::new(&key.0.into());
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

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn round_trip() {
        let key = generate_master_key();
        let blob = encrypt("name", &key, b"value").unwrap();
        let plain = decrypt("name", &key, &blob).unwrap();
        assert_eq!(&plain, "value");
    }

    #[test]
    fn aad_binding() {
        let key = generate_master_key();
        let blob = encrypt("alpha", &key, b"value").unwrap();
        assert!(decrypt("beta", &key, &blob).is_err());
    }

    #[test]
    fn tamper_detected() {
        let key = generate_master_key();
        let mut blob = encrypt("name", &key, b"value").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(decrypt("name", &key, &blob).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let key1 = generate_master_key();
        let key2 = generate_master_key();
        let blob = encrypt("name", &key1, b"value").unwrap();
        assert!(decrypt("name", &key2, &blob).is_err());
    }

    #[test]
    fn write_master_key_refuses_overwrite() {
        let tmp = tempdir().unwrap();
        let path = tmp.path();
        let key = generate_master_key();
        write_master_key(path, &key).unwrap();
        let err = write_master_key(path, &key).unwrap_err();
        assert!(
            matches!(err, SecretError::KeyIo(ref err) if err.kind() == io::ErrorKind::AlreadyExists)
        );
    }

    #[test]
    fn load_master_key_round_trip() {
        let tmp = tempdir().unwrap();
        let path = tmp.path();
        let key = generate_master_key();

        write_master_key(&path, &key).unwrap();
        let loaded = load_master_key(&path).unwrap();
        assert_eq!(loaded, key);
    }

    #[test]
    fn load_master_key_missing() {
        let tmp = tempdir().unwrap();
        let path = tmp.path();
        let err = load_master_key(path).unwrap_err();
        assert!(
            matches!(err, SecretError::KeyIo(ref err) if err.kind() == std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn load_master_key_malformed() {
        let tmp = tempdir().unwrap();
        let path = tmp.path();
        fs::write(&path.join("secrets.key"), b"too short").unwrap();
        let err = load_master_key(path).unwrap_err();
        assert!(matches!(err, SecretError::KeyMalformed));
    }

    #[test]
    fn truncated_blob_fails() {
        let key = generate_master_key();
        assert!(decrypt("name", &key, b"").is_err());
        assert!(decrypt("name", &key, &[0u8; 10]).is_err());
    }
}
