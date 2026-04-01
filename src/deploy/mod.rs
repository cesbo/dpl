#![allow(dead_code)]

pub mod app_entity;
mod config;
mod entity;
mod status;
mod version;

use std::path::PathBuf;

pub use entity::EntityType;

pub struct DeployEntity {
    pub dir: PathBuf,
}

pub(self) use config::load_entity_config;
pub(self) use status::*;
pub(self) use version::*;

impl DeployEntity {}
