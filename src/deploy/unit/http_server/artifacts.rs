use std::{
    fs,
    path::Path,
};

use crate::artifacts::ArtifactError;

/// Global nginx config dropped into the unit's conf volume as `00-dpl.conf`
const GLOBAL_CONFIG: &str = include_str!("templates/00-dpl.conf");

/// Write the global `00-dpl.conf` into `conf_dir` (the root of the unit's conf
/// volume).
pub fn write_global_config(conf_dir: &Path) -> Result<(), ArtifactError> {
    let path = conf_dir.join("00-dpl.conf");
    fs::write(&path, GLOBAL_CONFIG).map_err(ArtifactError::Write)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn write_global_config_drops_file() {
        let temp_dir = tempdir().unwrap();
        let conf_dir = temp_dir.path();

        write_global_config(conf_dir).unwrap();

        let path = conf_dir.join("00-dpl.conf");
        assert!(path.exists());

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("ssl_session_cache"));
        assert!(body.contains("access_log /dev/stdout json;"));
    }
}
