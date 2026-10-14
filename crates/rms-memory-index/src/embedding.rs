//! Embedding model profiles: which model turns text into vectors, the vector
//! size, and the text each model expects in front of queries and passages.
//!
//! One profile is active per process (`active()`), read from the global config
//! once. Indexes built with a non-default model live in their own directory
//! (`EmbeddingProfile::index_dir`), so switching models never mixes vectors
//! and switching back reuses the earlier index.

use anyhow::{Result, anyhow, bail};
use fastembed::EmbeddingModel;
use rms_memory_core::workspace::GlobalConfig;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub const DEFAULT_MODEL: &str = "multilingual-e5-small";
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const OLLAMA_PREFIX: &str = "ollama:";
/// Overrides the configured model for one process (tests, evaluation runs).
pub const MODEL_ENV: &str = "RMS_MEMORY_EMBEDDING_MODEL";

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CatalogEntry {
    pub id: &'static str,
    pub dimension: usize,
    pub multilingual: bool,
    /// Approximate one-time download size.
    pub download_mb: u32,
    pub description: &'static str,
    #[serde(skip)]
    query_prefix: &'static str,
    #[serde(skip)]
    passage_prefix: &'static str,
}

const E5_QUERY: &str = "query: ";
const E5_PASSAGE: &str = "passage: ";

/// Bundled local models. The first entry is the default; it keeps the
/// prefix-free input of indexes built before model choice existed.
pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: DEFAULT_MODEL,
        dimension: 384,
        multilingual: true,
        download_mb: 470,
        description: "Default. Small multilingual E5; fast on any machine.",
        query_prefix: "",
        passage_prefix: "",
    },
    CatalogEntry {
        id: "multilingual-e5-small-prefixed",
        dimension: 384,
        multilingual: true,
        download_mb: 470,
        description: "The default model with the query:/passage: prefixes E5 was trained with.",
        query_prefix: E5_QUERY,
        passage_prefix: E5_PASSAGE,
    },
    CatalogEntry {
        id: "multilingual-e5-base",
        dimension: 768,
        multilingual: true,
        download_mb: 1110,
        description: "Multilingual E5 base; better recall, about 3x slower than small.",
        query_prefix: E5_QUERY,
        passage_prefix: E5_PASSAGE,
    },
    CatalogEntry {
        id: "multilingual-e5-large",
        dimension: 1024,
        multilingual: true,
        download_mb: 2240,
        description: "Multilingual E5 large; strongest E5, needs a fast CPU and RAM.",
        query_prefix: E5_QUERY,
        passage_prefix: E5_PASSAGE,
    },
    CatalogEntry {
        id: "bge-m3",
        dimension: 1024,
        multilingual: true,
        download_mb: 2270,
        description: "BAAI bge-m3; 100+ languages, long passages.",
        query_prefix: "",
        passage_prefix: "",
    },
    CatalogEntry {
        id: "paraphrase-multilingual-mpnet",
        dimension: 768,
        multilingual: true,
        download_mb: 1110,
        description: "Sentence-transformers multilingual mpnet; tuned for paraphrases.",
        query_prefix: "",
        passage_prefix: "",
    },
    CatalogEntry {
        id: "embeddinggemma-300m",
        dimension: 768,
        multilingual: true,
        download_mb: 1230,
        description: "Google EmbeddingGemma 300M; multilingual retrieval model.",
        query_prefix: "task: search result | query: ",
        passage_prefix: "title: none | text: ",
    },
    CatalogEntry {
        id: "bge-small-en",
        dimension: 384,
        multilingual: false,
        download_mb: 130,
        description: "BAAI bge-small-en v1.5; English only, very light.",
        query_prefix: "Represent this sentence for searching relevant passages: ",
        passage_prefix: "",
    },
    CatalogEntry {
        id: "nomic-embed-text-v1.5",
        dimension: 768,
        multilingual: false,
        download_mb: 550,
        description: "Nomic embed text v1.5; English, long context.",
        query_prefix: "search_query: ",
        passage_prefix: "search_document: ",
    },
];

pub(crate) fn fastembed_model(id: &str) -> Option<EmbeddingModel> {
    Some(match id {
        DEFAULT_MODEL | "multilingual-e5-small-prefixed" => EmbeddingModel::MultilingualE5Small,
        "multilingual-e5-base" => EmbeddingModel::MultilingualE5Base,
        "multilingual-e5-large" => EmbeddingModel::MultilingualE5Large,
        "bge-m3" => EmbeddingModel::BGEM3,
        "paraphrase-multilingual-mpnet" => EmbeddingModel::ParaphraseMLMpnetBaseV2,
        "embeddinggemma-300m" => EmbeddingModel::EmbeddingGemma300M,
        "bge-small-en" => EmbeddingModel::BGESmallENV15,
        "nomic-embed-text-v1.5" => EmbeddingModel::NomicEmbedTextV15,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backend {
    Fastembed,
    Ollama { url: String, model: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmbeddingProfile {
    pub id: String,
    pub dimension: usize,
    pub query_prefix: String,
    pub passage_prefix: String,
    pub backend: Backend,
}

impl EmbeddingProfile {
    fn from_catalog(entry: &CatalogEntry) -> Self {
        Self {
            id: entry.id.to_string(),
            dimension: entry.dimension,
            query_prefix: entry.query_prefix.to_string(),
            passage_prefix: entry.passage_prefix.to_string(),
            backend: Backend::Fastembed,
        }
    }

    pub fn default_profile() -> Self {
        Self::from_catalog(&CATALOG[0])
    }

    /// Resolve a model id (`CATALOG` id or `ollama:<model>`) against the
    /// global config, which holds the Ollama endpoint and vector size.
    pub fn resolve(id: &str, global: &GlobalConfig) -> Result<Self> {
        let id = id.trim();
        if let Some(model) = id.strip_prefix(OLLAMA_PREFIX) {
            let model = model.trim();
            if model.is_empty() {
                bail!("Name the Ollama model, for example `ollama:nomic-embed-text`");
            }
            // The recorded vector size belongs to the configured Ollama model only.
            let configured = global
                .embedding_model
                .as_deref()
                .and_then(|configured| configured.trim().strip_prefix(OLLAMA_PREFIX))
                .map(str::trim);
            let dimension = global
                .embedding_dimension
                .filter(|dimension| *dimension > 0 && configured == Some(model))
                .ok_or_else(|| {
                    anyhow!(
                        "Embedding model `{id}` has no recorded vector size; run `rms-memory embedding use {id}` to detect it"
                    )
                })?;
            return Ok(Self {
                id: format!("{OLLAMA_PREFIX}{model}"),
                dimension,
                query_prefix: String::new(),
                passage_prefix: String::new(),
                backend: Backend::Ollama {
                    url: ollama_url(global)?,
                    model: model.to_string(),
                },
            });
        }
        CATALOG
            .iter()
            .find(|entry| entry.id == id)
            .map(Self::from_catalog)
            .ok_or_else(|| {
                anyhow!(
                    "Unknown embedding model `{id}`. Use one of: {}, or ollama:<model>",
                    CATALOG
                        .iter()
                        .map(|entry| entry.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    pub fn from_config(global: &GlobalConfig) -> Result<Self> {
        match global.embedding_model.as_deref().map(str::trim) {
            None | Some("") => Ok(Self::default_profile()),
            Some(id) => Self::resolve(id, global),
        }
    }

    pub fn is_default(&self) -> bool {
        self.id == DEFAULT_MODEL
    }

    /// Index directory for this model inside a project's index root.
    pub fn index_dir(&self, project_index_root: &Path) -> PathBuf {
        if self.is_default() {
            project_index_root.to_path_buf()
        } else {
            project_index_root.join("models").join(slug(&self.id))
        }
    }
}

/// Folder name for a model id. Ids that lose characters on the way (such as
/// `ollama:a/b` and `ollama:a-b`) get a hash suffix so they never share one.
fn slug(id: &str) -> String {
    let slug: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if slug == id {
        slug
    } else {
        format!("{slug}-{}", &blake3::hash(id.as_bytes()).to_hex()[..8])
    }
}

pub fn ollama_url(global: &GlobalConfig) -> Result<String> {
    let url = global
        .ollama_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .unwrap_or(DEFAULT_OLLAMA_URL)
        .trim_end_matches('/');
    ensure_loopback(url)?;
    Ok(url.to_string())
}

/// Notes never leave the machine: Ollama must listen on a loopback address.
fn ensure_loopback(url: &str) -> Result<()> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| anyhow!("Ollama URL must start with http:// or https:// (got {url})"))?;
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    if authority.contains('@') || !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        bail!("Ollama URL must point to this machine (localhost, 127.0.0.1 or [::1]); got {url}");
    }
    Ok(())
}

/// Point the global config at model `id`. Ollama models are asked for their
/// vector size, so Ollama must be running.
pub fn configure(
    global: &mut GlobalConfig,
    id: &str,
    ollama_url: Option<&str>,
) -> Result<EmbeddingProfile> {
    let mut updated = global.clone();
    if let Some(url) = ollama_url.map(str::trim).filter(|url| !url.is_empty()) {
        ensure_loopback(url)?;
        updated.ollama_url = Some(url.to_string());
    }
    let id = id.trim();
    updated.embedding_model = Some(id.to_string());
    updated.embedding_dimension = match id.strip_prefix(OLLAMA_PREFIX) {
        Some(model) => {
            let url = self::ollama_url(&updated)?;
            let probe = ollama_embed(&url, model.trim(), &["dimension probe".to_string()])?;
            match probe.first().map(Vec::len) {
                Some(dimension) if dimension > 0 => Some(dimension),
                _ => bail!("Ollama returned an empty embedding for `{model}`"),
            }
        }
        None => None,
    };
    let profile = EmbeddingProfile::resolve(id, &updated)?;
    updated.embedding_model = (!profile.is_default()).then(|| profile.id.clone());
    *global = updated;
    Ok(profile)
}

static ACTIVE: OnceLock<EmbeddingProfile> = OnceLock::new();

/// The model this process embeds with. An invalid configuration falls back
/// to the default model with a warning instead of breaking search.
pub fn active() -> &'static EmbeddingProfile {
    ACTIVE.get_or_init(|| {
        configured().unwrap_or_else(|error| {
            eprintln!("Warning: {error:#}. Using {DEFAULT_MODEL}.");
            EmbeddingProfile::default_profile()
        })
    })
}

/// The model the global config (or `RMS_MEMORY_EMBEDDING_MODEL`) asks for.
pub fn configured() -> Result<EmbeddingProfile> {
    let global = rms_memory_core::config_manager::load_registry()?.global;
    match std::env::var(MODEL_ENV) {
        Ok(id) if !id.trim().is_empty() => EmbeddingProfile::resolve(&id, &global),
        _ => EmbeddingProfile::from_config(&global),
    }
}

/// Embed through a local Ollama server (`POST /api/embed`).
pub fn ollama_embed(url: &str, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    #[derive(serde::Deserialize)]
    struct EmbedResponse {
        embeddings: Vec<Vec<f32>>,
    }
    // A proxy from HTTP(S)_PROXY or a redirect would send notes off the machine.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .proxy(None)
        .max_redirects(0)
        .build()
        .into();
    let response: EmbedResponse = agent
        .post(format!("{url}/api/embed"))
        .send_json(serde_json::json!({ "model": model, "input": inputs }))
        .map_err(|error| {
            anyhow!("Ollama embedding request to {url} failed ({error}); is `ollama serve` running and `{model}` pulled?")
        })?
        .body_mut()
        .read_json()
        .map_err(|error| anyhow!("Unexpected Ollama embedding response: {error}"))?;
    if response.embeddings.len() != inputs.len() {
        bail!(
            "Ollama returned {} embeddings for {} inputs",
            response.embeddings.len(),
            inputs.len()
        );
    }
    Ok(response.embeddings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_keeps_the_legacy_index_dir_and_prefix_free_input() {
        let profile = EmbeddingProfile::from_config(&GlobalConfig::default()).unwrap();
        assert!(profile.is_default());
        assert_eq!(profile.dimension, 384);
        assert!(profile.query_prefix.is_empty() && profile.passage_prefix.is_empty());
        let root = Path::new("/data/dbs/abc");
        assert_eq!(profile.index_dir(root), root);
    }

    #[test]
    fn every_catalog_model_maps_to_fastembed_with_its_dimension() {
        for entry in CATALOG {
            let model = fastembed_model(entry.id).expect(entry.id);
            let info = fastembed::TextEmbedding::get_model_info(&model).expect(entry.id);
            assert_eq!(info.dim, entry.dimension, "{}", entry.id);
        }
    }

    #[test]
    fn other_models_get_their_own_index_dir() {
        let global = GlobalConfig {
            embedding_model: Some("multilingual-e5-base".to_string()),
            ..GlobalConfig::default()
        };
        let profile = EmbeddingProfile::from_config(&global).unwrap();
        assert_eq!(profile.dimension, 768);
        assert_eq!(profile.query_prefix, "query: ");
        assert_eq!(
            profile.index_dir(Path::new("/dbs/abc")),
            Path::new("/dbs/abc/models/multilingual-e5-base")
        );
        assert!(EmbeddingProfile::resolve("no-such-model", &global).is_err());
    }

    #[test]
    fn ollama_needs_a_dimension_and_a_loopback_url() {
        let mut global = GlobalConfig {
            embedding_model: Some("ollama:nomic-embed-text".to_string()),
            ..GlobalConfig::default()
        };
        assert!(EmbeddingProfile::from_config(&global).is_err());

        global.embedding_dimension = Some(768);
        assert!(EmbeddingProfile::resolve("ollama:mxbai-embed-large", &global).is_err());
        let profile = EmbeddingProfile::from_config(&global).unwrap();
        assert_eq!(
            profile.backend,
            Backend::Ollama {
                url: DEFAULT_OLLAMA_URL.to_string(),
                model: "nomic-embed-text".to_string()
            }
        );
        let index_dir = profile.index_dir(Path::new("/dbs/abc"));
        let folder = index_dir.file_name().unwrap().to_string_lossy();
        assert!(folder.starts_with("ollama-nomic-embed-text-"), "{folder}");
        assert_ne!(slug("ollama:a/b"), slug("ollama:a-b"));
        assert_eq!(slug("bge-m3"), "bge-m3");

        for allowed in [
            "http://localhost:11434/",
            "http://[::1]:11434",
            "https://127.0.0.1",
        ] {
            global.ollama_url = Some(allowed.to_string());
            assert!(EmbeddingProfile::from_config(&global).is_ok(), "{allowed}");
        }
        for refused in [
            "http://192.168.1.5:11434",
            "http://localhost.evil.com",
            "http://user@localhost",
            "ftp://localhost",
        ] {
            global.ollama_url = Some(refused.to_string());
            assert!(EmbeddingProfile::from_config(&global).is_err(), "{refused}");
        }
    }
}
