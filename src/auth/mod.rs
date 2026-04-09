mod error;
mod middleware;
mod model;

use std::{
    fs,
    sync::LazyLock,
};

pub use error::AuthServiceError;
pub use middleware::authorize_request;
use model::AuthConfig;
use tracing::error;

use crate::error::ConfigError;

pub static SERVICE: LazyLock<AuthService> = LazyLock::new(|| AuthService::load());

pub struct AuthService {
    config: Option<AuthConfig>,
}

impl AuthService {
    fn load() -> Self {
        let config = match load_config() {
            Ok(config) => Some(config),
            Err(err) => {
                error!("load auth config: {}", err);
                None
            }
        };

        AuthService { config }
    }

    pub fn authorize(&self, token: &str, app: &str) -> Result<(), AuthServiceError> {
        let config = self.config.as_ref().ok_or(AuthServiceError::ServiceError)?;

        let key = config
            .keys
            .iter()
            .find(|key| key.token == token)
            .ok_or(AuthServiceError::PermissionDenied)?;

        if key.disabled {
            return Err(AuthServiceError::PermissionDenied);
        }

        if !key.apps.iter().any(|allowed_app| allowed_app == app) {
            return Err(AuthServiceError::PermissionDenied);
        }

        Ok(())
    }
}

fn load_config() -> Result<AuthConfig, ConfigError> {
    let path = crate::config::ENV.base_dir.join("auth.yaml");

    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(source) => {
            return Err(ConfigError::Read { path, source });
        }
    };

    let config: AuthConfig = match serde_yaml::from_str(&content) {
        Ok(config) => config,
        Err(source) => {
            return Err(ConfigError::Parse { path, source });
        }
    };

    if let Err(info) = config.validate() {
        return Err(ConfigError::Invalid { path, info });
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{
        AuthService,
        AuthServiceError,
        model::{
            AuthConfig,
            AuthKey,
        },
    };

    #[test]
    fn rejects_duplicate_key_ids() {
        let config = AuthConfig {
            keys: vec![
                AuthKey {
                    name: "user-1".to_string(),
                    token: "token-1".to_string(),
                    apps: Vec::new(),
                    disabled: false,
                },
                AuthKey {
                    name: "user-1".to_string(),
                    token: "token-2".to_string(),
                    apps: Vec::new(),
                    disabled: false,
                },
            ],
        };

        let err = config.validate().unwrap_err();
        assert_eq!(err, "duplicate key name: user-1");
    }

    #[test]
    fn authorize_with_no_config() {
        let service = AuthService { config: None };
        assert!(matches!(
            service.authorize("token-1", "frontend"),
            Err(AuthServiceError::ServiceError)
        ));
    }

    #[test]
    fn authorize_all() {
        let config = AuthConfig {
            keys: vec![
                AuthKey {
                    name: "user-1".to_string(),
                    token: "token-1".to_string(),
                    apps: vec!["frontend".to_string(), "backend".to_string()],
                    disabled: false,
                },
                AuthKey {
                    name: "user-2".to_string(),
                    token: "token-2".to_string(),
                    apps: vec!["frontend".to_string()],
                    disabled: false,
                },
            ],
        };

        let service = AuthService {
            config: Some(config),
        };
        assert!(service.authorize("token-1", "frontend").is_ok());
        assert!(service.authorize("token-1", "backend").is_ok());
        assert!(matches!(
            service.authorize("token-2", "backend"),
            Err(AuthServiceError::PermissionDenied)
        ));
    }
}
