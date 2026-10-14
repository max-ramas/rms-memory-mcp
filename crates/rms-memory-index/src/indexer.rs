use crate::embedding::{Backend, EmbeddingProfile};
use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Texts per ONNX run. The runtime keeps its arena at the peak batch size, so
/// larger batches permanently grow every long-lived server process.
const EMBED_BATCH_SIZE: usize = 8;

pub struct Indexer {
    engine: Engine,
    profile: EmbeddingProfile,
    last_used: Instant,
}

enum Engine {
    /// The ONNX session takes ~1.4 GB for the default model; it is loaded on
    /// first use and can be released while idle.
    Fastembed {
        model: EmbeddingModel,
        session: Option<Box<TextEmbedding>>,
    },
    Ollama {
        url: String,
        model: String,
    },
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Completed,
    Busy,
}

/// Model access for long index runs. A shared indexer is locked per batch,
/// so searches embed their query between batches instead of waiting for the
/// whole run.
pub enum EmbedderRef<'a> {
    Exclusive(&'a mut Indexer),
    Shared(&'a tokio::sync::Mutex<Indexer>),
}

impl EmbedderRef<'_> {
    pub async fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Self::Exclusive(indexer) => indexer.embed(texts),
            Self::Shared(indexer) => indexer.lock().await.embed(texts),
        }
    }

    pub fn reborrow(&mut self) -> EmbedderRef<'_> {
        match self {
            Self::Exclusive(indexer) => EmbedderRef::Exclusive(indexer),
            Self::Shared(indexer) => EmbedderRef::Shared(indexer),
        }
    }
}

impl<'a> From<&'a mut Indexer> for EmbedderRef<'a> {
    fn from(indexer: &'a mut Indexer) -> Self {
        Self::Exclusive(indexer)
    }
}

impl<'a> From<&'a mut tokio::sync::MutexGuard<'_, Indexer>> for EmbedderRef<'a> {
    fn from(guard: &'a mut tokio::sync::MutexGuard<'_, Indexer>) -> Self {
        Self::Exclusive(guard)
    }
}

impl<'a> From<&'a tokio::sync::Mutex<Indexer>> for EmbedderRef<'a> {
    fn from(indexer: &'a tokio::sync::Mutex<Indexer>) -> Self {
        Self::Shared(indexer)
    }
}

/// How long an unused embedding model stays loaded in a long-lived process.
/// `RMS_MEMORY_MODEL_IDLE_SECS` overrides the default of five minutes.
pub fn model_idle_timeout() -> Duration {
    std::env::var("RMS_MEMORY_MODEL_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(300))
}

/// Serializes process-local model init: concurrent `Indexer::new` races with
/// hf-hub blob locks, and TMPDIR overrides must not interleave.
static MODEL_INIT_LOCK: Mutex<()> = Mutex::new(());

fn system_temp_is_writable() -> bool {
    let probe = std::env::temp_dir().join(format!(
        "rms-memory-tmp-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl Indexer {
    /// Indexer for the process-wide model (`embedding::active`), loaded now.
    pub fn new() -> Result<Self> {
        Self::for_profile(crate::embedding::active().clone())
    }

    /// Like [`Indexer::new`], but the model is loaded by the first embedding
    /// call. Long-lived servers use this so idle processes stay small.
    pub fn new_lazy() -> Result<Self> {
        Self::unloaded(crate::embedding::active().clone())
    }

    /// A lazy indexer shared by a long-lived process. A background task drops
    /// the model after [`model_idle_timeout`] without embedding calls, so
    /// idle IDE servers and the GUI do not each hold ~1.4 GB. Must be called
    /// inside a Tokio runtime.
    pub fn shared_lazy() -> Result<std::sync::Arc<tokio::sync::Mutex<Self>>> {
        let indexer = std::sync::Arc::new(tokio::sync::Mutex::new(Self::new_lazy()?));
        let weak = std::sync::Arc::downgrade(&indexer);
        let idle = model_idle_timeout();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(idle.min(Duration::from_secs(30)));
            loop {
                tick.tick().await;
                let Some(indexer) = weak.upgrade() else {
                    break;
                };
                if let Ok(mut guard) = indexer.try_lock()
                    && guard.release_if_idle(idle)
                {
                    tracing::info!(
                        "Released the embedding model after {}s idle",
                        idle.as_secs()
                    );
                }
            }
        });
        Ok(indexer)
    }

    pub fn for_profile(profile: EmbeddingProfile) -> Result<Self> {
        let mut indexer = Self::unloaded(profile)?;
        indexer.ensure_loaded()?;
        Ok(indexer)
    }

    fn unloaded(profile: EmbeddingProfile) -> Result<Self> {
        let engine = match &profile.backend {
            Backend::Ollama { url, model } => Engine::Ollama {
                url: url.clone(),
                model: model.clone(),
            },
            Backend::Fastembed => Engine::Fastembed {
                model: crate::embedding::fastembed_model(&profile.id).with_context(|| {
                    format!("No bundled embedding model named `{}`", profile.id)
                })?,
                session: None,
            },
        };
        Ok(Self {
            engine,
            profile,
            last_used: Instant::now(),
        })
    }

    fn ensure_loaded(&mut self) -> Result<()> {
        if let Engine::Fastembed { model, session } = &mut self.engine
            && session.is_none()
        {
            *session = Some(Box::new(Self::load_fastembed(model.clone())?));
        }
        Ok(())
    }

    pub fn is_loaded(&self) -> bool {
        match &self.engine {
            Engine::Fastembed { session, .. } => session.is_some(),
            Engine::Ollama { .. } => true,
        }
    }

    /// Drops the local model when it has not been used for `idle`; the next
    /// embedding call loads it again. Returns whether memory was released.
    pub fn release_if_idle(&mut self, idle: Duration) -> bool {
        if self.last_used.elapsed() < idle {
            return false;
        }
        match &mut self.engine {
            Engine::Fastembed { session, .. } => session.take().is_some(),
            Engine::Ollama { .. } => false,
        }
    }

    pub fn profile(&self) -> &EmbeddingProfile {
        &self.profile
    }

    fn load_fastembed(model: EmbeddingModel) -> Result<TextEmbedding> {
        let _guard = MODEL_INIT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        const ATTEMPTS: u32 = 5;
        let mut last_error = None;
        for attempt in 0..ATTEMPTS {
            match Self::try_load_model(model.clone()) {
                Ok(loaded) => return Ok(loaded),
                Err(error) => {
                    let message = format!("{error:#}");
                    let transient = message.contains("Lock acquisition failed")
                        || message.contains("Failed to retrieve onnx");
                    if !transient || attempt + 1 == ATTEMPTS {
                        return Err(error);
                    }
                    eprintln!(
                        "Retrying embedding model init after cache lock contention (attempt {}/{}): {}",
                        attempt + 1,
                        ATTEMPTS,
                        message
                    );
                    std::thread::sleep(Duration::from_millis(200 * u64::from(attempt + 1)));
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("embedding model init exhausted retries")))
    }

    fn try_load_model(model: EmbeddingModel) -> Result<TextEmbedding> {
        let mut cache_dir = rms_memory_core::workspace::base_dir()
            .join("cache")
            .join("fastembed");

        // If we cannot create the primary cache dir (e.g. HOME is read-only or sandboxed), fallback to temp dir
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            eprintln!(
                "Warning: Failed to create primary cache dir {:?}: {}. Falling back to temp directory.",
                cache_dir, e
            );
            cache_dir = std::env::temp_dir()
                .join("rms-memory")
                .join("cache")
                .join("fastembed");
            std::fs::create_dir_all(&cache_dir)
                .context("Failed to create fallback cache directory")?;
        }

        // Sandboxed hosts (Claude Desktop) may make system temp read-only; fastembed/hf-hub
        // use TMPDIR for atomic downloads. Override only then — always rewriting process
        // TMPDIR races with parallel tests (tempfile under cache/.../tmp + gitignore skips).
        let override_tmpdir = !system_temp_is_writable();
        let original_tmp = std::env::var_os("TMPDIR");
        if override_tmpdir {
            let tmp_dir = cache_dir.join("tmp");
            std::fs::create_dir_all(&tmp_dir).ok();
            // SAFETY: Bounded to this constructor; restored below. Concurrent callers are
            // serialized by MODEL_INIT_LOCK so TMPDIR mutations do not interleave.
            unsafe {
                std::env::set_var("TMPDIR", &tmp_dir);
            }
        }

        eprintln!("Loading embedding model (Cache: {:?})...", cache_dir);
        let result = TextEmbedding::try_new(
            InitOptions::new(model)
                .with_cache_dir(cache_dir.clone())
                .with_intra_threads(1)
                .with_show_download_progress(false),
        );

        if override_tmpdir {
            unsafe {
                if let Some(orig_tmp) = original_tmp {
                    std::env::set_var("TMPDIR", orig_tmp);
                } else {
                    std::env::remove_var("TMPDIR");
                }
            }
        }

        result.context("Failed to initialize fastembed model")
    }

    fn split_large_node(heading: &str, text: &str, chunks: &mut Vec<Chunk>) {
        let lines: Vec<&str> = text.lines().collect();
        let mut current_idx = 0;

        while current_idx < lines.len() {
            let mut chunk_text = String::new();
            let next_idx = current_idx + 1;
            let mut overlap_idx = current_idx;

            while current_idx < lines.len() {
                let line = lines[current_idx];
                chunk_text.push_str(line);
                chunk_text.push('\n');

                // Mark potential overlap start (~1200 chars from beginning of chunk)
                if chunk_text.len() >= 1200 && overlap_idx == next_idx - 1 {
                    overlap_idx = current_idx;
                }

                if chunk_text.len() >= 1500 {
                    break;
                }
                current_idx += 1;
            }

            chunks.push(Chunk {
                heading: heading.to_string(),
                text: chunk_text.trim().to_string(),
            });

            if current_idx < lines.len() {
                current_idx = std::cmp::max(overlap_idx, next_idx);
            }
        }
    }

    pub fn chunk_text(text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let parser = Parser::new_ext(text, Options::all());

        let mut current_heading = String::new();
        let mut current_chunk_text = String::new();
        let mut depth = 0;
        let mut in_heading = false;
        let mut heading_text = String::new();

        let push_current =
            |current_chunk_text: &mut String, current_heading: &str, chunks: &mut Vec<Chunk>| {
                let trimmed = current_chunk_text.trim();
                if !trimmed.is_empty() {
                    if trimmed.len() > 1500 {
                        Self::split_large_node(current_heading, trimmed, chunks);
                    } else {
                        chunks.push(Chunk {
                            heading: current_heading.to_string(),
                            text: trimmed.to_string(),
                        });
                    }
                }
                current_chunk_text.clear();
            };

        for (event, range) in parser.into_offset_iter() {
            match event {
                Event::Start(Tag::Heading { .. }) => {
                    if depth == 0 {
                        push_current(&mut current_chunk_text, &current_heading, &mut chunks);
                        in_heading = true;
                        heading_text.clear();
                    }
                    depth += 1;
                }
                Event::Start(_) => {
                    depth += 1;
                }
                Event::End(pulldown_cmark::TagEnd::Heading(_)) => {
                    depth -= 1;
                    if depth == 0 {
                        in_heading = false;
                        current_heading = heading_text.trim().to_string();
                        // Include heading in the chunk
                        current_chunk_text.push_str(&text[range.clone()]);
                        current_chunk_text.push('\n');
                    }
                }
                Event::End(_) => {
                    depth -= 1;
                    if depth == 0 {
                        let node_text = &text[range.clone()];
                        if !current_chunk_text.trim().is_empty()
                            && current_chunk_text.len() + node_text.len() > 1500
                        {
                            push_current(&mut current_chunk_text, &current_heading, &mut chunks);
                        }
                        current_chunk_text.push_str(node_text);
                        current_chunk_text.push_str("\n\n");
                    }
                }
                Event::Text(t) | Event::Code(t) if in_heading => {
                    heading_text.push_str(&t);
                }
                _ => {}
            }
        }

        push_current(&mut current_chunk_text, &current_heading, &mut chunks);
        chunks
    }

    /// Embed stored passages (note chunks, code segments).
    pub fn embed(&mut self, chunks: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefix = self.profile.passage_prefix.clone();
        self.embed_with_prefix(&prefix, chunks)
    }

    /// Embed search queries; asymmetric models expect a different prefix.
    pub fn embed_queries(&mut self, queries: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefix = self.profile.query_prefix.clone();
        self.embed_with_prefix(&prefix, queries)
    }

    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        self.embed_queries(&[query.to_string()])?
            .into_iter()
            .next()
            .context("Embedding model returned no vector")
    }

    fn embed_with_prefix(&mut self, prefix: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed;
        let texts = if prefix.is_empty() {
            texts
        } else {
            prefixed = texts
                .iter()
                .map(|text| format!("{prefix}{text}"))
                .collect::<Vec<_>>();
            &prefixed
        };
        self.ensure_loaded()?;
        self.last_used = Instant::now();
        let vectors = match &mut self.engine {
            Engine::Fastembed { session, .. } => session
                .as_mut()
                .context("embedding model is not loaded")?
                .embed(texts, Some(EMBED_BATCH_SIZE))?,
            Engine::Ollama { url, model } => crate::embedding::ollama_embed(url, model, texts)?,
        };
        self.last_used = Instant::now();
        if let Some(vector) = vectors.iter().find(|v| v.len() != self.profile.dimension) {
            anyhow::bail!(
                "Embedding model `{}` returned {}-dimensional vectors, expected {}",
                self.profile.id,
                vector.len(),
                self.profile.dimension
            );
        }
        Ok(vectors)
    }
}

/// Modification time of a vault file; a link that escapes the vault falls back
/// to the linker's own mtime rather than reaching outside.
fn vault_file_mtime(workspace: &rms_memory_core::workspace::Workspace, file_path: &Path) -> String {
    let resolved_path = rms_memory_core::link::resolve_link_in_vault_or_self(
        file_path,
        &workspace.root,
        Some(&workspace.code_path),
    );
    std::fs::metadata(&resolved_path)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339())
}

/// Parses a vault document, swapping in linked source content. Escaping links
/// are ignored so indexed content stays inside the vault.
fn load_vault_document(
    workspace: &rms_memory_core::workspace::Workspace,
    file_path: &Path,
) -> Option<rms_memory_core::document::Document> {
    let mut doc = match rms_memory_core::document::Document::parse_for_index(file_path) {
        Ok((doc, frontmatter_error)) => {
            if let Some(error) = frontmatter_error {
                tracing::warn!(
                    "Indexing {} without frontmatter: {error:#}",
                    file_path.display()
                );
            }
            doc
        }
        Err(error) => {
            tracing::error!(
                "Skipping unreadable document {}: {:#}",
                file_path.display(),
                error
            );
            return None;
        }
    };
    if let Some(linked_content) = rms_memory_core::link::get_linked_content_in_vault(
        file_path,
        &workspace.root,
        Some(&workspace.code_path),
    ) {
        doc.content = linked_content;
    }
    strip_managed_rules(&mut doc);
    Some(doc)
}

/// Injected IDE rule blocks repeat the memory protocol in every linked rule
/// file; indexing them only adds identical noise to search results.
fn strip_managed_rules(doc: &mut rms_memory_core::document::Document) {
    if let std::borrow::Cow::Owned(stripped) = rms_memory_core::managed_rules::strip(&doc.content) {
        doc.content = stripped;
    }
}

/// How far the vault index lags behind the Markdown files on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct VaultFreshness {
    pub indexed_files: usize,
    /// Indexed, but changed on disk since.
    pub modified: usize,
    /// On disk, not in the index yet.
    pub added: usize,
    /// In the index, no longer on disk.
    pub removed: usize,
}

impl VaultFreshness {
    pub fn pending(&self) -> usize {
        self.modified + self.added + self.removed
    }

    pub fn describe(&self) -> String {
        if self.pending() == 0 {
            return format!("index up to date ({} files)", self.indexed_files);
        }
        format!(
            "index behind the vault by {} file(s): {} modified, {} new, {} removed; the MCP server syncs while running, or run `rms-memory sync`",
            self.pending(),
            self.modified,
            self.added,
            self.removed
        )
    }
}

/// Compares the vault on disk with the index without changing either.
pub async fn vault_freshness(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
) -> Result<VaultFreshness> {
    let table = store.open_table().await?;
    let indexed = store.get_file_timestamps(&table).await?;
    let mut freshness = VaultFreshness {
        indexed_files: indexed.len(),
        ..Default::default()
    };
    let mut on_disk = std::collections::HashSet::new();
    for file_path in workspace.find_markdown_files()? {
        let rel_path = file_path
            .strip_prefix(&workspace.root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();
        match indexed.get(&rel_path) {
            Some((_, stored)) if vault_file_mtime(workspace, &file_path) > *stored => {
                freshness.modified += 1
            }
            Some(_) => {}
            // Notes without indexable text (only frontmatter or an embed)
            // never get index rows.
            None if load_vault_document(workspace, &file_path)
                .is_none_or(|doc| Indexer::chunk_text(&doc.content).is_empty()) => {}
            None => freshness.added += 1,
        }
        on_disk.insert(rel_path);
    }
    freshness.removed = indexed
        .keys()
        .filter(|path| {
            !on_disk.contains(*path)
                && !rms_memory_core::path_policy::is_vault_wiki_relative_path(path)
        })
        .count();
    Ok(freshness)
}

pub async fn sync_vault<'a>(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
    indexer: impl Into<EmbedderRef<'a>>,
) -> Result<()> {
    let _lock = crate::index_lock::acquire(&store.storage_path).await?;
    sync_vault_inner(workspace, store, indexer.into()).await
}

pub async fn try_sync_vault<'a>(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
    indexer: impl Into<EmbedderRef<'a>>,
) -> Result<SyncStatus> {
    let Some(_lock) = crate::index_lock::try_acquire(&store.storage_path)? else {
        return Ok(SyncStatus::Busy);
    };
    sync_vault_inner(workspace, store, indexer.into()).await?;
    Ok(SyncStatus::Completed)
}

async fn sync_vault_inner(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
    mut indexer: EmbedderRef<'_>,
) -> Result<()> {
    // Try to open existing table
    let table = match store.open_table().await {
        Ok(t) => t,
        Err(_) => {
            // Fallback to full index
            return index_vault_full_inner(workspace, store, indexer).await;
        }
    };

    let mut existing_docs = match store.get_all_document_timestamps(&table).await {
        Ok(docs) => docs,
        Err(e) => {
            tracing::error!(
                "Failed to read document timestamps: {}. Performing full recheck.",
                e
            );
            std::collections::HashMap::new()
        }
    };
    // Path-based cache: skip parsing files whose mtime hasn't changed.
    // Returns map of path → (doc_id, last_seen_mtime).
    let mut path_info = match store.get_file_timestamps(&table).await {
        Ok(ts) => ts,
        Err(e) => {
            tracing::error!(
                "Failed to read file timestamps: {}. Falling back to full sync.",
                e
            );
            std::collections::HashMap::new()
        }
    };
    // One-time/continuous migration: purge any Wiki chunks written by older
    // versions. Delete by path (not document_id) because a generated page can
    // carry the same frontmatter ID as its canonical source.
    let excluded_paths = path_info
        .keys()
        .filter(|path| rms_memory_core::path_policy::is_vault_wiki_relative_path(path))
        .cloned()
        .collect::<Vec<_>>();
    let purged_excluded = !excluded_paths.is_empty();
    if purged_excluded {
        tracing::info!(
            "Sync: purging {} legacy Wiki path(s) from canonical Vault index",
            excluded_paths.len()
        );
        store.delete_document_paths(&table, &excluded_paths).await?;
        // Re-read both views after deletion so duplicate document IDs cannot
        // leave stale timestamps or suppress canonical re-indexing.
        existing_docs = store.get_all_document_timestamps(&table).await?;
        path_info = store.get_file_timestamps(&table).await?;
    }
    // Rows indexed before injected rule blocks were stripped; dropping them
    // makes their files look new, so they are reindexed below.
    let managed_paths = store.paths_containing_managed_rules(&table).await?;
    if !managed_paths.is_empty() {
        tracing::info!(
            "Sync: reindexing {} file(s) indexed with injected IDE rule blocks",
            managed_paths.len()
        );
        store.delete_document_paths(&table, &managed_paths).await?;
        existing_docs = store.get_all_document_timestamps(&table).await?;
        path_info = store.get_file_timestamps(&table).await?;
    }
    let purged_code = crate::code_indexer::purge_wiki_code_records(workspace, store).await?;
    let purged_graph_nodes = crate::vault_graph::purge_wiki_graph_records(workspace, store).await?;
    if purged_code > 0 || purged_graph_nodes > 0 {
        tracing::info!(
            "Sync: purged {purged_code} legacy Wiki code path(s) and {purged_graph_nodes} graph node(s)"
        );
    }

    let mut to_delete = Vec::new();
    let mut to_index = Vec::new();
    // Unchanged files skipped by the mtime fast path, keyed by doc_id. Deleting
    // a doc_id removes every chunk sharing it, so these must be reindexed too.
    let mut unchanged_by_doc: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();

    let files = workspace.find_markdown_files()?;
    let link_resolver =
        rms_memory_core::vault_links::VaultLinkIndex::from_files(&workspace.root, &files);
    let mut current_doc_ids = std::collections::HashSet::new();

    for file_path in files {
        let rel_path = file_path
            .strip_prefix(&workspace.root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();
        let mtime = vault_file_mtime(workspace, &file_path);

        // Fast path: skip if mtime unchanged, but still mark doc_id as current
        if let Some((doc_id, stored_ts)) = path_info.get(&rel_path)
            && &mtime <= stored_ts
        {
            current_doc_ids.insert(doc_id.clone());
            unchanged_by_doc
                .entry(doc_id.clone())
                .or_default()
                .push(file_path);
            continue;
        }

        // Past the fast path the file is either new at this path (created,
        // renamed, moved) or modified, so it is always reindexed. Comparing
        // against the doc_id timestamp would miss renames, which keep both
        // the frontmatter id and the mtime.
        let Some(doc) = load_vault_document(workspace, &file_path) else {
            continue;
        };
        let doc_id = doc.index_id(Path::new(&rel_path));
        current_doc_ids.insert(doc_id.clone());
        if existing_docs.contains_key(&doc_id) {
            to_delete.push(doc_id.clone());
        }
        to_index.push((file_path, doc, mtime, doc_id));
    }

    // Check for deleted files
    for doc_id in existing_docs.keys() {
        if !current_doc_ids.contains(doc_id) {
            to_delete.push(doc_id.clone());
        }
    }
    to_delete.sort();
    to_delete.dedup();

    for doc_id in &to_delete {
        for file_path in unchanged_by_doc.remove(doc_id).unwrap_or_default() {
            let Some(doc) = load_vault_document(workspace, &file_path) else {
                continue;
            };
            let mtime = vault_file_mtime(workspace, &file_path);
            to_index.push((file_path, doc, mtime, doc_id.clone()));
        }
    }

    let graph_needs_reconcile =
        purged_excluded || purged_graph_nodes > 0 || !to_delete.is_empty() || !to_index.is_empty();

    if to_index.is_empty() && to_delete.is_empty() {
        tracing::info!("Sync: No changes detected. Vault is up to date.");
        if let Err(error) = store.index_new_rows(&table).await {
            tracing::warn!("Failed to add new rows to the vault search index: {error:#}");
        }
        if graph_needs_reconcile {
            crate::vault_graph::reconcile_vault_links(workspace, store).await?;
        }
        return Ok(());
    }

    // Embed first and swap rows at the end: searches during a long sync still
    // find the previous version of changed notes.
    tracing::info!("Sync: Indexing {} modified/new files...", to_index.len());
    let mut records = Vec::new();
    for (file_path, doc, mtime, doc_id) in to_index {
        let rel_path = file_path
            .strip_prefix(&workspace.root)
            .unwrap_or(&file_path);
        let title = rel_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let doc_type = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.doc_type.clone())
            .unwrap_or_else(|| "note".to_string());
        let content_hash = blake3::hash(doc.content.as_bytes()).to_string();
        let confidence = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.confidence)
            .map(|c| c as f32);
        let status = doc.frontmatter.as_ref().and_then(|fm| fm.status.clone());
        let supersedes = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.supersedes.clone());
        let superseded_by = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.superseded_by.clone());
        let valid_from = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.valid_from.clone());
        let valid_until = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.valid_until.clone());
        let pinned = doc.frontmatter.as_ref().and_then(|fm| fm.pinned);

        let (links_raw_str, links_resolved_str) =
            link_columns(&link_resolver, &workspace.root, &file_path, &doc)?;

        let chunks = Indexer::chunk_text(&doc.content);
        if chunks.is_empty() {
            continue;
        }

        let mut embeddings = Vec::with_capacity(chunks.len());
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let batch_size = 32;
        let mut failed = false;

        for batch in chunk_texts.chunks(batch_size) {
            match indexer.embed(batch).await {
                Ok(mut e) => embeddings.append(&mut e),
                Err(err) => {
                    tracing::error!("Failed to embed batch for {}: {}", title, err);
                    failed = true;
                    break;
                }
            }
        }

        if failed {
            continue;
        }

        for (i, (chunk, vector)) in chunks.into_iter().zip(embeddings).enumerate() {
            records.push(crate::store::ChunkRecord {
                document_id: doc_id.clone(),
                path: rel_path.to_string_lossy().to_string(),
                doc_type: doc_type.clone(),
                title: title.clone(),
                content_hash: content_hash.clone(),
                updated_at: mtime.clone(),
                links_raw: links_raw_str.clone(),
                links_resolved: links_resolved_str.clone(),
                chunk_index: i as u32,
                heading: chunk.heading,
                text: chunk.text,
                vector,
                confidence,
                status: status.clone(),
                supersedes: supersedes.clone(),
                superseded_by: superseded_by.clone(),
                valid_from: valid_from.clone(),
                valid_until: valid_until.clone(),
                pinned,
            });
        }
    }

    for doc_id in &to_delete {
        tracing::info!("Sync: Deleting outdated/orphaned document_id: {}", doc_id);
        if let Err(e) = store.delete_document(&table, doc_id).await {
            tracing::warn!("Failed to delete document {doc_id}: {e}");
        }
    }
    if !records.is_empty() {
        store.insert_batch(&table, records).await?;
        tracing::info!("Sync: Upsert complete.");
    }
    if let Err(error) = store.index_new_rows(&table).await {
        tracing::warn!("Failed to add new rows to the vault search index: {error:#}");
    }
    if graph_needs_reconcile {
        crate::vault_graph::reconcile_vault_links(workspace, store).await?;
    }

    Ok(())
}

pub async fn index_vault_full<'a>(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
    indexer: impl Into<EmbedderRef<'a>>,
) -> Result<()> {
    let _lock = crate::index_lock::acquire(&store.storage_path).await?;
    index_vault_full_inner(workspace, store, indexer.into()).await
}

async fn index_vault_full_inner(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
    mut indexer: EmbedderRef<'_>,
) -> Result<()> {
    if let Err(e) = store.db.drop_table(&store.table_name, &[]).await {
        tracing::warn!("Failed to drop memory table: {e}");
    }
    let table = store.create_table().await?;

    let files = workspace.find_markdown_files()?;
    tracing::info!("Full Reindex: Found {} markdown files", files.len());
    let link_resolver =
        rms_memory_core::vault_links::VaultLinkIndex::from_files(&workspace.root, &files);

    let mut records = Vec::new();
    for file_path in files {
        let mtime = std::fs::metadata(&file_path)
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
            .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());

        let mut doc = match rms_memory_core::document::Document::parse_for_index(&file_path) {
            Ok((doc, frontmatter_error)) => {
                if let Some(error) = frontmatter_error {
                    tracing::warn!(
                        "Indexing {} without frontmatter: {error:#}",
                        file_path.display()
                    );
                }
                doc
            }
            Err(error) => {
                tracing::error!(
                    "Skipping unreadable document {}: {:#}",
                    file_path.display(),
                    error
                );
                continue;
            }
        };

        // If it's a linked document, swap the content with the source file content.
        // Escaping links are ignored so indexed content stays inside the vault.
        if let Some(linked_content) = rms_memory_core::link::get_linked_content_in_vault(
            &file_path,
            &workspace.root,
            Some(&workspace.code_path),
        ) {
            doc.content = linked_content;
        }
        strip_managed_rules(&mut doc);

        if doc.content.trim().is_empty() {
            continue;
        }

        let rel_path = file_path
            .strip_prefix(&workspace.root)
            .unwrap_or(&file_path);
        let doc_id = doc.index_id(rel_path);
        let title = rel_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let doc_type = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.doc_type.clone())
            .unwrap_or_else(|| "note".to_string());
        let content_hash = blake3::hash(doc.content.as_bytes()).to_string();
        let confidence = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.confidence)
            .map(|c| c as f32);
        let status = doc.frontmatter.as_ref().and_then(|fm| fm.status.clone());
        let supersedes = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.supersedes.clone());
        let superseded_by = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.superseded_by.clone());
        let valid_from = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.valid_from.clone());
        let valid_until = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.valid_until.clone());
        let pinned = doc.frontmatter.as_ref().and_then(|fm| fm.pinned);

        let (links_raw_str, links_resolved_str) =
            link_columns(&link_resolver, &workspace.root, &file_path, &doc)?;

        let chunks = Indexer::chunk_text(&doc.content);
        if chunks.is_empty() {
            continue;
        }

        let mut embeddings = Vec::with_capacity(chunks.len());
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let batch_size = 32;
        let mut failed = false;

        for batch in chunk_texts.chunks(batch_size) {
            match indexer.embed(batch).await {
                Ok(mut e) => embeddings.append(&mut e),
                Err(err) => {
                    tracing::error!("Failed to embed batch for {}: {}", title, err);
                    failed = true;
                    break;
                }
            }
        }

        if failed {
            continue;
        }

        for (i, (chunk, vector)) in chunks.into_iter().zip(embeddings).enumerate() {
            records.push(crate::store::ChunkRecord {
                document_id: doc_id.clone(),
                path: rel_path.to_string_lossy().to_string(),
                doc_type: doc_type.clone(),
                title: title.clone(),
                content_hash: content_hash.clone(),
                updated_at: mtime.clone(),
                links_raw: links_raw_str.clone(),
                links_resolved: links_resolved_str.clone(),
                chunk_index: i as u32,
                heading: chunk.heading,
                text: chunk.text,
                vector,
                confidence,
                status: status.clone(),
                supersedes: supersedes.clone(),
                superseded_by: superseded_by.clone(),
                valid_from: valid_from.clone(),
                valid_until: valid_until.clone(),
                pinned,
            });
        }
    }

    if !records.is_empty() {
        store.insert_batch(&table, records).await?;
        tracing::info!("Full Reindex complete.");
    } else {
        tracing::info!("Full Reindex: No indexable content found.");
    }
    // Built after the rows exist so they are covered by it.
    store.create_fts_index(&table).await?;
    crate::vault_graph::reconcile_vault_links(workspace, store).await?;
    crate::code_indexer::purge_wiki_code_records(workspace, store).await?;
    crate::vault_graph::purge_wiki_graph_records(workspace, store).await?;

    Ok(())
}

/// JSON columns with link targets as written and their vault resolution.
fn link_columns(
    resolver: &rms_memory_core::vault_links::VaultLinkIndex,
    root: &Path,
    file_path: &Path,
    doc: &rms_memory_core::document::Document,
) -> Result<(String, String)> {
    use rms_memory_core::vault_links::LinkResolution;
    let source = file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\\', "/");
    let links = doc.vault_links();
    let raw: Vec<&str> = links.iter().map(|link| link.target.as_str()).collect();
    let resolved: Vec<String> = links
        .iter()
        .map(|link| match resolver.resolve(&source, link) {
            LinkResolution::Note(path)
            | LinkResolution::Attachment(path)
            | LinkResolution::Missing(path) => path,
        })
        .collect();
    Ok((
        serde_json::to_string(&raw)?,
        serde_json::to_string(&resolved)?,
    ))
}

#[cfg(test)]
mod sync_tests {
    async fn indexed_paths(store: &crate::store::Store) -> Vec<String> {
        let table = store.open_table().await.unwrap();
        let mut paths: Vec<String> = store
            .get_file_timestamps(&table)
            .await
            .unwrap()
            .into_keys()
            .collect();
        paths.sort();
        paths
    }

    #[tokio::test]
    async fn sync_follows_renames_and_keeps_shared_id_copies() {
        let directory = tempfile::tempdir().unwrap();
        let vault = directory.path().join("vault");
        std::fs::create_dir_all(vault.join("notes")).unwrap();
        std::fs::create_dir_all(vault.join(".rms-memory/trash")).unwrap();
        std::fs::write(
            vault.join("notes/old-name.md"),
            "---\nid: fixed-id-1\n---\n# Old\n\nrename body\n",
        )
        .unwrap();
        std::fs::write(
            vault.join(".rms-memory/trash/deleted.md"),
            "---\nid: trashed-1\n---\n# Trashed\n\nnot indexed\n",
        )
        .unwrap();
        let workspace = rms_memory_core::workspace::Workspace {
            root: vault.clone(),
            code_path: directory.path().join("code"),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: rms_memory_core::workspace::CodeIndexMode::Manual,
            code_languages: vec![],
        };
        let store =
            crate::store::Store::init(&directory.path().join("db").to_string_lossy(), "memory")
                .await
                .unwrap();
        let mut indexer = crate::indexer::Indexer::new().unwrap();
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        assert_eq!(indexed_paths(&store).await, vec!["notes/old-name.md"]);

        std::fs::rename(
            vault.join("notes/old-name.md"),
            vault.join("notes/new-name.md"),
        )
        .unwrap();
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        assert_eq!(indexed_paths(&store).await, vec!["notes/new-name.md"]);
        assert!(
            store
                .read_document("notes/new-name.md")
                .await
                .unwrap()
                .contains("rename body")
        );

        // A copy shares the frontmatter id; reindexing it must not drop the
        // unchanged original.
        std::fs::copy(vault.join("notes/new-name.md"), vault.join("notes/copy.md")).unwrap();
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        assert_eq!(
            indexed_paths(&store).await,
            vec!["notes/copy.md", "notes/new-name.md"]
        );

        std::fs::write(vault.join("notes/embed-only.md"), "![[board.base]]\n").unwrap();
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        let fresh = super::vault_freshness(&workspace, &store).await.unwrap();
        assert_eq!(fresh.pending(), 0, "{fresh:?}");

        std::fs::write(vault.join("notes/added.md"), "# Added\n\nnew body\n").unwrap();
        std::fs::remove_file(vault.join("notes/copy.md")).unwrap();
        let stale = super::vault_freshness(&workspace, &store).await.unwrap();
        assert_eq!((stale.added, stale.removed, stale.modified), (1, 1, 0));
    }

    #[tokio::test]
    async fn injected_rule_blocks_are_not_indexed_and_legacy_rows_are_reindexed() {
        use rms_memory_core::managed_rules::{END_MARKER, START_MARKER};
        let directory = tempfile::tempdir().unwrap();
        let vault = directory.path().join("vault");
        let code = directory.path().join("code");
        std::fs::create_dir_all(vault.join("rules")).unwrap();
        std::fs::create_dir_all(vault.join("notes")).unwrap();
        std::fs::create_dir_all(&code).unwrap();
        let block =
            format!("{START_MARKER}\n# RMS Memory protocol\n\nsearch first\n{END_MARKER}\n");
        std::fs::write(code.join("AGENTS.md"), &block).unwrap();
        std::fs::write(
            vault.join("rules/AGENTS.md"),
            "---\nid: stub-1\nlink: ../../code/AGENTS.md\n---\n",
        )
        .unwrap();
        std::fs::write(
            vault.join("notes/own.md"),
            format!("# Own rules\n\nalways run the linter\n\n{block}"),
        )
        .unwrap();
        let workspace = rms_memory_core::workspace::Workspace {
            root: vault.clone(),
            code_path: code.clone(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: rms_memory_core::workspace::CodeIndexMode::Manual,
            code_languages: vec![],
        };
        let store =
            crate::store::Store::init(&directory.path().join("db").to_string_lossy(), "memory")
                .await
                .unwrap();
        let mut indexer = crate::indexer::Indexer::new().unwrap();
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        assert_eq!(indexed_paths(&store).await, vec!["notes/own.md"]);
        let own = store.read_document("notes/own.md").await.unwrap();
        assert!(own.contains("always run the linter"), "{own}");
        assert!(!own.contains("search first"), "{own}");

        // Rows written by an older version still hold the block.
        let table = store.open_table().await.unwrap();
        table
            .update()
            .only_if("path = 'notes/own.md'")
            .column("text", format!("'{START_MARKER} search first'"))
            .execute()
            .await
            .unwrap();
        assert_eq!(
            store.paths_containing_managed_rules(&table).await.unwrap(),
            vec!["notes/own.md"]
        );
        super::sync_vault(&workspace, &store, &mut indexer)
            .await
            .unwrap();
        let table = store.open_table().await.unwrap();
        assert!(
            store
                .paths_containing_managed_rules(&table)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(indexed_paths(&store).await, vec!["notes/own.md"]);
        assert!(
            store
                .read_document("notes/own.md")
                .await
                .unwrap()
                .contains("always run the linter")
        );
    }
}
