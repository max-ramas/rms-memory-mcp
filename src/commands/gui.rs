use crate::{companion_status, gui_install};
use anyhow::{Result, anyhow, bail};
use clap::Args;
use serde_json::json;

#[derive(Debug, Args)]
pub struct GuiArgs {
    /// Download the installer for this machine into your Downloads folder, do not install it
    #[arg(long)]
    pub download_only: bool,
    /// Install without opening the app afterwards
    #[arg(long)]
    pub no_open: bool,
    /// Install without asking for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Machine-readable output (one JSON object)
    #[arg(long)]
    pub json: bool,
}

impl GuiArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let args = GuiArgs {
            download_only: self.download_only,
            no_open: self.no_open,
            yes: self.yes,
            json: self.json,
        };
        tokio::task::spawn_blocking(move || args.run_blocking()).await?
    }

    fn run_blocking(&self) -> Result<()> {
        if !self.download_only && companion_status::detect().gui_installed {
            return self.open_installed();
        }
        let Some(opts) = gui_install::options_for_host() else {
            bail!(
                "there is no GUI build for {}/{} yet (published: {})",
                std::env::consts::OS,
                std::env::consts::ARCH,
                gui_install::supported_hosts()
            );
        };
        let candidate = gui_install::find_installer(&opts)?.ok_or_else(|| {
            anyhow!("the update server lists no RMS Memory GUI release for this machine yet")
        })?;
        if !self.yes && !self.json {
            eprintln!(
                "Install RMS Memory GUI {} ({:.1} MB)? It starts with a 14-day trial; the MCP server never needs it.",
                candidate.version,
                candidate.artifact.size as f64 / 1_048_576.0
            );
            if !dialoguer::Confirm::new().default(true).interact()? {
                bail!("cancelled");
            }
        }
        match gui_install::install(&candidate, self.download_only)? {
            gui_install::Placed::Downloaded(path) => self.emit(
                json!({"status": "downloaded", "version": candidate.version, "path": path}),
                &format!("Downloaded and verified: {}", path.display()),
            ),
            gui_install::Placed::InstallerStarted(path) => self.emit(
                json!({"status": "installer_started", "version": candidate.version, "path": path}),
                &format!(
                    "Verified and started {}. Finish the installer in its window; Windows SmartScreen may ask you to confirm.",
                    path.display()
                ),
            ),
            gui_install::Placed::Installed(path) => {
                self.emit(
                    json!({"status": "installed", "version": candidate.version, "path": path}),
                    &format!(
                        "Installed RMS Memory GUI {} at {}.",
                        candidate.version,
                        path.display()
                    ),
                );
                if !self.no_open {
                    gui_install::launch(&path)?;
                }
            }
        }
        Ok(())
    }

    fn open_installed(&self) -> Result<()> {
        let Some(path) = companion_status::installed_gui_path() else {
            self.emit(
                json!({"status": "installed", "path": null}),
                "RMS Memory GUI is installed (set by RMS_MEMORY_GUI_INSTALLED); open it from your applications.",
            );
            return Ok(());
        };
        if !self.no_open {
            gui_install::launch(&path)?;
        }
        self.emit(
            json!({"status": "opened", "path": path}),
            &format!("Opening RMS Memory GUI ({}).", path.display()),
        );
        Ok(())
    }

    fn emit(&self, value: serde_json::Value, text: &str) {
        if self.json {
            println!("{value}");
        } else {
            println!("{text}");
        }
    }
}
