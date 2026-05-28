use std::io;

use super::{
    DbServerUnit,
    model::DbConfig,
};
use crate::{
    MainContext,
    config::ResourceName,
    deploy::{
        DeployError,
        UnitConfig,
        state::DeployState,
    },
    log,
};

#[derive(Debug)]
pub struct DbUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: ResourceName,
    pub config: DbConfig,
}

impl<'a> DbUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &ResourceName, config: DbConfig) -> Self {
        Self {
            ctx,
            name: name.clone(),
            config,
        }
    }

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        let server_config = match UnitConfig::load(self.ctx, &self.config.server).map_err(|e| {
            DeployError::unit(
                format!("load db-server '{}' config", &self.config.server),
                e,
            )
        })? {
            UnitConfig::DbServer(cfg) => cfg,
            _ => {
                return Err(DeployError::unit(
                    format!("load db-server '{}' config", &self.config.server),
                    io::Error::other(format!(
                        "unit '{}' is not a db-server",
                        &self.config.server
                    )),
                ));
            }
        };

        let root_password = self.ctx.resolve_secret(&server_config.secret).map_err(|e| {
            DeployError::unit(format!("resolve secret '{}'", &server_config.secret), e)
        })?;
        let user_password = self.ctx.resolve_secret(&self.config.secret).map_err(|e| {
            DeployError::unit(format!("resolve secret '{}'", &self.config.secret), e)
        })?;

        DbServerUnit::new(self.ctx, &self.config.server, server_config.clone())
            .reload_or_deploy()?;

        let exists = {
            let _phase = log::phase(format!("checking database '{}'", &self.name));
            server_config
                .engine
                .ping(
                    &self.config.server,
                    &root_password,
                    Some(self.name.as_str()),
                )
                .is_ok()
        };

        if !exists {
            let _phase = log::phase(format!("creating database '{}'", &self.name));
            server_config
                .engine
                .create_database(
                    &self.config.server,
                    &root_password,
                    self.name.as_str(),
                    &self.config.user,
                    &user_password,
                )
                .map_err(|e| DeployError::unit(format!("create database '{}'", &self.name), e))?;
        }

        state.set_ready();
        Ok(())
    }
}
