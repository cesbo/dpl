use std::{
    io,
    path::Path,
};

use tokio::{
    fs,
    net::TcpListener,
};

const PORT_FILE_NAME: &str = "port.txt";

async fn find_free_port() -> io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))
        .await?
        .local_addr()
        .map(|addr| addr.port())
}

/// get_port returns the persisted host port for the unit, or picks a
/// random free port, persists it, and returns it.
pub async fn get_port(dir: &Path) -> io::Result<u16> {
    let path = dir.join(PORT_FILE_NAME);
    let port = match fs::read_to_string(&path).await {
        Ok(v) => Some(v),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err);
        }
    };

    if let Some(v) = port {
        v.trim()
            .parse::<u16>()
            .map_err(|_| io::ErrorKind::InvalidData.into())
    } else {
        let port = find_free_port().await?;
        fs::write(&path, port.to_string()).await?;
        Ok(port)
    }
}
