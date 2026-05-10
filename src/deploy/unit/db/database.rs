use super::model::{
    DbConfig,
    DbServerConfig,
};
use crate::{
    MainContext,
    deploy::DeployError,
};

#[derive(Debug)]
pub struct DbUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: String,
    pub config: DbConfig,
    pub server_config: DbServerConfig,
}

impl<'a> DbUnit<'a> {
    pub fn new(
        ctx: &'a MainContext,
        name: impl Into<String>,
        config: DbConfig,
        server_config: DbServerConfig,
    ) -> Self {
        Self {
            ctx,
            name: name.into(),
            config,
            server_config,
        }
    }

    /// Provision the database + user inside the running db-server.
    /// Resolves both the server's root password and the new user's password
    /// from the dpl secret store, then runs engine-specific SQL via
    /// `podman exec` against the server's container.
    pub fn create(self) -> Result<(), DeployError> {
        let root_password = self
            .ctx
            .resolve_secret(&self.server_config.secret)
            .map_err(|err| DeployError::UnitError {
                info: format!("decrypt server secret '{}'", self.server_config.secret),
                source: std::io::Error::other(err),
            })?;

        let user_password = self
            .ctx
            .resolve_secret(&self.config.secret)
            .map_err(|err| DeployError::UnitError {
                info: format!("decrypt user secret '{}'", self.config.secret),
                source: std::io::Error::other(err),
            })?;

        self.server_config
            .engine
            .create_database(
                &self.config.server,
                &root_password,
                &self.name,
                &self.config.user,
                &user_password,
            )
            .map_err(|source| DeployError::UnitError {
                info: format!(
                    "create database '{}' in '{}'",
                    self.name, self.config.server
                ),
                source,
            })?;

        Ok(())
    }
}
