mod artifacts;
mod model;
mod port;

use std::{
    io::{
        self,
        BufRead,
    },
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Stdio,
    },
    sync::Mutex,
};

use artifacts::ArtifactsContext;
use model::AppConfig;
use tokio::{
    fs,
    io::{
        AsyncRead,
        AsyncWriteExt,
    },
};
use tracing::{
    error,
    info,
};

use crate::{
    deploy::{
        DeployError,
        load_entity_config,
    },
    error::ConfigError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub name: String,
    pub entity_dir: PathBuf,
    pub config: AppConfig,
}

impl AppEntity {
    pub async fn load(name: &str, entity_dir: &Path) -> Result<Self, ConfigError> {
        let config: AppConfig = load_entity_config(entity_dir).await?;

        Ok(AppEntity {
            name: name.into(),
            entity_dir: entity_dir.into(),
            config,
        })
    }

    pub async fn prepare<R>(&self, version: u32, archive: R) -> Result<(), DeployError>
    where
        R: AsyncRead + Unpin + Send,
    {
        let deploy_dir = self.entity_dir.join(format!("deploy_{version}"));

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        let log_dir = deploy_dir.join("log");
        fs::create_dir(&log_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create log directory".to_string(),
                source,
            })?;

        let build_log = log_dir.join("build.log");
        fs::File::create(&build_log)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create build.log".to_string(),
                source,
            })?;

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to save archive".to_string(),
                source,
            })?;

        let port =
            port::get_port(&self.entity_dir)
                .await
                .map_err(|source| DeployError::EntityError {
                    info: "failed to get port".to_string(),
                    source,
                })?;

        let artifacts = ArtifactsContext {
            name: &self.name,
            config: &self.config,
            version,
            port,
        };
        artifacts.save(&deploy_dir).await?;

        Ok(())
    }

    pub fn build(self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.entity_dir.join(format!("deploy_{version}"));
        let log_path = deploy_dir.join("log").join("build.log");

        let log_file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();

        let subscriber = build_log_subscriber(log_file);
        tracing::subscriber::with_default(subscriber, || {
            match do_build(&self.name, &deploy_dir, version) {
                Ok(_) => {
                    info!("app image built successfully");
                    Ok(())
                }
                Err(err) => {
                    error!(error = %err, "app image build failed");
                    Err(err)
                }
            }
        })
    }
}

fn build_log_subscriber(file: std::fs::File) -> impl tracing::Subscriber {
    tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .finish()
}

fn do_build(name: &str, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
    info!("build started for {}", name);

    let archive_path = deploy_dir.join("app.tar.gz");
    let app_dir = deploy_dir.join("app");

    crate::archive::extract(&archive_path, &app_dir)?;

    info!("archive extracted");

    podman_build(name, deploy_dir, version).map_err(|source| DeployError::EntityError {
        info: "failed to build image".to_string(),
        source,
    })?;

    Ok(())
}

fn podman_build(name: &str, deploy_dir: &Path, version: u32) -> io::Result<()> {
    let image_tag = format!("localhost/{name}:{version}");
    let containerfile = deploy_dir.join("containerfile");

    let mut cmd = Command::new("podman");
    cmd.arg("build")
        .arg("--rm")
        .arg("--force-rm")
        .arg("--no-cache");

    let mut secrets: Vec<_> = std::fs::read_dir(deploy_dir)?
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
        .arg(&image_tag)
        .arg(deploy_dir);

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    info!("running: podman build --tag {image_tag}");

    let mut child = cmd.spawn()?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let stdout_handle = std::thread::spawn(move || {
        let reader = io::BufReader::new(stdout);
        for line in reader.lines().flatten() {
            info!(target: "podman_build", "{line}");
        }
    });

    let stderr_handle = std::thread::spawn(move || {
        let reader = io::BufReader::new(stderr);
        for line in reader.lines().flatten() {
            info!(target: "podman_build", "{line}");
        }
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

async fn save_archive<R>(archive: R, dst: &Path) -> io::Result<()>
where
    R: AsyncRead + Unpin + Send,
{
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst).await?;
    tokio::io::copy(&mut reader, &mut archive_file).await?;
    archive_file.flush().await?;
    Ok(())
}
