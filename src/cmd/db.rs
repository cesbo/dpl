use std::{
    fs::File,
    io::{
        self,
        BufWriter,
        Write,
    },
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

use anyhow::{
    Context,
    Result,
    bail,
    ensure,
};
use clap::Subcommand;
use flate2::{
    Compression,
    write::GzEncoder,
};

use crate::{
    MainContext,
    config::ResourceName,
    deploy::{
        UnitConfig,
        unit::db::{
            DbConfig,
            DbServerConfig,
        },
    },
};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Wait until a database is reachable through its db-server's CLI
    Wait {
        /// Database (and unit) name
        name: String,
        /// Timeout in seconds
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    /// Open an interactive SQL console to a database as its own login user
    Console {
        /// Database (and unit) name
        name: String,
        /// Connect as the engine superuser with the db-server's root password
        #[arg(long)]
        root: bool,
    },
    /// Dump a database to a SQL file (or stdout) as its own login user
    Backup {
        /// Database (and unit) name
        name: String,
        /// Destination file, or `-` for stdout
        #[arg(default_value = "-")]
        path: String,
        /// Compress the dump with gzip (implied when the path ends in `.gz`)
        #[arg(short = 'z')]
        gzip: bool,
    },
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Wait { name, timeout } => wait(ctx, &name, timeout),
        Cmd::Console { name, root } => console(ctx, &name, root),
        Cmd::Backup { name, path, gzip } => backup(ctx, &name, &path, gzip),
    }
}

fn wait(ctx: &MainContext, name: &str, timeout_secs: u64) -> Result<()> {
    let (_, db_config) = load_db(ctx, name)?;
    let (_, server_config) = load_db_server(ctx, db_config.server.as_str())?;

    let root_password = ctx.resolve_secret(&server_config.secret)?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_millis(800);

    loop {
        let result = server_config
            .engine
            .ping(&db_config.server, &root_password, Some(name));

        if result.is_ok() {
            return Ok(());
        }

        ensure!(
            Instant::now() < deadline,
            "timeout waiting for database '{name}'"
        );

        sleep(interval);
    }
}

fn console(ctx: &MainContext, name: &str, root: bool) -> Result<()> {
    let (db_name, db_config) = load_db(ctx, name)?;
    let (_, server_config) = load_db_server(ctx, db_config.server.as_str())?;

    let (user, password) = if root {
        (
            server_config.engine.superuser().to_string(),
            ctx.resolve_secret(&server_config.secret)?,
        )
    } else {
        (
            db_config.user.clone(),
            ctx.resolve_secret(&db_config.secret)?,
        )
    };

    server_config
        .engine
        .console(&db_config.server, &user, &password, db_name.as_str())
        .with_context(|| format!("open console to '{db_name}'"))
}

fn backup(ctx: &MainContext, name: &str, path: &str, gzip: bool) -> Result<()> {
    let (db_name, db_config) = load_db(ctx, name)?;
    let (_, server_config) = load_db_server(ctx, db_config.server.as_str())?;
    let password = ctx.resolve_secret(&db_config.secret)?;

    let raw: Box<dyn Write> = if path == "-" {
        Box::new(io::stdout().lock())
    } else {
        let file = File::create(path).with_context(|| format!("create backup file '{path}'"))?;
        Box::new(BufWriter::new(file))
    };

    // Compress when asked explicitly, or inferred from a `.gz` destination.
    let mut out: Box<dyn Write> = if gzip || path.ends_with(".gz") {
        Box::new(GzEncoder::new(raw, Compression::default()))
    } else {
        raw
    };

    let mut on_stderr = |line: &[u8]| {
        let mut err = io::stderr().lock();
        let _ = err.write_all(line);
        let _ = err.write_all(b"\n");
    };
    server_config
        .engine
        .dump(
            &db_config.server,
            &db_config.user,
            &password,
            db_name.as_str(),
            &mut out,
            &mut on_stderr,
        )
        .with_context(|| format!("back up database '{db_name}'"))?;

    out.flush().context("flush backup output")?;

    // Progress goes to stderr so a `-` dump keeps stdout clean for piping.
    if path != "-" {
        eprintln!("backed up '{db_name}' to {path}");
    }

    Ok(())
}

fn load_db_server(ctx: &MainContext, name: &str) -> Result<(ResourceName, DbServerConfig)> {
    let unit_name = ResourceName::new(name)?;
    let unit = UnitConfig::load(ctx, &unit_name)?;
    let UnitConfig::DbServer(config) = unit else {
        bail!("unit '{unit_name}' is not a db-server");
    };
    Ok((unit_name, config))
}

fn load_db(ctx: &MainContext, name: &str) -> Result<(ResourceName, DbConfig)> {
    let unit_name = ResourceName::new(name)?;
    let unit = UnitConfig::load(ctx, &unit_name)?;
    let UnitConfig::Db(config) = unit else {
        bail!("unit '{unit_name}' is not a db");
    };
    Ok((unit_name, config))
}
