#![allow(dead_code)]

pub mod app_entity;
mod entity;
mod version;

use std::path::PathBuf;

pub use entity::EntityType;

pub struct DeployEntity {
    pub dir: PathBuf,
}

impl DeployEntity {}
