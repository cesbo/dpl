use std::{
    io::{
        self,
        Write,
    },
    process::{
        Command,
        Stdio,
    },
};

use sea_query::{
    Alias,
    Iden,
    MysqlQueryBuilder,
    PostgresQueryBuilder,
    QueryBuilder,
    Quote,
    QuotedBuilder,
};

use super::model::DbServerEngine;
use crate::config::ResourceName;

impl DbServerEngine {
    /// Returns `Ok` only when the container is up and the SQL server accepts
    /// the root login.
    pub fn ping(
        self,
        server: &ResourceName,
        root_password: &str,
        db_name: Option<&str>,
    ) -> io::Result<()> {
        let server = server.scoped_unit_name();
        let password_env = self.client_password_env();
        let mut cmd = Command::new("podman");
        cmd.env(password_env, root_password);
        cmd.args(["exec", "-e", password_env, &server]);
        match self {
            DbServerEngine::Postgresql => {
                cmd.args([
                    "psql",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "-U",
                    "postgres",
                    "-d",
                    db_name.unwrap_or("postgres"),
                    "-tAc",
                    "SELECT 1",
                ]);
            }
            DbServerEngine::Mariadb => {
                cmd.args(["mariadb", "-u", "root", "-N", "-B", "-e", "SELECT 1"]);
                if let Some(name) = db_name {
                    cmd.arg(name);
                }
            }
            DbServerEngine::Mysql => {
                cmd.args(["mysql", "-u", "root", "-N", "-B", "-e", "SELECT 1"]);
                if let Some(name) = db_name {
                    cmd.arg(name);
                }
            }
        }

        let status = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("ping exited with {status}")))
        }
    }

    /// Provision a new database + login user inside a running db-server container.
    pub fn create_database(
        self,
        server: &ResourceName,
        root_password: &str,
        db_name: &str,
        username: &str,
        password: &str,
    ) -> io::Result<()> {
        let server = server.scoped_unit_name();
        let sql = match self {
            DbServerEngine::Postgresql => build_postgres_sql(db_name, username, password),
            DbServerEngine::Mariadb | DbServerEngine::Mysql => {
                build_mysql_sql(db_name, username, password)
            }
        };

        let client_password_env = self.client_password_env();
        let client_args = self.client_args();

        let mut cmd = Command::new("podman");
        cmd.env(client_password_env, root_password);
        cmd.args(["exec", "-i", "-e", client_password_env, &server]);
        cmd.args(client_args);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| io::Error::other("failed to capture podman exec stdin"))?;
            stdin.write_all(sql.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        let detail = if trimmed.is_empty() {
            format!("podman exec exited with {}", output.status)
        } else {
            trimmed.to_string()
        };
        Err(io::Error::other(detail))
    }
}

fn build_postgres_sql(db_name: &str, username: &str, password: &str) -> String {
    let b = PostgresQueryBuilder;
    let q = b.quote();

    let safe_db_name = quote_identifier(db_name, q);
    let safe_username = quote_identifier(username, q);
    let safe_password = quote_literal(password, b);

    format!(
        "CREATE ROLE {safe_username} LOGIN PASSWORD {safe_password}; \
         CREATE DATABASE {safe_db_name} OWNER {safe_username};"
    )
}

/// MySQL-dialect provisioning SQL, shared by the `mariadb` and `mysql` engines
fn build_mysql_sql(db_name: &str, username: &str, password: &str) -> String {
    let b = MysqlQueryBuilder;
    let q = b.quote();

    let safe_db_name = quote_identifier(db_name, q);
    let safe_username = quote_identifier(username, q);
    let safe_password = quote_literal(password, b);

    format!(
        "CREATE DATABASE {safe_db_name}; \
         CREATE USER {safe_username}@'%' IDENTIFIED BY {safe_password}; \
         GRANT ALL PRIVILEGES ON {safe_db_name}.* TO {safe_username}@'%';"
    )
}

fn quote_identifier(value: &str, q: Quote) -> String {
    let mut s = String::new();
    Alias::new(value).prepare(&mut s, q);
    s
}

fn quote_literal(value: &str, b: impl QueryBuilder) -> String {
    b.value_to_string(&value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_sql_structure_and_password_escape() {
        let sql = build_postgres_sql("app1", "app1", "p'ss");
        println!("{sql}");
        assert!(sql.contains("CREATE ROLE \"app1\" LOGIN PASSWORD E'p\\'ss';"));
        assert!(sql.contains("CREATE DATABASE \"app1\" OWNER \"app1\";"));
    }

    #[test]
    fn mysql_sql_structure_and_password_escape() {
        let sql = build_mysql_sql("app1", "app1", "p'ss");
        println!("{sql}");
        assert!(sql.contains("CREATE DATABASE `app1`;"));
        assert!(sql.contains("CREATE USER `app1`@'%' IDENTIFIED BY 'p\\'ss';"));
        assert!(sql.contains("GRANT ALL PRIVILEGES ON `app1`.* TO `app1`@'%';"));
    }
}
