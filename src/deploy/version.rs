use std::{
    fs,
    io,
    path::Path,
};

const VERSION_FILE_NAME: &str = "version.txt";

fn read_version(dir: &Path) -> io::Result<u32> {
    let path = dir.join(VERSION_FILE_NAME);

    let version = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            return Err(err);
        }
    };

    let version = match version.trim().parse::<u32>() {
        Ok(v) => v,
        Err(_) => return Err(io::ErrorKind::InvalidData.into()),
    };

    Ok(version)
}

fn write_version(dir: &Path, version: u32) -> io::Result<()> {
    let path = dir.join(VERSION_FILE_NAME);
    fs::write(path, version.to_string())
}
