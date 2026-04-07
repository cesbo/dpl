use std::{
    fs,
    io,
    path::Path,
};

use serde::Serialize;
use thiserror::Error;

const STATUS_FILE_NAME: &str = "status.txt";

#[derive(Debug, Error)]
pub enum StatusError {
    #[error("read status file: {0}")]
    Read(io::Error),
    #[error("write status file: {0}")]
    Write(io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    Idle,
    Building,
    Ready,
    Failed,
}

pub fn read_entity_status(dir: &Path) -> Result<DeployStatus, StatusError> {
    let path = dir.join(STATUS_FILE_NAME);

    let line = match fs::read_to_string(path) {
        Ok(line) => line,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(DeployStatus::Idle),
        Err(err) => return Err(StatusError::Read(err)),
    };

    let status = match line.trim() {
        "idle" => DeployStatus::Idle,
        "building" => DeployStatus::Building,
        "ready" => DeployStatus::Ready,
        "failed" => DeployStatus::Failed,
        _ => {
            return Err(StatusError::Read(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid status format",
            )));
        }
    };

    Ok(status)
}

pub fn write_entity_status(dir: &Path, status: DeployStatus) -> Result<(), StatusError> {
    let status = match status {
        DeployStatus::Idle => "idle",
        DeployStatus::Building => "building",
        DeployStatus::Ready => "ready",
        DeployStatus::Failed => "failed",
    };

    let path = dir.join(STATUS_FILE_NAME);

    fs::write(path, status).map_err(StatusError::Write)
}
