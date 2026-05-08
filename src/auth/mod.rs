mod error;
mod hash;
mod middleware;
pub mod model;

use std::path::Path;

pub use error::AuthServiceError;
pub use hash::bearer_hash;
pub use middleware::authorize_request;
use tracing::error;

use crate::{
    config::load_config,
    validate,
};

pub fn authorize(base: &Path, name: &str, token: &str, app: &str) -> Result<(), AuthServiceError> {
    if !validate::resource_name(name) {
        return Err(AuthServiceError::InvalidToken);
    }

    let path = base.join(".tokens").join(format!("{name}.yaml"));
    let entry: model::AuthEntry = match load_config(&path) {
        Ok(entry) => entry,
        Err(err) if err.is_not_found() => return Err(AuthServiceError::PermissionDenied),
        Err(err) => {
            error!("load auth entry '{name}': {}", err);
            return Err(AuthServiceError::ServiceError);
        }
    };

    if entry.hash != bearer_hash(name, token) {
        return Err(AuthServiceError::PermissionDenied);
    }

    let allow_app = entry
        .apps
        .iter()
        .any(|allowed| allowed == app || allowed == "*");

    if !allow_app {
        return Err(AuthServiceError::PermissionDenied);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        AuthServiceError,
        authorize,
        bearer_hash,
    };

    fn write_entry(base: &std::path::Path, name: &str, token: &str, apps: &str) {
        let dir = base.join(".tokens");
        fs::create_dir_all(&dir).unwrap();
        let hash = bearer_hash(name, token);
        let contents = format!("hash: {hash}\napps: {apps}\n");
        fs::write(dir.join(format!("{name}.yaml")), contents).unwrap();
    }

    #[test]
    fn authorize_ok_wildcard() {
        let tmp = tempdir().unwrap();
        write_entry(tmp.path(), "user-1", "t1", "[\"*\"]");
        assert!(authorize(tmp.path(), "user-1", "t1", "frontend").is_ok());
        assert!(authorize(tmp.path(), "user-1", "t1", "backend").is_ok());
    }

    #[test]
    fn authorize_ok_explicit_app() {
        let tmp = tempdir().unwrap();
        write_entry(tmp.path(), "user-1", "t1", "[frontend, backend]");
        assert!(authorize(tmp.path(), "user-1", "t1", "frontend").is_ok());
        assert!(authorize(tmp.path(), "user-1", "t1", "backend").is_ok());
    }

    #[test]
    fn authorize_app_not_listed() {
        let tmp = tempdir().unwrap();
        write_entry(tmp.path(), "user-1", "t1", "[frontend]");
        assert!(matches!(
            authorize(tmp.path(), "user-1", "t1", "backend"),
            Err(AuthServiceError::PermissionDenied)
        ));
    }

    #[test]
    fn authorize_wrong_token() {
        let tmp = tempdir().unwrap();
        write_entry(tmp.path(), "user-1", "t1", "[\"*\"]");
        assert!(matches!(
            authorize(tmp.path(), "user-1", "wrong", "frontend"),
            Err(AuthServiceError::PermissionDenied)
        ));
    }

    #[test]
    fn authorize_missing_file() {
        let tmp = tempdir().unwrap();
        assert!(matches!(
            authorize(tmp.path(), "ghost", "t", "frontend"),
            Err(AuthServiceError::PermissionDenied)
        ));
    }

    #[test]
    fn authorize_invalid_name() {
        let tmp = tempdir().unwrap();
        assert!(matches!(
            authorize(tmp.path(), "Bad_Name", "t", "frontend"),
            Err(AuthServiceError::InvalidToken)
        ));
        assert!(matches!(
            authorize(tmp.path(), "../etc", "t", "frontend"),
            Err(AuthServiceError::InvalidToken)
        ));
        assert!(matches!(
            authorize(tmp.path(), "", "t", "frontend"),
            Err(AuthServiceError::InvalidToken)
        ));
    }
}
