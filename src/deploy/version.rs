use std::{
    fs,
    io,
    path::Path,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("read version file: {0}")]
    Read(io::Error),
    #[error("write version file: {0}")]
    Write(io::Error),
    #[error("entity version overflow")]
    Overflow,
}

const VERSION_FILE_NAME: &str = "version.txt";

pub fn get_entity_version(dir: &Path) -> Result<u32, VersionError> {
    let path = dir.join(VERSION_FILE_NAME);

    let version = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(VersionError::Read(err));
        }
    };

    let version = match version.trim().parse::<u32>() {
        Ok(v) => v,
        Err(_) => {
            return Err(VersionError::Read(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid version format",
            )));
        }
    };

    Ok(version)
}

pub fn write_entity_version(dir: &Path, version: u32) -> Result<(), VersionError> {
    let path = dir.join(VERSION_FILE_NAME);
    fs::write(path, version.to_string()).map_err(VersionError::Write)
}

pub fn reserve_entity_version(dir: &Path) -> Result<u32, VersionError> {
    let current_version = get_entity_version(dir)?;
    let next_version = current_version
        .checked_add(1)
        .ok_or(VersionError::Overflow)?;

    write_entity_version(dir, next_version)?;

    Ok(next_version)
}
