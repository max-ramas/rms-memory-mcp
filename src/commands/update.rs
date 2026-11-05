use crate::updater::{self, UpdateOptions};
use anyhow::{Result, bail};
use clap::Args;
use serde_json::json;

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Only report whether a newer version exists; install nothing
    #[arg(long)]
    pub check: bool,
    /// Install without asking for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Update even when the binary was installed by a package manager (brew, cargo, .deb)
    #[arg(long)]
    pub force: bool,
    /// Restore the binary saved by the previous update
    #[arg(long)]
    pub rollback: bool,
    /// Release channel
    #[arg(long, default_value = updater::DEFAULT_CHANNEL)]
    pub channel: String,
    /// Machine-readable output (one JSON object)
    #[arg(long)]
    pub json: bool,
    /// Install to this path instead of the running binary (used by the GUI for first installs)
    #[arg(long, value_name = "PATH")]
    pub target: Option<std::path::PathBuf>,
}

impl UpdateArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let args = UpdateArgs {
            check: self.check,
            yes: self.yes,
            force: self.force,
            rollback: self.rollback,
            channel: self.channel.clone(),
            json: self.json,
            target: self.target.clone(),
        };
        tokio::task::spawn_blocking(move || args.run_blocking()).await?
    }

    fn run_blocking(&self) -> Result<()> {
        let current = crate::update_check::CURRENT_VERSION;
        let exe = match &self.target {
            Some(path) => path.clone(),
            None => {
                let exe = std::env::current_exe()?;
                std::fs::canonicalize(&exe).unwrap_or(exe)
            }
        };
        if self.rollback {
            let restored = updater::rollback(&exe)?;
            self.emit(
                json!({"status": "rolled_back", "path": restored}),
                &format!("Restored the previous binary at {}", restored.display()),
            );
            return Ok(());
        }
        let opts = UpdateOptions {
            channel: self.channel.clone(),
            ..UpdateOptions::default()
        };
        // A first install (target missing) has no current version: treat as 0.0.0.
        let installed = if exe.is_file() { current } else { "0.0.0" };
        let Some(candidate) = updater::check(&opts, installed)? else {
            self.emit(
                json!({"status": "up_to_date", "current": installed}),
                &format!("rms-memory {installed} is up to date."),
            );
            crate::tips::maybe_show_gui_tip(self.json);
            return Ok(());
        };
        if self.check {
            self.emit(
                json!({"status": "available", "current": installed, "latest": candidate.version, "notes": candidate.notes}),
                &format!(
                    "rms-memory {} is available (installed: {installed}). Run `rms-memory update`.",
                    candidate.version
                ),
            );
            return Ok(());
        }
        if self.target.is_none() && !self.force {
            let hint = crate::update_check::upgrade_hint(&exe);
            if hint.starts_with("brew")
                || hint.starts_with("cargo")
                || hint.contains("package manager")
            {
                bail!(
                    "this binary is managed by a package manager; update it with {hint} (or pass --force)"
                );
            }
        }
        if !self.yes && !self.json {
            eprintln!(
                "Update rms-memory {installed} -> {} at {}?",
                candidate.version,
                exe.display()
            );
            if !dialoguer::Confirm::new().default(true).interact()? {
                bail!("cancelled");
            }
        }
        let staged = updater::download_and_stage(&candidate)?;
        let outcome = updater::install_staged(&staged, &exe)?;
        self.emit(
            json!({
                "status": "updated", "from": installed, "to": outcome.version,
                "path": outcome.target, "backup": outcome.backup
            }),
            &format!(
                "Updated rms-memory {installed} -> {} ({}). Restart MCP clients to load it.{}",
                outcome.version,
                outcome.target.display(),
                outcome
                    .backup
                    .as_ref()
                    .map(|b| format!(
                        " Previous binary kept at {} (`rms-memory update --rollback`).",
                        b.display()
                    ))
                    .unwrap_or_default()
            ),
        );
        crate::tips::maybe_show_gui_tip(self.json);
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
