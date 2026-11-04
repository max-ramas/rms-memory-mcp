use crate::tool_stats;
use anyhow::Result;
use clap::Args;

#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Look back this many days
    #[arg(long, default_value_t = 30)]
    pub days: i64,
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
    /// Delete the recorded statistics
    #[arg(long)]
    pub reset: bool,
}

impl StatsArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let path = tool_stats::default_path();
        if self.reset {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("jsonl.1"));
            println!("Statistics deleted.");
            return Ok(());
        }
        let rows =
            tool_stats::summarize(&tool_stats::read_all(&path), self.days, chrono::Utc::now());
        if self.json {
            println!("{}", serde_json::to_string_pretty(&rows)?);
            return Ok(());
        }
        if tool_stats::disabled() {
            println!(
                "Recording is off (RMS_MEMORY_NO_STATS is set); showing what was recorded earlier."
            );
        }
        if rows.is_empty() {
            println!("No calls recorded in the last {} days.", self.days);
            return Ok(());
        }
        println!(
            "Characters returned to the agent, last {} days (tokens ~ chars/{}):\n",
            self.days,
            tool_stats::CHARS_PER_TOKEN
        );
        println!(
            "{:<26} {:>6} {:>6} {:>10} {:>8} {:>8} {:>9}",
            "tool / method", "calls", "errors", "total", "avg", "max", "~tokens"
        );
        for r in &rows {
            println!(
                "{:<26} {:>6} {:>6} {:>10} {:>8} {:>8} {:>9}",
                r.name, r.calls, r.errors, r.total_chars, r.avg_chars, r.max_chars, r.approx_tokens
            );
        }
        println!(
            "\nLocal only; no queries, paths or note content are stored. Disable with RMS_MEMORY_NO_STATS=1."
        );
        Ok(())
    }
}
