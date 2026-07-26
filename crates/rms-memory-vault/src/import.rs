use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub enum ImportAction {
    Import,
    ImportAndOrganize,
    LinkOnly,
    Ignore,
    Skip,
}

pub struct ImportService {
    workspace_root: PathBuf,
    vault_path: PathBuf,
}

impl ImportService {
    pub fn new(workspace_root: PathBuf, vault_path: PathBuf) -> Self {
        Self {
            workspace_root,
            vault_path,
        }
    }

    pub fn detect_existing_docs(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();

        // 1. Explicit directory targets and rule files without .md
        let targets = vec![
            "docs",
            "ADR",
            "adr",
            ".cursorrules",
            ".windsurfrules",
            ".clinerules",
            ".rules",
            "architecture",
            "decisions",
        ];

        for t in targets {
            let path = self.workspace_root.join(t);
            if path.exists() {
                found.push(path);
            }
        }

        // 2. All .md files in the root directory
        if let Ok(entries) = fs::read_dir(&self.workspace_root) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type()
                    && file_type.is_file()
                {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("md")
                        && !found.contains(&path)
                    {
                        found.push(path);
                    }
                }
            }
        }

        found
    }

    pub fn prompt_action(&self, found_docs: &[PathBuf]) -> Result<ImportAction> {
        if found_docs.is_empty() {
            return Ok(ImportAction::Skip);
        }

        println!("Existing project knowledge detected:");
        for doc in found_docs {
            let rel = doc.strip_prefix(&self.workspace_root).unwrap_or(doc);
            println!("✓ {}", rel.display());
        }

        let options = vec![
            "Import (Copy files into the Vault preserving their structure)",
            "Import & Organize [Recommended] (Copy files into the Vault and classify them)",
            "Link Only (Keep files where they are and create references)",
            "Ignore (Leave existing documentation untouched)",
            "Skip (You can import later with `rms-memory import`)",
        ];

        let selection = dialoguer::Select::new()
            .with_prompt("Choose how RMS Memory should use them:")
            .items(&options)
            .default(1)
            .interact()?;

        match selection {
            0 => Ok(ImportAction::Import),
            1 => Ok(ImportAction::ImportAndOrganize),
            2 => Ok(ImportAction::LinkOnly),
            3 => Ok(ImportAction::Ignore),
            4 => Ok(ImportAction::Skip),
            _ => Ok(ImportAction::Skip),
        }
    }

    pub fn execute(&self, action: ImportAction, docs: Vec<PathBuf>) -> Result<()> {
        match action {
            ImportAction::Import => self.execute_import(docs, false),
            ImportAction::ImportAndOrganize => self.execute_import(docs, true),
            ImportAction::LinkOnly => self.execute_link(docs),
            ImportAction::Ignore | ImportAction::Skip => Ok(()),
        }
    }

    fn execute_import(&self, docs: Vec<PathBuf>, organize: bool) -> Result<()> {
        for doc in docs {
            self.copy_recursive(&doc, organize)?;
        }
        tracing::info!("Import completed.");
        Ok(())
    }

    fn copy_recursive(&self, src: &Path, organize: bool) -> Result<()> {
        if src.is_file() {
            let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
            let fname = src.file_name().unwrap_or_default().to_string_lossy();
            if ext == "md"
                || fname.starts_with(".cursorrules")
                || fname.starts_with(".windsurfrules")
                || fname.starts_with(".clinerules")
                || fname == ".rules"
            {
                self.import_single_file(src, organize)?;
            }
        } else if src.is_dir() {
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                self.copy_recursive(&entry.path(), organize)?;
            }
        }
        Ok(())
    }

    fn import_single_file(&self, src: &Path, organize: bool) -> Result<()> {
        let dest_dir = if organize {
            self.map_category(src)
        } else {
            let rel = src.strip_prefix(&self.workspace_root).unwrap_or(src);
            if let Some(parent) = rel.parent() {
                self.vault_path.join(parent)
            } else {
                self.vault_path.clone()
            }
        };

        fs::create_dir_all(&dest_dir)?;
        let file_name = src.file_name().unwrap().to_string_lossy();
        let safe_name = if file_name.starts_with(".cursorrules")
            || file_name.starts_with(".windsurfrules")
            || file_name.starts_with(".clinerules")
            || file_name == ".rules"
        {
            format!("{}.md", file_name)
        } else {
            file_name.to_string()
        };
        let dest_file = dest_dir.join(safe_name);
        fs::copy(src, dest_file)?;
        Ok(())
    }

    fn execute_link(&self, docs: Vec<PathBuf>) -> Result<()> {
        for doc in docs {
            self.link_recursive(&doc)?;
        }
        tracing::info!("Link creation completed.");
        Ok(())
    }

    fn link_recursive(&self, src: &Path) -> Result<()> {
        if src.is_file() {
            let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("");
            let fname = src.file_name().unwrap_or_default().to_string_lossy();
            if ext == "md"
                || fname.starts_with(".cursorrules")
                || fname.starts_with(".windsurfrules")
                || fname.starts_with(".clinerules")
                || fname == ".rules"
            {
                self.create_link_file(src)?;
            }
        } else if src.is_dir() {
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                self.link_recursive(&entry.path())?;
            }
        }
        Ok(())
    }

    fn create_link_file(&self, src: &Path) -> Result<()> {
        let category = self.map_category(src);
        fs::create_dir_all(&category)?;

        let file_name = src.file_name().unwrap().to_string_lossy();
        let safe_name = if file_name.starts_with(".cursorrules")
            || file_name.starts_with(".windsurfrules")
            || file_name.starts_with(".clinerules")
            || file_name == ".rules"
        {
            format!("{}.md", file_name)
        } else {
            file_name.to_string()
        };

        let dest_file = category.join(safe_name);

        let rel_source = pathdiff::diff_paths(src, &category).unwrap_or_else(|| src.to_path_buf());

        let id = uuid::Uuid::new_v4().to_string();
        let doc_type = category.file_name().unwrap_or_default().to_string_lossy();

        let frontmatter = format!(
            "---\nmemory_version: 1\nid: {}\ntype: {}\nlink: {}\n---\n",
            id,
            doc_type,
            rel_source.display()
        );

        fs::write(dest_file, frontmatter)?;
        Ok(())
    }

    fn map_category(&self, src: &Path) -> PathBuf {
        let rel_path = src
            .strip_prefix(&self.workspace_root)
            .unwrap_or(src)
            .to_string_lossy()
            .to_lowercase();

        let category = if rel_path.starts_with("readme") {
            "guides"
        } else if rel_path.starts_with("adr") || rel_path.starts_with("decisions") {
            "decisions"
        } else if rel_path.starts_with("docs/api") {
            "api"
        } else if rel_path.starts_with("docs/architecture") {
            "architecture"
        } else if rel_path.ends_with(".cursorrules")
            || rel_path.ends_with(".windsurfrules")
            || rel_path.ends_with(".clinerules")
            || rel_path.ends_with(".rules")
            || rel_path.contains("claude.md")
            || rel_path.contains("gemini.md")
            || rel_path.contains("agent")
        {
            "rules"
        } else if rel_path.contains("plan") || rel_path.contains("roadmap") {
            "architecture"
        } else if rel_path.contains("changelog")
            || rel_path.contains("history")
            || rel_path.contains("walkthrough")
            || rel_path.contains("task.md")
            || rel_path.contains("implementation_plan")
            || rel_path.contains("architecture_analysis")
        {
            "artifacts"
        } else {
            "docs"
        };

        self.vault_path.join(category)
    }
}
