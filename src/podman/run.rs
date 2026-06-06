use std::{
    ffi::OsStr,
    io,
    path::Path,
    process::Command,
};

use thiserror::Error;

use super::{
    NETWORK,
    run_podman,
    supervise,
};

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
        run_podman(&["network", "create", "--ignore", NETWORK])
            .map_err(PodmanRunError::CreateNetwork)?;

        let mut cmd = Command::new("podman");

        cmd.args([
            "run",
            "--name",
            container,
            "--replace",
            "--rm",
            "--cgroups=split",
            "--sdnotify=conmon",
        ]);

        // Network
        cmd.arg(format!("--network={}", crate::podman::NETWORK));

        // dpl supervises this process and writes the log itself.
        cmd.arg("--log-driver=none");

        Ok(PodmanRun {
            cmd,
            container: container.to_string(),
        })
    }

    /// Adds an argument to pass to the program.
    fn arg(&mut self, arg: impl AsRef<OsStr>) {
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
    /// Adds arguemnt `--volume={src}:{dst}` to pass to the podman.
    /// - `src` - volume name or absolute path to the host dir
    /// - `dst` - absolute path to the container dir
    pub fn volume(&mut self, src: impl AsRef<str>, dst: impl AsRef<str>) {
        let src = src.as_ref();
        let dst = dst.as_ref();
        self.arg(format!("--volume={src}:{dst}"));
    }

    /// Publish a container’s port, or range of ports, to the host.
    pub fn publish(&mut self, host_port: u16, container_port: u16) {
        self.arg(format!("--publish={host_port}:{container_port}"))
    }

    /// Run the container in the foreground under dpl's supervision.
    pub fn supervise(mut self, image: impl AsRef<str>, log_path: &Path) -> io::Result<()> {
        self.arg(image.as_ref());
        supervise::supervise(self.cmd, &self.container, log_path)
    }
}
