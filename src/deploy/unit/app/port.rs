use std::{
    fs,
    io,
    net::TcpListener,
    path::Path,
};

const PORT_FILE_NAME: &str = "port.txt";

fn find_free_port() -> io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()
        .map(|addr| addr.port())
}

/// get_port returns the persisted port for the unit, or picks a
/// random free port, persists it, and returns it.
pub fn get_port(dir: &Path) -> io::Result<u16> {
    match read_port(dir)? {
        Some(port) => Ok(port),
        None => {
            let port = find_free_port()?;
            let path = dir.join(PORT_FILE_NAME);
            fs::write(&path, port.to_string())?;
            Ok(port)
        }
    }
}

/// read_port reads the persisted port from `port.txt` without any
/// side effects. Returns `Ok(None)` when the file does not exist (unit
/// not yet deployed).
pub fn read_port(dir: &Path) -> io::Result<Option<u16>> {
    let path = dir.join(PORT_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(v) => v
            .trim()
            .parse::<u16>()
            .map(Some)
            .map_err(|_| io::ErrorKind::InvalidData.into()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}
