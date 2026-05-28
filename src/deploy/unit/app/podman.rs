use std::{
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use tracing::{
    debug,
    warn,
};

use super::model::ExportConfig;
use crate::{
    config::ResourceName,
    log::child_output,
    podman::{
        NGINX_WWW_VOLUME,
        ensure_volume,
        run_podman,
        volume_mountpoint,
    },
};

pub struct PodmanContext<'a> {
    name: &'a ResourceName,
    version: u32,
    image_tag: String,
}

impl<'a> PodmanContext<'a> {
    pub fn new(name: &'a ResourceName, version: u32) -> Self {
        let image_tag = format!("localhost/{name}:{version}");
        Self {
            name,
            version,
            image_tag,
        }
    }

    /// Build podman image, streams output into the build log.
    pub fn build(&self, deploy_dir: &Path) -> io::Result<()> {
        let artifacts_dir = deploy_dir.join("artifacts");
        let containerfile = artifacts_dir.join("containerfile");

        let mut cmd = Command::new("podman");
        cmd.arg("build")
            .arg("--rm")
            .arg("--force-rm")
            .arg("--no-cache");

        let mut secrets: Vec<_> = std::fs::read_dir(&artifacts_dir)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                if name.starts_with("build-") && name.ends_with(".sh") {
                    let id = name.strip_suffix(".sh")?.to_owned();
                    Some((id, path))
                } else {
                    None
                }
            })
            .collect();
        secrets.sort();

        for (id, path) in &secrets {
            cmd.arg("--secret")
                .arg(format!("id={id},src={}", path.display()));
        }

        cmd.arg("--file")
            .arg(&containerfile)
            .arg("--tag")
            .arg(&self.image_tag)
            .arg(deploy_dir);

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("running: podman build --tag {}", &self.image_tag);

        let mut child = cmd.spawn()?;

        // Worker threads don't inherit the deploy's thread-default subscriber,
        // so capture it here and re-establish it inside each thread.
        let dispatch = tracing::dispatcher::get_default(|d| d.clone());

        let stdout = child.stdout.take().unwrap();
        let stdout_dispatch = dispatch.clone();
        let stdout_handle = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&stdout_dispatch, || child_output(stdout));
        });

        let stderr = child.stderr.take().unwrap();
        let stderr_handle = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || child_output(stderr));
        });

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

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

    /// Export files from the podman image into the static volume under
    /// `<name>_<version>/<path>/` at the volume root.
    pub fn export(&self, exports: &[ExportConfig]) -> io::Result<()> {
        if exports.is_empty() {
            return Ok(());
        }

        // Ensure the volume exists, then resolve its host mountpoint so we can
        // copy into it with `podman cp` (cp targets host paths, not volumes).
        ensure_volume(NGINX_WWW_VOLUME)?;
        let exports_root = volume_mountpoint(NGINX_WWW_VOLUME)?;
        std::fs::create_dir_all(&exports_root)?;

        let version_dir = exports_root.join(format!("{}_{}", self.name, self.version));
        std::fs::create_dir_all(&version_dir)?;

        let container = format!("dpl-export-{}", cuid::cuid2());

        run_podman(&["create", "--name", &container, &self.image_tag])?;

        for export in exports {
            let mut dst = version_dir.clone();
            for item in export.path.trim_start_matches('/').split('/') {
                if !item.is_empty() {
                    dst = dst.join(item);
                }
            }
            std::fs::create_dir_all(&dst)?;

            let source = export.source.trim_matches('/');
            let src = format!("{container}:/app/{source}/.");

            match run_podman(&["cp", "-a", "--overwrite", &src, &dst.to_string_lossy()]) {
                Ok(_) => {
                    debug!("export {src} -> {} completed", dst.display());
                }
                Err(err) => {
                    warn!("export {src} failed: {err}");
                }
            }
        }

        // Always remove the temporary container
        let _ = run_podman(&["rm", &container]);

        Ok(())
    }

    pub fn remove(&self) {
        let _ = run_podman(&["rmi", &self.image_tag]);

        // Remove dangling images from local storage
        let _ = run_podman(&["image", "prune", "-f"]);
        let _ = run_podman(&["image", "prune", "-f", "--external"]);

        debug!("removed app image");
    }

    /// Remove this version's exported files from the static volume.
    pub fn remove_exports(&self) {
        // If the volume does not exist there is nothing to clean up.
        let Ok(exports_root) = volume_mountpoint(NGINX_WWW_VOLUME) else {
            return;
        };

        match remove_export_dir(&exports_root, self.name.as_str(), self.version) {
            Ok(true) => debug!("removed exports {}_{}", self.name, self.version),
            Ok(false) => {}
            Err(err) => warn!(
                "remove exports {}_{} failed: {err}",
                self.name, self.version
            ),
        }
    }
}

/// Remove a single `<name>_<version>` export dir. Returns whether it existed.
fn remove_export_dir(exports_root: &Path, name: &str, version: u32) -> io::Result<bool> {
    let dir = exports_root.join(format!("{name}_{version}"));

    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn remove_export_dir_removes_only_the_given_version() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        for name in ["web_1", "web_2", "webapp_1"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }

        assert!(remove_export_dir(root, "web", 1).unwrap());

        assert!(!root.join("web_1").exists());
        // Other versions and prefix-related units are untouched.
        assert!(root.join("web_2").exists());
        assert!(root.join("webapp_1").exists());
    }

    #[test]
    fn remove_export_dir_missing_is_ok() {
        let dir = TempDir::new().unwrap();
        assert!(!remove_export_dir(dir.path(), "web", 9).unwrap());
    }
}
