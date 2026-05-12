use std::{
    error::Error,
    fs,
    io,
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

use clap::Subcommand;
use dialoguer::{
    FuzzySelect,
    Input,
    Select,
    theme::ColorfulTheme,
};
use serde::Serialize;

use crate::{
    MainContext,
    cmd::secret as secret_cmd,
    deploy::{
        DbConfig,
        DbServerConfig,
        DbServerEngine,
        DbServerUnit,
        UnitConfig,
        acquire,
        list_units,
    },
    secret,
    validate,
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
}

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
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
    }
}

fn init(
    ctx: &MainContext,
    name: Option<String>,
    engine: Option<String>,
    version: Option<String>,
    secret_name: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let unit_name = match name {
        Some(value) => {
            validate_unit_name(ctx, &value)?;
            value
        }
        None => prompt_name(ctx)?,
    };

    let engine = match engine {
        Some(value) => parse_engine(&value)?,
        None => prompt_engine()?,
    };

    let version = match version {
        Some(value) => {
            if value.trim().is_empty() {
                return Err("version must not be empty".into());
            }
            check_image_exists(&engine.image(&value))?;
            value
        }
        None => prompt_version(engine)?,
    };

    let secret_name = match secret_name {
        Some(value) => {
            validate_secret(ctx, &value)?;
            value
        }
        None => secret_cmd::prompt_secret(ctx)?,
    };

    let config = DbServerConfig {
        engine,
        version,
        secret: secret_name,
    };

    let unit_dir = scopeguard::guard(ctx.base().join(&unit_name), |unit_dir| {
        let _ = fs::remove_dir_all(unit_dir);
    });

    write_unit_config(&unit_name, &unit_dir, "db-server", &config)?;

    let unit = DbServerUnit::new(ctx, unit_name.clone(), config.clone());
    let (_guard, state) = acquire(&unit_dir)?;
    unit.init(state)?;

    scopeguard::ScopeGuard::into_inner(unit_dir);

    println!(
        "started db server '{unit_name}' ({engine} {version})",
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
) -> Result<(), Box<dyn Error>> {
    let db_name = match name {
        Some(value) => {
            validate_unit_name(ctx, &value)?;
            value
        }
        None => prompt_name(ctx)?,
    };

    let (server_name, server_config) = match db_server {
        Some(value) => load_db_server(ctx, &value)?,
        None => prompt_db_server(ctx)?,
    };

    let user = match user {
        Some(value) => {
            if !validate::resource_name(&value) {
                return Err(format!("invalid user name '{value}'").into());
            }
            value
        }
        None => db_name.clone(),
    };

    let secret_name = match secret_name {
        Some(value) => {
            validate_secret(ctx, &value)?;
            value
        }
        None => secret_cmd::prompt_secret(ctx)?,
    };

    let config = DbConfig {
        server: server_name,
        user,
        secret: secret_name,
    };

    let root_password = ctx.resolve_secret(&server_config.secret).map_err(|err| {
        format!(
            "resolve db-server secret '{secret}': {err}",
            secret = &server_config.secret
        )
    })?;

    let password = ctx.resolve_secret(&config.secret).map_err(|err| {
        format!(
            "resolve db secret '{secret}': {err}",
            secret = &config.secret
        )
    })?;

    let unit_dir = scopeguard::guard(ctx.base().join(&db_name), |unit_dir| {
        let _ = fs::remove_dir_all(unit_dir);
    });

    write_unit_config(&db_name, &unit_dir, "db", &config)?;

    server_config
        .engine
        .create_database(
            &config.server,
            &root_password,
            &db_name,
            &config.user,
            &password,
        )
        .map_err(|err| {
            format!(
                "create database '{db_name}' in '{server}': {err}",
                server = &config.server
            )
        })?;

    scopeguard::ScopeGuard::into_inner(unit_dir);

    println!(
        "created database '{db_name}' in '{server}' (user '{user}')",
        server = &config.server,
        user = &config.user,
    );

    Ok(())
}

fn wait(ctx: &MainContext, name: &str, timeout_secs: u64) -> Result<(), Box<dyn Error>> {
    if !validate::resource_name(name) {
        return Err(format!("invalid unit name '{name}'").into());
    }

    let db_config = {
        let unit =
            UnitConfig::load(ctx, name).map_err(|err| format!("load db unit '{name}': {err}"))?;
        match unit {
            UnitConfig::Db(c) => c,
            _ => return Err(format!("unit '{name}' is not a database").into()),
        }
    };

    let server_config = {
        let server = &db_config.server;
        let unit = UnitConfig::load(ctx, server)
            .map_err(|err| format!("load db-server '{server}': {err}"))?;
        match unit {
            UnitConfig::DbServer(c) => c,
            _ => return Err(format!("unit '{}' is not a db-server", server).into()),
        }
    };

    let root_password = ctx
        .resolve_secret(&server_config.secret)
        .map_err(|err| format!("decrypt secret '{}': {err}", server_config.secret))?;

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let interval = Duration::from_millis(800);

    loop {
        let result = server_config
            .engine
            .ping(&db_config.server, &root_password, name);

        if result.is_ok() {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!("timeout waiting for database '{name}'").into());
        }

        sleep(interval);
    }
}

fn validate_unit_name(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::resource_name(name) {
        return Err(format!("invalid unit name '{name}'").into());
    }

    let unit_dir = ctx.base().join(name);
    if unit_dir.exists() {
        return Err(format!("unit '{name}' already exists").into());
    }

    Ok(())
}

fn write_unit_config<C: Serialize>(
    unit_name: &str,
    unit_dir: &Path,
    kind: &'static str,
    config: &C,
) -> Result<(), String> {
    #[derive(Serialize)]
    struct UnitFile<'a, C: Serialize> {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        config: &'a C,
    }

    fs::create_dir_all(unit_dir)
        .map_err(|err| format!("create unit '{unit_name}' directory: {err}"))?;

    let payload = UnitFile { kind, config };
    let path = unit_dir.join("config.yaml");
    crate::config::save_config(path, &payload)
        .map_err(|err| format!("write unit '{unit_name}' config: {err}"))?;

    Ok(())
}

fn load_db_server(
    ctx: &MainContext,
    name: &str,
) -> Result<(String, DbServerConfig), Box<dyn Error>> {
    let config =
        UnitConfig::load(ctx, name).map_err(|err| format!("load db-server '{name}': {err}"))?;
    match config {
        UnitConfig::DbServer(server) => Ok((name.to_string(), server)),
        _ => Err(format!("unit '{name}' is not a db-server").into()),
    }
}

fn prompt_db_server(ctx: &MainContext) -> Result<(String, DbServerConfig), Box<dyn Error>> {
    let servers: Vec<(String, DbServerConfig)> =
        list_units(ctx, |c| matches!(c, UnitConfig::DbServer(_)))
            .into_iter()
            .filter_map(|(name, cfg)| match cfg {
                UnitConfig::DbServer(server) => Some((name, server)),
                _ => None,
            })
            .collect();

    if servers.is_empty() {
        return Err("no db-server units found — run `dpl db init` first".into());
    }

    let labels: Vec<String> = servers
        .iter()
        .map(|(name, cfg)| format!("{name} ({} {})", cfg.engine.as_str(), cfg.version))
        .collect();

    let index = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Database server")
        .items(&labels)
        .default(0)
        .interact()?;

    Ok(servers.into_iter().nth(index).unwrap())
}

fn parse_engine(value: &str) -> Result<DbServerEngine, Box<dyn Error>> {
    ENGINES
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, engine)| *engine)
        .ok_or_else(|| format!("unsupported engine '{value}'").into())
}

fn prompt_name(ctx: &MainContext) -> Result<String, Box<dyn Error>> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Unit name")
            .interact_text()?;
        match validate_unit_name(ctx, &value) {
            Ok(()) => return Ok(value),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn prompt_engine() -> Result<DbServerEngine, Box<dyn Error>> {
    let labels: Vec<&str> = ENGINES.iter().map(|(name, _)| *name).collect();
    let index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Database engine")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(ENGINES[index].1)
}

fn prompt_version(engine: DbServerEngine) -> Result<String, Box<dyn Error>> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Engine version")
            .default(engine.default_version().to_string())
            .interact_text()?;
        if value.trim().is_empty() {
            eprintln!("version must not be empty");
            continue;
        }
        match check_image_exists(&engine.image(&value)) {
            Ok(()) => return Ok(value),
            Err(err) => eprintln!("{err}"),
        }
    }
}

fn check_image_exists(image: &str) -> Result<(), Box<dyn Error>> {
    let output = Command::new("podman")
        .args(["manifest", "inspect", image])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("failed to run podman: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("image '{image}' not found: {}", stderr.trim()).into())
}

fn validate_secret(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }
    if !ctx.secret_exists(name) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("secret '{name}' not found"),
        )
        .into());
    }
    Ok(())
}
