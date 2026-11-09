use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use rms_memory_dialogs::collector::{self, Mode};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct DialogsArgs {
    /// Config file (default: ~/.rms-memory/dialogs/config.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: DialogsCommands,
}

#[derive(Debug, Subcommand)]
pub enum DialogsCommands {
    /// Create config and archive folder; with --encrypt also create the key pair
    Init {
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        encrypt: bool,
    },
    /// Set the mode: off | collect | collect_and_serve
    Mode { mode: Mode },
    /// Copy new/changed sessions into the archive (once, or repeatedly with --watch)
    Collect {
        #[arg(long)]
        watch: bool,
        /// Seconds between scans in --watch mode
        #[arg(long, default_value_t = 300)]
        interval: u64,
        /// Also update the search index after every pass
        #[arg(long)]
        index: bool,
    },
    /// Add new and changed sessions to the search index (user and assistant turns only)
    Index,
    /// Search the archived dialogs
    Search {
        query: String,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Print one archived session as Markdown (path like claude-code/<project>/<file>)
    Session { path: String },
    /// Show mode, sources and archive counts
    Status,
    /// Print an archived session (asks for the passphrase when encrypted)
    Read {
        /// A segment file or the logical name inside the archive
        path: PathBuf,
        /// Older generation (default: latest)
        #[arg(long)]
        generation: Option<u32>,
    },
    /// Write the archive back out as ordinary files
    Restore {
        /// Materialize the whole archive under this folder
        #[arg(long)]
        to: Option<PathBuf>,
        /// Put files back at their original tool locations, never overwriting existing ones
        #[arg(long)]
        native: bool,
    },
    /// Check the archive against the recorded state (--deep decrypts and checks sha256)
    Verify {
        #[arg(long)]
        deep: bool,
    },
}

impl DialogsArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let cp = self
            .config
            .clone()
            .unwrap_or_else(collector::default_config_path);
        let command = match &self.command {
            DialogsCommands::Init { out, encrypt } => {
                let (out, encrypt) = (out.clone(), *encrypt);
                Task::Init(out, encrypt)
            }
            DialogsCommands::Mode { mode } => Task::Mode(*mode),
            DialogsCommands::Collect {
                watch,
                interval,
                index,
            } => Task::Collect(*watch, *interval, *index),
            DialogsCommands::Index => Task::Index,
            DialogsCommands::Search {
                query,
                tool,
                project,
                limit,
            } => Task::Search(query.clone(), tool.clone(), project.clone(), *limit),
            DialogsCommands::Session { path } => Task::Session(path.clone()),
            DialogsCommands::Status => Task::Status,
            DialogsCommands::Read { path, generation } => Task::Read(path.clone(), *generation),
            DialogsCommands::Restore { to, native } => Task::Restore(to.clone(), *native),
            DialogsCommands::Verify { deep } => Task::Verify(*deep),
        };
        // The collector is blocking (SQLite, file IO, passphrase prompts).
        tokio::task::spawn_blocking(move || command.run(&cp))
            .await
            .map_err(|e| anyhow!("{e}"))?
            .map_err(|e| anyhow!(e))
    }
}

enum Task {
    Init(Option<PathBuf>, bool),
    Mode(Mode),
    Collect(bool, u64, bool),
    Index,
    Search(String, Option<String>, Option<String>, usize),
    Session(String),
    Status,
    Read(PathBuf, Option<u32>),
    Restore(Option<PathBuf>, bool),
    Verify(bool),
}

impl Task {
    fn run(self, cp: &std::path::Path) -> Result<(), String> {
        match self {
            Task::Init(out, enc) => collector::init(cp, out, enc),
            Task::Mode(m) => collector::set_mode(cp, m),
            Task::Collect(w, i, x) => collector::collect(cp, w, i, x),
            Task::Index => {
                let (n, t) = rms_memory_dialogs::index::update(cp)?;
                println!("indexed {n} sessions, {t} turns");
                Ok(())
            }
            Task::Search(q, tool, project, limit) => {
                let hits = rms_memory_dialogs::index::search(
                    cp,
                    &q,
                    tool.as_deref(),
                    project.as_deref(),
                    limit,
                )?;
                if hits.is_empty() {
                    println!("no matches");
                }
                for h in hits {
                    let _ = writeln!(
                        std::io::stdout(),
                        "{}  [{} {}]\n  {}\n",
                        h.rel,
                        h.role,
                        h.ts.unwrap_or_default(),
                        h.snippet.replace('\n', " ")
                    );
                }
                Ok(())
            }
            Task::Session(p) => {
                let md = rms_memory_dialogs::index::read_session(cp, &p)?.0;
                let _ = std::io::stdout().write_all(md.as_bytes()); // a closed pipe (| head) is not an error
                Ok(())
            }
            Task::Status => collector::status(cp),
            Task::Read(p, g) => {
                let data = collector::read(cp, &p, g)?;
                std::io::stdout()
                    .write_all(&data)
                    .map_err(|e| e.to_string())
            }
            Task::Restore(to, native) => collector::restore_cmd(cp, to.as_deref(), native),
            Task::Verify(deep) => collector::verify_cmd(cp, deep),
        }
    }
}
