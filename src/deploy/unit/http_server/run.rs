use std::{
    io,
    path::Path,
    process::{
        Command,
        ExitStatus,
        Stdio,
    },
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    thread,
    time::Duration,
};

use signal_hook::consts::{
    SIGINT,
    SIGTERM,
};

use crate::{
    log,
    log::{
        cri_log::CriLog,
        jsonl_log::JsonlLog,
    },
    podman::{
        podman_spawn_error,
        run_podman,
    },
};

/// How often the foreground nginx runner polls the child for exit and shutdown.
const POLL: Duration = Duration::from_millis(200);

/// Run nginx in the foreground, writing stdout JSONL to a separate access log
/// and stderr to the normal runtime log.
pub fn run_foreground(
    mut cmd: Command,
    container: &str,
    runtime_log_path: &Path,
    access_log_path: &Path,
) -> io::Result<()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(podman_spawn_error)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))?;
    }

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let status = thread::scope(|scope| -> io::Result<ExitStatus> {
        scope.spawn(|| {
            let result = JsonlLog::open(access_log_path).and_then(|mut log| log.capture(stdout));
            if let Err(err) = result {
                log::warn(format!(
                    "write access log {}: {err}",
                    access_log_path.display()
                ));
            }
        });

        scope.spawn(|| {
            let result = CriLog::open(runtime_log_path, None)
                .and_then(|log| log.capture_stderr_with(stderr, strip_nginx_error_timestamp));
            if let Err(err) = result {
                log::warn(format!(
                    "write runtime log {}: {err}",
                    runtime_log_path.display()
                ));
            }
        });

        let mut stopping = false;
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }

            if shutdown.load(Ordering::Relaxed) && !stopping {
                stopping = true;
                // podman sends the stop signal, waits, then SIGKILLs; the run
                // child exits once the container is down.
                let _ = run_podman(&["stop", "--ignore", container]);
            }

            thread::sleep(POLL);
        }
    })?;

    if status.success() || shutdown.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err(io::Error::other(format!("podman run exited with {status}")))
    }
}

fn strip_nginx_error_timestamp(line: &[u8]) -> &[u8] {
    let skip = nginx_error_prefix(line).unwrap_or(0);
    &line[skip ..]
}

fn nginx_error_prefix(line: &[u8]) -> Option<usize> {
    const LEN: usize = "YYYY/MM/DD HH:MM:SS ".len();

    fn digits(s: &[u8], range: std::ops::Range<usize>) -> bool {
        s[range].iter().all(u8::is_ascii_digit)
    }

    let ok = line.len() >= LEN
        && digits(line, 0 .. 4)
        && line[4] == b'/'
        && digits(line, 5 .. 7)
        && line[7] == b'/'
        && digits(line, 8 .. 10)
        && line[10] == b' '
        && digits(line, 11 .. 13)
        && line[13] == b':'
        && digits(line, 14 .. 16)
        && line[16] == b':'
        && digits(line, 17 .. 19)
        && line[19] == b' ';

    ok.then_some(LEN)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn split_access_log_writes_jsonl_stdout_and_cri_stderr() {
        let dir = TempDir::new().unwrap();
        let runtime_log = dir.path().join("runtime.log");
        let access_log = dir.path().join("access.log");

        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "printf '{\"status\":200}\\nnot-json\\n{\"status\":404}\\n'; \
             echo '2026/06/14 10:11:12 [notice] 1#1: start' 1>&2; \
             echo 'plain err' 1>&2",
        ]);

        run_foreground(cmd, "test-container", &runtime_log, &access_log).unwrap();

        assert_eq!(
            std::fs::read_to_string(&access_log).unwrap(),
            "{\"status\":200}\n{\"status\":404}\n"
        );
        let runtime = std::fs::read_to_string(&runtime_log).unwrap();
        assert!(
            runtime.contains("stderr F [notice] 1#1: start"),
            "{runtime}"
        );
        assert!(runtime.contains("stderr F plain err"), "{runtime}");
        assert!(!runtime.contains("2026/06/14 10:11:12"), "{runtime}");
        assert!(!runtime.contains("stdout"), "{runtime}");
    }

    #[test]
    fn strips_only_nginx_error_timestamp_prefix() {
        assert_eq!(
            strip_nginx_error_timestamp(b"2026/06/14 10:11:12 [error] failed"),
            b"[error] failed"
        );
        assert_eq!(
            strip_nginx_error_timestamp(b"2026-06-14T10:11:12Z [error] failed"),
            b"2026-06-14T10:11:12Z [error] failed"
        );
        assert_eq!(strip_nginx_error_timestamp(b"plain error"), b"plain error");
    }
}
