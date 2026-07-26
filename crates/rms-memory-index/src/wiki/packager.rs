use crate::wiki::diagnostics::Diagnostics;
use crate::wiki::manifest::{SENTINEL_FILE, WikiManifest};
use crate::wiki::providers::ResolvedItem;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct Packager {
    wiki_root: PathBuf,
}

impl Packager {
    pub fn new(wiki_root: PathBuf) -> Self {
        Self { wiki_root }
    }

    pub fn generation_dir(&self) -> PathBuf {
        self.wiki_root.join(".generation")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.wiki_root).ok();
        std::fs::create_dir_all(self.generation_dir()).ok();
        std::fs::write(self.wiki_root.join(SENTINEL_FILE), "rms-wiki sentinel\n")?;
        Ok(())
    }

    pub fn write_context_pack(
        &self,
        sections: &[(String, Vec<ResolvedItem>)],
        manifest: &WikiManifest,
    ) -> Result<PathBuf> {
        let path = self.generation_dir().join("context-pack.md");
        let content = self.build_context_pack(sections, manifest);
        self.atomic_write(&path, &content)?;
        Ok(path)
    }

    pub fn write_manifest(&self, manifest: &WikiManifest) -> Result<PathBuf> {
        let path = self.generation_dir().join("manifest.yaml");
        let yaml = serde_yaml::to_string(manifest)?;
        self.atomic_write(&path, &yaml)?;
        Ok(path)
    }

    pub fn write_agent_task(&self, manifest: &WikiManifest) -> Result<PathBuf> {
        let path = self.generation_dir().join("agent-task.md");
        let content = self.build_agent_task(manifest);
        self.atomic_write(&path, &content)?;
        Ok(path)
    }

    pub fn write_sources(&self, sections: &[(String, Vec<ResolvedItem>)]) -> Result<PathBuf> {
        let path = self.generation_dir().join("sources.json");
        let sources: Vec<serde_json::Value> = sections
            .iter()
            .flat_map(|(section_id, items)| {
                items.iter().map(move |item| {
                    serde_json::json!({
                        "section": section_id,
                        "source_type": item.provenance.source_type,
                        "path": item.provenance.path,
                        "line_range": item.provenance.line_range,
                        "symbol_id": item.provenance.symbol_id,
                        "retrieval_score": item.provenance.retrieval_score,
                        "content_hash": item.provenance.content_hash,
                    })
                })
            })
            .collect();
        self.atomic_write(&path, &serde_json::to_string_pretty(&sources)?)?;
        Ok(path)
    }

    pub fn write_diagnostics(&self, diagnostics: &Diagnostics) -> Result<PathBuf> {
        let path = self.generation_dir().join("diagnostics.json");
        self.atomic_write(&path, &diagnostics.to_json()?)?;
        Ok(path)
    }

    fn build_context_pack(
        &self,
        sections: &[(String, Vec<ResolvedItem>)],
        manifest: &WikiManifest,
    ) -> String {
        let mut output = String::with_capacity(manifest.pack.max_chars);
        output.push_str("# RMS Memory Context Pack\n\n");
        output.push_str(&format!(
            "> Budget: {} chars max, {} per section, {} per item\n\n",
            manifest.pack.max_chars, manifest.pack.max_section_chars, manifest.pack.max_item_chars
        ));

        for (section_id, items) in sections {
            let title = manifest
                .sections
                .iter()
                .find(|s| &s.id == section_id)
                .map(|s| s.title.as_str())
                .unwrap_or(section_id);
            output.push_str(&format!("---\n\n## {title}\n\n"));
            for item in items {
                output.push_str(&item.content);
                output.push_str("\n\n");
            }
        }
        output
    }

    fn build_agent_task(&self, manifest: &WikiManifest) -> String {
        let mut task = String::new();
        task.push_str("# Agent Task: Create Wiki\n\n");
        task.push_str("You are given a context pack with verified source material.\n");
        task.push_str("Create human-readable wiki pages in `wiki/` directory.\n\n");
        task.push_str("## Rules\n\n");
        task.push_str("- Use ONLY the information in `context-pack.md`\n");
        task.push_str("- Write pages to `wiki/` directory (NOT `.generation/`)\n");
        task.push_str("- Use proper Markdown formatting\n");
        task.push_str("- Each section should be a separate page or directory\n\n");
        task.push_str("## Sections\n\n");
        for section in &manifest.sections {
            task.push_str(&format!("- `{}`: {}\n", section.id, section.title));
        }
        task
    }

    fn atomic_write(&self, path: &Path, content: &str) -> Result<()> {
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, content)?;
        std::fs::rename(&temp_path, path).context(format!("Failed to write {}", path.display()))?;
        Ok(())
    }
}
