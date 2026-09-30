pub mod db;
pub mod down;
pub mod secret;
pub mod serve;
pub mod unit;
pub mod upgrade;

use dialoguer::theme::ColorfulTheme;

pub fn prompt_theme() -> ColorfulTheme {
    ColorfulTheme {
        success_prefix: console::style("✓".to_string()).for_stderr().green(),
        ..ColorfulTheme::default()
    }
}
