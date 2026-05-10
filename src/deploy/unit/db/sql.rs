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

/// Provision a new database + login user inside a running db-server container.
pub fn create_database(
    engine: DbServerEngine,
    server: &str,
    root_password: &str,
    db_name: &str,
    username: &str,
    password: &str,
) -> io::Result<()> {
    let sql = match engine {
        DbServerEngine::Postgresql => build_postgres_sql(db_name, username, password),
        DbServerEngine::Mariadb => build_mariadb_sql(db_name, username, password),
    };

    let client_password_env = engine.client_password_env();
    let client_args = engine.client_args();

    let mut cmd = Command::new("podman");
    cmd.env(client_password_env, root_password);
    cmd.args(["exec", "-i", "-e", client_password_env, server]);
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

fn build_mariadb_sql(db_name: &str, username: &str, password: &str) -> String {
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
    fn mariadb_sql_structure_and_password_escape() {
        let sql = build_mariadb_sql("app1", "app1", "p'ss");
        println!("{sql}");
        assert!(sql.contains("CREATE DATABASE `app1`;"));
        assert!(sql.contains("CREATE USER `app1`@'%' IDENTIFIED BY 'p\\'ss';"));
        assert!(sql.contains("GRANT ALL PRIVILEGES ON `app1`.* TO `app1`@'%';"));
    }
}
