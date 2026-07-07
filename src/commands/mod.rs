use anyhow::Result;

pub mod config;
pub mod gc;
pub mod init;
pub mod simple;

/// A trait for executing CLI commands
pub trait CommandRunner {
    #[allow(async_fn_in_trait)]
    async fn run(&self) -> Result<()>;
}
