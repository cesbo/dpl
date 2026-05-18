mod args;
mod error;
mod node;

pub use self::{
    args::*,
    error::*,
    node::*,
};

pub trait ValidateConfig {
    fn validate_config(&self) -> Result<(), String> {
        Ok(())
    }
}
