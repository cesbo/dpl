use std::{
    error::Error,
    fs,
    io::{
        self,
        Read,
    },
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

/// Load the base's master key, generating and persisting one if absent.
pub fn load_or_create_key(ctx: &MainContext) -> Result<secret::MasterKey, secret::SecretError> {
    match secret::MasterKey::load(ctx.base()) {
        Ok(key) => Ok(key),
        Err(secret::SecretError::KeyNotFound) => {
            let key = secret::MasterKey::generate(ctx.base());
            key.save()?;
            Ok(key)
        }
        Err(err) => Err(err),
    }
}

/// Interactive prompt for a secret value; empty input generates a random one.
pub fn prompt_value_or_random() -> Result<String, Box<dyn Error>> {
    let value = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Secret value (empty = generate random)")
        .allow_empty_password(true)
        .interact()?;

    Ok(if value.is_empty() {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(RANDOM_SECRET_LEN)
            .map(char::from)
            .collect()
    } else {
        value
    })
}

const CREATE_NEW_SECRET: &str = "+ Create new secret";

/// Pick an existing secret with a fuzzy selector, or create a new one inline.
/// Falls back to a plain Input prompt when no secrets exist yet.
pub fn prompt_secret(ctx: &MainContext) -> Result<String, Box<dyn Error>> {
    let names = secret::list_secrets(ctx.base())?;
    if names.is_empty() {
        return create_new_secret(ctx);
    }

    let mut items: Vec<&str> = names.iter().map(String::as_str).collect();
    items.push(CREATE_NEW_SECRET);

    let index = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Secret name")
        .items(&items)
        .default(0)
        .interact()?;

    if index < names.len() {
        return Ok(names.into_iter().nth(index).unwrap());
    }

    create_new_secret(ctx)
}

fn create_new_secret(ctx: &MainContext) -> Result<String, Box<dyn Error>> {
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

        let key = load_or_create_key(ctx)?;
        let text = prompt_value_or_random()?;
        key.encrypt_to_file(&value, &text)?;
        return Ok(value);
    }
}

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

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Create { name, source } => create(ctx, &name, source.as_deref()),
        Cmd::Cat { name } => cat(ctx, &name),
        Cmd::Rm { name } => rm(ctx, &name),
        Cmd::Ls => ls(ctx),
    }
}

fn create(ctx: &MainContext, name: &str, source: Option<&str>) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }

    if secret::secret_exists(ctx.base(), name) {
        return Err(format!("secret '{name}' already exists").into());
    }

    let key = load_or_create_key(ctx)?;
    let text = match source {
        Some(source) => read_external(source)?,
        None => prompt_value_or_random()?,
    };
    key.encrypt_to_file(name, &text)?;

    Ok(())
}

fn read_external(source: &str) -> Result<String, Box<dyn Error>> {
    let buf = if source == "-" {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf)?;
        if buf.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty stdin").into());
        }
        buf
    } else {
        let buf = fs::read(source)?;
        if buf.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty file").into());
        }
        buf
    };
    Ok(String::from_utf8(buf)?)
}

fn cat(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }

    let key = secret::MasterKey::load(ctx.base())?;
    let plaintext = key.decrypt_from_file(name)?;
    println!("{plaintext}");

    Ok(())
}

fn rm(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }

    match secret::secret_rm(ctx.base(), name) {
        Ok(_) => {
            println!("secret '{}' removed", name);
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("secret '{}' not found", name);
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

fn ls(ctx: &MainContext) -> Result<(), Box<dyn Error>> {
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
