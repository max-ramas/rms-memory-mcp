use crate::retrieval::RetrievalService;
use crate::wiki::manifest::WikiManifest;
use crate::wiki::packager::Packager;
use crate::wiki::progress::{WikiEvent, WikiPhase};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct WikiService {
    retrieval: RetrievalService,
    workspace_root: PathBuf,
    scope: String,
    tx: broadcast::Sender<WikiEvent>,
}

impl WikiService {
    pub fn new(retrieval: RetrievalService, workspace_root: PathBuf, scope: String) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            retrieval,
            workspace_root,
            scope,
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WikiEvent> {
        self.tx.subscribe()
    }

    fn emit(&self, phase: WikiPhase) {
        let _ = self.tx.send(WikiEvent::new(phase));
    }

    pub fn wiki_root(&self) -> PathBuf {
        self.workspace_root.join("wiki")
    }

    pub fn manifest_init(&self, _scope: &str) -> WikiManifest {
        WikiManifest::default_manifest()
    }

    pub async fn generate(&self, request: WikiGenerateRequest) -> Result<WikiGenerateResult> {
        self.emit(WikiPhase::Resolving);
        request.manifest.validate()?;

        let packager = Packager::new(self.wiki_root());
        packager.ensure_dirs()?;

        if request.refresh_code {
            let idx = Arc::new(tokio::sync::Mutex::new(
                crate::indexer::Indexer::new().context("Failed to initialize embedding model")?,
            ));
            let temp_store = Arc::new(
                crate::store::Store::init(
                    &crate::workspace::base_dir()
                        .join("dbs")
                        .join("temp")
                        .to_string_lossy(),
                    "memory",
                )
                .await?,
            );
            let ws = crate::workspace::Workspace {
                root: self.workspace_root.clone(),
                code_path: self.workspace_root.clone(),
                include: vec!["**/*".to_string()],
                exclude: vec![
                    "node_modules/**".to_string(),
                    "vendor/**".to_string(),
                    ".git/**".to_string(),
                    "target/**".to_string(),
                    ".next/**".to_string(),
                ],
                code_index_mode: Default::default(),
                code_languages: vec![],
            };
            let mut idx = idx.lock().await;
            crate::code_indexer::index_code_full(&ws, &temp_store, &mut idx).await?;
        }

        let mut sections: Vec<(String, Vec<crate::wiki::providers::ResolvedItem>)> =
            Vec::with_capacity(request.manifest.sections.len());
        let mut diagnostics = crate::wiki::diagnostics::Diagnostics::default();
        let mut total_items = 0usize;

        for (idx, section) in request.manifest.sections.iter().enumerate() {
            self.emit(WikiPhase::Retrieving {
                section: section.id.clone(),
                items_done: idx,
                items_total: request.manifest.sections.len(),
            });

            let mut section_items: Vec<crate::wiki::providers::ResolvedItem> = Vec::new();

            for source in &section.sources {
                match self.resolve_source(source).await {
                    Ok(items) => section_items.extend(items),
                    Err(e) => {
                        diagnostics.add_error(
                            &format!("{}:{}", section.id, source.label()),
                            &e.to_string(),
                        );
                    }
                }
            }

            section_items = crate::wiki::budget::dedup_by_id(section_items);
            let budget = crate::wiki::budget::BudgetManager::new(request.manifest.pack.clone());
            section_items = budget.allocate(section_items);
            total_items += section_items.len();
            sections.push((section.id.clone(), section_items));
        }

        self.emit(WikiPhase::Budgeting);
        if total_items == 0 {
            diagnostics.add_warning("No items retrieved. Check if vault/code index exists.");
        }

        self.emit(WikiPhase::Packaging);
        let context_pack_path = packager.write_context_pack(&sections, &request.manifest)?;
        let _manifest_path = packager.write_manifest(&request.manifest)?;
        let agent_task_path = packager.write_agent_task(&request.manifest)?;
        let sources_path = packager.write_sources(&sections)?;
        let diagnostics_path = packager.write_diagnostics(&diagnostics)?;

        let total_chars: usize = sections
            .iter()
            .flat_map(|(_, items)| items.iter().map(|i| i.char_count))
            .sum();

        let pack_id = self.compute_pack_id(&request.manifest, &sections);

        self.emit(WikiPhase::Complete);

        Ok(WikiGenerateResult {
            context_pack_path,
            agent_task_path,
            wiki_root: self.wiki_root(),
            sources_path,
            diagnostics_path,
            pack_id,
            total_chars,
            sections_generated: sections.len(),
        })
    }

    async fn resolve_source(
        &self,
        source: &crate::wiki::manifest::WikiSource,
    ) -> Result<Vec<crate::wiki::providers::ResolvedItem>> {
        match source {
            crate::wiki::manifest::WikiSource::VaultSearch { queries, limit } => {
                self.resolve_vault_search(queries, limit.unwrap_or(8)).await
            }
            crate::wiki::manifest::WikiSource::CodeSearch { queries, .. } => {
                self.resolve_code_search(queries).await
            }
            crate::wiki::manifest::WikiSource::Files { globs, .. } => {
                self.resolve_files(globs).await
            }
            crate::wiki::manifest::WikiSource::SelfCliHelp { commands } => {
                self.resolve_cli_help(commands).await
            }
        }
    }

    async fn resolve_vault_search(
        &self,
        queries: &[String],
        limit: usize,
    ) -> Result<Vec<crate::wiki::providers::ResolvedItem>> {
        let mut items = Vec::new();
        for query in queries {
            let results = self.retrieval.search_vault(query, limit).await?;
            for result in results {
                let provenance = crate::wiki::providers::ItemProvenance::new("vault", &result.text);
                items.push(crate::wiki::providers::ResolvedItem::new(
                    result.text,
                    crate::wiki::providers::ItemProvenance {
                        path: Some(result.path),
                        retrieval_score: result.score,
                        ..provenance
                    },
                    result.score,
                ));
            }
        }
        Ok(items)
    }

    async fn resolve_code_search(
        &self,
        queries: &[String],
    ) -> Result<Vec<crate::wiki::providers::ResolvedItem>> {
        let mut items = Vec::new();
        for query in queries {
            match self.retrieval.search_code(query, 10).await {
                Ok(results) => {
                    for result in results {
                        let content = format!("```{}\n{}\n```\n", result.language, result.content);
                        let provenance =
                            crate::wiki::providers::ItemProvenance::new("code", &content);
                        items.push(crate::wiki::providers::ResolvedItem::new(
                            content,
                            crate::wiki::providers::ItemProvenance {
                                path: Some(result.file_path),
                                symbol_id: Some(result.qualified_symbol),
                                line_range: Some((
                                    result.start_line as usize,
                                    result.end_line as usize,
                                )),
                                retrieval_score: result.score,
                                ..provenance
                            },
                            result.score,
                        ));
                    }
                }
                Err(e) => {
                    tracing::warn!("Code search failed for '{}': {}", query, e);
                }
            }
        }
        Ok(items)
    }

    async fn resolve_files(
        &self,
        globs: &[String],
    ) -> Result<Vec<crate::wiki::providers::ResolvedItem>> {
        use crate::wiki::providers::{ItemProvenance, ResolvedItem};
        let mut items = Vec::new();

        static EXCLUDE_PATTERNS: &[&str] = &[
            ".env",
            ".env.local",
            ".env.production",
            ".env.development",
            "*.pem",
            "*.key",
            "*secret*",
            "*credential*",
            "*password*",
        ];

        for glob_pattern in globs {
            let full_pattern = self.workspace_root.join(glob_pattern);
            let pattern_str = full_pattern.to_string_lossy().to_string();

            let pat = match glob::Pattern::new(&pattern_str) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Wiki: invalid glob '{}': {}", glob_pattern, e);
                    continue;
                }
            };

            let mut walker = ignore::WalkBuilder::new(&self.workspace_root);
            walker
                .git_ignore(true)
                .git_global(true)
                .parents(true)
                .standard_filters(true);

            for entry in walker.build() {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::debug!("Wiki: walk error: {}", e);
                        continue;
                    }
                };

                if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                    continue;
                }

                let path = entry.path();
                if !pat.matches_path(path) {
                    continue;
                }

                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                let is_excluded = EXCLUDE_PATTERNS.iter().any(|pat| {
                    if pat.contains('*') || pat.contains('?') {
                        glob::Pattern::new(pat)
                            .map(|p| p.matches(&file_name))
                            .unwrap_or(false)
                    } else {
                        file_name.contains(pat)
                    }
                });
                if is_excluded {
                    tracing::debug!("Wiki: excluding sensitive file: {}", path.display());
                    continue;
                }

                let canonical = match std::fs::canonicalize(path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Wiki: cannot canonicalize {}: {}", path.display(), e);
                        continue;
                    }
                };

                if !canonical.starts_with(&self.workspace_root) {
                    tracing::warn!("Wiki: path escapes workspace root: {}", path.display());
                    continue;
                }

                match std::fs::read_to_string(&canonical) {
                    Ok(content) => {
                        let mut provenance = ItemProvenance::new("file", &content);
                        provenance.path = Some(
                            canonical
                                .strip_prefix(&self.workspace_root)
                                .unwrap_or(&canonical)
                                .to_string_lossy()
                                .to_string(),
                        );
                        items.push(ResolvedItem::new(content, provenance, None));
                    }
                    Err(e) => {
                        tracing::warn!("Wiki: failed to read {}: {}", canonical.display(), e);
                    }
                }
            }
        }

        Ok(items)
    }

    async fn resolve_cli_help(
        &self,
        commands: &[String],
    ) -> Result<Vec<crate::wiki::providers::ResolvedItem>> {
        let mut items = Vec::new();
        for cmd in commands {
            let label = if cmd.is_empty() {
                "rms-memory".to_string()
            } else {
                format!("rms-memory {}", cmd)
            };
            let help_text = Self::render_subcommand_help(cmd);
            let content = format!("$ {} --help\n\n{}", label, help_text);
            let provenance = crate::wiki::providers::ItemProvenance::new("cli_help", &content);
            items.push(crate::wiki::providers::ResolvedItem::new(
                content, provenance, None,
            ));
        }
        Ok(items)
    }

    fn render_subcommand_help(subcommand: &str) -> String {
        use clap::CommandFactory;
        let mut app = crate::cli::Cli::command();
        if subcommand.is_empty() {
            return app.render_help().to_string();
        }
        let parts: Vec<&str> = subcommand.split_whitespace().collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if let Some(cmd) = app.find_subcommand(part) {
                app = cmd.clone();
            }
        }
        if let Some(cmd) = app.find_subcommand(parts.last().unwrap_or(&"")) {
            cmd.clone().render_help().to_string()
        } else {
            app.render_help().to_string()
        }
    }

    fn compute_pack_id(
        &self,
        manifest: &WikiManifest,
        sections: &[(String, Vec<crate::wiki::providers::ResolvedItem>)],
    ) -> String {
        let mut input = String::new();
        input.push_str(&manifest.schema_version.to_string());
        input.push_str(&self.scope);
        if let Ok(rev) = Self::git_revision(&self.workspace_root) {
            input.push_str(&rev);
        }
        for (section_id, items) in sections {
            input.push_str(section_id);
            for item in items {
                input.push_str(&item.provenance.content_hash);
            }
        }
        blake3::hash(input.as_bytes()).to_hex().to_string()
    }

    fn git_revision(dir: &std::path::Path) -> Result<String> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .context("Failed to get git revision")?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Ok("unknown".to_string())
        }
    }

    pub async fn clean(&self) -> Result<()> {
        let wiki_root = self.wiki_root();
        let sentinel = wiki_root.join(crate::wiki::manifest::SENTINEL_FILE);
        if !sentinel.exists() {
            anyhow::bail!("Wiki sentinel not found. Refusing to clean non-wiki directory.");
        }
        let gen_dir = self.wiki_root().join(".generation");
        if gen_dir.exists() {
            std::fs::remove_dir_all(&gen_dir).context("Failed to remove .generation directory")?;
        }
        Ok(())
    }
}

pub struct WikiGenerateRequest {
    pub manifest: WikiManifest,
    pub refresh_code: bool,
}

pub struct WikiGenerateResult {
    pub context_pack_path: PathBuf,
    pub agent_task_path: PathBuf,
    pub wiki_root: PathBuf,
    pub sources_path: PathBuf,
    pub diagnostics_path: PathBuf,
    pub pack_id: String,
    pub total_chars: usize,
    pub sections_generated: usize,
}

impl crate::wiki::manifest::WikiSource {
    fn label(&self) -> String {
        match self {
            crate::wiki::manifest::WikiSource::VaultSearch { queries, .. } => {
                format!("vault_search({})", queries.join(","))
            }
            crate::wiki::manifest::WikiSource::CodeSearch { queries, .. } => {
                format!("code_search({})", queries.join(","))
            }
            crate::wiki::manifest::WikiSource::Files { globs, .. } => {
                format!("files({})", globs.join(","))
            }
            crate::wiki::manifest::WikiSource::SelfCliHelp { commands } => {
                format!("cli_help({})", commands.join(","))
            }
        }
    }
}
