use anyhow::Result;
use clap::Args;
use rms_memory_vault::prune::{PruneOptions, prune_vault};

#[derive(Args, Debug)]
pub struct PruneArgs {
    /// Minimum age in days for a superseded note to be eligible (default: 30)
    #[arg(long, default_value_t = 30)]
    pub older_than_days: u32,
    /// Actually move eligible notes under artifacts/pruned/ (default is dry-run)
    #[arg(long)]
    pub apply: bool,
}

impl PruneArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = rms_memory_core::workspace::Workspace::discover_with_scope(
            scope.as_deref(),
            &current_dir,
            None,
        )?;

        let report = prune_vault(
            &workspace.root,
            PruneOptions {
                older_than_days: self.older_than_days,
                apply: self.apply,
            },
        )?;

        if report.candidates.is_empty() {
            println!(
                "No superseded notes older than {} days.",
                report.older_than_days
            );
            return Ok(());
        }

        println!(
            "{} {} superseded candidate(s) older than {} days:",
            if report.dry_run {
                "Would archive"
            } else {
                "Archived"
            },
            report.candidates.len(),
            report.older_than_days
        );
        for candidate in &report.candidates {
            println!(
                "  {} ({}d via {}, id={})",
                candidate.path,
                candidate.age_days,
                candidate.age_source,
                candidate.document_id.as_deref().unwrap_or("-")
            );
        }

        if report.dry_run {
            println!("\nDry-run only. Re-run with --apply to move notes under artifacts/pruned/.");
        } else {
            if let Some(batch) = &report.batch_dir {
                println!("\nBatch: {batch}/");
                println!("Manifest: {batch}/manifest.jsonl");
            }
            println!("Run `rms-memory reindex` (or sync) so Lance drops archived paths.");
        }

        Ok(())
    }
}
