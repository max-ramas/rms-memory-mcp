use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphNodeKey(String);

impl GraphNodeKey {
    pub fn vault(document_id: &str) -> Result<Self> {
        Self::typed("vault", document_id)
    }

    pub fn code(item_key: &str) -> Result<Self> {
        Self::typed("code", item_key)
    }

    pub fn external(identifier: &str) -> Result<Self> {
        Self::typed("external", identifier)
    }

    fn typed(corpus: &str, source_id: &str) -> Result<Self> {
        let source_id = source_id.trim();
        if source_id.is_empty() {
            return Err(anyhow!("graph node source ID must be non-empty"));
        }
        Ok(Self(format!("{corpus}:{source_id}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EdgeRelation(String);

impl EdgeRelation {
    pub fn new(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.is_empty()
            || !value
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '_')
        {
            return Err(anyhow!(
                "edge relation must contain only lowercase ASCII letters and underscores"
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    Derived,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeResolution {
    Resolved,
    Unresolved,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOverrideAction {
    Suppress,
    Restore,
}

pub fn derived_edge_key(
    extractor: &str,
    source: &GraphNodeKey,
    target: &GraphNodeKey,
    relation: &EdgeRelation,
) -> Result<String> {
    if extractor.trim().is_empty() {
        return Err(anyhow!("derived edge extractor must be non-empty"));
    }
    Ok(blake3::hash(
        format!(
            "derived\0{}\0{}\0{}\0{}",
            extractor.trim(),
            source.as_str(),
            target.as_str(),
            relation.as_str()
        )
        .as_bytes(),
    )
    .to_string())
}

pub fn new_user_edge_key() -> String {
    format!("user:{}", uuid::Uuid::new_v4())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNodeRecord {
    pub node_key: GraphNodeKey,
    pub corpus: String,
    pub source_id: String,
    pub kind: String,
    pub label: String,
    pub path: Option<String>,
    pub metadata_json: String,
    /// `None` is reserved for nodes created explicitly by a user.
    pub generation: Option<u64>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdgeRecord {
    pub edge_key: String,
    pub source_key: GraphNodeKey,
    pub target_key: GraphNodeKey,
    pub relation: EdgeRelation,
    pub origin: EdgeOrigin,
    pub extractor: Option<String>,
    pub resolution: EdgeResolution,
    pub confidence: Option<f32>,
    /// `None` is reserved for user-created edges.
    pub generation: Option<u64>,
    pub metadata_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdgeOverride {
    pub edge_key: String,
    pub action: EdgeOverrideAction,
    pub revision: u64,
    pub author: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_node_keys_are_typed_and_stable() {
        assert_eq!(
            GraphNodeKey::vault("doc-1").unwrap().as_str(),
            "vault:doc-1"
        );
        assert_eq!(
            GraphNodeKey::code("item-1").unwrap().as_str(),
            "code:item-1"
        );
        assert!(GraphNodeKey::external(" ").is_err());
    }

    #[test]
    fn derived_edges_are_stable_but_extractor_versioned() {
        let source = GraphNodeKey::code("source").unwrap();
        let target = GraphNodeKey::external("crate::Thing").unwrap();
        let relation = EdgeRelation::new("uses").unwrap();
        let first = derived_edge_key("rust-tree-sitter-v1", &source, &target, &relation).unwrap();
        let same = derived_edge_key("rust-tree-sitter-v1", &source, &target, &relation).unwrap();
        let upgraded =
            derived_edge_key("rust-tree-sitter-v2", &source, &target, &relation).unwrap();
        assert_eq!(first, same);
        assert_ne!(first, upgraded);
    }

    #[test]
    fn custom_relations_are_safe_for_storage_and_queries() {
        assert!(EdgeRelation::new("related_to").is_ok());
        assert!(EdgeRelation::new("Related To").is_err());
        assert!(EdgeRelation::new("calls-symbol").is_err());
    }

    #[test]
    fn graph_schemas_preserve_provenance_and_user_overrides() {
        let edges = crate::store::Store::graph_edges_schema();
        for field in [
            "edge_key",
            "source_key",
            "target_key",
            "origin",
            "extractor",
            "resolution",
            "generation",
        ] {
            assert!(edges.column_with_name(field).is_some(), "missing {field}");
        }
        let overrides = crate::store::Store::graph_edge_overrides_schema();
        for field in ["edge_key", "action", "revision", "author"] {
            assert!(
                overrides.column_with_name(field).is_some(),
                "missing {field}"
            );
        }
    }
}
