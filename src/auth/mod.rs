mod error;
mod middleware;
mod model;

use std::sync::LazyLock;

pub use error::AuthServiceError;
pub use middleware::authorize_request;
use model::AuthConfig;
use tracing::error;

use crate::config::load_config;

pub static SERVICE: LazyLock<AuthService> = LazyLock::new(AuthService::load);

pub struct AuthService {
    config: Option<AuthConfig>,
}

impl AuthService {
    fn load() -> Self {
        let path = crate::config().base.join("auth.yaml");
        let config = match load_config(&path) {
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

        let allow_app = key
            .apps
            .iter()
            .any(|allowed_app| allowed_app == app || allowed_app == "*");

        if !allow_app {
            return Err(AuthServiceError::PermissionDenied);
        }

        Ok(())
    }
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
    use crate::config::ValidateConfig;

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

        let err = config.validate_config().unwrap_err();
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
