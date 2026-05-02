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
    config::load_config,
    model::MainConfig,
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
    /// Encrypt and store a secret
    Set {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: String,
        /// Source: omit for an interactive prompt, "-" to read stdin, or a path to a file.
        /// On an empty interactive prompt a random secret is generated and printed.
        source: Option<String>,
    },
    /// Remove an encrypted secret
    Rm {
        /// Secret name: `name` or `group/name` (lowercase letters, digits, `-`)
        name: String,
    },
    /// List existing secret names
    List,
}

pub fn run(args: Args, config_path: &Path) -> Result<(), Box<dyn Error>> {
    let config: MainConfig = load_config(config_path)?;

    match args.cmd {
        Cmd::Set { name, source } => set(&config.base, &name, source.as_deref()),
        Cmd::Rm { name } => rm(&config.base, &name),
        Cmd::List => list(&config.base),
    }
}

fn set(base: &Path, name: &str, source: Option<&str>) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }

    let key = ensure_master_key(base)?;
    let value = read_plaintext(source)?;
    let blob = secret::encrypt(name, &key, &value)?;

    let target = secret::get_secret_path(base, name);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&target, blob)?;

    Ok(())
}

fn read_plaintext(source: Option<&str>) -> Result<Vec<u8>, Box<dyn Error>> {
    match source {
        Some("-") => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf)?;
            if buf.is_empty() {
                return Err("empty stdin".into());
            }
            Ok(buf)
        }
        Some(path) => {
            let buf = fs::read(path)?;
            if buf.is_empty() {
                return Err("empty file".into());
            }
            Ok(buf)
        }
        None => {
            let value = Password::with_theme(&ColorfulTheme::default())
                .with_prompt("Secret value (empty = generate random)")
                .allow_empty_password(true)
                .interact()?;

            if value.is_empty() {
                let buf = rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(RANDOM_SECRET_LEN)
                    .collect::<Vec<u8>>();
                Ok(buf)
            } else {
                Ok(value.into_bytes())
            }
        }
    }
}

fn rm(base: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    if !validate::secret_name(name) {
        return Err(secret::SecretError::InvalidName.into());
    }

    let target = secret::get_secret_path(base, name);
    match fs::remove_file(&target) {
        Ok(_) => {
            println!("removed {}", target.display());
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            println!("{} not found", target.display());
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

fn list(base: &Path) -> Result<(), Box<dyn Error>> {
    let mut names = Vec::new();
    let secrets_dir = secret::get_secrets_dir(base);
    walk_secrets(&secrets_dir, &secrets_dir, &mut names)?;
    if names.is_empty() {
        println!("no secrets");
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

        let Some(stem) = name.strip_suffix(".bin") else {
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

fn ensure_master_key(path: &Path) -> Result<secret::MasterKey, secret::SecretError> {
    match secret::load_master_key(path) {
        Ok(key) => Ok(key),
        Err(secret::SecretError::KeyIo(ref e)) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(secret::SecretError::KeyIo)?;
            }
            let key = secret::generate_master_key();
            secret::write_master_key(path, &key)?;
            Ok(key)
        }
        Err(err) => Err(err),
    }
}
