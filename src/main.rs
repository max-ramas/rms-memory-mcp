use rms_memory_mcp::{cli, workspace};

#[tokio::main(worker_threads = 2)]
async fn main() {
    let log_dir = crate::workspace::base_dir();
    std::fs::create_dir_all(&log_dir).ok();
    rotate_log(&log_dir.join("rms.log"));
    let file_appender = tracing_appender::rolling::never(&log_dir, "rms.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    // LanceDB logs every dataset load and query plan at INFO.
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(tracing::Level::INFO)
        .with_target("lance", tracing::Level::WARN);
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .with(filter)
        .init();

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    let exit_code = tokio::select! {
        result = cli::Cli::execute() => {
            let code = if let Err(e) = result {
                tracing::error!("Server error: {:#}", e);
                eprintln!("Error: {e:#}");
                1
            } else {
                0
            };
            tracing::info!("Server shutting down normally.");
            code
        }
        _ = &mut shutdown => {
            tracing::info!("Received Ctrl+C, shutting down gracefully.");
            0
        }
    };

    std::process::exit(exit_code);
}

/// Every CLI and server process appends to one log; past this size the start
/// of a new process moves it to `rms.log.1`, replacing the previous one.
const MAX_LOG_BYTES: u64 = 20 * 1024 * 1024;

fn rotate_log(path: &std::path::Path) {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}
