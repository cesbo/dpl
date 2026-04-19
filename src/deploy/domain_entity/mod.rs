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
        unimplemented!()
    }
}
