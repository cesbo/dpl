use std::{
    fs::{
        self,
        File,
    },
    io::{
        self,
        BufRead,
        BufReader,
        BufWriter,
        Read,
        Write,
    },
    path::Path,
    process::{
        Command,
        Stdio,
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
    anyhow,
    bail,
    ensure,
};
use clap::Subcommand;
use dialoguer::{
    FuzzySelect,
    Input,
    Select,
};
use flate2::{
    Compression,
    read::GzDecoder,
    write::GzEncoder,
};

use crate::{
    MainContext,
    config::{
        ResourceName,
        SecretName,
    },
    deploy::{
        UnitConfig,
        unit::{
            db::{
                DbConfig,
                DbServerConfig,
                DbServerEngine,
            },
            list_units,
        },
    },
};

const ENGINES: &[(&str, DbServerEngine)] = &[
    ("postgresql", DbServerEngine::Postgresql),
    ("mariadb", DbServerEngine::Mariadb),
];

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Bring up a containerized DBMS unit (renders artifacts, creates a podman
    /// secret, installs and starts the systemd service)
    Init {
        /// Unit name
        name: Option<String>,
        /// Database engine
        #[arg(long)]
        engine: Option<String>,
        /// Engine version (e.g. `18-alpine`)
        #[arg(long)]
        version: Option<String>,
        /// Name of an existing dpl secret holding the root password
        #[arg(long)]
        secret: Option<String>,
    },
    /// Create a database + login user inside a running db-server unit
    Create {
        /// Database (and unit) name
        name: Option<String>,
        /// Name of an existing db-server unit
        #[arg(long = "db-server")]
        db_server: Option<String>,
        /// SQL user name (defaults to the db name)
        #[arg(long)]
        user: Option<String>,
        /// Name of an existing dpl secret holding the new user's password
        #[arg(long)]
        secret: Option<String>,
    },
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
    /// Replay a SQL dump into a database from a file (or stdin) as its login user
    Restore {
        /// Database (and unit) name
        name: String,
        /// Source file, or `-` for stdin
        #[arg(default_value = "-")]
        path: String,
    },
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Init {
            name,
            engine,
            version,
            secret,
        } => init(ctx, name, engine, version, secret),
        Cmd::Create {
            name,
            db_server,
            user,
            secret,
        } => create(ctx, name, db_server, user, secret),
        Cmd::Wait { name, timeout } => wait(ctx, &name, timeout),
        Cmd::Console { name, root } => console(ctx, &name, root),
        Cmd::Backup { name, path, gzip } => backup(ctx, &name, &path, gzip),
        Cmd::Restore { name, path } => restore(ctx, &name, &path),
    }
}

fn init(
    ctx: &MainContext,
    name: Option<String>,
    engine: Option<String>,
    version: Option<String>,
    secret_name: Option<String>,
) -> Result<()> {
    let unit_name = match name {
        Some(value) => check_unit_name(ctx, &value)?,
        None => prompt_name(ctx, "Server name")?,
    };

    let engine = match engine {
        Some(value) => parse_engine(&value)?,
        None => prompt_engine()?,
    };

    let version = match version {
        Some(value) => {
            let value = value.trim().to_owned();
            ensure!(!value.is_empty(), "version must not be empty");
            check_image_exists(&engine.image(&value))?;
            value
        }
        None => prompt_version(engine)?,
    };

    let secret_name = match secret_name {
        Some(value) => SecretName::new(value)?,
        None => super::secret::prompt_secret(ctx, "Secret for the root user")?,
    };

    let config = DbServerConfig {
        engine,
        version,
        secret: secret_name,
    };

    let root_password = resolve_secret(ctx, &config.secret)?;

    let unit_dir = scopeguard::guard(unit_name.unit_dir(ctx), |unit_dir| {
        let _ = fs::remove_dir_all(unit_dir);
    });

    UnitConfig::DbServer(config.clone())
        .save(ctx, &unit_name)
        .with_context(|| format!("write unit '{unit_name}' config"))?;

    let service_name = crate::deploy::unit::db::create_service_file(
        Path::new(crate::systemd::SYSTEMD_DIR),
        &unit_name,
        config.engine,
        &config.version,
        &root_password,
    )
    .with_context(|| format!("create serivce file for db-server '{unit_name}'"))?;

    crate::systemd::reload().context("reload systemd")?;
    crate::spinner::Spinner::run(
        format!(
            "starting db-server '{unit_name}' ({} {})",
            config.engine.as_str(),
            &config.version
        ),
        |_| crate::systemd::enable_service(&service_name),
    )
    .with_context(|| format!("start service for db-server '{unit_name}'"))?;

    scopeguard::ScopeGuard::into_inner(unit_dir);

    println!(
        "{} Started db server '{unit_name}' ({engine} {version})",
        console::style("✓").green(),
        engine = config.engine.as_str(),
        version = &config.version
    );

    Ok(())
}

fn create(
    ctx: &MainContext,
    name: Option<String>,
    db_server: Option<String>,
    user: Option<String>,
    secret_name: Option<String>,
) -> Result<()> {
    let db_name = match name {
        Some(value) => check_unit_name(ctx, &value)?,
        None => prompt_name(ctx, "Database name")?,
    };

    let (server_name, server_config) = match db_server {
        Some(value) => load_db_server(ctx, &value)?,
        None => prompt_db_server(ctx)?,
    };

    let user = match user {
        Some(value) => value.trim().to_string(),
        None => db_name.to_string(),
    };

    let secret_name = match secret_name {
        Some(value) => SecretName::new(value)?,
        None => super::secret::prompt_secret(ctx, "Secret for the database user")?,
    };

    let config = DbConfig {
        server: server_name,
        user,
        secret: secret_name,
    };

    let root_password = resolve_secret(ctx, &server_config.secret)?;
    let password = resolve_secret(ctx, &config.secret)?;

    let unit_dir = scopeguard::guard(db_name.unit_dir(ctx), |unit_dir| {
        let _ = fs::remove_dir_all(unit_dir);
    });

    UnitConfig::Db(config.clone())
        .save(ctx, &db_name)
        .with_context(|| format!("write unit '{db_name}' config"))?;

    server_config
        .engine
        .create_database(
            &config.server,
            &root_password,
            db_name.as_str(),
            &config.user,
            &password,
        )
        .with_context(|| format!("create database '{}' in '{}'", db_name, &config.server))?;

    scopeguard::ScopeGuard::into_inner(unit_dir);

    println!(
        "{} Created database '{db_name}' in '{server}' (user '{user}')",
        console::style("✓").green(),
        server = &config.server,
        user = &config.user,
    );

    Ok(())
}

fn wait(ctx: &MainContext, name: &str, timeout_secs: u64) -> Result<()> {
    let (_, db_config) = load_db(ctx, name)?;
    let (_, server_config) = load_db_server(ctx, db_config.server.as_str())?;

    let root_password = resolve_secret(ctx, &server_config.secret)?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_millis(800);

    loop {
        let result = server_config
            .engine
            .ping(&db_config.server, &root_password, name);

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
            resolve_secret(ctx, &server_config.secret)?,
        )
    } else {
        (
            db_config.user.clone(),
            resolve_secret(ctx, &db_config.secret)?,
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
    let password = resolve_secret(ctx, &db_config.secret)?;

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

    crate::spinner::Spinner::run(format!("backing up '{db_name}'"), |bar| {
        let mut on_stderr = crate::spinner::stderr_sink(bar);
        server_config.engine.dump(
            &db_config.server,
            &db_config.user,
            &password,
            db_name.as_str(),
            &mut out,
            &mut on_stderr,
        )
    })
    .with_context(|| format!("back up database '{db_name}'"))?;

    out.flush().context("flush backup output")?;

    // Progress goes to stderr so a `-` dump keeps stdout clean for piping.
    if path != "-" {
        eprintln!("backed up '{db_name}' to {path}");
    }

    Ok(())
}

fn restore(ctx: &MainContext, name: &str, path: &str) -> Result<()> {
    let (db_name, db_config) = load_db(ctx, name)?;
    let (_, server_config) = load_db_server(ctx, db_config.server.as_str())?;
    let password = resolve_secret(ctx, &db_config.secret)?;

    let raw: Box<dyn Read> = if path == "-" {
        Box::new(io::stdin().lock())
    } else {
        let file = File::open(path).with_context(|| format!("open backup file '{path}'"))?;
        Box::new(file)
    };

    // Peek the gzip magic bytes so a `.gz` (or piped gzip) source is decoded
    // transparently; the peeked bytes stay buffered for whichever reader wraps it.
    let mut reader = BufReader::new(raw);
    let gzipped = reader
        .fill_buf()
        .with_context(|| format!("read backup source '{path}'"))?
        .starts_with(&[0x1f, 0x8b]);

    let mut input: Box<dyn Read> = if gzipped {
        Box::new(GzDecoder::new(reader))
    } else {
        Box::new(reader)
    };

    crate::spinner::Spinner::run(format!("restoring '{db_name}'"), |bar| {
        let mut on_stderr = crate::spinner::stderr_sink(bar);
        server_config.engine.restore(
            &db_config.server,
            &db_config.user,
            &password,
            db_name.as_str(),
            &mut input,
            &mut on_stderr,
        )
    })
    .with_context(|| format!("restore database '{db_name}'"))?;

    let source = if path == "-" { "stdin" } else { path };
    eprintln!(
        "{} restored '{db_name}' from {source}",
        console::style("✓").green(),
    );

    Ok(())
}

fn resolve_secret(ctx: &MainContext, name: &SecretName) -> Result<String> {
    ctx.resolve_secret(name)
        .with_context(|| format!("resolve secret '{name}'"))
}

/// Validates the unit name format and checks its presence.
fn check_unit_name(ctx: &MainContext, name: &str) -> Result<ResourceName> {
    let unit_name = ResourceName::new(name)?;
    match UnitConfig::load(ctx, &unit_name) {
        Ok(_) => bail!("unit '{unit_name}' already exists"),
        Err(err) if err.is_not_found() => Ok(unit_name),
        Err(err) => Err(err.into()),
    }
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

fn prompt_name(ctx: &MainContext, prompt: &str) -> Result<ResourceName> {
    loop {
        let value: String = Input::with_theme(&crate::cmd::prompt_theme())
            .with_prompt(prompt)
            .interact_text()?;

        match check_unit_name(ctx, &value) {
            Ok(unit_name) => return Ok(unit_name),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn prompt_db_server(ctx: &MainContext) -> Result<(ResourceName, DbServerConfig)> {
    let servers: Vec<(ResourceName, DbServerConfig)> =
        list_units(ctx, |c| matches!(c, UnitConfig::DbServer(_)))
            .into_iter()
            .filter_map(|(name, cfg)| match cfg {
                UnitConfig::DbServer(server) => Some((name, server)),
                _ => None,
            })
            .collect();

    ensure!(!servers.is_empty(), "no db-server units");

    let labels: Vec<String> = servers
        .iter()
        .map(|(name, cfg)| format!("{name} ({} {})", cfg.engine.as_str(), cfg.version))
        .collect();

    let index = FuzzySelect::with_theme(&crate::cmd::prompt_theme())
        .with_prompt("Database server")
        .items(&labels)
        .default(0)
        .interact()?;

    Ok(servers.into_iter().nth(index).unwrap())
}

fn parse_engine(value: &str) -> Result<DbServerEngine> {
    ENGINES
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, engine)| *engine)
        .ok_or_else(|| anyhow!("unsupported engine '{value}'"))
}

fn prompt_engine() -> Result<DbServerEngine> {
    let labels: Vec<&str> = ENGINES.iter().map(|(name, _)| *name).collect();
    let index = Select::with_theme(&crate::cmd::prompt_theme())
        .with_prompt("Database engine")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(ENGINES[index].1)
}

fn prompt_version(engine: DbServerEngine) -> Result<String> {
    loop {
        let value: String = Input::with_theme(&crate::cmd::prompt_theme())
            .with_prompt("Engine version")
            .default(engine.default_version().to_string())
            .interact_text()?
            .trim()
            .to_owned();

        if value.is_empty() {
            eprintln!("version must not be empty");
            continue;
        }

        match check_image_exists(&engine.image(&value)) {
            Ok(()) => return Ok(value),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn check_image_exists(image: &str) -> Result<()> {
    let output = Command::new("podman")
        .args(["manifest", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("failed to run podman")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("image '{image}' not found: {err}", err = stderr.trim());
}
