use std::{
    io::Write,
    process::{
        Command,
        Stdio,
    },
};

use anyhow::{
    Context,
    Result,
    bail,
};

const INSTALL_URL: &str = "https://dpl.cesbo.com/install.sh";

/// Run the install script, which checks the latest release itself and leaves
/// the install alone when the version is unchanged. It is fetched to memory
/// first: `curl | sh` would report success when the download fails.
pub fn run() -> Result<()> {
    let script = Command::new("curl")
        .args(["-fsSL", INSTALL_URL])
        .stderr(Stdio::inherit())
        .output()
        .context("run curl")?;
    if !script.status.success() {
        bail!("download {INSTALL_URL} failed");
    }

    let mut sh = Command::new("sh")
        .stdin(Stdio::piped())
        .spawn()
        .context("run sh")?;
    sh.stdin
        .take()
        .context("open sh stdin")?
        .write_all(&script.stdout)
        .context("pass the script to sh")?;

    if !sh.wait().context("wait for sh")?.success() {
        bail!("install script failed");
    }
    Ok(())
}
