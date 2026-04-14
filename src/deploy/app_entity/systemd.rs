use std::{
    fs,
    io::{
        self,
        Write,
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

pub struct SystemdContext<'a> {
    systemd_dir: &'a Path,
    name: &'a str,
}

impl<'a> SystemdContext<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            systemd_dir: Path::new("/etc/systemd/system"),
            name,
        }
    }

    pub fn install_app(&self, deploy_dir: &Path) -> io::Result<()> {
        let unit = format!("dpl--{}.service", self.name);
        let src = deploy_dir.join("artifacts").join(&unit);
        let dst = self.systemd_dir.join(&unit);

        fs::copy(&src, &dst)?;

        reload_systemd();

        if let Err(err) = run_systemctl(&["-q", "enable", "--now", &unit]) {
            let _ = fs::remove_file(&dst);
            reload_systemd();
            Err(err)
        } else {
            Ok(())
        }
    }

    pub fn uninstall_app(&self) {
        let prefix = format!("dpl--{}", self.name);

        let app_service = format!("{prefix}.service");
        let app_service_path = self.systemd_dir.join(&app_service);
        let _ = run_systemctl(&["-q", "disable", "--now", &app_service]);

        // Check if the container failed to stop
        if run_systemctl(&["-q", "is-failed", &app_service]).is_ok() {
            let _ = run_systemctl(&["-q", "reset-failed", &app_service]);
        }

        if let Err(err) = fs::remove_file(&app_service_path) {
            if err.kind() == io::ErrorKind::NotFound {
                info!("app service {prefix} not found");
            } else {
                error!("failed to remove app service {prefix}: {err}");
            }
        } else {
            info!("app service {prefix} removed");
            reload_systemd();
        }
    }

    pub fn install_timers(&self, deploy_dir: &Path) {
        let prefix = format!("dpl--{}--", self.name);
        let artifacts_dir = deploy_dir.join("artifacts");

        let mut timers: Vec<String> = list_timers(&artifacts_dir, &prefix);
        timers.retain(|p: &String| copy_timer(self.systemd_dir, &artifacts_dir, p));
        if timers.is_empty() {
            return;
        }

        reload_systemd();

        for prefix in &timers {
            let unit = format!("{prefix}.timer");
            match run_systemctl(&["-q", "enable", "--now", &unit]) {
                Ok(_) => {
                    info!("timer {unit} installed")
                }
                Err(err) => {
                    remove_timer(self.systemd_dir, prefix);
                    error!("failed to install timer {unit}: {err}");
                }
            }
        }
    }

    pub fn uninstall_timers(&self) {
        let prefix = format!("dpl--{}--", self.name);

        let timers = list_timers(self.systemd_dir, &prefix);
        if timers.is_empty() {
            return;
        }

        timers
            .iter()
            .for_each(|t| remove_timer(self.systemd_dir, t));
        reload_systemd();
    }

    /// Rewrites the installed service file
    pub fn set_restart_value(&self, value: &str) -> io::Result<()> {
        let unit = format!("dpl--{}.service", self.name);
        let path = self.systemd_dir.join(&unit);

        let mut in_service = false;
        let mut result = Vec::new();

        let content = fs::read_to_string(&path)?;
        for line in content.lines() {
            let line = line.trim();

            if line.starts_with('[') && line.ends_with(']') {
                in_service = line == "[Service]";
                result.push(line.to_string());
                continue;
            }

            if in_service && line.starts_with("Restart=") {
                result.push(format!("Restart={}", value));
                continue;
            }

            result.push(line.to_string());
        }
        let content = result.join("\n");

        let mut tmp = tempfile::NamedTempFile::new_in(self.systemd_dir)?;
        tmp.write_all(content.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&path)?;

        reload_systemd();

        Ok(())
    }
}

/// Lists all timer units in the specified directory with filenames starting with the given prefix
fn list_timers(dir: &Path, prefix: &str) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            error!("failed to read directory {}: {err}", dir.display());
            return Vec::new();
        }
    };

    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| n.starts_with(prefix))
                .and_then(|n| n.strip_suffix(".timer"))
                .map(|n| n.to_owned())
        })
        .collect()
}

/// Removes a timer and its associated service from the systemd.
fn remove_timer(systemd_dir: &Path, prefix: &str) {
    let timer_unit = format!("{prefix}.timer");
    let _ = run_systemctl(&["-q", "disable", "--now", &timer_unit]);
    let removed = remove_timer_unit(systemd_dir, &timer_unit);

    let timer_service = format!("{prefix}.service");
    let _ = run_systemctl(&["-q", "stop", &timer_service]);
    remove_timer_unit(systemd_dir, &timer_service);

    if removed {
        info!("timer {prefix} removed");
    }
}

/// Removes a timer unit file from the systemd.
/// Returns `true` if the file was successfully removed.
fn remove_timer_unit(systemd_dir: &Path, unit: &str) -> bool {
    let path = systemd_dir.join(unit);
    match fs::remove_file(&path) {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            info!("timer unit {unit} not found");
            false
        }
        Err(err) => {
            error!("failed to remove timer unit {unit}: {err}");
            false
        }
    }
}

/// Copies a timer and its associated service from the artifacts directory to systemd.
/// Returns `true` if both were successfully copied.
fn copy_timer(systemd_dir: &Path, artifacts_dir: &Path, prefix: &str) -> bool {
    let timer_service = format!("{prefix}.service");
    if !copy_timer_unit(systemd_dir, artifacts_dir, &timer_service) {
        return false;
    }

    let timer_unit = format!("{prefix}.timer");
    if !copy_timer_unit(systemd_dir, artifacts_dir, &timer_unit) {
        remove_timer_unit(systemd_dir, &timer_service);
        return false;
    }

    true
}

/// Copies a timer unit file from the artifacts directory to systemd.
/// Returns `true` if the file was successfully copied.
fn copy_timer_unit(systemd_dir: &Path, artifacts_dir: &Path, unit: &str) -> bool {
    let src = artifacts_dir.join(unit);
    let dst = systemd_dir.join(unit);
    match fs::copy(&src, &dst) {
        Ok(_) => true,
        Err(err) => {
            error!("failed to copy timer service {unit}: {err}");
            false
        }
    }
}

fn reload_systemd() {
    if let Err(err) = run_systemctl(&["-q", "daemon-reload"]) {
        error!("reload systemd: {}", err);
    }
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
