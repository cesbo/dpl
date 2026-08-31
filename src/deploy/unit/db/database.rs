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
    },
    log,
    reference::ReferenceError,
    state::DeployState,
};

pub struct DbConnectionParams<'a> {
    pub server: &'a UnitName,
    pub user: &'a str,
    pub password: &'a str,
    pub db_name: &'a str,
}

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
                ReferenceError::wrong_unit_type(self.config.server.to_string(), "db-server"),
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
            log::phase("checking database");
            server_config
                .engine
                .ping(
                    &self.config.server,
                    &root_password,
                    Some(self.name.as_str()),
                    super::server::PING_PROBE_TIMEOUT,
                )
                .is_ok()
        };

        if backup.is_some() && exists {
            return Err(DeployError::step_install(
                format!("restore database '{}'", self.name),
                io::Error::other("already exists; delete it manually to re-import"),
            ));
        }

        let params = DbConnectionParams {
            server: &self.config.server,
            user: &self.config.user,
            password: &user_password,
            db_name: self.name.as_str(),
        };

        if !exists {
            log::phase("creating database");
            server_config
                .engine
                .create_database(&params, &root_password)
                .map_err(|e| {
                    DeployError::step_install(format!("create database '{}'", self.name), e)
                })?;
        }

        if let Some(backup) = backup {
            log::phase("restoring database");
            let mut input = open_backup(backup).map_err(|e| {
                DeployError::step_install(format!("restore database '{}'", self.name), e)
            })?;
            server_config
                .engine
                .restore(&params, &self.ctx.build_log_path(self.name), &mut input)
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
