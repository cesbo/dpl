use std::{
    error::Error,
    fs,
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use clap::Subcommand;
use dialoguer::{
    Confirm,
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
        DbUnit,
        UnitConfig,
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
    }
}

fn init(
    ctx: &MainContext,
    name: Option<String>,
    engine: Option<String>,
    version: Option<String>,
    secret_name: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let name = match name {
        Some(value) => {
            validate_unit_name(ctx, &value)?;
            value
        }
        None => prompt_name(ctx)?,
    };

    let unit_dir = ctx.base().join(&name);

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
        None => prompt_secret(ctx)?,
    };

    let config = DbServerConfig {
        engine,
        version,
        secret: secret_name,
    };

    fs::create_dir_all(&unit_dir)?;

    if let Err(err) = write_unit_config(&unit_dir, "db-server", &config) {
        let _ = fs::remove_dir_all(&unit_dir);
        return Err(err);
    }

    let unit = DbServerUnit::new(ctx, name.clone(), config.clone());
    unit.init()?;

    println!(
        "started db unit '{name}' ({} {})",
        config.engine.as_str(),
        config.version
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
    let name = match name {
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
        None => name.clone(),
    };

    let secret_name = match secret_name {
        Some(value) => {
            validate_secret(ctx, &value)?;
            value
        }
        None => prompt_secret(ctx)?,
    };

    let config = DbConfig {
        server: server_name,
        user,
        secret: secret_name,
    };

    let unit_dir = ctx.base().join(&name);
    fs::create_dir_all(&unit_dir)?;

    if let Err(err) = write_unit_config(&unit_dir, "db", &config) {
        let _ = fs::remove_dir_all(&unit_dir);
        return Err(err);
    }

    let unit = DbUnit::new(ctx, name.clone(), config.clone(), server_config);
    if let Err(err) = unit.create() {
        let _ = fs::remove_dir_all(&unit_dir);
        return Err(err.into());
    }

    println!(
        "created database '{name}' in '{}' (user '{}')",
        config.server, config.user
    );
    Ok(())
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
    unit_dir: &Path,
    kind: &'static str,
    config: &C,
) -> Result<(), Box<dyn Error>> {
    #[derive(Serialize)]
    struct UnitFile<'a, C: Serialize> {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        config: &'a C,
    }

    let payload = UnitFile { kind, config };
    let yaml = serde_yaml::to_string(&payload)?;
    let path = unit_dir.join("config.yaml");
    fs::write(&path, yaml)?;
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

    let index = Select::with_theme(&ColorfulTheme::default())
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

fn prompt_secret(ctx: &MainContext) -> Result<String, Box<dyn Error>> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Secret name")
            .interact_text()?;

        if !validate::secret_name(&value) {
            eprintln!("{}", secret::SecretError::InvalidName);
            continue;
        }

        if ctx.secret_exists(&value) {
            return Ok(value);
        }

        let create = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("secret '{value}' does not exist — create it now?"))
            .default(true)
            .interact()?;
        if !create {
            continue;
        }

        let key = secret_cmd::load_or_create_key(ctx)?;
        let text = secret_cmd::prompt_value_or_random()?;
        key.encrypt_to_file(&value, &text)?;
        return Ok(value);
    }
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
