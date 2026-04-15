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
    auth::model::{
        AuthConfig,
        AuthKey,
    },
    model::MainConfig,
    validate,
};

const SERVICE_PATH: &str = "/etc/systemd/system/dpl.service";

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
        if Path::new(SERVICE_PATH).exists() {
            let reinstall = Confirm::with_theme(self.theme)
                .with_prompt("dpl service is already installed. Reinstall?")
                .default(false)
                .interact()?;
            if !reinstall {
                return Ok(());
            }
            let _ = run_systemctl(&["-q", "disable", "--now", "dpl"]);
        }

        let mut config = MainConfig::default();

        config.base = Input::with_theme(self.theme)
            .with_prompt("Base directory")
            .default(config.base.to_string_lossy().to_string())
            .interact_text()
            .map(|s| PathBuf::from(s))?;

        config.server.addr = Input::with_theme(self.theme)
            .with_prompt("Bind address")
            .default(config.server.addr.clone())
            .interact_text()?;

        config.server.port = Input::with_theme(self.theme)
            .with_prompt("Port")
            .default(config.server.port)
            .interact_text()?;

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

        let autostart: bool = Confirm::with_theme(self.theme)
            .with_prompt("Enable autostart?")
            .default(true)
            .interact()?;

        fs::create_dir_all(&config.base)?;

        self.write_yaml(&config.base.join("config.yaml"), &config)?;

        let auth_config = AuthConfig {
            keys: vec![AuthKey {
                name: token_name,
                token,
                apps: vec!["*".into()],
                disabled: false,
            }],
        };
        self.write_yaml(&config.base.join("auth.yaml"), &auth_config)?;

        self.install_service(&config.base)?;

        if autostart {
            println!("Enabling dpl.service...");
            run_systemctl(&["-q", "enable", "--now", "dpl"])?;
            println!("dpl.service enabled and started");
        }

        println!("Done.");
        Ok(())
    }

    fn write_yaml<T: serde::Serialize>(
        &self,
        path: &Path,
        value: &T,
    ) -> Result<(), Box<dyn Error>> {
        if path.exists() {
            let overwrite = Confirm::with_theme(self.theme)
                .with_prompt(format!("{} already exists. Overwrite?", path.display()))
                .default(false)
                .interact()?;
            if !overwrite {
                println!("Keeping existing {}", path.display());
                return Ok(());
            }
        }

        let content = serde_yaml::to_string(value)?;
        fs::write(path, content)?;

        Ok(())
    }

    fn install_service(&self, base: &Path) -> Result<(), Box<dyn Error>> {
        let exe = std::env::current_exe()?;
        let config_path = base.join("config.yaml");
        let service = format!(
            r#"[Unit]
Description=dpl deploy server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} -c {}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
"#,
            exe.display(),
            config_path.display(),
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
