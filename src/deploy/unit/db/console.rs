use std::{
    io,
    process::Command,
};

use super::model::DbServerEngine;
use crate::{
    config::UnitName,
    podman::podman_spawn_error,
};

impl DbServerEngine {
    /// Open an interactive SQL client against `db_name` (the engine's default
    /// when `None`) as `user` inside the running db-server container. stdin/stdout/stderr inherit the parent
    /// terminal (the default), so the client gets a real TTY via `-it`.
    pub fn console(
        self,
        server: &UnitName,
        user: &str,
        password: &str,
        db_name: Option<&str>,
    ) -> io::Result<()> {
        let server = server.scoped_unit_name();
        let password_env = self.client_password_env();

        let mut cmd = Command::new("podman");
        cmd.env(password_env, password);
        cmd.args(["exec", "-it", "-e", password_env, &server]);
        cmd.args(self.console_args(user, db_name));

        let status = cmd.status().map_err(podman_spawn_error)?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("console exited with {status}")))
        }
    }

    /// Args to open an interactive client session as `user` against `db_name`.
    /// Without `db_name`, psql opens the database named after the user and the
    /// MySQL clients open none.
    fn console_args(self, user: &str, db_name: Option<&str>) -> Vec<String> {
        let mut args = match self {
            DbServerEngine::Postgresql => vec!["psql", "-U", user],
            DbServerEngine::Mariadb => vec!["mariadb", "-u", user],
            DbServerEngine::Mysql => vec!["mysql", "-u", user],
        };
        if let Some(db_name) = db_name {
            if self == DbServerEngine::Postgresql {
                args.push("-d");
            }
            args.push(db_name);
        }
        args.into_iter().map(String::from).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_args() {
        assert_eq!(
            DbServerEngine::Postgresql.console_args("app1", Some("app-db")),
            ["psql", "-U", "app1", "-d", "app-db"]
        );
        assert_eq!(
            DbServerEngine::Mariadb.console_args("app1", Some("app-db")),
            ["mariadb", "-u", "app1", "app-db"]
        );
        assert_eq!(
            DbServerEngine::Mysql.console_args("app1", Some("app-db")),
            ["mysql", "-u", "app1", "app-db"]
        );
        assert_eq!(
            DbServerEngine::Postgresql.console_args("postgres", None),
            ["psql", "-U", "postgres"]
        );
        assert_eq!(
            DbServerEngine::Mariadb.console_args("root", None),
            ["mariadb", "-u", "root"]
        );
    }
}
