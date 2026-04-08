use std::{
    io,
    path::Path,
    sync::Mutex,
};

use tracing_subscriber::EnvFilter;

pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}

pub fn init_tracing_log(path: &Path) -> io::Result<impl tracing::Subscriber> {
    let file = std::fs::OpenOptions::new().append(true).open(&path)?;

    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .finish();

    Ok(subscriber)
}
