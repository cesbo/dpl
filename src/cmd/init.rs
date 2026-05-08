use std::{
    error::Error,
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Stdio,
    },
};

use dialoguer::{
    Confirm,
    Input,
    Password,
    theme::ColorfulTheme,
};

use crate::{
    auth::model::AuthEntry,
    model::MainConfig,
    validate,
};

const SERVICE_PATH: &str = "/etc/systemd/system/dpl.service";
const SYSTEMD_RUNTIME_DIR: &str = "/run/systemd/system";

unsafe extern "C" {
    fn geteuid() -> u32;
}

struct RunContext<'a> {
    theme: &'a ColorfulTheme,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let theme = ColorfulTheme::default();
    let ctx = RunContext { theme: &theme };
    ctx.run()
}

impl<'a> RunContext<'a> {
    fn run(&self) -> Result<(), Box<dyn Error>> {
        if unsafe { geteuid() } != 0 {
            return Err("dpl init must be run as root (try: sudo dpl init)".into());
        }

        if !Path::new(SYSTEMD_RUNTIME_DIR).exists() {
            return Err(
                "systemd is not running on this system (missing /run/systemd/system)".into(),
            );
        }

        let mut config = MainConfig::default();

        let base: PathBuf = Input::with_theme(self.theme)
            .with_prompt("Base directory")
            .default("/opt/dpl".to_string())
            .interact_text()
            .map(PathBuf::from)?;

        let config_path = base.join("config.yaml");
        let tokens_dir = base.join(".tokens");

        if base.is_dir() {
            println!("Base directory already exists");

            let mut ask_overwrite = false;
            if config_path.is_file() {
                println!("  - config.yaml will be overwritten");
                ask_overwrite = true;
            }
            if tokens_dir.is_dir() {
                println!("  - .tokens/ (access tokens) may be overwritten");
                ask_overwrite = true;
            }

            if ask_overwrite {
                let proceed = Confirm::with_theme(self.theme)
                    .with_prompt("Overwrite and continue?")
                    .default(false)
                    .interact()?;

                if !proceed {
                    return Ok(());
                }
            }
        }

        fs::create_dir_all(&base)?;
        fs::create_dir_all(&tokens_dir)?;

        config.server.addr = Input::with_theme(self.theme)
            .with_prompt("Bind address")
            .default(config.server.addr.clone())
            .interact_text()?;

        config.server.port = Input::with_theme(self.theme)
            .with_prompt("Port")
            .default(config.server.port)
            .interact_text()?;

        self.write_yaml(&config_path, &config)?;

        println!();
        println!("Access token is the bearer credential for the HTTP API.");
        println!("Send it as `Authorization: Bearer {{name}}:{{token}}`.");
        println!();

        let token_name: String = Input::with_theme(self.theme)
            .with_prompt("Token name")
            .default("admin".into())
            .validate_with(|input: &String| -> Result<(), &str> {
                if validate::resource_name(input) {
                    Ok(())
                } else {
                    Err("name must be lowercase letters, digits, hyphens (no leading/trailing/double hyphens)")
                }
            })
            .interact_text()?;

        let token: String = Password::with_theme(self.theme)
            .with_prompt("Token value")
            .interact()?;

        let auth_entry = AuthEntry {
            token,
            apps: vec!["*".into()],
        };

        let token_path = tokens_dir.join(format!("{token_name}.yaml"));
        self.write_yaml(&token_path, &auth_entry)?;

        self.install_service(&base)?;

        let autostart: bool = Confirm::with_theme(self.theme)
            .with_prompt("Enable autostart?")
            .default(true)
            .interact()?;

        if autostart {
            run_systemctl(&["-q", "enable", "--now", "dpl"])?;
        }

        println!("dpl installed");
        Ok(())
    }

    fn write_yaml<T: serde::Serialize>(
        &self,
        path: &Path,
        value: &T,
    ) -> Result<(), Box<dyn Error>> {
        let content = serde_yaml::to_string(value)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn install_service(&self, base: &Path) -> Result<(), Box<dyn Error>> {
        if Path::new(SERVICE_PATH).exists() {
            let _ = run_systemctl(&["-q", "disable", "--now", "dpl"]);
        }

        let exe = std::env::current_exe()?;
        let service = format!(
            r#"[Unit]
Description=dpl deploy server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} --base {}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
"#,
            exe.display(),
            base.display(),
        );

        let path = Path::new(SERVICE_PATH);
        fs::write(path, service)?;

        let _ = run_systemctl(&["-q", "daemon-reload"]);

        Ok(())
    }
}
fn run_systemctl(args: &[&str]) -> std::io::Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "systemctl {:?} exited with {status}",
            args
        )))
    }
}
