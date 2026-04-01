#![allow(dead_code)]

pub mod app_entity;
mod config;
mod entity;
mod version;

use std::path::PathBuf;

pub use entity::EntityType;

pub struct DeployEntity {
    pub dir: PathBuf,
}

pub(super) use config::load_entity_config;
pub(super) use version::{
    get_entity_version,
    write_entity_version,
};

impl DeployEntity {}
