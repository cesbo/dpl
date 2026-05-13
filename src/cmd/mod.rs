pub mod db;
pub mod secret;
pub mod unit;

use anyhow::{
    Result,
    ensure,
};

use crate::{
    MainContext,
    validate,
};

/// Validates the secret name format and checks its presence.
///
/// When `exists` is `true`, returns an error if no secret with this name exists.
/// When `exists` is `false`, returns an error if a secret with this name already exists.
fn check_secret_name(ctx: &MainContext, name: &str, exists: bool) -> Result<()> {
    ensure!(validate::secret_name(name), "invalid secret name: {name}");

    if exists {
        ensure!(ctx.secret_exists(name), "secret not found: {name}");
    } else {
        ensure!(!ctx.secret_exists(name), "secret already exists: {name}");
    }

    Ok(())
}
