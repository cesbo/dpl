use std::io::{
    self,
    BufRead,
    BufReader,
    Read,
};

use flate2::read::GzDecoder;

use super::{
    DbServerUnit,
    model::DbConfig,
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        UnitConfig,
        state::DeployState,
    },
    error::RefError,
    log,
};

#[derive(Debug)]
pub struct DbUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: DbConfig,
}

impl<'a> DbUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: DbConfig) -> Self {
        Self { ctx, name, config }
    }

    pub fn deploy(
        self,
        state: &mut DeployState,
        backup: Option<Box<dyn Read>>,
    ) -> Result<(), DeployError> {
        let unit = UnitConfig::load(self.ctx, &self.config.server).map_err(|e| {
            DeployError::step_prepare(
                format!("load db-server '{}' config", &self.config.server),
                e,
            )
        })?;

        let UnitConfig::DbServer(server_config) = unit else {
            return Err(DeployError::step_prepare(
                format!("load db-server '{}' config", &self.config.server),
                RefError::wrong_unit_type(self.config.server.to_string(), "db-server"),
            ));
        };

        let root_password = self
            .ctx
            .resolve_secret(&server_config.secret)
            .map_err(|e| {
                DeployError::step_prepare(
                    format!(
                        "resolve secret '{}' with root password",
                        &server_config.secret
                    ),
                    e,
                )
            })?;

        let user_password = self.ctx.resolve_secret(&self.config.secret).map_err(|e| {
            DeployError::step_prepare(
                format!(
                    "resolve secret '{}' with user password",
                    &self.config.secret
                ),
                e,
            )
        })?;

        DbServerUnit::new(self.ctx, &self.config.server, server_config.clone())
            .reload_or_deploy()?;

        let exists = {
            let _phase = log::phase("checking database");
            server_config
                .engine
                .ping(
                    &self.config.server,
                    &root_password,
                    Some(self.name.as_str()),
                )
                .is_ok()
        };

        if backup.is_some() && exists {
            return Err(DeployError::step_install(
                format!("restore database '{}'", self.name),
                io::Error::other("already exists; delete it manually to re-import"),
            ));
        }

        if !exists {
            let _phase = log::phase("creating database");
            server_config
                .engine
                .create_database(
                    &self.config.server,
                    &root_password,
                    self.name.as_str(),
                    &self.config.user,
                    &user_password,
                )
                .map_err(|e| {
                    DeployError::step_install(format!("create database '{}'", self.name), e)
                })?;
        }

        if let Some(backup) = backup {
            let _phase = log::phase("restoring database");
            let mut input = open_backup(backup).map_err(|e| {
                DeployError::step_install(format!("restore database '{}'", self.name), e)
            })?;
            server_config
                .engine
                .restore(
                    &self.config.server,
                    &self.config.user,
                    &user_password,
                    self.name.as_str(),
                    &mut input,
                )
                .map_err(|e| {
                    DeployError::step_install(format!("restore database '{}'", self.name), e)
                })?;
        }

        state.set_ready();
        Ok(())
    }
}

/// Peek the gzip magic bytes so a `.gz` (or piped gzip) source is decoded
/// transparently; the peeked bytes stay buffered for whichever reader wraps it.
fn open_backup(backup: Box<dyn Read>) -> io::Result<Box<dyn Read>> {
    let mut reader = BufReader::new(backup);
    let gzipped = reader.fill_buf()?.starts_with(&[0x1f, 0x8b]);
    if gzipped {
        Ok(Box::new(GzDecoder::new(reader)))
    } else {
        Ok(Box::new(reader))
    }
}
