use std::{
    fs,
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
    theme::ColorfulTheme,
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
            UnitConfigError,
            db::{
                DbConfig,
                DbServerConfig,
                DbServerEngine,
            },
            list_units,
        },
    },
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
        None => prompt_name(ctx)?,
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
        None => super::secret::prompt_secret(ctx)?,
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
        unit_name.as_str(),
        config.engine,
        &config.version,
        &root_password,
    )
    .with_context(|| format!("create serivce file for db-server '{unit_name}'"))?;

    crate::systemd::reload().context("reload systemd")?;
    crate::systemd::enable_service(&service_name)
        .with_context(|| format!("start service for db-server '{unit_name}'"))?;

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
) -> Result<()> {
    let db_name = match name {
        Some(value) => check_unit_name(ctx, &value)?,
        None => prompt_name(ctx)?,
    };

    let (server_name, server_config) = match db_server {
        Some(value) => load_db_server(ctx, &value)?,
        None => prompt_db_server(ctx)?,
    };

    let user = match user {
        Some(value) => {
            let value = value.trim().to_owned();
            ensure!(
                validate::resource_name(&value),
                "invalid user name '{value}'"
            );
            value
        }
        None => db_name.to_string(),
    };

    let secret_name = match secret_name {
        Some(value) => SecretName::new(value)?,
        None => super::secret::prompt_secret(ctx)?,
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
            config.server.as_str(),
            &root_password,
            db_name.as_str(),
            &config.user,
            &password,
        )
        .with_context(|| format!("create database '{}' in '{}'", db_name, &config.server))?;

    scopeguard::ScopeGuard::into_inner(unit_dir);

    println!(
        "created database '{db_name}' in '{server}' (user '{user}')",
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
            .ping(db_config.server.as_str(), &root_password, name);

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

fn resolve_secret(ctx: &MainContext, name: &SecretName) -> Result<String> {
    ctx.resolve_secret(name)
        .with_context(|| format!("resolve secret '{name}'"))
}

/// Validates the unit name format and checks its presence.
fn check_unit_name(ctx: &MainContext, name: &str) -> Result<ResourceName> {
    let unit_name = ResourceName::new(name)?;
    match UnitConfig::load(ctx, &unit_name) {
        Ok(_) => bail!("unit '{unit_name}' already exists"),
        Err(UnitConfigError::NotFound { .. }) => Ok(unit_name),
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

fn prompt_name(ctx: &MainContext) -> Result<ResourceName> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Unit name")
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

    ensure!(servers.is_empty(), "no db-server units");

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

fn parse_engine(value: &str) -> Result<DbServerEngine> {
    ENGINES
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, engine)| *engine)
        .ok_or_else(|| anyhow!("unsupported engine '{value}'"))
}

fn prompt_engine() -> Result<DbServerEngine> {
    let labels: Vec<&str> = ENGINES.iter().map(|(name, _)| *name).collect();
    let index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Database engine")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(ENGINES[index].1)
}

fn prompt_version(engine: DbServerEngine) -> Result<String> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
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
