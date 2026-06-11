use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use crate::{
    config::UnitName,
    log,
    log::cri_log::CriLog,
    podman::{
        podman_spawn_error,
        run_podman,
    },
};

pub struct PodmanContext {
    image_tag: String,
}

impl PodmanContext {
    pub fn new(name: &UnitName, version: u32) -> Self {
        let image_tag = format!("localhost/{name}:{version}");
        Self { image_tag }
    }

    pub fn build(
        &self,
        deploy_dir: &Path,
        log_path: &Path,
        build_env: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let artifacts_dir = deploy_dir.join("artifacts");
        let containerfile = artifacts_dir.join("containerfile");

        let mut build = PodmanBuild::new();
        for (key, value) in build_env {
            build.build_env(key, value);
        }

        let mut cmd = build.into_command();
        cmd.arg("--file")
            .arg(&containerfile)
            .arg("--tag")
            .arg(&self.image_tag)
            .arg(deploy_dir);

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(podman_spawn_error)?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        if let Err(err) = CriLog::open(log_path, None).and_then(|log| log.capture(stdout, stderr)) {
            log::warn(format!("write build log {}: {err}", log_path.display()));
        }

        let status = child.wait()?;
        if !status.success() {
            // Drops the partially-built layers (dangling `<none>` image)
            let _ = run_podman(&["image", "prune", "-f"]);
            return Err(io::Error::other(format!(
                "podman build exited with {status}"
            )));
        }

        Ok(())
    }

    pub fn remove(&self) {
        let _ = run_podman(&["rmi", &self.image_tag]);

        // Remove dangling images from local storage
        let _ = run_podman(&["image", "prune", "-f"]);
        let _ = run_podman(&["image", "prune", "-f", "--external"]);
    }
}

struct PodmanBuild {
    cmd: Command,
}

impl PodmanBuild {
    fn new() -> Self {
        let mut cmd = Command::new("podman");
        cmd.arg("build")
            .arg("--rm")
            .arg("--force-rm")
            .arg("--no-cache");
        Self { cmd }
    }

    /// Sets a build argument from the spawned process environment.
    fn build_env(&mut self, key: &str, value: impl AsRef<str>) {
        self.cmd.env(key, value.as_ref());
        self.arg("--build-arg");
        self.arg(key);
    }

    fn arg(&mut self, arg: impl AsRef<OsStr>) {
        self.cmd.arg(arg);
    }

    fn into_command(self) -> Command {
        self.cmd
    }
}

/// Place an export's `path` (a URL prefix) under `version_dir` to form the
/// www-relative destination, e.g. `("web_3", "/static") -> "web_3/static"`.
pub(super) fn export_destination(version_dir: &str, path: &str) -> String {
    let mut dst = version_dir.to_string();
    for item in path.trim_start_matches('/').split('/') {
        if !item.is_empty() {
            dst.push('/');
            dst.push_str(item);
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_passes_build_arg_from_process_env() {
        let mut build = PodmanBuild::new();
        build.build_env("API_KEY", "secret-value");

        let args: Vec<_> = build
            .cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|items| items == ["--build-arg", "API_KEY"]),
            "{args:?}"
        );

        let value = build
            .cmd
            .get_envs()
            .find(|(key, _)| *key == "API_KEY")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(value.as_deref(), Some("secret-value"));
    }

    #[test]
    fn export_destination_places_path_under_version_dir() {
        assert_eq!(
            export_destination("web_3", "/"),
            "web_3"
        );
        assert_eq!(
            export_destination("web_3", "/static"),
            "web_3/static"
        );
        assert_eq!(
            export_destination("web_3", "assets/css"),
            "web_3/assets/css"
        );
    }
}
