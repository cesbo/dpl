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

use tracing::{
    error,
    info,
};

use super::model::ExportConfig;

pub struct PodmanContext<'a> {
    name: &'a str,
    deploy_dir: &'a Path,
    version: u32,
    image_tag: String,
}

impl<'a> PodmanContext<'a> {
    pub fn new(name: &'a str, deploy_dir: &'a Path, version: u32) -> Self {
        let image_tag = format!("localhost/{name}:{version}");
        Self {
            name,
            deploy_dir,
            version,
            image_tag,
        }
    }

    /// Build podman image
    pub fn build(&self) -> io::Result<()> {
        let artifacts_dir = self.deploy_dir.join("artifacts");
        let containerfile = artifacts_dir.join("containerfile");
        let dispatch = tracing::dispatcher::get_default(|dispatch| dispatch.clone());

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
            .arg(self.deploy_dir);

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        info!("running: podman build --tag {}", &self.image_tag);

        let mut child = cmd.spawn()?;

        let stdout = child.stdout.take().unwrap();
        let stdout_dispatch = dispatch.clone();
        let stdout_handle = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&stdout_dispatch, || {
                log_podman_output(stdout, "stdout");
            });
        });

        let stderr = child.stderr.take().unwrap();
        let stderr_dispatch = dispatch.clone();
        let stderr_handle = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&stderr_dispatch, || {
                log_podman_output(stderr, "stderr");
            });
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
    pub fn export(&self, exports: &[ExportConfig]) -> io::Result<()> {
        if exports.is_empty() {
            return Ok(());
        }

        let exports_dir = self.deploy_dir.join("exports");
        std::fs::create_dir(&exports_dir)?;

        let container = format!("dpl-export-{}", cuid::cuid2());

        let status = Command::new("podman")
            .args(["create", "--name", &container, &self.image_tag])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .status()?;

        if !status.success() {
            return Err(io::Error::other(format!(
                "podman create exited with {status}"
            )));
        }

        for export in exports {
            if export.url == "/" {
                continue;
            }

            let mut dst = exports_dir.clone();
            for item in export.url.trim_start_matches('/').split('/') {
                if item.is_empty() {
                    continue;
                }
                dst = dst.join(item);
                std::fs::create_dir(&dst)?;
            }

            let src = format!("{}:{}", container, export.source);

            let output = Command::new("podman")
                .args(["cp", "-a", "--overwrite", &src, &dst.to_string_lossy()])
                .output()?;

            if output.status.success() {
                info!("podman cp {} -> {} ok", &src, dst.display());
            } else {
                error!("podman cp {} -> {} failed", &src, dst.display());
            }
        }

        // Always remove the temporary container
        let _ = Command::new("podman")
            .args(["rm", &container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        Ok(())
    }
}

fn log_podman_output<R>(reader: R, stream: &'static str)
where
    R: io::Read,
{
    let reader = io::BufReader::new(reader);

    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        info!(target: "podman_build", stream, line);
    }
}
