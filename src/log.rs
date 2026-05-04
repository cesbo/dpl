use std::{
    io,
    path::Path,
    sync::Mutex,
};

use tracing_subscriber::EnvFilter;

pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact();

    if std::env::var_os("JOURNAL_STREAM").is_some() {
        builder.without_time().init();
    } else {
        builder.init();
    }
}

pub fn init_tracing_log(path: &Path) -> io::Result<impl tracing::Subscriber> {
    let file = std::fs::OpenOptions::new().append(true).open(path)?;

    let subscriber = tracing_subscriber::fmt::Subscriber::builder()
        .json()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .with_current_span(false)
        .with_span_list(false)
        .finish();

    Ok(subscriber)
}
