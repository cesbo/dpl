use std::{
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use super::model::ExportConfig;
use crate::{
    config::UnitName,
    log,
    log::cri_log::CriLog,
    podman::{
        NGINX_WWW_VOLUME,
        copy_image_dir_to_volume,
        podman_spawn_error,
        run_podman,
    },
};

pub struct PodmanContext<'a> {
    name: &'a UnitName,
    version: u32,
    image_tag: String,
}

impl<'a> PodmanContext<'a> {
    pub fn new(name: &'a UnitName, version: u32) -> Self {
        let image_tag = format!("localhost/{name}:{version}");
        Self {
            name,
            version,
            image_tag,
        }
    }

    pub fn build(&self, deploy_dir: &Path, log_path: &Path) -> io::Result<()> {
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

    /// Export files from the podman image into the static volume under
    /// `<name>_<version>/<path>/` at the volume root.
    pub fn export(&self, exports: &[ExportConfig]) -> io::Result<()> {
        if exports.is_empty() {
            return Ok(());
        }

        let version_dir = format!("{}_{}", self.name, self.version);

        for export in exports {
            let src = export.source.trim_end_matches('/');
            let dst = export_destination(&version_dir, &export.path);
            copy_image_dir_to_volume(&self.image_tag, src, NGINX_WWW_VOLUME, &dst)?;
        }

        Ok(())
    }

    pub fn remove(&self) {
        let _ = run_podman(&["rmi", &self.image_tag]);

        // Remove dangling images from local storage
        let _ = run_podman(&["image", "prune", "-f"]);
        let _ = run_podman(&["image", "prune", "-f", "--external"]);
    }

    /// Remove this version's exported files from the static volume.
    pub fn remove_exports(&self) {
        if run_podman(&["volume", "exists", NGINX_WWW_VOLUME]).is_err() {
            return;
        }

        let mount = export_mount_path();
        let volume_arg = format!("{NGINX_WWW_VOLUME}:{mount}");
        let dir = format!("{mount}/{}_{}", self.name, self.version);
        let script = format!("rm -rf -- '{dir}'");

        if let Err(err) = run_podman(&[
            "run",
            "--rm",
            "--volume",
            &volume_arg,
            &self.image_tag,
            "/bin/sh",
            "-c",
            &script,
        ]) {
            log::warn(format!(
                "remove exports {}_{} failed: {err}",
                self.name, self.version
            ));
        }
    }
}

fn export_mount_path() -> String {
    format!("/tmp/dpl-export-{}", cuid::cuid2())
}

fn export_destination(version_dir: &str, path: &str) -> String {
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
