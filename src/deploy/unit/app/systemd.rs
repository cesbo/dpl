use std::{
    fs,
    io::{
        self,
        Write,
    },
    path::Path,
};

use tracing::{
    debug,
    error,
};

use crate::{
    config::ResourceName,
    systemd::{
        disable_service,
        enable_service,
        run_systemctl,
        stop_service,
    },
};

pub struct SystemdContext<'a> {
    systemd_dir: &'a Path,
    name: &'a ResourceName,
}

impl<'a> SystemdContext<'a> {
    pub fn new(name: &'a ResourceName) -> Self {
        Self {
            systemd_dir: Path::new(crate::systemd::SYSTEMD_DIR),
            name,
        }
    }

    pub fn install_app(&self, deploy_dir: &Path) -> io::Result<()> {
        let service_name = format!("{}.service", self.name.scoped_unit_name());
        let src = deploy_dir.join("artifacts").join(&service_name);
        let dst = self.systemd_dir.join(&service_name);

        fs::copy(&src, &dst)?;

        reload_systemd();

        if let Err(err) = enable_service(&service_name) {
            let _ = fs::remove_file(&dst);
            reload_systemd();
            Err(err)
        } else {
            Ok(())
        }
    }

    pub fn uninstall_app(&self) {
        let prefix = self.name.scoped_unit_name();

        let app_service = format!("{prefix}.service");
        let app_service_path = self.systemd_dir.join(&app_service);
        let _ = disable_service(&app_service);

        // Check if the container failed to stop
        if run_systemctl(&["-q", "is-failed", &app_service]).is_ok() {
            let _ = run_systemctl(&["-q", "reset-failed", &app_service]);
        }

        if let Err(err) = fs::remove_file(&app_service_path) {
            if err.kind() != io::ErrorKind::NotFound {
                error!("failed to remove service for unit {}: {err}", self.name);
            }
        } else {
            debug!("removed service for unit {}", self.name);
            reload_systemd();
        }
    }

    pub fn install_timers(&self, deploy_dir: &Path) {
        let prefix = format!("{}--", self.name.scoped_unit_name());
        let artifacts_dir = deploy_dir.join("artifacts");

        let mut timers: Vec<String> = list_timers(&artifacts_dir, &prefix);
        timers.retain(|p: &String| copy_timer(self.systemd_dir, &artifacts_dir, p));
        if timers.is_empty() {
            return;
        }

        reload_systemd();

        for prefix in &timers {
            let timer_name = format!("{prefix}.timer");
            match enable_service(&timer_name) {
                Ok(_) => {
                    debug!("timer {timer_name} installed");
                }
                Err(err) => {
                    remove_timer(self.systemd_dir, prefix);
                    error!("failed to install timer {timer_name}: {err}");
                }
            }
        }
    }

    pub fn uninstall_timers(&self) {
        let prefix = format!("{}--", self.name.scoped_unit_name());

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
        let service_name = format!("{}.service", self.name.scoped_unit_name());
        let path = self.systemd_dir.join(&service_name);

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

        // Reload without log access — only used internally during install_app,
        // which already logs its own reload.
        let _ = crate::systemd::reload();

        Ok(())
    }
}

/// Lists all timers in the specified directory with filenames starting with the given prefix
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
    let _ = disable_service(&timer_unit);
    let removed = remove_timer_file(systemd_dir, &timer_unit);

    let timer_service = format!("{prefix}.service");
    let _ = stop_service(&timer_service);
    remove_timer_file(systemd_dir, &timer_service);

    if removed {
        debug!("timer {prefix} removed");
    }
}

/// Removes a timer file from the systemd.
/// Returns `true` if the file was successfully removed.
fn remove_timer_file(systemd_dir: &Path, file_name: &str) -> bool {
    let path = systemd_dir.join(file_name);
    match fs::remove_file(&path) {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            debug!("timer service file {file_name} not found");
            false
        }
        Err(err) => {
            error!("failed to remove timer service file {file_name}: {err}");
            false
        }
    }
}

/// Copies a timer and its associated service from the artifacts directory to systemd.
/// Returns `true` if both were successfully copied.
fn copy_timer(systemd_dir: &Path, artifacts_dir: &Path, prefix: &str) -> bool {
    let timer_service = format!("{prefix}.service");
    if !copy_timer_file(systemd_dir, artifacts_dir, &timer_service) {
        return false;
    }

    let timer_unit = format!("{prefix}.timer");
    if !copy_timer_file(systemd_dir, artifacts_dir, &timer_unit) {
        remove_timer_file(systemd_dir, &timer_service);
        return false;
    }

    true
}

/// Copies a timer file from the artifacts directory to systemd.
/// Returns `true` if the file was successfully copied.
fn copy_timer_file(systemd_dir: &Path, artifacts_dir: &Path, file_name: &str) -> bool {
    let src = artifacts_dir.join(file_name);
    let dst = systemd_dir.join(file_name);
    match fs::copy(&src, &dst) {
        Ok(_) => true,
        Err(err) => {
            error!("failed to copy timer service {file_name}: {err}");
            false
        }
    }
}

fn reload_systemd() {
    if let Err(err) = crate::systemd::reload() {
        error!("reload systemd: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
    };

    use tempfile::tempdir;

    use super::SystemdContext;
    use crate::config::ResourceName;

    #[test]
    fn systemd_set_restart_value() {
        let tmp = tempdir().expect("create temp directory");
        let service_name = "dpl--demo.service";
        let service_path = tmp.path().join(service_name);

        let source = "[Unit]\nDescription=Demo\n\n[Service]\nRestart=no\nExecStart=/usr/bin/true\n";
        fs::write(&service_path, source).expect("write source service file");

        let name = ResourceName::new("demo").unwrap();
        let ctx = SystemdContext {
            systemd_dir: Path::new(tmp.path()),
            name: &name,
        };

        ctx.set_restart_value("always")
            .expect("rewrite Restart directive");

        let updated = fs::read_to_string(&service_path).expect("read updated service file");
        assert!(updated.contains("Restart=always"));
        assert!(!updated.contains("Restart=no"));
    }
}
