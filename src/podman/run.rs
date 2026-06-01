use std::{
    ffi::OsStr,
    io,
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
};

use thiserror::Error;

use super::{
    NETWORK,
    PODMAN_LOG_DIR,
    podman_spawn_error,
    run_podman,
};

#[derive(Debug, Error)]
pub enum PodmanRunError {
    #[error("create dpl network")]
    CreateNetwork(#[source] io::Error),

    #[error("create podman log dir")]
    CreateLogDir(#[source] io::Error),
}

pub struct PodmanRun(Command);

impl PodmanRun {
    pub fn new(container: &str) -> Result<Self, PodmanRunError> {
        run_podman(&["network", "create", "--ignore", NETWORK])
            .map_err(PodmanRunError::CreateNetwork)?;

        std::fs::create_dir_all(PODMAN_LOG_DIR).map_err(PodmanRunError::CreateLogDir)?;

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

        // Log
        let log_name = format!("{container}.log");
        let log_path = Path::new(crate::podman::PODMAN_LOG_DIR).join(log_name);
        let log_arg = format!("--log-opt=path={}", log_path.display());
        cmd.args(["--log-driver=k8s-file", &log_arg, "--log-opt=max-size=20mb"]);

        Ok(PodmanRun(cmd))
    }

    /// Adds an argument to pass to the program.
    fn arg(&mut self, arg: impl AsRef<OsStr>) {
        self.0.arg(arg);
    }

    /// Sets environment variables.
    /// Adds argument `--env={key}` to pass to the podman.
    /// Adds an environment variable to the spawned process.
    pub fn env(&mut self, key: &str, value: impl AsRef<str>) {
        self.0.env(key, value.as_ref());
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

    pub fn exec(&mut self, image: impl AsRef<str>) -> io::Error {
        self.arg(image.as_ref());

        let err = self.0.exec();
        podman_spawn_error(err)
    }
}
