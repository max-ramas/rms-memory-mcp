use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::fs;
use anyhow::{anyhow, Result};
use uuid::Uuid;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct Document {
    pub path: PathBuf,
    pub frontmatter: Option<Frontmatter>,
    pub content: String,
    pub original_text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Frontmatter {
    pub memory_version: Option<i32>,
    pub id: Option<String>,
    pub alias: Option<String>,
    #[serde(rename = "type")]
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub link: Option<String>,
}

impl Document {
    pub fn parse(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let mut frontmatter = None;
        let mut content = text.clone();

        if text.starts_with("---\n") || text.starts_with("---\r\n") {
            if let Some(end_idx) = text.find("\n---\n").or_else(|| text.find("\r\n---\r\n")) {
                let fm_text = &text[4..end_idx];
                if let Ok(fm) = serde_yaml::from_str::<Frontmatter>(fm_text) {
                    frontmatter = Some(fm);
                }
                
                let content_start = end_idx + if text[end_idx..].starts_with("\r\n") { 7 } else { 5 };
                if content_start <= text.len() {
                    content = text[content_start..].to_string();
                }
            }
        }

        Ok(Document {
            path: path.to_path_buf(),
            frontmatter,
            content,
            original_text: text,
        })
    }

    pub fn ensure_id(&mut self) -> Result<String> {
        if let Some(fm) = &self.frontmatter {
            if let Some(id) = &fm.id {
                return Ok(id.clone());
            }
        }

        let new_id = Uuid::new_v4().to_string();
        
        let new_text = if self.original_text.starts_with("---\n") || self.original_text.starts_with("---\r\n") {
            let id_line = format!("id: {}\n", new_id);
            self.original_text.replacen("---\n", &format!("---\n{}", id_line), 1)
                .replacen("---\r\n", &format!("---\r\n{}", id_line), 1)
        } else {
            format!("---\nid: {}\n---\n\n{}", new_id, self.original_text)
        };

        fs::write(&self.path, &new_text)?;
        self.original_text = new_text;
        
        // Re-parse to update frontmatter
        let updated = Self::parse(&self.path)?;
        self.frontmatter = updated.frontmatter;
        self.content = updated.content;

        Ok(new_id)
    }

    pub fn extract_links(&self) -> Vec<String> {
        let re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
        let mut links = Vec::new();
        for cap in re.captures_iter(&self.content) {
            let link = cap[2].to_string();
            if !link.starts_with("http") && !link.starts_with("mailto:") && !link.starts_with('#') {
                let cleaned = link.split('#').next().unwrap_or("").to_string();
                if !cleaned.is_empty() {
                    links.push(cleaned);
                }
            }
        }
        links
    }
}
