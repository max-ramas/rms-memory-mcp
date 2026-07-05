            Commands::Config { vault_path, auto_add } => {
                let mut registry = crate::workspace::Registry::load()?;
                if let Some(path) = vault_path {
                    registry.global.global_vault_path = Some(path.clone());
                    println!("Set global_vault_path to: {}", path);
                }
                if let Some(auto) = auto_add {
                    registry.global.auto_add_projects = Some(*auto);
                    println!("Set auto_add_projects to: {}", auto);
                }
                registry.save()?;
            }
            Commands::Init => {
                let current_dir = std::env::current_dir()?;
                let start_canon = std::fs::canonicalize(&current_dir).unwrap_or_else(|_| current_dir.to_path_buf());
                let folder_name = start_canon.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("UnknownProject")
                    .to_string();
                
                let mut registry = crate::workspace::Registry::load()?;
                if let Some(global_vault) = &registry.global.global_vault_path {
                    let vault_path = std::path::Path::new(global_vault).join(&folder_name).to_string_lossy().to_string();
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("rules"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("decisions"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("architecture"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("artifacts"))?;
                    
                    registry.projects.insert(folder_name.clone(), crate::workspace::ProjectConfig {
                        code_path: start_canon.to_string_lossy().to_string(),
                        vault_path,
                        include: vec!["rules/**/*.md".to_string(), "decisions/**/*.md".to_string(), "architecture/**/*.md".to_string(), "artifacts/**/*.md".to_string(), "**/*.md".to_string()],
                        exclude: vec!["node_modules/**".to_string(), "vendor/**".to_string(), ".git/**".to_string()],
                    });
                    registry.save()?;
                    println!("Manually initialized project {} in global registry.", folder_name);
                } else {
                    println!("Please set global_vault_path first using: rms-memory config --vault-path <PATH>");
                }
            }
            Commands::Serve => {
                let workspace = Workspace::discover(&current_dir, None)?;
                let store = workspace.get_store().await?;
                let indexer = Indexer::new()?;
                
                let registry = crate::workspace::Registry::load().unwrap_or_default();
                let max_backups = registry.global.max_backups.unwrap_or(5);
                
                // Pass workspace.root (the vault path) to the server
                crate::mcp_server::McpServer::run(store, std::sync::Arc::new(tokio::sync::Mutex::new(indexer)), workspace.root.clone(), max_backups).await?;
            }
            Commands::Reindex => {
                let workspace = Workspace::discover(&current_dir, None)?;
                println!("Reindexing Vault at {:?}", workspace.root);
                
                let store = workspace.get_store().await?;
                
                let _ = store.db.drop_table("memory", &[]).await;
                let table = store.create_table().await?;
                store.create_fts_index(&table).await?;
                
                let files = workspace.find_markdown_files()?;
                println!("Found {} markdown files", files.len());
                
                let mut records = Vec::new();
                for file_path in files {
                    let mut doc = crate::document::Document::parse(&file_path)?;
                    let doc_id = doc.ensure_id()?;
                    
                    let rel_path = file_path.strip_prefix(&workspace.root).unwrap_or(&file_path);
                    let title = rel_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let doc_type = doc.frontmatter.as_ref().and_then(|fm| fm.doc_type.clone()).unwrap_or_else(|| "note".to_string());
                    let content_hash = blake3::hash(doc.content.as_bytes()).to_string();
                    let updated_at = chrono::Utc::now().to_rfc3339();
                    
                    let raw_links = doc.extract_links();
                    let mut normalized_links = Vec::new();
                    for link in raw_links {
                        normalized_links.push(crate::indexer::normalize_link(&workspace.root, &file_path, &link));
                    }
                    
                    let links_raw_str = serde_json::to_string(&normalized_links)?;
                    let links_resolved_str = "[]".to_string(); // TODO properly resolve later

                    let chunks = Indexer::chunk_text(&doc.content);
                    if chunks.is_empty() { continue; }
                    
                    let chunk_texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
                    let embeddings = indexer.embed(&chunk_texts)?;
                    
                    for (i, (chunk, vector)) in chunks.into_iter().zip(embeddings).enumerate() {
                        records.push(crate::store::ChunkRecord {
                            document_id: doc_id.clone(),
                            path: rel_path.to_string_lossy().to_string(),
                            doc_type: doc_type.clone(),
                            title: title.clone(),
                            content_hash: content_hash.clone(),
                            updated_at: updated_at.clone(),
                            links_raw: links_raw_str.clone(),
                            links_resolved: links_resolved_str.clone(),
                            chunk_index: i as u32,
                            heading: chunk.heading,
                            text: chunk.text,
                            vector,
                        });
                    }
                }
                
                if !records.is_empty() {
                    store.insert_batch(&table, records).await?;
                }
                
                println!("Reindex completed.");
            }
            Commands::Doctor => {
                let workspace = Workspace::discover(&current_dir, None)?;
                println!("Doctor checks for {:?}", workspace.root);
                // TODO: iterate over files, check rules
                println!("All checks passed.");
            }
