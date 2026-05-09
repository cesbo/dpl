use std::{
    error::Error,
    fs,
    io::{
        self,
        Read,
    },
    path::{
        Path,
        PathBuf,
    },
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
    secret,
    validate,
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

    let base = ctx.base();

    if secret::secret_exists(base, name) {
        return Err(format!("secret '{name}' already exists").into());
    }

    let key = match secret::MasterKey::load(base) {
        Ok(key) => key,
        Err(secret::SecretError::KeyNotFound) => {
            let key = secret::MasterKey::generate(base);
            key.save()?;
            key
        }
        Err(err) => return Err(err.into()),
    };
    let text = read_plaintext(source)?;
    key.encrypt_to_file(name, &text)?;

    Ok(())
}

fn read_plaintext(source: Option<&str>) -> Result<String, Box<dyn Error>> {
    let value = match source {
        Some("-") => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            if buf.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty stdin").into());
            }
            String::from_utf8(buf)?
        }
        Some(path) => {
            let buf = fs::read(path)?;
            if buf.is_empty() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty file").into());
            }
            String::from_utf8(buf)?
        }
        None => {
            let value = Password::with_theme(&ColorfulTheme::default())
                .with_prompt("Secret value (empty = generate random)")
                .allow_empty_password(true)
                .interact()?;

            if value.is_empty() {
                rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(RANDOM_SECRET_LEN)
                    .map(char::from)
                    .collect()
            } else {
                value
            }
        }
    };

    Ok(value)
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
    let mut names = Vec::new();
    let secrets_dir = secret::get_secrets_dir(ctx.base());
    walk_secrets(&secrets_dir, &secrets_dir, &mut names)?;
    if names.is_empty() {
        println!("No secrets found");
    } else {
        names.sort();
        for name in names {
            println!("{name}");
        }
    }
    Ok(())
}

fn walk_secrets(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            walk_secrets(root, &path, out)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        let Some(stem) = name.strip_suffix(".yaml") else {
            continue;
        };

        let rel = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };

        let mut display = PathBuf::new();
        if let Some(parent) = rel.parent() {
            display.push(parent);
        }
        display.push(stem);
        out.push(display.to_string_lossy().into_owned());
    }

    Ok(())
}
