mod artifacts;
mod backup;
mod console;
mod database;
mod model;
mod server;
mod sql;

use std::{
    io,
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

pub use self::{
    database::DbUnit,
    model::{
        DbConfig,
        DbServerConfig,
        DbServerEngine,
    },
    server::DbServerUnit,
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::UnitConfig,
};

/// Poll a `db` unit until its server accepts the root login for that database,
/// or `timeout` elapses. Shared by `dpl db wait` and an app unit's startup gate.
pub fn wait_until_ready(ctx: &MainContext, name: &UnitName, timeout: Duration) -> io::Result<()> {
    let db_config = match UnitConfig::load(ctx, name).map_err(io::Error::other)? {
        UnitConfig::Db(config) => config,
        _ => return Err(io::Error::other(format!("unit '{name}' is not a db"))),
    };

    let server_config = match UnitConfig::load(ctx, &db_config.server).map_err(io::Error::other)? {
        UnitConfig::DbServer(config) => config,
        _ => {
            return Err(io::Error::other(format!(
                "unit '{}' is not a db-server",
                db_config.server
            )));
        }
    };

    let root_password = ctx
        .resolve_secret(&server_config.secret)
        .map_err(io::Error::other)?;

    let deadline = Instant::now() + timeout;
    let interval = Duration::from_millis(800);
    loop {
        let ready = server_config
            .engine
            .ping(&db_config.server, &root_password, Some(name.as_str()))
            .is_ok();
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "timeout waiting for database '{name}'"
            )));
        }
        sleep(interval);
    }
}
