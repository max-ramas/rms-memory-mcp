use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

pub const SENTINEL_FILE: &str = ".rms-wiki";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiManifest {
    pub schema_version: u32,
    pub pack: PackConfig,
    pub sections: Vec<WikiSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackConfig {
    pub max_chars: usize,
    pub max_section_chars: usize,
    pub max_item_chars: usize,
}

impl Default for PackConfig {
    fn default() -> Self {
        Self {
            max_chars: 120_000,
            max_section_chars: 30_000,
            max_item_chars: 8_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiSection {
    pub id: String,
    pub title: String,
    pub sources: Vec<WikiSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WikiSource {
    #[serde(rename = "vault_search")]
    VaultSearch {
        queries: Vec<String>,
        limit: Option<usize>,
    },
    #[serde(rename = "code_search")]
    CodeSearch {
        queries: Vec<String>,
        kinds: Option<Vec<String>>,
        paths: Option<Vec<String>>,
        limit: Option<usize>,
    },
    #[serde(rename = "files")]
    Files {
        globs: Vec<String>,
        extraction: Option<String>,
        required: Option<bool>,
    },
    #[serde(rename = "self_cli_help")]
    SelfCliHelp { commands: Vec<String> },
}

impl WikiManifest {
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let manifest: WikiManifest =
            serde_yaml::from_str(yaml).context("Failed to parse wiki manifest YAML")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context(format!("Failed to read manifest: {}", path.display()))?;
        Self::from_yaml_str(&content)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            anyhow::bail!(
                "Unsupported schema version: {} (expected 1)",
                self.schema_version
            );
        }
        if self.sections.is_empty() {
            anyhow::bail!("Manifest must have at least one section");
        }
        let mut ids = HashSet::new();
        for section in &self.sections {
            if !ids.insert(&section.id) {
                anyhow::bail!("Duplicate section id: {}", section.id);
            }
            if section.sources.is_empty() {
                anyhow::bail!("Section '{}' has no sources", section.id);
            }
        }
        if self.pack.max_chars == 0 {
            anyhow::bail!("pack.max_chars must be > 0");
        }
        Ok(())
    }

    pub fn default_manifest() -> Self {
        Self {
            schema_version: 1,
            pack: PackConfig::default(),
            sections: vec![
                WikiSection {
                    id: "overview".into(),
                    title: "Overview".into(),
                    sources: vec![
                        WikiSource::VaultSearch {
                            queries: vec![
                                "overview".into(),
                                "architecture".into(),
                                "purpose".into(),
                            ],
                            limit: Some(8),
                        },
                        WikiSource::Files {
                            globs: vec!["README.md".into()],
                            extraction: Some("full".into()),
                            required: Some(false),
                        },
                    ],
                },
                WikiSection {
                    id: "requirements".into(),
                    title: "System Requirements".into(),
                    sources: vec![
                        WikiSource::VaultSearch {
                            queries: vec![
                                "requirements".into(),
                                "dependencies".into(),
                                "supported os".into(),
                            ],
                            limit: Some(8),
                        },
                        WikiSource::Files {
                            globs: vec![
                                "Cargo.toml".into(),
                                "package.json".into(),
                                "go.mod".into(),
                            ],
                            extraction: Some("full".into()),
                            required: Some(false),
                        },
                    ],
                },
                WikiSection {
                    id: "installation".into(),
                    title: "Installation".into(),
                    sources: vec![
                        WikiSource::VaultSearch {
                            queries: vec!["installation".into(), "setup".into(), "install".into()],
                            limit: Some(8),
                        },
                        WikiSource::Files {
                            globs: vec![
                                "install.sh".into(),
                                "install.ps1".into(),
                                ".github/workflows/*release*".into(),
                            ],
                            extraction: Some("relevant_sections".into()),
                            required: Some(false),
                        },
                    ],
                },
                WikiSection {
                    id: "configuration".into(),
                    title: "Configuration".into(),
                    sources: vec![
                        WikiSource::VaultSearch {
                            queries: vec![
                                "configuration".into(),
                                "config".into(),
                                "settings".into(),
                            ],
                            limit: Some(8),
                        },
                        WikiSource::CodeSearch {
                            queries: vec![
                                "configuration options".into(),
                                "environment variables".into(),
                            ],
                            kinds: Some(vec!["struct".into(), "enum".into()]),
                            paths: Some(vec!["src/config*".into(), "src/cli*".into()]),
                            limit: Some(10),
                        },
                    ],
                },
                WikiSection {
                    id: "usage".into(),
                    title: "Usage".into(),
                    sources: vec![
                        WikiSource::SelfCliHelp {
                            commands: vec![
                                "".into(),
                                "config".into(),
                                "reindex".into(),
                                "doctor".into(),
                            ],
                        },
                        WikiSource::VaultSearch {
                            queries: vec!["usage".into(), "examples".into(), "workflows".into()],
                            limit: Some(8),
                        },
                    ],
                },
            ],
        }
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(serde_yaml::to_string(self)?)
    }
}
