use crate::indexer::Indexer;
use crate::workspace::Workspace;
use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct ImportArgs;

impl ImportArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
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
}

#[derive(Args, Debug)]
pub struct ServeArgs;

impl ServeArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let registry = crate::config_manager::load_registry().unwrap_or_default();
        let max_backups = registry.global.max_backups.unwrap_or(5);
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
        let store = workspace.get_store().await?;
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
    /// Repair duplicate top-level `id:` keys after creating a backup
    #[arg(long)]
    pub repair_frontmatter: bool,
    /// Repair one explicit file inside a registered project or global vault
    #[arg(long, requires = "repair_frontmatter")]
    pub repair_path: Option<std::path::PathBuf>,
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

            if crate::document::Document::repair_duplicate_ids(&path)? {
                println!("Repaired duplicate IDs: {}", path.display());
            } else {
                println!("No duplicate IDs found: {}", path.display());
            }
            return Ok(());
        }

        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        println!("Doctor checks for {:?}", workspace.root);
        println!("{}", "─".repeat(60));

        let mut issues = 0u32;

        // 1. Check vault directory structure
        println!("\n[1/5] Vault directory structure...");
        let required_dirs = [
            "rules",
            "decisions",
            "architecture",
            "artifacts",
            "docs",
            "api",
        ];
        for dir in &required_dirs {
            let p = workspace.root.join(dir);
            if p.exists() {
                println!("  ✅ {}/ exists", dir);
            } else {
                println!("  ⚠️  {}/ missing", dir);
                issues += 1;
            }
        }

        // 2. Check for files missing document IDs
        println!("\n[2/5] Document IDs...");
        let files = workspace.find_markdown_files().unwrap_or_default();
        let mut missing_ids = Vec::new();
        let mut invalid_frontmatter = Vec::new();
        for f in &files {
            match crate::document::Document::parse(f) {
                Ok(doc) => {
                    if doc
                        .frontmatter
                        .as_ref()
                        .and_then(|fm| fm.id.as_ref())
                        .is_none()
                    {
                        missing_ids.push(f.to_string_lossy().to_string());
                    }
                }
                Err(error) => {
                    if self.repair_frontmatter {
                        match crate::document::Document::repair_duplicate_ids(f) {
                            Ok(true) => {
                                println!("  🔧 Repaired duplicate IDs: {}", f.display());
                                continue;
                            }
                            Ok(false) => {}
                            Err(repair_error) => {
                                invalid_frontmatter.push(format!(
                                    "{}: {:#} (repair failed: {:#})",
                                    f.display(),
                                    error,
                                    repair_error
                                ));
                                continue;
                            }
                        }
                    }
                    invalid_frontmatter.push(format!("{}: {:#}", f.display(), error));
                }
            }
        }
        if missing_ids.is_empty() {
            println!("  ✅ All {} documents have IDs", files.len());
        } else {
            println!(
                "  ⚠️  {} files missing 'id' in frontmatter:",
                missing_ids.len()
            );
            for path in &missing_ids {
                println!("     - {}", path);
            }
            issues += missing_ids.len() as u32;
        }
        if !invalid_frontmatter.is_empty() {
            println!(
                "  ❌ {} files have invalid YAML frontmatter:",
                invalid_frontmatter.len()
            );
            for error in &invalid_frontmatter {
                println!("     - {}", error);
            }
            println!("     Run `rms-memory doctor --repair-frontmatter` for duplicate id keys.");
            issues += invalid_frontmatter.len() as u32;
        }

        // 3. Check for broken Markdown links
        println!("\n[3/5] Cross-document links...");
        let mut broken_links = Vec::new();
        let file_set: std::collections::HashSet<_> = files
            .iter()
            .filter_map(|f| {
                f.strip_prefix(&workspace.root)
                    .ok()
                    .map(|r| r.to_string_lossy().to_string())
            })
            .collect();
        for f in &files {
            if let Ok(doc) = crate::document::Document::parse(f) {
                let links = doc.extract_links();
                for link in links {
                    let target = workspace.root.join(&link);
                    if !target.exists() && !file_set.contains(&link) {
                        broken_links.push((
                            f.strip_prefix(&workspace.root)
                                .unwrap_or(f)
                                .to_string_lossy()
                                .to_string(),
                            link,
                        ));
                    }
                }
            }
        }
        if broken_links.is_empty() {
            println!("  ✅ No broken cross-document links found");
        } else {
            println!("  ⚠️  {} broken links:", broken_links.len());
            for (source, target) in &broken_links {
                println!("     - {} → {} (not found)", source, target);
            }
            issues += broken_links.len() as u32;
        }

        // 4. Check LanceDB store
        println!("\n[4/5] LanceDB store...");
        match workspace.get_store().await {
            Ok(store) => {
                match crate::index_lock::inspect(&store.storage_path) {
                    Ok(crate::index_lock::LockInspection::Active(Some(owner))) => println!(
                        "  ℹ️  Index writer active: PID {} (since {})",
                        owner.pid, owner.acquired_at
                    ),
                    Ok(crate::index_lock::LockInspection::Active(None)) => {
                        println!("  ℹ️  Index writer active (owner metadata unavailable)")
                    }
                    Ok(crate::index_lock::LockInspection::StaleMetadataCleared(owner)) => println!(
                        "  🔧 Cleared stale lock metadata for PID {} (recorded {})",
                        owner.pid, owner.acquired_at
                    ),
                    Ok(crate::index_lock::LockInspection::Unlocked) => {
                        println!("  ✅ Index writer lock is free")
                    }
                    Err(e) => {
                        println!("  ⚠️  Cannot inspect index writer lock: {}", e);
                        issues += 1;
                    }
                }
                match store.open_table().await {
                    Ok(_table) => println!("  ✅ LanceDB table accessible"),
                    Err(e) => {
                        println!("  ⚠️  LanceDB table not accessible: {}", e);
                        issues += 1;
                    }
                }
            }
            Err(e) => {
                println!("  ⚠️  Cannot connect to LanceDB: {}", e);
                issues += 1;
            }
        }

        // 5. Check registry coherence
        println!("\n[5/5] Registry coherence...");
        if let Ok(registry) = crate::config_manager::load_registry() {
            let vault_canon =
                std::fs::canonicalize(&workspace.root).unwrap_or_else(|_| workspace.root.clone());
            let vault_str = vault_canon.to_string_lossy().to_string();
            let mut found = false;
            for proj in registry.projects.values() {
                if let Ok(p) = std::fs::canonicalize(&proj.vault_path)
                    && p.to_string_lossy() == vault_str
                {
                    found = true;
                    println!("  ✅ Project registered in registry.toml");
                    break;
                }
            }
            if !found {
                // Try by code_path
                let code_canon = std::fs::canonicalize(&workspace.code_path)
                    .unwrap_or_else(|_| workspace.code_path.clone());
                let code_str = code_canon.to_string_lossy().to_string();
                for proj in registry.projects.values() {
                    if proj.code_path == code_str {
                        found = true;
                        println!("  ✅ Found by code_path: {}", proj.code_path);
                        break;
                    }
                }
            }
            if !found {
                println!("  ⚠️  Project not found in registry — may be orphaned");
                issues += 1;
            }
        } else {
            println!("  ⚠️  Cannot read registry.toml");
            issues += 1;
        }

        // Summary
        println!("\n{}", "─".repeat(60));
        if issues == 0 {
            println!("✅ All checks passed. Vault is healthy.");
        } else {
            println!(
                "⚠️  {} issue(s) found. Run `rms-memory reindex` or `rms-memory init` to repair.",
                issues
            );
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
        let mut child = std::process::Command::new("tail")
            .arg("-f")
            .arg(&log_file)
            .spawn()?;
        let _ = child.wait()?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct SyncArgs;

impl SyncArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
        let store = workspace.get_store().await?;
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
