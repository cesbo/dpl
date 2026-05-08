use std::{
    fs::{
        File,
        OpenOptions,
    },
    path::Path,
};

use fs4::fs_std::FileExt;

use super::DeployError;

const LOCK_FILE_NAME: &str = ".deploy.lock";

/// Holds an OS-level exclusive `flock` on `{unit_dir}/.deploy.lock` for the
/// lifetime of the value. The kernel releases the lock when the file
/// descriptor is closed, including on process crash.
pub struct BusyGuard(File);

impl BusyGuard {
    pub fn lock(unit_dir: &Path) -> Result<Self, DeployError> {
        let path = unit_dir.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| DeployError::UnitError {
                info: format!("open lock file {}", path.display()),
                source,
            })?;

        match file.try_lock_exclusive() {
            Ok(true) => Ok(BusyGuard(file)),
            Ok(false) => Err(DeployError::UnitBusy),
            Err(source) => Err(DeployError::UnitError {
                info: format!("flock {}", path.display()),
                source,
            }),
        }
    }
}
