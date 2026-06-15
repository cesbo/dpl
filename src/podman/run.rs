use std::{
    ffi::OsStr,
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
use thiserror::Error;

use super::{
    NETWORK,
    ensure_network,
    podman_spawn_error,
    run_podman,
};
use crate::{
    log,
    log::cri_log::CriLog,
};

/// How often the foreground runner polls the child for exit and the shutdown flag.
const POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Error)]
pub enum PodmanRunError {
    #[error("create dpl network")]
    CreateNetwork(#[source] io::Error),
}

pub struct PodmanRun {
    cmd: Command,
    container: String,
}

impl PodmanRun {
    pub fn new(container: &str) -> Result<Self, PodmanRunError> {
        ensure_network(NETWORK).map_err(PodmanRunError::CreateNetwork)?;

        let mut cmd = Command::new("podman");
        cmd.args(["run", "--name", container, "--replace", "--rm"]);

        if !podman_service_is_remote() {
            cmd.arg("--cgroups=split");
        }

        // dpl supervises readiness itself.
        cmd.arg("--sdnotify=ignore");

        // Network
        cmd.arg(format!("--network={NETWORK}"));

        // dpl runs this process in the foreground and writes the log itself.
        cmd.arg("--log-driver=none");

        Ok(PodmanRun {
            cmd,
            container: container.to_string(),
        })
    }

    /// Adds an argument to pass to the program.
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) {
        self.cmd.arg(arg);
    }

    /// Sets environment variables.
    /// Adds argument `--env={key}` to pass to the podman.
    /// Adds an environment variable to the spawned process.
    pub fn env(&mut self, key: &str, value: impl AsRef<str>) {
        self.cmd.env(key, value.as_ref());
        self.arg(format!("--env={key}"));
    }

    /// Creates a bind mount.
    /// Adds argument `--volume={src}:{dst}[:params]` to pass to podman.
    /// - `src` - volume name or absolute path to the host dir
    /// - `dst` - absolute path to the container dir
    /// - `params` - optional mount parameters, for example `["ro"]` or `["U", "Z"]`
    pub fn volume(&mut self, src: impl AsRef<str>, dst: impl AsRef<str>, params: &[&str]) {
        let src = src.as_ref();
        let dst = dst.as_ref();
        let mut arg = format!("--volume={src}:{dst}");
        if !params.is_empty() {
            arg.push(':');
            arg.push_str(&params.join(","));
        }
        self.arg(arg);
    }

    /// Publish a container’s port, or range of ports, to the host.
    pub fn publish(&mut self, host_port: u16, container_port: u16) {
        self.arg(format!("--publish={host_port}:{container_port}"))
    }

    /// Run the container in the foreground under dpl's control.
    pub fn run_foreground(mut self, image: impl AsRef<str>, log_path: &Path) -> io::Result<()> {
        self.arg(image.as_ref());
        run_foreground(self.cmd, &self.container, log_path)
    }

    pub fn into_command(self) -> Command {
        self.cmd
    }
}

fn podman_service_is_remote() -> bool {
    run_podman(&["info", "--format", "{{.Host.ServiceIsRemote}}"])
        .map(|out| out == "true")
        .unwrap_or(false)
}

/// Spawn a command as a child and keep it in the foreground for its whole life.
/// Drain its stdout/stderr to `log_path` in CRI format.
/// Returns once the child (and the log drain) finish.
fn run_foreground(mut cmd: Command, container: &str, log_path: &Path) -> io::Result<()> {
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
        // ran, so keep running and report its real exit status. The scope joins
        // this thread before returning, so the log is fully flushed.
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

    #[test]
    fn captures_output_and_reaps_a_clean_exit() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("unit.runtime.log");

        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err 1>&2"]);

        run_foreground(cmd, "test-container", &log).unwrap();

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

        assert!(run_foreground(cmd, "test-container", &log).is_err());
    }
}
