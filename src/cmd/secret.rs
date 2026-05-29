use std::{
    fs,
    io::{
        self,
        Read,
    },
};

use anyhow::{
    Context,
    Result,
    bail,
    ensure,
};
use clap::Subcommand;
use dialoguer::{
    Input,
    Password,
};
use rand::{
    Rng,
    distributions::Alphanumeric,
};

use crate::{
    MainContext,
    config::SecretName,
    secret::{
        self,
        SecretError,
    },
};

const RANDOM_SECRET_LEN: usize = 32;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create and store a new secret
    Create {
        /// Secret name (`name` or `group/name`); omit for an interactive prompt
        name: Option<SecretName>,
        /// Source: omit for an interactive prompt, "-" to read stdin, or a path to a file.
        /// On an empty interactive prompt a random secret is generated and printed.
        source: Option<String>,
    },
    /// Print a secret's plaintext to stdout
    Cat {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: SecretName,
    },
    /// Remove an encrypted secret
    Rm {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: SecretName,
    },
    /// List existing secret names
    Ls,
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Create { name, source } => create(ctx, name, source.as_deref()),
        Cmd::Cat { name } => cat(ctx, &name),
        Cmd::Rm { name } => rm(ctx, &name),
        Cmd::Ls => ls(ctx),
    }
}

fn create(ctx: &MainContext, name: Option<SecretName>, source: Option<&str>) -> Result<()> {
    let name = match name {
        Some(value) => {
            match ctx.check_secret(&value) {
                Ok(_) => bail!(SecretError::AlreadyExists {
                    name: value.to_string(),
                }),
                Err(SecretError::NotFound { .. }) => {}
                Err(err) => bail!(err),
            }
            value
        }
        None => prompt_name(ctx)?,
    };

    let key = load_or_create_key(ctx)?;
    let text = match source {
        Some(source) => read_external(source)?,
        None => prompt_value_or_random()?,
    };
    key.encrypt_to_file(&name, &text)
        .with_context(|| format!("save new secret '{name}' to file"))?;

    Ok(())
}

fn cat(ctx: &MainContext, name: &SecretName) -> Result<()> {
    let value = ctx.resolve_secret(name)?;
    println!("{value}");
    Ok(())
}

fn rm(ctx: &MainContext, name: &SecretName) -> Result<()> {
    secret::remove(ctx.base(), name)?;
    println!("secret '{}' removed", name);
    Ok(())
}

fn ls(ctx: &MainContext) -> Result<()> {
    let names = secret::list_secrets(ctx.base())?;
    if names.is_empty() {
        println!("No secrets found");
    } else {
        for name in names {
            println!("{name}");
        }
    }

    Ok(())
}

/// Load the base's master key, generating and persisting one if absent.
pub fn load_or_create_key(ctx: &MainContext) -> Result<secret::MasterKey> {
    match secret::MasterKey::load(ctx.base()) {
        Ok(key) => Ok(key),
        Err(secret::SecretError::KeyNotFound) => {
            let key = secret::MasterKey::generate(ctx.base());
            key.save().context("save new master key")?;
            Ok(key)
        }
        err => err.context("load master key"),
    }
}

fn read_external(source: &str) -> Result<String> {
    let buf = if source == "-" {
        let mut buf = Vec::new();
        io::stdin()
            .read_to_end(&mut buf)
            .context("read secret value from stdin")?;
        buf
    } else {
        fs::read(source).context("read secret value from file")?
    };

    let value = String::from_utf8(buf).context("secret value is not valid UTF8")?;
    ensure!(!value.is_empty(), "secret value is empty");

    Ok(value)
}

/// Interactive prompt for a secret value; empty input generates a random one.
pub fn prompt_value_or_random() -> Result<String> {
    let value = Password::with_theme(&crate::cmd::prompt_theme())
        .with_prompt("Secret value (empty = generate random)")
        .allow_empty_password(true)
        .interact()?;

    if value.is_empty() {
        let value = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(RANDOM_SECRET_LEN)
            .map(char::from)
            .collect();
        Ok(value)
    } else {
        Ok(value)
    }
}

/// Prompt for a new secret name, validating format and uniqueness.
fn prompt_name(ctx: &MainContext) -> Result<SecretName> {
    loop {
        let raw: String = Input::with_theme(&crate::cmd::prompt_theme())
            .with_prompt("Secret name")
            .interact_text()?;

        let name = match SecretName::new(raw) {
            Ok(name) => name,
            Err(err) => {
                eprintln!("{err}");
                continue;
            }
        };

        match ctx.check_secret(&name) {
            Ok(_) => {
                eprintln!(
                    "{}",
                    SecretError::AlreadyExists {
                        name: name.to_string(),
                    }
                );
                continue;
            }
            Err(SecretError::NotFound { .. }) => return Ok(name),
            Err(err) => {
                eprintln!("{err}");
                continue;
            }
        }
    }
}
