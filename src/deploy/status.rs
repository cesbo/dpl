use std::{
    fs,
    io,
    path::Path,
};

use serde::Serialize;

const STATUS_FILE_NAME: &str = "status.txt";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    Idle,
    Building,
    Ready,
    Failed,
}

pub fn read_entity_status(dir: &Path) -> io::Result<DeployStatus> {
    let path = dir.join(STATUS_FILE_NAME);

    let line = match fs::read_to_string(path) {
        Ok(line) => line,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(DeployStatus::Idle),
        Err(err) => return Err(err),
    };

    let status = match line.trim() {
        "idle" => DeployStatus::Idle,
        "building" => DeployStatus::Building,
        "ready" => DeployStatus::Ready,
        "failed" => DeployStatus::Failed,
        _ => return Err(io::ErrorKind::InvalidData.into()),
    };

    Ok(status)
}

pub fn write_entity_status(dir: &Path, status: DeployStatus) -> io::Result<()> {
    let status = match status {
        DeployStatus::Idle => "idle",
        DeployStatus::Building => "building",
        DeployStatus::Ready => "ready",
        DeployStatus::Failed => "failed",
    };

    let path = dir.join(STATUS_FILE_NAME);

    fs::write(path, status)
}
