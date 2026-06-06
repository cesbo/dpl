use std::{
    io::{
        self,
        BufRead,
        BufReader,
        Read,
        Write,
    },
    path::Path,
    process::{
        Command,
        ExitStatus,
        Stdio,
    },
    thread,
};

use super::{
    DbConnectionParams,
    model::DbServerEngine,
};
use crate::{
    log::cri_log::CriLog,
    podman::podman_spawn_error,
};

impl DbServerEngine {
    /// Stream a logical SQL dump of `db_name` to `out`, running the engine's
    /// dump tool inside the running db-server container as `user`. The child's
    /// stderr is drained on a separate thread and handed to `on_stderr`
    /// line-by-line as it arrives, so a chatty tool can't fill the stderr pipe
    /// and stall the dump.
    pub fn dump<W: Write>(
        self,
        params: &DbConnectionParams,
        out: &mut W,
        on_stderr: &mut (dyn FnMut(&[u8]) + Send),
    ) -> io::Result<()> {
        let server = params.server.scoped_unit_name();
        let password_env = self.client_password_env();

        let mut cmd = Command::new("podman");
        cmd.env(password_env, params.password);
        cmd.args(["exec", "-e", password_env, &server]);
        cmd.args(self.dump_args(params.user, params.db_name));

        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(podman_spawn_error)?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to capture podman exec stdout"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("failed to capture podman exec stderr"))?;

        // Pump stdout in this thread while a second thread drains stderr; both
        // pipes are read concurrently, so neither can block the other.
        let copy_res = thread::scope(|scope| {
            let drainer = scope.spawn(|| drain_stderr(stderr, on_stderr));
            let copy_res = io::copy(&mut stdout, out);
            drainer.join().expect("stderr drain thread panicked");
            copy_res
        });

        finish(child.wait()?, copy_res)
    }

    /// Replay a logical SQL dump read from `input` into `db_name`, piping it to
    /// the engine's client inside the running db-server container as `user`.
    /// The client's stdout is discarded (command tags are noise) and its stderr
    /// is streamed to the build log.
    pub fn restore<R: Read>(
        self,
        params: &DbConnectionParams,
        log_path: &Path,
        input: &mut R,
    ) -> io::Result<()> {
        let server = params.server.scoped_unit_name();
        let password_env = self.client_password_env();

        let mut cmd = Command::new("podman");
        cmd.env(password_env, params.password);
        cmd.args(["exec", "-i", "-e", password_env, &server]);
        cmd.args(self.restore_args(params.user, params.db_name));

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(podman_spawn_error)?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("failed to capture podman exec stdin"))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("failed to capture podman exec stderr"))?;

        // Stream the client's stderr into the unit's build log (CRI k8s-file,
        // tagged `stderr`). If the log can't be opened, still drain stderr to a
        // sink so the pipe empties and the client isn't wedged - logging is
        // best-effort, the restore is not.
        let log = match CriLog::open(log_path, None) {
            Ok(log) => Some(log),
            Err(err) => {
                crate::log::warn(format!("write build log {}: {err}", log_path.display()));
                None
            }
        };
        let copy_res = thread::scope(|scope| {
            let drainer = scope.spawn(|| match &log {
                Some(log) => log.capture(io::empty(), stderr),
                None => io::copy(&mut BufReader::new(stderr), &mut io::sink()).map(|_| ()),
            });
            let copy_res = io::copy(input, &mut stdin);
            // Close stdin so the client sees EOF, exits, and lets the drain
            // thread reach end-of-stderr; only then can the join below return.
            drop(stdin);
            let _ = drainer.join().expect("stderr drain thread panicked");
            copy_res
        });

        finish(child.wait()?, copy_res)
    }

    /// Args to dump `db_name` to stdout as `user`.
    fn dump_args(self, user: &str, db_name: &str) -> Vec<String> {
        let args = match self {
            DbServerEngine::Postgresql => vec!["pg_dump", "-U", user, db_name],
            DbServerEngine::Mariadb => vec!["mariadb-dump", "-u", user, db_name],
            DbServerEngine::Mysql => vec!["mysqldump", "-u", user, db_name],
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
            DbServerEngine::Mysql => vec!["mysql", "-u", user, db_name],
        };
        args.into_iter().map(String::from).collect()
    }
}

/// Read `stderr` to EOF, calling `on_line` with each line (newline trimmed).
fn drain_stderr<R: Read>(stderr: R, on_line: &mut (dyn FnMut(&[u8]) + Send)) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(len) if len > 0 => on_line(trim_newline(&line)),
            _ => break,
        }
    }
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    &line[.. end]
}

fn finish(status: ExitStatus, copy_res: io::Result<u64>) -> io::Result<()> {
    if !status.success() {
        return Err(io::Error::other(format!(
            "podman exec exited with {status}"
        )));
    }

    copy_res?;

    Ok(())
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
    fn dump_args_mysql() {
        assert_eq!(
            DbServerEngine::Mysql.dump_args("app1", "app-db"),
            ["mysqldump", "-u", "app1", "app-db"]
        );
    }

    #[test]
    fn restore_args_postgres() {
        assert_eq!(
            DbServerEngine::Postgresql.restore_args("app1", "app-db"),
            [
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                "app1",
                "-d",
                "app-db"
            ]
        );
    }

    #[test]
    fn restore_args_mariadb() {
        assert_eq!(
            DbServerEngine::Mariadb.restore_args("app1", "app-db"),
            ["mariadb", "-u", "app1", "app-db"]
        );
    }

    #[test]
    fn restore_args_mysql() {
        assert_eq!(
            DbServerEngine::Mysql.restore_args("app1", "app-db"),
            ["mysql", "-u", "app1", "app-db"]
        );
    }

    #[test]
    fn trim_newline_variants() {
        assert_eq!(trim_newline(b"hello\n"), b"hello");
        assert_eq!(trim_newline(b"hello\r\n"), b"hello");
        assert_eq!(trim_newline(b"hello"), b"hello");
        assert_eq!(trim_newline(b""), b"");
    }
}
