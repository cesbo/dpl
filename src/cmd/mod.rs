pub mod db;
pub mod secret;
pub mod unit;

use dialoguer::theme::ColorfulTheme;

pub fn prompt_theme() -> ColorfulTheme {
    ColorfulTheme {
        success_prefix: console::style("✓".to_string()).for_stderr().green(),
        ..ColorfulTheme::default()
    }
}
