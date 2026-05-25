use std::{
    io::{
        self,
        Read,
        Write,
    },
    process::{
        Command,
        Stdio,
    },
};

use super::model::DbServerEngine;

impl DbServerEngine {
    /// Stream a logical SQL dump of `db_name` to `out`, running the engine's
    /// dump tool inside the running db-server container as `user`.
    pub fn dump<W: Write>(
        self,
        server: &str,
        user: &str,
        password: &str,
        db_name: &str,
        out: &mut W,
    ) -> io::Result<()> {
        let password_env = self.client_password_env();

        let mut cmd = Command::new("podman");
        cmd.env(password_env, password);
        cmd.args(["exec", "-e", password_env, server]);
        cmd.args(self.dump_args(user, db_name));

        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Drain stdout into the destination while the child runs, so the pipe
        // never fills up and stalls the dump.
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture podman exec stdout"))?;
        io::copy(&mut stdout, out)?;

        // `stdout` was taken, so `wait_with_output` only collects stderr.
        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(());
        }

        Err(io::Error::other(exec_error(&output.stderr, output.status)))
    }

    /// Replay a logical SQL dump read from `input` into `db_name`, piping it to
    /// the engine's client inside the running db-server container as `user`.
    pub fn restore<R: Read>(
        self,
        server: &str,
        user: &str,
        password: &str,
        db_name: &str,
        input: &mut R,
    ) -> io::Result<()> {
        let password_env = self.client_password_env();

        let mut cmd = Command::new("podman");
        cmd.env(password_env, password);
        cmd.args(["exec", "-i", "-e", password_env, server]);
        cmd.args(self.restore_args(user, db_name));

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
            io::copy(input, stdin)?;
        }

        // `wait_with_output` closes stdin (signalling EOF) before collecting output.
        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(());
        }

        Err(io::Error::other(exec_error(&output.stderr, output.status)))
    }

    /// Args to dump `db_name` to stdout as `user`.
    fn dump_args(self, user: &str, db_name: &str) -> Vec<String> {
        let args = match self {
            DbServerEngine::Postgresql => vec!["pg_dump", "-U", user, db_name],
            DbServerEngine::Mariadb => vec!["mariadb-dump", "-u", user, db_name],
        };
        args.into_iter().map(String::from).collect()
    }

    /// Args to replay a dump from stdin into `db_name` as `user`.
    fn restore_args(self, user: &str, db_name: &str) -> Vec<String> {
        let args = match self {
            DbServerEngine::Postgresql => {
                vec!["psql", "-v", "ON_ERROR_STOP=1", "-U", user, "-d", db_name]
            }
            DbServerEngine::Mariadb => vec!["mariadb", "-u", user, db_name],
        };
        args.into_iter().map(String::from).collect()
    }
}

fn exec_error(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let trimmed = String::from_utf8_lossy(stderr);
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
        format!("podman exec exited with {status}")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_args_postgres() {
        assert_eq!(
            DbServerEngine::Postgresql.dump_args("app1", "app-db"),
            ["pg_dump", "-U", "app1", "app-db"]
        );
    }

    #[test]
    fn dump_args_mariadb() {
        assert_eq!(
            DbServerEngine::Mariadb.dump_args("app1", "app-db"),
            ["mariadb-dump", "-u", "app1", "app-db"]
        );
    }

    #[test]
    fn restore_args_postgres() {
        assert_eq!(
            DbServerEngine::Postgresql.restore_args("app1", "app-db"),
            ["psql", "-v", "ON_ERROR_STOP=1", "-U", "app1", "-d", "app-db"]
        );
    }

    #[test]
    fn restore_args_mariadb() {
        assert_eq!(
            DbServerEngine::Mariadb.restore_args("app1", "app-db"),
            ["mariadb", "-u", "app1", "app-db"]
        );
    }
}
