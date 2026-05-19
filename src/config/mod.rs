mod args;
mod error;
mod node;
mod resource_name;
mod secret_name;

pub use self::{
    args::*,
    error::*,
    node::*,
    resource_name::ResourceName,
    secret_name::SecretName,
};

pub trait ValidateConfig {
    fn validate_config(&self) -> Result<(), String> {
        Ok(())
    }
}
