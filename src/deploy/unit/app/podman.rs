use std::{
    io::{
        self,
        BufRead,
    },
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use super::model::ExportConfig;
use crate::log::DeployLog;

pub struct PodmanContext<'a> {
    name: &'a str,
    version: u32,
    image_tag: String,
}

impl<'a> PodmanContext<'a> {
    pub fn new(name: &'a str, version: u32) -> Self {
        let image_tag = format!("localhost/{name}:{version}");
        Self {
            name,
            version,
            image_tag,
        }
    }

    /// Build podman image, streams output into the build log.
    pub fn build(&self, deploy_dir: &Path, log: &DeployLog) -> io::Result<()> {
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

        log.detail(&format!("running: podman build --tag {}", &self.image_tag));

        let mut child = cmd.spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stdout_log = log.clone();
        let stdout_handle = std::thread::spawn(move || {
            log_podman_output(stdout, &stdout_log);
        });

        let stderr = child.stderr.take().unwrap();
        let stderr_log = log.clone();
        let stderr_handle = std::thread::spawn(move || {
            log_podman_output(stderr, &stderr_log);
        });

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();

        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "podman build exited with {status}"
            )));
        }

        Ok(())
    }

    /// Export files from podman image into deploy directory
    pub fn export(
        &self,
        deploy_dir: &Path,
        exports: &[ExportConfig],
        log: &DeployLog,
    ) -> io::Result<()> {
        if exports.is_empty() {
            return Ok(());
        }

        let exports_dir = deploy_dir.join("exports");
        std::fs::create_dir(&exports_dir)?;

        let container = format!("dpl-export-{}", cuid::cuid2());

        run_podman(&["create", "--name", &container, &self.image_tag])?;

        for export in exports {
            if export.path == "/" {
                continue;
            }

            let mut dst = exports_dir.clone();
            for item in export.path.trim_start_matches('/').split('/') {
                if item.is_empty() {
                    continue;
                }
                dst = dst.join(item);
                std::fs::create_dir(&dst)?;
            }

            let source = export.source.trim_end_matches('/');
            let src = format!("{container}:{source}/.");

            match run_podman(&["cp", "-a", "--overwrite", &src, &dst.to_string_lossy()]) {
                Ok(_) => {
                    log.detail(&format!("export {src} completed"));
                }
                Err(err) => {
                    log.warn(&format!("export {src} failed: {err}"));
                }
            }
        }

        // Always remove the temporary container
        let _ = run_podman(&["rm", &container]);

        Ok(())
    }

    pub fn remove(&self, log: &DeployLog) {
        let _ = run_podman(&["rmi", &self.image_tag]);

        // Remove dangling images from local storage
        let _ = run_podman(&["image", "prune", "-f"]);
        let _ = run_podman(&["image", "prune", "-f", "--external"]);

        log.detail("removed app image");
    }
}

fn log_podman_output<R>(reader: R, log: &DeployLog)
where
    R: io::Read,
{
    let reader = io::BufReader::new(reader);

    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        log.podman_line(&line);
    }
}

pub fn run_podman(args: &[&str]) -> io::Result<()> {
    let status = Command::new("podman")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("podman exited with {status}")))
    }
}
