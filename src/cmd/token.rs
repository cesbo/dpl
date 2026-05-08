use std::{
    error::Error,
    fs,
    io,
};

use clap::Subcommand;
use dialoguer::{
    Password,
    theme::ColorfulTheme,
};
use rand::{
    Rng,
    distributions::Alphanumeric,
};

use crate::{
    MainContext,
    auth::model::AuthEntry,
    validate,
};

const RANDOM_TOKEN_LEN: usize = 32;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add or replace a token
    Add {
        /// Token name (lowercase letters, digits, hyphens)
        name: String,
        /// Comma-separated allowed app names. Default: `*` (all apps).
        #[arg(long)]
        apps: Option<String>,
    },
    /// Remove a token
    Rm {
        /// Token name
        name: String,
    },
    /// List existing tokens
    Ls,
}

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Add { name, apps } => add(ctx, &name, apps.as_deref()),
        Cmd::Rm { name } => rm(ctx, &name),
        Cmd::Ls => ls(ctx),
    }
}

fn add(ctx: &MainContext, name: &str, apps_arg: Option<&str>) -> Result<(), Box<dyn Error>> {
    if !validate::resource_name(name) {
        return Err("invalid token name (use lowercase letters, digits, hyphens)".into());
    }

    let apps = parse_apps(apps_arg)?;

    let input = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Token value (empty = generate random)")
        .allow_empty_password(true)
        .interact()?;

    let (token, generated) = if input.is_empty() {
        (random_token(), true)
    } else {
        (input, false)
    };

    let entry = AuthEntry {
        token: token.clone(),
        apps,
    };

    let tokens_dir = ctx.base().join(".tokens");
    fs::create_dir_all(&tokens_dir)?;
    let path = tokens_dir.join(format!("{name}.yaml"));
    fs::write(&path, serde_yaml::to_string(&entry)?)?;

    println!("token '{name}' saved");
    if generated {
        println!("generated token: {token}");
    }

    Ok(())
}

fn rm(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::resource_name(name) {
        return Err("invalid token name (use lowercase letters, digits, hyphens)".into());
    }

    let path = ctx.base().join(".tokens").join(format!("{name}.yaml"));
    match fs::remove_file(&path) {
        Ok(()) => {
            println!("token '{name}' removed");
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("token '{name}' not found");
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

fn ls(ctx: &MainContext) -> Result<(), Box<dyn Error>> {
    let tokens_dir = ctx.base().join(".tokens");
    let entries = match fs::read_dir(&tokens_dir) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("No tokens found");
            return Ok(());
        }
        Err(err) => return Err(err.into()),
    };

    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().and_then(|s| s.strip_suffix(".yaml")) else {
            continue;
        };
        names.push(name.to_string());
    }

    if names.is_empty() {
        println!("No tokens found");
    } else {
        names.sort();
        for name in names {
            println!("{name}");
        }
    }

    Ok(())
}

fn parse_apps(arg: Option<&str>) -> Result<Vec<String>, Box<dyn Error>> {
    let Some(raw) = arg else {
        return Ok(vec!["*".into()]);
    };

    let mut apps = Vec::new();
    for item in raw.split(',') {
        let app = item.trim();
        if !validate::resource_name(app) {
            return Err(format!(
                "invalid app name '{app}' (use lowercase letters, digits, hyphens)"
            )
            .into());
        }
        apps.push(app.to_string());
    }

    if apps.is_empty() {
        return Err("--apps must list at least one app".into());
    }

    Ok(apps)
}

fn random_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(RANDOM_TOKEN_LEN)
        .map(char::from)
        .collect()
}
