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
    ensure,
};
use clap::Subcommand;
use dialoguer::{
    Confirm,
    FuzzySelect,
    Input,
    Password,
    theme::ColorfulTheme,
};
use rand::{
    Rng,
    distributions::Alphanumeric,
};

use crate::{
    MainContext,
    secret,
    validate,
};

const RANDOM_SECRET_LEN: usize = 32;
const CREATE_NEW_SECRET: &str = "+ Create new secret";

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create and store a new secret
    Create {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: String,
        /// Source: omit for an interactive prompt, "-" to read stdin, or a path to a file.
        /// On an empty interactive prompt a random secret is generated and printed.
        source: Option<String>,
    },
    /// Print a secret's plaintext to stdout
    Cat {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: String,
    },
    /// Remove an encrypted secret
    Rm {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: String,
    },
    /// List existing secret names
    Ls,
}

pub fn run(ctx: &MainContext, args: Args) -> Result<()> {
    match args.cmd {
        Cmd::Create { name, source } => create(ctx, &name, source.as_deref()),
        Cmd::Cat { name } => cat(ctx, &name),
        Cmd::Rm { name } => rm(ctx, &name),
        Cmd::Ls => ls(ctx),
    }
}

fn create(ctx: &MainContext, name: &str, source: Option<&str>) -> Result<()> {
    super::check_secret_name(ctx, name, false)?;

    let key = load_or_create_key(ctx)?;
    let text = match source {
        Some(source) => read_external(source)?,
        None => prompt_value_or_random()?,
    };
    key.encrypt_to_file(name, &text)
        .with_context(|| format!("save new secret '{name}' to file"))?;

    Ok(())
}

fn cat(ctx: &MainContext, name: &str) -> Result<()> {
    super::check_secret_name(ctx, name, true)?;

    let key = secret::MasterKey::load(ctx.base()).with_context(|| "load master key")?;
    let plaintext = key.decrypt_from_file(name).with_context(|| "load secret")?;

    println!("{plaintext}");

    Ok(())
}

fn rm(ctx: &MainContext, name: &str) -> Result<()> {
    super::check_secret_name(ctx, name, true)?;

    secret::secret_rm(ctx.base(), name).context("remove secret")?;
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
    let value = Password::with_theme(&ColorfulTheme::default())
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

/// Pick an existing secret with a fuzzy selector, or create a new one inline.
pub fn prompt_secret(ctx: &MainContext) -> Result<String> {
    let names = secret::list_secrets(ctx.base())?;

    let mut items: Vec<&str> = names.iter().map(String::as_str).collect();
    items.push(CREATE_NEW_SECRET);

    let index = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Secret name")
        .items(&items)
        .default(0)
        .interact()?;

    if index < names.len() {
        Ok(names.into_iter().nth(index).unwrap())
    } else {
        create_new_secret(ctx)
    }
}

fn create_new_secret(ctx: &MainContext) -> Result<String> {
    loop {
        let name: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Secret name")
            .interact_text()?;

        if !validate::secret_name(&name) {
            eprintln!("{}", secret::SecretError::InvalidName);
            continue;
        }

        if ctx.secret_exists(&name) {
            return Ok(name);
        }

        let create = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("secret '{name}' does not exist - create it now?"))
            .default(true)
            .interact()?;

        if !create {
            continue;
        }

        let key = load_or_create_key(ctx)?;
        let text = prompt_value_or_random()?;
        key.encrypt_to_file(&name, &text)
            .with_context(|| format!("save new secret '{name}' to file"))?;

        return Ok(name);
    }
}
