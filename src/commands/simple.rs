use crate::indexer::Indexer;
use crate::workspace::Workspace;
use anyhow::Result;
use clap::Args;
use std::io::Write;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Copy an existing Obsidian vault (notes, attachments, folder structure)
    /// into this project's vault. The source is only read; hidden folders such
    /// as .obsidian/ and .trash/ are skipped and existing files are never
    /// overwritten. To use an Obsidian vault in place without copying, see
    /// `rms-memory projects attach-obsidian`.
    #[arg(long, value_name = "DIR")]
    pub obsidian: Option<std::path::PathBuf>,
    /// Vault folder that receives the Obsidian notes
    #[arg(long, default_value = "obsidian", requires = "obsidian")]
    pub into: String,
    /// Show what would be imported without writing anything
    #[arg(long, requires = "obsidian")]
    pub dry_run: bool,
    /// Print the import report as JSON
    #[arg(long, requires = "obsidian")]
    pub json: bool,
}

impl ImportArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        if let Some(source) = &self.obsidian {
            return self.run_obsidian(source, &workspace);
        }
        let import_service =
            crate::import::ImportService::new(workspace.code_path.clone(), workspace.root.clone());
        let docs = import_service.detect_existing_docs();
        if docs.is_empty() {
            println!("No existing project knowledge files found to import.");
        } else {
            let action = import_service.prompt_action(&docs)?;
            import_service.execute(action, docs)?;
        }
        Ok(())
    }

    fn run_obsidian(&self, source: &Path, workspace: &Workspace) -> Result<()> {
        let report = crate::obsidian_import::import_obsidian_vault(
            source,
            &workspace.root,
            &self.into,
            self.dry_run,
        )?;
        if self.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(());
        }
        let verb = if report.dry_run {
            "Would import"
        } else {
            "Imported"
        };
        println!("{} {} → {}", verb, report.source, report.destination);
        println!(
            "  {} notes, {} attachments",
            report.notes.len(),
            report.attachments.len()
        );
        if !report.unchanged.is_empty() {
            println!("  {} already present, unchanged", report.unchanged.len());
        }
        if report.skipped_hidden + report.skipped_symlinks > 0 {
            println!(
                "  skipped {} hidden entries (.obsidian/, .trash/, …) and {} symlinks",
                report.skipped_hidden, report.skipped_symlinks
            );
        }
        if !report.conflicts.is_empty() {
            println!(
                "  ⚠️  {} files already exist with different content (kept as is):",
                report.conflicts.len()
            );
            for path in &report.conflicts {
                println!("     - {path}");
            }
        }
        if !report.skipped_secrets.is_empty() {
            println!(
                "  🔒 {} files not imported because they contain what looks like a credential (values not shown):",
                report.skipped_secrets.len()
            );
            for line in &report.skipped_secrets {
                println!("     - {line}");
            }
        }
        if !report.invalid_frontmatter.is_empty() {
            println!(
                "  ⚠️  {} notes have frontmatter that is not valid YAML (copied as is):",
                report.invalid_frontmatter.len()
            );
            for path in &report.invalid_frontmatter {
                println!("     - {path}");
            }
        }
        let imported = !(report.notes.is_empty() && report.attachments.is_empty());
        if !report.dry_run && imported {
            println!("  Run `rms-memory reindex` (or let the MCP server sync) to search them.");
        }
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Serve MCP Streamable HTTP on 127.0.0.1 instead of stdio (bearer token in ~/.rms-memory/http-token)
    #[arg(long)]
    pub http: bool,
    /// Port for --http
    #[arg(long, default_value_t = crate::http_transport::DEFAULT_PORT, requires = "http")]
    pub port: u16,
}

impl ServeArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let registry = crate::config_manager::load_registry().unwrap_or_default();
        let max_backups = registry.global.max_backups.unwrap_or(5);
        if self.http {
            return crate::http_transport::serve(self.port, max_backups, scope).await;
        }
        crate::mcp_server::McpServer::run(None, None, None, max_backups, scope).await?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ReindexArgs {
    /// Reindex Vault memory (the default when no corpus flag is provided)
    #[arg(long, conflicts_with_all = ["code", "all"])]
    pub vault: bool,
    /// Reindex the derived semantic code memory
    #[arg(long, conflicts_with_all = ["vault", "all"])]
    pub code: bool,
    /// Reindex both Vault and semantic code memory
    #[arg(long, conflicts_with_all = ["vault", "code"])]
    pub all: bool,
}

impl ReindexArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        let store = crate::store::Store::for_workspace(&workspace).await?;
        let mut indexer = Indexer::new()?;

        if self.vault || self.all || !self.code {
            println!("Reindexing Vault at {:?}", workspace.root);
            crate::indexer::index_vault_full(&workspace, &store, &mut indexer).await?;
        }
        if self.code || self.all {
            println!("Reindexing semantic code at {:?}", workspace.code_path);
            let stats =
                crate::code_indexer::index_code_full(&workspace, &store, &mut indexer).await?;
            println!(
                "Code index: {} files, {} items, {} segments ({} embedded, {} reused, {} removed; {} skipped)",
                stats.files_indexed,
                stats.items_indexed,
                stats.segments_indexed,
                stats.segments_embedded,
                stats.segments_reused,
                stats.segments_deleted,
                stats.files_skipped
            );
        }

        println!("Reindex completed.");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Repair duplicate, missing, or attached frontmatter IDs after creating a backup
    #[arg(long)]
    pub repair_frontmatter: bool,
    /// Repair one explicit file inside a registered project or global vault
    #[arg(long, requires = "repair_frontmatter")]
    pub repair_path: Option<std::path::PathBuf>,
    /// Stamp project label on documents (dry-run by default)
    #[arg(long)]
    pub stamp_project: bool,
    /// Apply stamp-project changes (requires --stamp-project)
    #[arg(long, requires = "stamp_project")]
    pub apply: bool,
    /// Explicit file for stamp-project
    #[arg(long, requires = "stamp_project")]
    pub file: Option<String>,
    /// Explicit project key for stamp-project
    #[arg(long, requires = "stamp_project")]
    pub project: Option<String>,
}

impl DoctorArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        if let Some(requested_path) = &self.repair_path {
            let path = std::fs::canonicalize(requested_path)?;
            if !path.is_file()
                || !path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                return Err(anyhow::anyhow!(
                    "Repair path must be an existing Markdown file"
                ));
            }

            let registry = crate::config_manager::load_registry()?;
            let mut vault_roots = registry
                .projects
                .values()
                .map(|project| &project.vault_path)
                .collect::<Vec<_>>();
            if let Some(global) = &registry.global.global_vault_path {
                vault_roots.push(global);
            }
            let is_in_registered_vault = vault_roots.iter().any(|root| {
                std::fs::canonicalize(root)
                    .map(|canonical_root| path.starts_with(canonical_root))
                    .unwrap_or(false)
            });
            if !is_in_registered_vault {
                return Err(anyhow::anyhow!(
                    "Repair path is outside every registered RMS Memory vault"
                ));
            }
            crate::workspace::ensure_vault_writable_in(&registry, &path)?;

            if crate::document::Document::repair_duplicate_ids(&path)? {
                println!("Repaired frontmatter IDs: {}", path.display());
            } else {
                println!("No repairable frontmatter IDs found: {}", path.display());
            }
            return Ok(());
        }

        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        let report = crate::doctor::diagnose(
            &workspace,
            crate::doctor::DiagnoseOptions {
                repair_frontmatter: self.repair_frontmatter,
            },
        )
        .await?;
        print_doctor_report(&report);

        let companion = crate::companion_status::detect();
        print!(
            "\n{}",
            crate::companion_status::format_status_banner(&companion)
        );
        if !companion.gui_installed {
            println!(
                "Tip: `rms-memory features` lists optional GUI/AI capabilities (informational only)."
            );
        }

        // ─── Stamp Project ───
        if self.stamp_project {
            let project_key = self.project.clone().or_else(|| workspace.project_key());

            if let Some(key) = &project_key {
                if self.apply {
                    workspace.ensure_writable()?;
                }
                println!(
                    "\n[🔖] Stamp-project mode: project = '{key}'{}",
                    if self.apply {
                        " (applying)"
                    } else {
                        " (dry-run)"
                    }
                );

                let files = workspace.find_markdown_files()?;
                let mut stamped = 0;
                let mut skipped = 0;

                for f in &files {
                    if let Some(file) = &self.file {
                        let target = std::path::Path::new(file);
                        if f.file_name() != target.file_name() {
                            continue;
                        }
                    }

                    let stamp = |mapping: &mut serde_yaml::Mapping| {
                        let project = serde_yaml::Value::from("project");
                        if mapping.contains_key(&project) {
                            return false;
                        }
                        mapping.insert(project, key.as_str().into());
                        true
                    };
                    match crate::frontmatter::update_file(f, self.apply, stamp) {
                        Ok(true) if self.apply => {
                            stamped += 1;
                            println!("  ✅ Stamped: {}", f.display());
                        }
                        Ok(true) => {
                            stamped += 1;
                            println!("  [DRY-RUN] Would stamp: {}", f.display());
                        }
                        Ok(false) => skipped += 1,
                        Err(e) => eprintln!("  ❌ Skipped {}: {e:#}", f.display()),
                    }
                }

                println!(
                    "\n    {} stamped, {} skipped (already tagged)",
                    stamped, skipped
                );

                if !self.apply {
                    println!("    Use --apply to write changes.\n");
                }
            } else {
                eprintln!(
                    "No project key found for this workspace. Use --project <key> to specify."
                );
            }
        }

        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Automatically approve all patching
    #[arg(short, long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
}

impl InstallArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        crate::installer::run_installer(self.yes, self.dry_run).await?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Automatically approve all removals
    #[arg(short, long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
}

impl UninstallArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        crate::installer::run_uninstaller(self.yes, self.dry_run).await?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct LogArgs;

impl LogArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let log_file = crate::workspace::base_dir().join("rms.log");
        if !log_file.exists() {
            println!("Log file does not exist yet.");
            return Ok(());
        }

        // Do not invoke `tail -f`: it is not available on Windows and would
        // make the CLI's logging command depend on a shell utility. Start by
        // showing the conventional final ten lines, then follow appended data
        // until the user presses Ctrl+C. A smaller file after rotation resets
        // the offset safely.
        let (initial, mut offset) = read_log_end(&log_file)?;
        let initial_tail = tail_lines(&initial, 10);
        if !initial_tail.is_empty() {
            println!("{initial_tail}");
        }
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal?;
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(350)) => {
                    let appended = read_appended_log(&log_file, &mut offset).await?;
                    if !appended.is_empty() {
                        print!("{appended}");
                        std::io::stdout().flush()?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn tail_lines(content: &str, max_lines: usize) -> String {
    let mut lines = content.lines().rev().take(max_lines).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

/// The last 64 KiB of the log (without a leading partial line) and its length.
fn read_log_end(path: &Path) -> Result<(String, u64)> {
    use std::io::{Read, Seek};
    const MAX_TAIL_BYTES: u64 = 64 * 1024;
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_TAIL_BYTES);
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let end = start + bytes.len() as u64;
    let text = String::from_utf8_lossy(&bytes);
    let text = match (start > 0, text.find('\n')) {
        (true, Some(newline)) => text[newline + 1..].to_string(),
        _ => text.into_owned(),
    };
    Ok((text, end))
}

async fn read_appended_log(path: &Path, offset: &mut u64) -> Result<String> {
    // Another process may have just moved the log to `rms.log.1`.
    let length = match tokio::fs::metadata(path).await {
        Ok(meta) => meta.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if length < *offset {
        *offset = 0;
    }
    if length == *offset {
        return Ok(String::new());
    }

    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(*offset)).await?;
    let mut bytes = Vec::with_capacity((length - *offset) as usize);
    file.read_to_end(&mut bytes).await?;
    *offset += bytes.len() as u64;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod log_tests {
    use super::{read_log_end, tail_lines};

    #[test]
    fn log_end_skips_the_partial_first_line_of_a_large_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("rms.log");
        let text = (0..5_000)
            .map(|n| format!("line {n} {}", "x".repeat(40)))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&log, &text).unwrap();
        let (end, length) = read_log_end(&log).unwrap();
        assert_eq!(length, text.len() as u64);
        assert!(end.starts_with("line "));
        assert_eq!(tail_lines(&end, 1), text.lines().last().unwrap());
    }

    #[test]
    fn tail_lines_keeps_the_last_lines_in_original_order() {
        assert_eq!(tail_lines("one\ntwo\nthree\nfour", 2), "three\nfour");
        assert_eq!(tail_lines("one", 10), "one");
    }
}

#[derive(Args, Debug)]
pub struct SyncArgs;

impl SyncArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        let store = crate::store::Store::for_workspace(&workspace).await?;
        let mut indexer = Indexer::new()?;
        crate::indexer::sync_vault(&workspace, &store, &mut indexer).await?;
        println!("Sync complete.");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ExportLlmsArgs {
    #[arg(short, long)]
    pub out: Option<String>,
}

impl ExportLlmsArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        let files = workspace.find_markdown_files()?;
        let file_count = files.len();

        let mut combined = String::from("# RMS Memory Vault\n\n");
        combined.push_str(&format!(
            "> Exported from {}\n> Generated: {}\n\n",
            workspace.root.display(),
            chrono::Utc::now().to_rfc3339()
        ));

        for f in &files {
            if let Ok(doc) = crate::document::Document::parse(f) {
                let rel = f.strip_prefix(&workspace.root).unwrap_or(f);
                let title = doc
                    .frontmatter
                    .as_ref()
                    .and_then(|fm| fm.alias.as_ref())
                    .cloned()
                    .unwrap_or_else(|| {
                        rel.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    });
                let desc = doc.content.lines().take(2).collect::<Vec<_>>().join(" ");

                combined.push_str(&format!(
                    "- [{}]({}) — {}\n",
                    title,
                    rel.to_string_lossy(),
                    if desc.len() > 120 {
                        format!("{}...", &desc[..117])
                    } else {
                        desc
                    }
                ));
            }
        }

        // Append full contents section
        combined.push_str("\n\n---\n# Full Contents\n\n");
        for f in &files {
            let rel = f.strip_prefix(&workspace.root).unwrap_or(f);
            if let Ok(content) = std::fs::read_to_string(f) {
                combined.push_str(&format!("\n## {}\n\n{}\n", rel.to_string_lossy(), content));
            }
        }

        let out_path = self.out.clone().unwrap_or_else(|| "llms.txt".to_string());
        std::fs::write(&out_path, combined)?;
        println!("Exported {} files to {}", file_count, out_path);
        Ok(())
    }
}

fn print_doctor_report(report: &crate::doctor::DoctorReport) {
    println!("Doctor checks for {}", report.vault_root);
    println!("{}", "─".repeat(60));
    for check in &report.checks {
        let mut title = check.name.replace('_', " ");
        title[..1].make_ascii_uppercase();
        println!("\n[{}/{}] {title}", check.id, report.checks.len());
        if check.ok {
            println!("  ✅ ok");
        }
        for issue in &check.issues {
            println!("  ⚠️  {issue}");
        }
        for note in &check.notes {
            println!("  ℹ️  {note}");
        }
    }
    println!("\n{}", "─".repeat(60));
    if report.healthy {
        println!("✅ All checks passed. Vault is healthy.");
    } else {
        println!("⚠️  {} issue(s) found.", report.issue_count);
    }
    for notice in &report.notices {
        println!("\nℹ️  {notice}");
    }
}
