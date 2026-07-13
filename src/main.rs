use rms_memory_mcp::{cli, workspace};

#[tokio::main(worker_threads = 2)]
async fn main() {
    let log_dir = crate::workspace::base_dir();
    std::fs::create_dir_all(&log_dir).ok();
    let file_appender = tracing_appender::rolling::never(&log_dir, "rms.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);

    let exit_code = tokio::select! {
        result = cli::Cli::execute() => {
            let code = if let Err(e) = result {
                tracing::error!("Server error: {:#}", e);
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
