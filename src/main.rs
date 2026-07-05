mod cli;
mod document;
mod indexer;
mod install;
mod mcp_server;
mod rules_injector;
mod store;
mod workspace;
mod link;
mod import;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let log_dir = crate::workspace::base_dir();
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::never(&log_dir, "rms.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    cli::Cli::execute().await?;
    Ok(())
}
