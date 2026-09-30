use std::{
    fs::{
        self,
        File,
    },
    io::{
        self,
        BufWriter,
        Write,
    },
    time::Duration,
};

use anyhow::{
    Context,
    Result,
    bail,
};
use chrono::Utc;
use clap::Subcommand;
use dialoguer::Confirm;
use flate2::{
    Compression,
    write::GzEncoder,
};

use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        UnitConfig,
        db::{
            DbConfig,
            DbConnectionParams,
            DbServerConfig,
            wait_until_ready,
        },
    },
    log::success_mark,
    state::DeployState,
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
        /// Database unit name
        name: UnitName,
        /// Timeout in seconds
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
    /// Open an interactive SQL console to a database as its own login user,
    /// or to a db-server as the engine superuser
    Console {
        /// Database or db-server unit name
        name: UnitName,
        /// Connect as the engine superuser with the db-server's root password
        /// (implied for a db-server)
        #[arg(long)]
        root: bool,
    },
    /// Dump a database to a SQL file (or stdout) as its own login user
    Backup {
        /// Database unit name
        name: UnitName,
        /// Destination file, or `-` for stdout
        #[arg(default_value = "-")]
        path: String,
        /// Compress the dump with gzip (implied when the path ends in `.gz`)
        #[arg(short = 'z')]
        gzip: bool,
    },
    /// Drop a database, its login user, and local state (keeps config.yaml)
    Drop {
        /// Database unit name
        name: UnitName,
        /// Drop without interactive prompts
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Wait { name, timeout } => wait(ctx, &name, timeout),
        Cmd::Console { name, root } => console(ctx, &name, root),
        Cmd::Backup { name, path, gzip } => backup(ctx, &name, &path, gzip),
        Cmd::Drop { name, yes } => drop(ctx, &name, yes),
    }
}

fn wait(ctx: &MainContext, name: &UnitName, timeout_secs: u64) -> Result<()> {
    wait_until_ready(ctx, name, Duration::from_secs(timeout_secs))
        .with_context(|| format!("wait for database '{name}'"))
}

fn console(ctx: &MainContext, name: &UnitName, root: bool) -> Result<()> {
    let db_config = match UnitConfig::load(ctx, name)? {
        UnitConfig::Db(config) => config,
        UnitConfig::DbServer(server_config) => {
            let password = ctx.resolve_secret(&server_config.secret)?;
            return server_config
                .engine
                .console(name, server_config.engine.superuser(), &password, None)
                .with_context(|| format!("open console to '{name}'"));
        }
        _ => bail!("unit '{name}' is not a db or db-server"),
    };
    let server_config = load_db_server(ctx, &db_config.server)?;

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
        .console(&db_config.server, &user, &password, Some(name.as_str()))
        .with_context(|| format!("open console to '{name}'"))
}

fn backup(ctx: &MainContext, name: &UnitName, path: &str, gzip: bool) -> Result<()> {
    let db_config = load_db(ctx, name)?;
    let server_config = load_db_server(ctx, &db_config.server)?;
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
    let params = DbConnectionParams {
        server: &db_config.server,
        user: &db_config.user,
        password: &password,
        db_name: name.as_str(),
    };
    server_config
        .engine
        .dump(&params, &mut out, &mut on_stderr)
        .with_context(|| format!("back up database '{name}'"))?;

    out.flush().context("flush backup output")?;

    // Progress goes to stderr so a `-` dump keeps stdout clean for piping.
    if path != "-" {
        eprintln!("{} backed up '{name}' to {path}", success_mark());
    }

    Ok(())
}

fn drop(ctx: &MainContext, name: &UnitName, yes: bool) -> Result<()> {
    let db_config = load_db(ctx, name)?;
    let server_config = load_db_server(ctx, &db_config.server)?;

    let (_guard, _state) =
        DeployState::acquire(ctx, name).with_context(|| format!("acquire unit '{name}'"))?;

    if !yes {
        let confirm = Confirm::with_theme(&crate::cmd::prompt_theme())
            .with_prompt(format!("Drop database '{name}'?"))
            .default(false)
            .interact()?;

        if !confirm {
            eprintln!("Cancelled");
            return Ok(());
        }
    }

    let make_backup = yes
        || Confirm::with_theme(&crate::cmd::prompt_theme())
            .with_prompt("Create a backup before dropping?")
            .default(true)
            .interact()?;

    if make_backup {
        let stamp = Utc::now().format("%Y%m%d-%H%M%S");
        let backup_dir = ctx.backup_dir();
        fs::create_dir_all(&backup_dir)?;
        let path = backup_dir.join(format!("{name}-{stamp}.sql.gz"));
        let path = path.to_str().unwrap();
        backup(ctx, name, path, true)?;
    }

    // Drop the database and login user on the server. `IF EXISTS` keeps this
    // idempotent when the unit was never deployed (or already torn down).
    let root_password = ctx.resolve_secret(&server_config.secret)?;
    server_config
        .engine
        .drop_database(
            &db_config.server,
            &root_password,
            name.as_str(),
            &db_config.user,
        )
        .with_context(|| format!("drop database '{name}'"))?;

    let _ = fs::remove_dir_all(ctx.unit_state_dir(name));

    eprintln!("{} dropped database '{name}'", success_mark());

    Ok(())
}

fn load_db_server(ctx: &MainContext, name: &UnitName) -> Result<DbServerConfig> {
    let unit = UnitConfig::load(ctx, name)?;
    let UnitConfig::DbServer(config) = unit else {
        bail!("unit '{name}' is not a db-server");
    };
    Ok(config)
}

fn load_db(ctx: &MainContext, name: &UnitName) -> Result<DbConfig> {
    let unit = UnitConfig::load(ctx, name)?;
    let UnitConfig::Db(config) = unit else {
        bail!("unit '{name}' is not a db");
    };
    Ok(config)
}
