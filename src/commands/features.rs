use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct FeaturesArgs {
    /// Disable ANSI colors even on a TTY
    #[arg(long)]
    pub no_color: bool,
}

impl FeaturesArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let status = crate::companion_status::detect();
        let color = !self.no_color && crate::companion_status::color_enabled();
        print!(
            "{}",
            crate::companion_status::format_features_catalog(&status, color)
        );
        Ok(())
    }
}
