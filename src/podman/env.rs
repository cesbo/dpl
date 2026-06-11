//! Encrypted runtime env storage: one `.env-v{version}.json` per deployed
//! version, written at deploy and decrypted by `dpl start`.

use std::{
    collections::BTreeMap,
    fs,
    io,
};

use chrono::Utc;
use thiserror::Error;

use crate::{
    MainContext,
    config::UnitName,
    secret::{
        SecretError,
        SecretFile,
        SecretMetadata,
    },
};

#[derive(Debug, Error)]
pub enum RuntimeEnvError {
    #[error(transparent)]
    Secret(#[from] SecretError),

    #[error("read runtime env file")]
    Read(#[source] io::Error),

    #[error("write runtime env file")]
    Write(#[source] io::Error),

    #[error("serialize runtime env")]
    Serialize(#[source] serde_json::Error),

    #[error("deserialize runtime env file")]
    Deserialize(#[source] serde_json::Error),
}

/// Label used in encrypt/decrypt error messages.
fn env_label(unit: &UnitName, version: u32) -> String {
    format!("{unit} v{version} runtime env")
}

/// Encrypt the resolved env map and write `state/{unit}/.env-v{version}.json`.
/// Uses the secret file container format (AES-256-GCM via the master key).
pub fn save(
    ctx: &MainContext,
    unit: &UnitName,
    version: u32,
    env: &BTreeMap<String, String>,
) -> Result<(), RuntimeEnvError> {
    let master_key = ctx.master_key.as_ref().ok_or(SecretError::KeyNotFound)?;

    let plaintext = serde_json::to_string(env).map_err(RuntimeEnvError::Serialize)?;
    let metadata = SecretMetadata {
        created_at: Utc::now(),
    };
    let file = master_key.encrypt(&env_label(unit, version), &metadata, &plaintext)?;
    let json = serde_json::to_string_pretty(&file).map_err(RuntimeEnvError::Serialize)?;

    let path = ctx.runtime_env_path(unit, version);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(RuntimeEnvError::Write)?;
    }
    fs::write(&path, json).map_err(RuntimeEnvError::Write)?;

    Ok(())
}

/// Decrypt `state/{unit}/.env-v{version}.json` into an env map.
/// A missing file means the version was deployed without env: empty map.
pub fn load(
    ctx: &MainContext,
    unit: &UnitName,
    version: u32,
) -> Result<BTreeMap<String, String>, RuntimeEnvError> {
    let path = ctx.runtime_env_path(unit, version);
    let content = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(RuntimeEnvError::Read(err)),
    };

    let master_key = ctx.master_key.as_ref().ok_or(SecretError::KeyNotFound)?;

    let file: SecretFile = serde_json::from_str(&content).map_err(RuntimeEnvError::Deserialize)?;
    let plaintext = master_key.decrypt(&env_label(unit, version), &file)?;
    let env = serde_json::from_str(&plaintext).map_err(RuntimeEnvError::Deserialize)?;

    Ok(env)
}

/// Remove a version's env file. A missing file is a no-op.
pub fn remove(ctx: &MainContext, unit: &UnitName, version: u32) -> io::Result<()> {
    match fs::remove_file(ctx.runtime_env_path(unit, version)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::secret::MasterKey;

    fn test_ctx(base: &TempDir) -> MainContext {
        MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::generate(base.path())),
        }
    }

    fn sample_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "DATABASE_URL".to_string(),
                "postgres://u:p@h/db".to_string(),
            ),
            ("LOG_LEVEL".to_string(), "info".to_string()),
        ])
    }

    #[test]
    fn round_trip() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let unit = UnitName::new("web").unwrap();

        let env = sample_env();
        save(&ctx, &unit, 3, &env).unwrap();

        assert_eq!(load(&ctx, &unit, 3).unwrap(), env);
    }

    #[test]
    fn file_holds_no_plaintext() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let unit = UnitName::new("web").unwrap();

        save(&ctx, &unit, 1, &sample_env()).unwrap();

        let content = std::fs::read_to_string(ctx.runtime_env_path(&unit, 1)).unwrap();
        assert!(!content.contains("DATABASE_URL"));
        assert!(!content.contains("postgres://"));
    }

    #[test]
    fn missing_file_is_empty_env() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let unit = UnitName::new("web").unwrap();

        assert!(load(&ctx, &unit, 1).unwrap().is_empty());
    }

    #[test]
    fn wrong_key_fails() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let unit = UnitName::new("web").unwrap();

        save(&ctx, &unit, 1, &sample_env()).unwrap();

        let other = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::generate(base.path())),
        };
        assert!(load(&other, &unit, 1).is_err());
    }

    #[test]
    fn save_without_key_fails() {
        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let unit = UnitName::new("web").unwrap();

        assert!(matches!(
            save(&ctx, &unit, 1, &sample_env()),
            Err(RuntimeEnvError::Secret(SecretError::KeyNotFound))
        ));
    }

    #[test]
    fn remove_is_idempotent() {
        let base = TempDir::new().unwrap();
        let ctx = test_ctx(&base);
        let unit = UnitName::new("web").unwrap();

        save(&ctx, &unit, 2, &sample_env()).unwrap();
        remove(&ctx, &unit, 2).unwrap();
        assert!(load(&ctx, &unit, 2).unwrap().is_empty());
        // second remove: file already gone
        remove(&ctx, &unit, 2).unwrap();
    }
}
