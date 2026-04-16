mod model;

use std::path::{
    Path,
    PathBuf,
};

pub use model::DomainConfig;

use crate::deploy::{
    DeployError,
    state::DeployState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainEntity {
    pub name: String,
    pub entity_dir: PathBuf,
    pub config: DomainConfig,
}

impl DomainEntity {
    pub fn new(name: &str, entity_dir: &Path, config: &DomainConfig) -> Self {
        Self {
            name: name.into(),
            entity_dir: entity_dir.into(),
            config: config.clone(),
        }
    }

    pub async fn deploy(self) -> Result<DeployState, DeployError> {
        Err(DeployError::EntityNotAllowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn domain_entity_direct_deploy_is_not_allowed() {
        let entity = DomainEntity::new(
            "example-domain",
            Path::new("/tmp/example-domain"),
            &DomainConfig {
                proxy: Some(model::ProxyConfig::Cloudflare),
                https: Some(model::HttpsConfig::Proxy),
                custom_config: String::new(),
                routes: Vec::new(),
            },
        );

        let err = entity.deploy().await.unwrap_err();
        assert!(matches!(err, DeployError::EntityNotAllowed));
    }
}
