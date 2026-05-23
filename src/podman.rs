use std::{
    io,
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
};

/// Podman volume for app static exports; mounts to `/var/www` in the nginx
/// container. Holds `<name>_<version>/…` directories at its root.
pub const NGINX_WWW_VOLUME: &str = "dpl-nginx-www";

/// Podman volume for nginx configs; mounts to `/etc/nginx/conf.d` in the nginx
/// container. Holds `<domain>.conf` files at its root.
pub const NGINX_CONF_VOLUME: &str = "dpl-nginx-conf";

/// Run podman and capture its trimmed stdout.
pub fn run_podman(args: &[&str]) -> io::Result<String> {
    let output = Command::new("podman")
        .args(args)
        .stderr(Stdio::null())
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(io::Error::other(format!(
            "podman exited with {}",
            output.status
        )))
    }
}

/// Create the named volume if it does not already exist.
pub fn ensure_volume(name: &str) -> io::Result<()> {
    if run_podman(&["volume", "exists", name]).is_err() {
        run_podman(&["volume", "create", name])?;
    }

    Ok(())
}

/// Resolve the host mountpoint of a named volume.
pub fn volume_mountpoint(name: &str) -> io::Result<PathBuf> {
    let mountpoint = run_podman(&["volume", "inspect", name, "--format", "{{.Mountpoint}}"])?;
    if mountpoint.is_empty() {
        return Err(io::Error::other(format!("volume {name} has no mountpoint")));
    }
    Ok(PathBuf::from(mountpoint))
}
