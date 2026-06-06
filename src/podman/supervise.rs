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
    log::cri_log::CriLog,
    podman::{
        podman_spawn_error,
        run_podman,
    },
};

/// How often the supervisor polls the child for exit and the shutdown flag.
const POLL: Duration = Duration::from_millis(200);

/// Spawn a command as a child and supervise it for the whole life.
/// Drain its stdout/stderr to `log_path` in CRI format.
/// Returns once the child (and the log drain) finish.
pub fn supervise(mut cmd: Command, container: &str, log_path: &Path) -> io::Result<()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(podman_spawn_error)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))?;
    }

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let status = thread::scope(|scope| -> io::Result<ExitStatus> {
        // Drain the pipes for the container's whole life; returns on EOF, i.e.
        // the child exiting. A log failure is non-fatal: the container still
        // ran, so keep supervising and report its real exit status. The scope
        // joins this thread before returning, so the log is fully flushed.
        scope.spawn(|| {
            if let Err(err) =
                CriLog::open(log_path, None).and_then(|cri| cri.capture(stdout, stderr))
            {
                log::warn(format!("write runtime log {}: {err}", log_path.display()));
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    // Drives the supervisor with a plain short-lived command (no podman): it
    // exits before any signal, so the stop branch never fires. Covers the
    // capture -> reap -> success path and the CRI log integration end to end.
    #[test]
    fn captures_output_and_reaps_a_clean_exit() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("unit.runtime.log");

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err 1>&2"]);

        supervise(cmd, "test-container", &log).unwrap();

        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("stdout F out"), "{contents}");
        assert!(contents.contains("stderr F err"), "{contents}");
    }

    #[test]
    fn nonzero_exit_is_an_error() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("unit.runtime.log");

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 3"]);

        assert!(supervise(cmd, "test-container", &log).is_err());
    }
}
