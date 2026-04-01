use std::io;

use tokio::net::TcpListener;

async fn select_port() -> io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))
        .await?
        .local_addr()
        .map(|addr| addr.port())
}
