use std::{
    error::Error,
    fs,
    io,
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
        DbEngine,
        DbUnit,
    },
    secret,
    validate,
};

const ENGINES: &[(&str, DbEngine)] = &[
    ("postgresql", DbEngine::Postgresql),
    ("mariadb", DbEngine::Mariadb),
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
        /// Cluster name — also the unit name and the running container name
        #[arg(long)]
        cluster: Option<String>,
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
}

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Init {
            cluster,
            engine,
            version,
            secret,
        } => init(ctx, cluster, engine, version, secret),
    }
}

fn init(
    ctx: &MainContext,
    cluster: Option<String>,
    engine: Option<String>,
    version: Option<String>,
    secret_name: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let cluster = match cluster {
        Some(value) => {
            if !validate::resource_name(&value) {
                return Err(format!("invalid cluster name '{value}'").into());
            }
            value
        }
        None => prompt_cluster()?,
    };

    let unit_dir = ctx.base().join(&cluster);
    if unit_dir.exists() {
        return Err(format!("unit directory already exists: {}", unit_dir.display()).into());
    }

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

    let config = DbConfig {
        engine,
        version,
        secret: secret_name,
    };

    fs::create_dir_all(&unit_dir)?;

    if let Err(err) = write_config(&unit_dir, &config) {
        let _ = fs::remove_dir_all(&unit_dir);
        return Err(err);
    }

    let unit = DbUnit::new(ctx, cluster.clone(), config.clone());
    unit.init()?;

    println!(
        "started db unit '{cluster}' ({} {})",
        config.engine.as_str(),
        config.version
    );
    Ok(())
}

fn write_config(unit_dir: &std::path::Path, config: &DbConfig) -> Result<(), Box<dyn Error>> {
    #[derive(Serialize)]
    struct UnitFile<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        #[serde(flatten)]
        config: &'a DbConfig,
    }

    let payload = UnitFile { kind: "db", config };
    let yaml = serde_yaml::to_string(&payload)?;
    let path = unit_dir.join("config.yaml");
    fs::write(&path, yaml)?;
    Ok(())
}

fn parse_engine(value: &str) -> Result<DbEngine, Box<dyn Error>> {
    ENGINES
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, engine)| *engine)
        .ok_or_else(|| format!("unsupported engine '{value}'").into())
}

fn prompt_cluster() -> Result<String, Box<dyn Error>> {
    loop {
        let value: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Cluster name")
            .interact_text()?;
        if validate::resource_name(&value) {
            return Ok(value);
        }
        eprintln!(
            "invalid cluster name; use lowercase letters, digits and hyphens (no leading, trailing or doubled '-')"
        );
    }
}

fn prompt_engine() -> Result<DbEngine, Box<dyn Error>> {
    let labels: Vec<&str> = ENGINES.iter().map(|(name, _)| *name).collect();
    let index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Database engine")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(ENGINES[index].1)
}

fn prompt_version(engine: DbEngine) -> Result<String, Box<dyn Error>> {
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
