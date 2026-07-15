use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::path::Path;

pub struct Indexer {
    pub model: TextEmbedding,
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

impl Indexer {
    pub fn new() -> Result<Self> {
        let mut cache_dir = crate::workspace::base_dir().join("cache").join("fastembed");

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

        // Workaround: Claude Desktop sandbox might make the system temp_dir read-only.
        // fastembed (via hf-hub and tempfile) uses the system temp directory for atomic downloads.
        // We override TMPDIR to our cache directory which we know is writable.
        let tmp_dir = cache_dir.join("tmp");
        std::fs::create_dir_all(&tmp_dir).ok();
        let original_tmp = std::env::var_os("TMPDIR");
        // SAFETY: We temporarily override TMPDIR to a user-writable directory because
        // fastembed (via hf-hub and tempfile) uses the system /tmp for atomic downloads.
        // In sandboxed environments (Claude Desktop, macOS sandbox), /tmp may be read-only.
        // We save and restore the original value immediately after model initialization,
        // so this mutation is bounded to the Indexer constructor scope.
        unsafe {
            std::env::set_var("TMPDIR", &tmp_dir);
        }

        let original_dir = std::env::current_dir().ok();
        std::env::set_current_dir(&cache_dir).ok();

        eprintln!("Loading embedding model (Cache: {:?})...", cache_dir);
        let result = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::MultilingualE5Small)
                .with_cache_dir(cache_dir.clone())
                .with_intra_threads(1)
                .with_show_download_progress(false),
        );

        if let Some(orig) = original_dir {
            std::env::set_current_dir(orig).ok();
        }

        unsafe {
            if let Some(orig_tmp) = original_tmp {
                std::env::set_var("TMPDIR", orig_tmp);
            } else {
                std::env::remove_var("TMPDIR");
            }
        }

        let model = result.context("Failed to initialize fastembed model")?;
        Ok(Self { model })
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

    pub fn embed(&mut self, chunks: &[String]) -> Result<Vec<Vec<f32>>> {
        let embeddings = self.model.embed(chunks, None)?;
        Ok(embeddings)
    }
}

pub async fn sync_vault(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut Indexer,
) -> Result<()> {
    let _lock = crate::index_lock::acquire(&store.storage_path).await?;
    sync_vault_inner(workspace, store, indexer).await
}

pub async fn try_sync_vault(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut Indexer,
) -> Result<SyncStatus> {
    let Some(_lock) = crate::index_lock::try_acquire(&store.storage_path)? else {
        return Ok(SyncStatus::Busy);
    };
    sync_vault_inner(workspace, store, indexer).await?;
    Ok(SyncStatus::Completed)
}

async fn sync_vault_inner(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut Indexer,
) -> Result<()> {
    // Try to open existing table
    let table = match store.open_table().await {
        Ok(t) => t,
        Err(_) => {
            // Fallback to full index
            return index_vault_full_inner(workspace, store, indexer).await;
        }
    };

    let existing_docs = match store.get_all_document_timestamps(&table).await {
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
    let path_info = match store.get_file_timestamps(&table).await {
        Ok(ts) => ts,
        Err(e) => {
            tracing::error!(
                "Failed to read file timestamps: {}. Falling back to full sync.",
                e
            );
            std::collections::HashMap::new()
        }
    };
    let mut to_delete = Vec::new();
    let mut to_index = Vec::new();

    let files = workspace.find_markdown_files()?;
    let mut current_doc_ids = std::collections::HashSet::new();

    for file_path in files {
        let rel_path = file_path
            .strip_prefix(&workspace.root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        // Read file mtime
        let resolved_path = crate::link::resolve_link(&file_path);
        let mtime = std::fs::metadata(&resolved_path)
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
            .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());

        // Fast path: skip if mtime unchanged, but still mark doc_id as current
        if let Some((doc_id, stored_ts)) = path_info.get(&rel_path)
            && &mtime <= stored_ts
        {
            current_doc_ids.insert(doc_id.clone());
            continue;
        }

        let mut doc = match crate::document::Document::parse(&file_path) {
            Ok(d) => d,
            Err(error) => {
                tracing::error!(
                    "Skipping invalid document {}: {:#}",
                    file_path.display(),
                    error
                );
                continue;
            }
        };
        let doc_id = doc.index_id(Path::new(&rel_path));

        // If it's a linked document, swap the content with the source file content
        if let Some(linked_content) = crate::link::get_linked_content(&file_path) {
            doc.content = linked_content;
        }

        current_doc_ids.insert(doc_id.clone());

        let needs_update = if let Some(stored_time) = existing_docs.get(&doc_id) {
            &mtime > stored_time
        } else {
            true // New file
        };

        if needs_update {
            if existing_docs.contains_key(&doc_id) {
                to_delete.push(doc_id.clone());
            }
            to_index.push((file_path, doc, mtime, doc_id));
        }
    }

    // Check for deleted files
    for doc_id in existing_docs.keys() {
        if !current_doc_ids.contains(doc_id) {
            to_delete.push(doc_id.clone());
        }
    }

    let graph_needs_reconcile = !to_delete.is_empty() || !to_index.is_empty();

    // 1. Delete old vectors
    for doc_id in &to_delete {
        tracing::info!("Sync: Deleting outdated/orphaned document_id: {}", doc_id);
        if let Err(e) = store.delete_document(&table, doc_id).await {
            tracing::warn!("Failed to delete document {doc_id}: {e}");
        }
    }

    // 2. Insert new vectors
    if to_index.is_empty() {
        tracing::info!("Sync: No changes detected. Vault is up to date.");
        if graph_needs_reconcile {
            crate::vault_graph::reconcile_vault_links(workspace, store).await?;
        }
        return Ok(());
    }

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

        let raw_links = doc.extract_links();
        let mut normalized_links = Vec::new();
        for link in &raw_links {
            normalized_links.push(crate::indexer::normalize_link(
                &workspace.root,
                &file_path,
                link,
            ));
        }

        let links_raw_str = serde_json::to_string(&raw_links)?;
        let links_resolved_str = serde_json::to_string(&normalized_links)?;

        let chunks = Indexer::chunk_text(&doc.content);
        if chunks.is_empty() {
            continue;
        }

        let mut embeddings = Vec::with_capacity(chunks.len());
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let batch_size = 8;
        let mut failed = false;

        for batch in chunk_texts.chunks(batch_size) {
            match indexer.embed(batch) {
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
            });
        }
    }

    if !records.is_empty() {
        store.insert_batch(&table, records).await?;
        tracing::info!("Sync: Upsert complete.");
    }
    if graph_needs_reconcile {
        crate::vault_graph::reconcile_vault_links(workspace, store).await?;
    }

    Ok(())
}

pub async fn index_vault_full(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut Indexer,
) -> Result<()> {
    let _lock = crate::index_lock::acquire(&store.storage_path).await?;
    index_vault_full_inner(workspace, store, indexer).await
}

async fn index_vault_full_inner(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut Indexer,
) -> Result<()> {
    if let Err(e) = store.db.drop_table(&store.table_name, &[]).await {
        tracing::warn!("Failed to drop memory table: {e}");
    }
    let table = store.create_table().await?;
    store.create_fts_index(&table).await?;

    let files = workspace.find_markdown_files()?;
    tracing::info!("Full Reindex: Found {} markdown files", files.len());

    let mut records = Vec::new();
    for file_path in files {
        let mtime = std::fs::metadata(&file_path)
            .and_then(|m| m.modified())
            .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
            .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339());

        let mut doc = match crate::document::Document::parse(&file_path) {
            Ok(d) => d,
            Err(error) => {
                tracing::error!(
                    "Skipping invalid document {}: {:#}",
                    file_path.display(),
                    error
                );
                continue;
            }
        };

        // If it's a linked document, swap the content with the source file content
        if let Some(linked_content) = crate::link::get_linked_content(&file_path) {
            doc.content = linked_content;
        }

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

        let raw_links = doc.extract_links();
        let mut normalized_links = Vec::new();
        for link in &raw_links {
            normalized_links.push(crate::indexer::normalize_link(
                &workspace.root,
                &file_path,
                link,
            ));
        }

        let links_raw_str = serde_json::to_string(&raw_links)?;
        let links_resolved_str = serde_json::to_string(&normalized_links)?;

        let chunks = Indexer::chunk_text(&doc.content);
        if chunks.is_empty() {
            continue;
        }

        let mut embeddings = Vec::with_capacity(chunks.len());
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let batch_size = 8;
        let mut failed = false;

        for batch in chunk_texts.chunks(batch_size) {
            match indexer.embed(batch) {
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
            });
        }
    }

    if !records.is_empty() {
        store.insert_batch(&table, records).await?;
        tracing::info!("Full Reindex complete.");
    } else {
        tracing::info!("Full Reindex: No indexable content found.");
    }
    crate::vault_graph::reconcile_vault_links(workspace, store).await?;

    Ok(())
}

pub fn normalize_link(workspace_root: &Path, current_file: &Path, link: &str) -> String {
    let mut current_dir = current_file.parent().unwrap_or(Path::new("")).to_path_buf();
    for part in link.split('/') {
        if part == "." {
            continue;
        } else if part == ".." {
            current_dir.pop();
        } else {
            current_dir.push(part);
        }
    }

    if let Ok(rel) = current_dir.strip_prefix(workspace_root) {
        rel.to_string_lossy().to_string()
    } else {
        current_dir.to_string_lossy().to_string()
    }
}
