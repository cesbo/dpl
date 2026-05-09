use std::{
    fs,
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use super::artifacts::service_file_name;

const SYSTEMD_DIR: &str = "/etc/systemd/system";

pub fn install_service(unit_name: &str, deploy_dir: &Path) -> io::Result<()> {
    let service_name = service_file_name(unit_name);
    let src = deploy_dir.join("artifacts").join(&service_name);
    let dst = Path::new(SYSTEMD_DIR).join(&service_name);

    fs::copy(&src, &dst)?;

    if let Err(err) = run_systemctl(&["-q", "daemon-reload"]) {
        let _ = fs::remove_file(&dst);
        return Err(err);
    }

    if let Err(err) = run_systemctl(&["-q", "enable", "--now", &service_name]) {
        let _ = fs::remove_file(&dst);
        let _ = run_systemctl(&["-q", "daemon-reload"]);
        return Err(err);
    }

    Ok(())
}

fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("systemctl exited with {status}")))
    }
}
