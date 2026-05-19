mod args;
mod error;
mod node;
mod resource_name;

pub use self::{
    args::*,
    error::*,
    node::*,
    resource_name::ResourceName,
};

pub trait ValidateConfig {
    fn validate_config(&self) -> Result<(), String> {
        Ok(())
    }
}
