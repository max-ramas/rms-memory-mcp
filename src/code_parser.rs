use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tree_sitter::{Node, Parser};

/// A language supported by the semantic code index.  This is deliberately
/// separate from file extensions: callers ask the registry to detect a
/// language, then dispatch through the matching adapter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    Rust,
    Go,
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
    Python,
    C,
    Cpp,
    Java,
    Ruby,
    Swift,
    Vue,
}

impl LanguageId {
    pub const ALL: [Self; 13] = [
        Self::Rust,
        Self::Go,
        Self::JavaScript,
        Self::Jsx,
        Self::TypeScript,
        Self::Tsx,
        Self::Python,
        Self::C,
        Self::Cpp,
        Self::Java,
        Self::Ruby,
        Self::Swift,
        Self::Vue,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::Jsx => "jsx",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Python => "python",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Java => "java",
            Self::Ruby => "ruby",
            Self::Swift => "swift",
            Self::Vue => "vue",
        }
    }

    pub fn extractor_version(self) -> &'static str {
        match self {
            Self::Rust => "rust-tree-sitter-v1",
            Self::Go => "go-tree-sitter-v1",
            Self::JavaScript => "javascript-tree-sitter-v1",
            Self::Jsx => "javascript-tree-sitter-v1",
            Self::TypeScript => "typescript-tree-sitter-v1",
            Self::Tsx => "typescript-tree-sitter-v1",
            Self::Python => "python-tree-sitter-v1",
            Self::C => "c-tree-sitter-v1",
            Self::Cpp => "cpp-tree-sitter-v1",
            Self::Java => "java-tree-sitter-v1",
            Self::Ruby => "ruby-tree-sitter-v1",
            Self::Swift => "swift-tree-sitter-v1",
            Self::Vue => "vue-sfc-tree-sitter-v1",
        }
    }

    pub fn from_config_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "rust" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "javascript" | "js" => Some(Self::JavaScript),
            "jsx" => Some(Self::Jsx),
            "typescript" | "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "python" | "py" => Some(Self::Python),
            "c" => Some(Self::C),
            "cpp" | "c++" => Some(Self::Cpp),
            "java" => Some(Self::Java),
            "ruby" | "rb" => Some(Self::Ruby),
            "swift" => Some(Self::Swift),
            "vue" => Some(Self::Vue),
            _ => None,
        }
    }
}

/// `auto` means every bundled adapter. An empty legacy setting is treated as
/// `auto` so upgrading an existing registry never disables a code corpus.
pub fn language_is_enabled(language: LanguageId, configured: &[String]) -> bool {
    configured.is_empty()
        || configured.iter().any(|value| {
            value.eq_ignore_ascii_case("auto")
                || LanguageId::from_config_name(value) == Some(language)
        })
}

pub fn validate_language_config(configured: &[String]) -> Result<()> {
    for value in configured {
        if !value.eq_ignore_ascii_case("auto") && LanguageId::from_config_name(value).is_none() {
            return Err(anyhow!(
                "Unknown code language {value:?}; use auto or one of: {}",
                LanguageId::ALL
                    .iter()
                    .map(|language| language.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    Ok(())
}

/// The language registry used by the indexer and source watcher.  It accepts a
/// path that no longer exists too, which is necessary to react to delete and
/// rename events.
pub fn language_for_path(path: &Path) -> Option<LanguageId> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("rs") => Some(LanguageId::Rust),
        Some(extension) if extension.eq_ignore_ascii_case("go") => Some(LanguageId::Go),
        Some("js" | "mjs" | "cjs") => Some(LanguageId::JavaScript),
        Some("jsx") => Some(LanguageId::Jsx),
        Some("ts" | "mts" | "cts") => Some(LanguageId::TypeScript),
        Some("tsx") => Some(LanguageId::Tsx),
        Some("py" | "pyi") => Some(LanguageId::Python),
        // Headers are classified once, never parsed by both grammars. `.h`
        // defaults to C; C++ headers use `.hpp`, `.hh`, or `.hxx`.
        Some("c" | "h") => Some(LanguageId::C),
        Some("cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx") => Some(LanguageId::Cpp),
        Some("java") => Some(LanguageId::Java),
        Some("rb") => Some(LanguageId::Ruby),
        Some("swift") => Some(LanguageId::Swift),
        Some("vue") => Some(LanguageId::Vue),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ParsedCodeFile {
    pub language: LanguageId,
    pub items: Vec<ParsedCodeItem>,
    pub relation_hints: Vec<CodeRelationHint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRelationHint {
    pub source_item_key: String,
    pub relation: String,
    pub target_identifier: String,
}

/// Dispatches source parsing through the language registry.  Adapters return
/// syntax-only relationship hints; storage, embeddings, locks, and MCP remain
/// outside this boundary.
pub fn parse_code_file(file_path: &str, source: &str) -> Result<ParsedCodeFile> {
    let language = language_for_path(Path::new(file_path))
        .ok_or_else(|| anyhow!("Unsupported semantic code language: {file_path}"))?;
    let items = match language {
        LanguageId::Rust => parse_rust_file(file_path, source)?,
        LanguageId::Go => parse_go_file(file_path, source)?,
        LanguageId::JavaScript | LanguageId::Jsx | LanguageId::TypeScript | LanguageId::Tsx => {
            parse_web_file(file_path, source, language)?
        }
        LanguageId::Python => parse_python_file(file_path, source)?,
        LanguageId::C | LanguageId::Cpp => parse_native_file(file_path, source, language)?,
        LanguageId::Java | LanguageId::Ruby => parse_native_file(file_path, source, language)?,
        LanguageId::Swift => parse_native_file(file_path, source, language)?,
        LanguageId::Vue => parse_vue_file(file_path, source)?,
    };
    let relation_hints = match language {
        LanguageId::Rust => extract_rust_relation_hints(file_path, source, &items)?
            .into_iter()
            .map(|hint| CodeRelationHint {
                source_item_key: hint.source_item_key,
                relation: hint.relation.as_str().to_string(),
                target_identifier: hint.target_identifier,
            })
            .collect(),
        LanguageId::Go => extract_go_relation_hints(file_path, source, &items)?,
        LanguageId::JavaScript | LanguageId::Jsx | LanguageId::TypeScript | LanguageId::Tsx => {
            extract_web_relation_hints(file_path, source, &items, language)?
        }
        LanguageId::Python => extract_python_relation_hints(file_path, source, &items)?,
        LanguageId::C
        | LanguageId::Cpp
        | LanguageId::Java
        | LanguageId::Ruby
        | LanguageId::Swift => extract_native_relation_hints(file_path, source, &items, language)?,
        LanguageId::Vue => Vec::new(),
    };
    Ok(ParsedCodeFile {
        language,
        items,
        relation_hints,
    })
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    ModuleDoc,
    Interface,
    TypeAlias,
    Constant,
    Variable,
    Class,
}

impl CodeKind {
    pub fn as_str(self) -> &'static str {
        kind_name(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedCodeItem {
    pub item_key: String,
    pub file_path: String,
    pub module_path: String,
    pub symbol_name: String,
    pub qualified_symbol: String,
    pub kind: CodeKind,
    pub start_line: u32,
    pub end_line: u32,
    pub preamble: String,
    #[serde(skip)]
    pub signature: String,
    pub body: String,
    pub content: String,
    pub item_hash: String,
    #[serde(skip)]
    source_start_byte: usize,
    #[serde(skip)]
    source_end_byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustRelationKind {
    Uses,
    Implements,
    CallsSymbol,
}

impl RustRelationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uses => "uses",
            Self::Implements => "implements",
            Self::CallsSymbol => "calls_symbol",
        }
    }
}

/// A syntactic relationship hint. Targets deliberately remain lexical; a later
/// resolver may promote them from unresolved to resolved/ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustRelationHint {
    pub source_item_key: String,
    pub relation: RustRelationKind,
    pub target_identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeSegment {
    pub item_key: String,
    pub segment_index: u32,
    pub content: String,
    pub content_hash: String,
}

pub const CODE_SEGMENT_MAX_CHARS: usize = 1500;
pub const CODE_SEGMENT_OVERLAP_CHARS: usize = 200;

pub fn parse_rust_file(file_path: &str, source: &str) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| anyhow!("Failed to load Rust grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Rust syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Rust source contains syntax errors: {file_path}"));
    }

    let mut items = Vec::new();
    walk_container(tree.root_node(), file_path, source, &[], &mut items);
    Ok(items)
}

fn walk_container(
    container: Node<'_>,
    file_path: &str,
    source: &str,
    module_path: &[String],
    items: &mut Vec<ParsedCodeItem>,
) {
    let mut cursor = container.walk();
    let children = container.named_children(&mut cursor).collect::<Vec<_>>();
    let mut pending_preamble = Vec::new();
    let mut module_docs = Vec::new();
    let mut accepting_inner_docs = true;

    for child in children {
        let text = node_text(child, source);
        match child.kind() {
            // Rust doc comments are siblings of the item they document. Attributes
            // may appear between them, so both stay pending until the next item.
            "line_comment" if is_outer_doc_comment(text) => {
                accepting_inner_docs = false;
                pending_preamble.push(child);
            }
            "block_comment" if is_outer_block_doc_comment(text) => {
                accepting_inner_docs = false;
                pending_preamble.push(child);
            }
            "line_comment" if is_inner_doc_comment(text) && accepting_inner_docs => {
                pending_preamble.clear();
                module_docs.push(child);
            }
            "block_comment" if is_inner_block_doc_comment(text) && accepting_inner_docs => {
                pending_preamble.clear();
                module_docs.push(child);
            }
            "attribute_item" => {
                accepting_inner_docs = false;
                pending_preamble.push(child);
            }
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "impl_item" => {
                accepting_inner_docs = false;
                items.push(build_item(
                    child,
                    &pending_preamble,
                    file_path,
                    source,
                    module_path,
                ));
                pending_preamble.clear();
            }
            "mod_item" => {
                accepting_inner_docs = false;
                if let Some(name) = child.child_by_field_name("name") {
                    let mut nested_path = module_path.to_vec();
                    nested_path.push(node_text(name, source).to_string());
                    let docs = pending_preamble
                        .iter()
                        .copied()
                        .filter(|node| {
                            let text = node_text(*node, source);
                            is_outer_doc_comment(text) || is_outer_block_doc_comment(text)
                        })
                        .collect::<Vec<_>>();
                    if !docs.is_empty() {
                        items.push(build_module_doc(&docs, file_path, source, &nested_path));
                    }
                    if let Some(body) = child.child_by_field_name("body") {
                        walk_container(body, file_path, source, &nested_path, items);
                    }
                }
                pending_preamble.clear();
            }
            "inner_attribute_item" => {
                accepting_inner_docs = false;
                pending_preamble.clear();
            }
            _ => {
                accepting_inner_docs = false;
                pending_preamble.clear();
            }
        }
    }

    if !module_docs.is_empty() {
        items.push(build_module_doc(
            &module_docs,
            file_path,
            source,
            module_path,
        ));
    }
}

fn build_item(
    node: Node<'_>,
    preamble_nodes: &[Node<'_>],
    file_path: &str,
    source: &str,
    module_path: &[String],
) -> ParsedCodeItem {
    let kind = match node.kind() {
        "function_item" => CodeKind::Function,
        "struct_item" => CodeKind::Struct,
        "enum_item" => CodeKind::Enum,
        "trait_item" => CodeKind::Trait,
        "impl_item" => CodeKind::Impl,
        other => unreachable!("unsupported code item: {other}"),
    };
    let body = node_text(node, source).to_string();
    let signature = item_signature(node, source);
    let preamble = join_nodes(preamble_nodes, source);
    let content = if preamble.is_empty() {
        body.clone()
    } else {
        format!("{preamble}\n{body}")
    };
    let (symbol_name, identity_symbol) = if kind == CodeKind::Impl {
        impl_symbols(node, source)
    } else {
        let name = node
            .child_by_field_name("name")
            .map(|name| node_text(name, source).to_string())
            .unwrap_or_default();
        (name.clone(), name)
    };
    let module = module_path.join("::");
    let qualified_symbol = qualify(&module, &symbol_name);
    let stable_qualified_symbol = qualify(&module, &identity_symbol);
    let item_key = blake3::hash(
        format!(
            "{file_path}\0{module}\0{}\0{stable_qualified_symbol}",
            kind_name(kind)
        )
        .as_bytes(),
    )
    .to_string();
    let start = preamble_nodes.first().copied().unwrap_or(node);

    ParsedCodeItem {
        item_key,
        file_path: file_path.to_string(),
        module_path: module,
        symbol_name,
        qualified_symbol,
        kind,
        start_line: start.start_position().row as u32 + 1,
        end_line: inclusive_end_line(node),
        preamble,
        signature,
        body,
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
        source_start_byte: start.start_byte(),
        source_end_byte: node.end_byte(),
    }
}

fn build_module_doc(
    nodes: &[Node<'_>],
    file_path: &str,
    source: &str,
    module_path: &[String],
) -> ParsedCodeItem {
    let content = join_nodes(nodes, source);
    let module = module_path.join("::");
    let identity = qualify(&module, "<module_doc>");
    ParsedCodeItem {
        item_key: blake3::hash(format!("{file_path}\0{module}\0module_doc\0{identity}").as_bytes())
            .to_string(),
        file_path: file_path.to_string(),
        module_path: module.clone(),
        symbol_name: String::new(),
        qualified_symbol: identity,
        kind: CodeKind::ModuleDoc,
        start_line: nodes.first().unwrap().start_position().row as u32 + 1,
        end_line: inclusive_end_line(*nodes.last().unwrap()),
        preamble: String::new(),
        signature: String::new(),
        body: content.clone(),
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
        source_start_byte: nodes.first().unwrap().start_byte(),
        source_end_byte: nodes.last().unwrap().end_byte(),
    }
}

pub fn extract_rust_relation_hints(
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
) -> Result<Vec<RustRelationHint>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| anyhow!("Failed to load Rust grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Rust syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Rust source contains syntax errors: {file_path}"));
    }
    let mut hints = Vec::new();
    walk_relation_nodes(tree.root_node(), file_path, source, items, &[], &mut hints);
    hints.sort_by(|left, right| {
        (
            &left.source_item_key,
            left.relation.as_str(),
            &left.target_identifier,
        )
            .cmp(&(
                &right.source_item_key,
                right.relation.as_str(),
                &right.target_identifier,
            ))
    });
    hints.dedup_by(|left, right| {
        left.source_item_key == right.source_item_key
            && left.relation == right.relation
            && left.target_identifier == right.target_identifier
    });
    Ok(hints)
}

/// Graph-only stable identity for a Rust file/module container. It deliberately
/// differs from chunk item keys because a module can have imports without a
/// documentable declaration that would produce a search chunk.
pub fn rust_module_item_key(file_path: &str, module_path: &[String]) -> String {
    blake3::hash(format!("{file_path}\0{}\0rust_module", module_path.join("::")).as_bytes())
        .to_string()
}

fn walk_relation_nodes(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
    module_path: &[String],
    hints: &mut Vec<RustRelationHint>,
) {
    let owner = || {
        items
            .iter()
            .filter(|item| {
                item.source_start_byte <= node.start_byte()
                    && node.end_byte() <= item.source_end_byte
            })
            .min_by_key(|item| item.source_end_byte - item.source_start_byte)
            .map(|item| item.item_key.clone())
            .unwrap_or_else(|| rust_module_item_key(file_path, module_path))
    };
    match node.kind() {
        "use_declaration" => {
            if let Some(argument) = node.child_by_field_name("argument") {
                hints.push(RustRelationHint {
                    source_item_key: owner(),
                    relation: RustRelationKind::Uses,
                    target_identifier: format!("rust-use:{}", node_text(argument, source).trim()),
                });
            }
        }
        "impl_item" => {
            if let Some(trait_name) = node.child_by_field_name("trait") {
                hints.push(RustRelationHint {
                    source_item_key: owner(),
                    relation: RustRelationKind::Implements,
                    target_identifier: format!(
                        "rust-trait:{}",
                        node_text(trait_name, source).trim()
                    ),
                });
            }
        }
        "call_expression" => {
            if let Some(function) = node.child_by_field_name("function") {
                hints.push(RustRelationHint {
                    source_item_key: owner(),
                    relation: RustRelationKind::CallsSymbol,
                    target_identifier: format!(
                        "rust-symbol:{}",
                        node_text(function, source).trim()
                    ),
                });
            }
        }
        _ => {}
    }

    let mut child_module_path = module_path.to_vec();
    if node.kind() == "mod_item"
        && let Some(name) = node.child_by_field_name("name")
    {
        child_module_path.push(node_text(name, source).to_string());
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_relation_nodes(child, file_path, source, items, &child_module_path, hints);
    }
}

/// Parses one Go source file into language-neutral semantic items.  The first
/// Go adapter intentionally stays lexical: package names, declarations,
/// receivers, imports, and calls are useful without pretending to perform
/// compiler-accurate module or type resolution.
pub fn parse_go_file(file_path: &str, source: &str) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| anyhow!("Failed to load Go grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Go syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Go source contains syntax errors: {file_path}"));
    }

    let root = tree.root_node();
    let package = go_package_name(root, source);
    let mut items = Vec::new();
    let mut cursor = root.walk();
    let mut pending_preamble = Vec::new();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "comment" => pending_preamble.push(child),
            "package_clause" => {
                if !pending_preamble.is_empty() {
                    items.push(build_go_package_doc(
                        &pending_preamble,
                        file_path,
                        source,
                        &package,
                    ));
                }
                pending_preamble.clear();
            }
            "function_declaration" | "method_declaration" => {
                items.push(build_go_item(
                    child,
                    &pending_preamble,
                    file_path,
                    source,
                    &package,
                    CodeKind::Function,
                ));
                pending_preamble.clear();
            }
            "type_declaration" => {
                let mut declaration_cursor = child.walk();
                for spec in child.named_children(&mut declaration_cursor) {
                    if spec.kind() == "type_spec" {
                        items.push(build_go_item(
                            spec,
                            &pending_preamble,
                            file_path,
                            source,
                            &package,
                            go_type_kind(spec),
                        ));
                        pending_preamble.clear();
                    }
                }
            }
            "const_declaration" => {
                collect_go_value_specs(
                    child,
                    "const_spec",
                    CodeKind::Constant,
                    &pending_preamble,
                    file_path,
                    source,
                    &package,
                    &mut items,
                );
                pending_preamble.clear();
            }
            "var_declaration" => {
                collect_go_value_specs(
                    child,
                    "var_spec",
                    CodeKind::Variable,
                    &pending_preamble,
                    file_path,
                    source,
                    &package,
                    &mut items,
                );
                pending_preamble.clear();
            }
            _ => pending_preamble.clear(),
        }
    }
    Ok(items)
}

fn go_package_name(root: Node<'_>, source: &str) -> String {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .find(|node| node.kind() == "package_clause")
        .and_then(|node| {
            node.child_by_field_name("name").or_else(|| {
                let mut cursor = node.walk();
                node.named_children(&mut cursor).next()
            })
        })
        .map(|node| node_text(node, source).to_string())
        .unwrap_or_default()
}

fn go_type_kind(spec: Node<'_>) -> CodeKind {
    match spec.child_by_field_name("type").map(|node| node.kind()) {
        Some("struct_type") => CodeKind::Struct,
        Some("interface_type") => CodeKind::Interface,
        _ => CodeKind::TypeAlias,
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_go_value_specs(
    declaration: Node<'_>,
    spec_kind: &str,
    kind: CodeKind,
    preamble_nodes: &[Node<'_>],
    file_path: &str,
    source: &str,
    package: &str,
    items: &mut Vec<ParsedCodeItem>,
) {
    let mut cursor = declaration.walk();
    for spec in declaration.named_children(&mut cursor) {
        if spec.kind() == spec_kind {
            items.push(build_go_item(
                spec,
                preamble_nodes,
                file_path,
                source,
                package,
                kind,
            ));
        }
    }
}

fn build_go_item(
    node: Node<'_>,
    preamble_nodes: &[Node<'_>],
    file_path: &str,
    source: &str,
    package: &str,
    kind: CodeKind,
) -> ParsedCodeItem {
    let body = go_item_body(node, source);
    let signature = if node.child_by_field_name("body").is_some() {
        item_signature(node, source)
    } else {
        body.clone()
    };
    let preamble = join_nodes(preamble_nodes, source);
    let content = if preamble.is_empty() {
        body.clone()
    } else {
        format!("{preamble}\n{body}")
    };
    let name = node
        .child_by_field_name("name")
        .map(|name| node_text(name, source).to_string())
        .or_else(|| first_go_identifier(node, source))
        .unwrap_or_default();
    let receiver = if node.kind() == "method_declaration" {
        node.child_by_field_name("receiver")
            .map(|receiver| normalize_go_receiver(node_text(receiver, source)))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let identity = if receiver.is_empty() {
        name.clone()
    } else {
        format!("{receiver}.{name}")
    };
    let qualified_symbol = qualify(package, &identity);
    let item_key = blake3::hash(
        format!(
            "go\0{file_path}\0{package}\0{}\0{identity}",
            kind_name(kind)
        )
        .as_bytes(),
    )
    .to_string();
    let start = preamble_nodes.first().copied().unwrap_or(node);
    ParsedCodeItem {
        item_key,
        file_path: file_path.to_string(),
        module_path: package.to_string(),
        symbol_name: name,
        qualified_symbol,
        kind,
        start_line: start.start_position().row as u32 + 1,
        end_line: inclusive_end_line(node),
        preamble,
        signature,
        body,
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
        source_start_byte: start.start_byte(),
        source_end_byte: node.end_byte(),
    }
}

fn go_item_body(node: Node<'_>, source: &str) -> String {
    let prefix = match node.kind() {
        "type_spec" => "type ",
        "const_spec" => "const ",
        "var_spec" => "var ",
        _ => "",
    };
    format!("{prefix}{}", node_text(node, source))
}

fn build_go_package_doc(
    nodes: &[Node<'_>],
    file_path: &str,
    source: &str,
    package: &str,
) -> ParsedCodeItem {
    let content = join_nodes(nodes, source);
    let qualified_symbol = qualify(package, "<package_doc>");
    ParsedCodeItem {
        item_key: blake3::hash(
            format!("go\0{file_path}\0{package}\0module_doc\0{qualified_symbol}").as_bytes(),
        )
        .to_string(),
        file_path: file_path.to_string(),
        module_path: package.to_string(),
        symbol_name: String::new(),
        qualified_symbol,
        kind: CodeKind::ModuleDoc,
        start_line: nodes
            .first()
            .expect("package docs are non-empty")
            .start_position()
            .row as u32
            + 1,
        end_line: inclusive_end_line(*nodes.last().expect("package docs are non-empty")),
        preamble: String::new(),
        signature: String::new(),
        body: content.clone(),
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
        source_start_byte: nodes
            .first()
            .expect("package docs are non-empty")
            .start_byte(),
        source_end_byte: nodes.last().expect("package docs are non-empty").end_byte(),
    }
}

fn first_go_identifier(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == "identifier")
        .map(|child| node_text(child, source).to_string())
}

fn normalize_go_receiver(receiver: &str) -> String {
    receiver
        .trim_matches(|character: char| {
            character.is_whitespace() || character == '(' || character == ')'
        })
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .trim_start_matches('*')
        .to_string()
}

pub fn extract_go_relation_hints(
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
) -> Result<Vec<CodeRelationHint>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| anyhow!("Failed to load Go grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Go syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Go source contains syntax errors: {file_path}"));
    }
    let package = go_package_name(tree.root_node(), source);
    let mut hints = Vec::new();
    walk_go_relation_nodes(
        tree.root_node(),
        file_path,
        source,
        items,
        &package,
        &mut hints,
    );
    hints.sort_by(|left, right| {
        (
            &left.source_item_key,
            &left.relation,
            &left.target_identifier,
        )
            .cmp(&(
                &right.source_item_key,
                &right.relation,
                &right.target_identifier,
            ))
    });
    hints.dedup();
    Ok(hints)
}

fn go_package_item_key(file_path: &str, package: &str) -> String {
    blake3::hash(format!("go\0{file_path}\0{package}\0package").as_bytes()).to_string()
}

fn walk_go_relation_nodes(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
    package: &str,
    hints: &mut Vec<CodeRelationHint>,
) {
    let owner = || {
        items
            .iter()
            .filter(|item| {
                item.source_start_byte <= node.start_byte()
                    && node.end_byte() <= item.source_end_byte
            })
            .min_by_key(|item| item.source_end_byte - item.source_start_byte)
            .map(|item| item.item_key.clone())
            .unwrap_or_else(|| go_package_item_key(file_path, package))
    };
    match node.kind() {
        "import_spec" => {
            let target = node_text(node, source)
                .split_whitespace()
                .last()
                .unwrap_or_default()
                .trim_matches('"');
            if !target.is_empty() {
                hints.push(CodeRelationHint {
                    source_item_key: owner(),
                    relation: "uses".to_string(),
                    target_identifier: format!("go-import:{target}"),
                });
            }
        }
        "call_expression" => {
            if let Some(function) = node.child_by_field_name("function") {
                hints.push(CodeRelationHint {
                    source_item_key: owner(),
                    relation: "calls_symbol".to_string(),
                    target_identifier: format!("go-symbol:{}", node_text(function, source).trim()),
                });
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_go_relation_nodes(child, file_path, source, items, package, hints);
    }
}

fn parse_vue_file(file_path: &str, source: &str) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_vue3::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Vue syntax tree"))?;
    let mut items = Vec::new();
    fn walk(
        node: Node<'_>,
        file: &str,
        source: &str,
        items: &mut Vec<ParsedCodeItem>,
    ) -> Result<()> {
        if node.kind() == "script_element" {
            let mut c = node.walk();
            if let Some(raw) = node.named_children(&mut c).find(|n| n.kind() == "raw_text") {
                let header = &source[node.start_byte()..raw.start_byte()];
                let dialect = if header.contains("lang=\"ts\"") || header.contains("lang='ts'") {
                    LanguageId::TypeScript
                } else {
                    LanguageId::JavaScript
                };
                for mut item in parse_web_file(file, node_text(raw, source), dialect)? {
                    item.file_path = file.to_string();
                    item.start_line += raw.start_position().row as u32;
                    item.end_line += raw.start_position().row as u32;
                    item.source_start_byte += raw.start_byte();
                    item.source_end_byte += raw.start_byte();
                    items.push(item);
                }
            }
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            walk(child, file, source, items)?;
        }
        Ok(())
    }
    walk(tree.root_node(), file_path, source, &mut items)?;
    Ok(items)
}

fn parse_web_file(
    file_path: &str,
    source: &str,
    language: LanguageId,
) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    let grammar = match language {
        LanguageId::JavaScript | LanguageId::Jsx => tree_sitter_javascript::LANGUAGE.into(),
        LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => unreachable!("web parser called for non-web language"),
    };
    parser
        .set_language(&grammar)
        .map_err(|error| anyhow!("Failed to load {} grammar: {error}", language.as_str()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no {} syntax tree", language.as_str()))?;
    if tree.root_node().has_error() {
        return Err(anyhow!(
            "{} source contains syntax errors: {file_path}",
            language.as_str()
        ));
    }
    let module = Path::new(file_path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::");
    let mut items = Vec::new();
    walk_web_items(
        tree.root_node(),
        file_path,
        source,
        language,
        &module,
        &mut items,
    );
    Ok(items)
}

fn parse_native_file(
    file_path: &str,
    source: &str,
    language: LanguageId,
) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    let grammar = match language {
        LanguageId::C => tree_sitter_c::LANGUAGE.into(),
        LanguageId::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        LanguageId::Java => tree_sitter_java::LANGUAGE.into(),
        LanguageId::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        LanguageId::Swift => tree_sitter_swift::LANGUAGE.into(),
        _ => unreachable!(),
    };
    parser.set_language(&grammar)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no native syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!(
            "{} source contains syntax errors: {file_path}",
            language.as_str()
        ));
    }
    let module = Path::new(file_path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::");
    let mut items = Vec::new();
    fn walk(
        node: Node<'_>,
        file: &str,
        source: &str,
        language: LanguageId,
        module: &str,
        items: &mut Vec<ParsedCodeItem>,
    ) {
        let kind = match node.kind() {
            "function_definition"
            | "function_declaration"
            | "method_declaration"
            | "method"
            | "singleton_method" => Some(CodeKind::Function),
            "struct_specifier" => Some(CodeKind::Struct),
            "enum_specifier" => Some(CodeKind::Enum),
            "class_specifier" | "class_declaration" | "class" | "module" => Some(CodeKind::Class),
            "interface_declaration" => Some(CodeKind::Interface),
            "type_definition" => Some(CodeKind::TypeAlias),
            _ => None,
        };
        if let Some(kind) = kind {
            items.push(build_web_item(
                node,
                file,
                source,
                language,
                module,
                &[],
                kind,
            ));
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            walk(child, file, source, language, module, items);
        }
    }
    walk(
        tree.root_node(),
        file_path,
        source,
        language,
        &module,
        &mut items,
    );
    Ok(items)
}

fn parse_python_file(file_path: &str, source: &str) -> Result<Vec<ParsedCodeItem>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| anyhow!("Failed to load Python grammar: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Python syntax tree"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Python source contains syntax errors: {file_path}"));
    }
    let module = Path::new(file_path)
        .with_extension("")
        .to_string_lossy()
        .replace(['/', '\\'], "::");
    let mut items = Vec::new();
    walk_python_items(
        tree.root_node(),
        file_path,
        source,
        &module,
        &[],
        &mut items,
    );
    Ok(items)
}

fn walk_python_items(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    module: &str,
    container: &[String],
    items: &mut Vec<ParsedCodeItem>,
) {
    let kind = match node.kind() {
        "function_definition" => Some(CodeKind::Function),
        "class_definition" => Some(CodeKind::Class),
        _ => None,
    };
    if let Some(kind) = kind {
        items.push(build_web_item(
            node,
            file_path,
            source,
            LanguageId::Python,
            module,
            container,
            kind,
        ));
    }
    let mut next = container.to_vec();
    if node.kind() == "class_definition"
        && let Some(name) = web_symbol_name(node, source)
    {
        next.push(name);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_python_items(child, file_path, source, module, &next, items);
    }
}

fn walk_web_items(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    language: LanguageId,
    module: &str,
    items: &mut Vec<ParsedCodeItem>,
) {
    walk_web_items_in(node, file_path, source, language, module, &[], items);
}

fn walk_web_items_in(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    language: LanguageId,
    module: &str,
    container: &[String],
    items: &mut Vec<ParsedCodeItem>,
) {
    let kind = match node.kind() {
        "function_declaration" | "generator_function_declaration" | "method_definition" => {
            Some(CodeKind::Function)
        }
        "class_declaration" | "abstract_class_declaration" => Some(CodeKind::Class),
        "interface_declaration" => Some(CodeKind::Interface),
        "enum_declaration" => Some(CodeKind::Enum),
        "type_alias_declaration" => Some(CodeKind::TypeAlias),
        "variable_declarator"
            if node.child_by_field_name("value").is_some_and(|value| {
                matches!(value.kind(), "arrow_function" | "function_expression")
            }) =>
        {
            Some(CodeKind::Function)
        }
        _ => None,
    };
    if let Some(kind) = kind {
        items.push(build_web_item(
            node, file_path, source, language, module, container, kind,
        ));
    }
    let mut child_container = container.to_vec();
    if matches!(
        node.kind(),
        "class_declaration" | "abstract_class_declaration"
    ) && let Some(name) = web_symbol_name(node, source)
    {
        child_container.push(name);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_web_items_in(
            child,
            file_path,
            source,
            language,
            module,
            &child_container,
            items,
        );
    }
}

fn build_web_item(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    language: LanguageId,
    module: &str,
    container: &[String],
    kind: CodeKind,
) -> ParsedCodeItem {
    let body = node_text(node, source).to_string();
    let signature = item_signature(node, source);
    let preamble = if language == LanguageId::Python {
        python_preamble(node, source)
    } else {
        web_preamble(node, source)
    };
    let content = if preamble.is_empty() {
        body.clone()
    } else {
        format!("{preamble}\n{body}")
    };
    let name = web_symbol_name(node, source).unwrap_or_default();
    let identity = if container.is_empty() {
        name.clone()
    } else {
        format!("{}.{}", container.join("."), name)
    };
    let qualified_symbol = qualify(module, &identity);
    let item_key = blake3::hash(
        format!(
            "{}\0{file_path}\0{module}\0{}\0{identity}",
            language.as_str(),
            kind_name(kind)
        )
        .as_bytes(),
    )
    .to_string();
    ParsedCodeItem {
        item_key,
        file_path: file_path.to_string(),
        module_path: module.to_string(),
        symbol_name: name,
        qualified_symbol,
        kind,
        start_line: node.start_position().row as u32 + 1,
        end_line: inclusive_end_line(node),
        preamble,
        signature,
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
        body,
        source_start_byte: node.start_byte(),
        source_end_byte: node.end_byte(),
    }
}

fn web_symbol_name(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .map(|value| node_text(value, source).to_string())
        .or_else(|| first_web_identifier(node, source))
}

fn web_preamble(node: Node<'_>, source: &str) -> String {
    let prefix = &source[..node.start_byte()];
    let mut lines = prefix.lines().rev().peekable();
    while lines.peek().is_some_and(|line| line.trim().is_empty()) {
        lines.next();
    }
    let mut docs = Vec::new();
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed.starts_with('@')
            || trimmed.starts_with("///")
            || trimmed.starts_with("//")
            || trimmed.ends_with("*/")
            || trimmed.starts_with('*')
            || trimmed.starts_with("/**")
        {
            docs.push(lines.next().unwrap().trim_end().to_string());
        } else {
            break;
        }
    }
    docs.reverse();
    docs.join("\n")
}

fn python_preamble(node: Node<'_>, source: &str) -> String {
    let mut parts = web_preamble(node, source);
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        if let Some(first) = body.named_children(&mut cursor).next()
            && first.kind() == "expression_statement"
        {
            let text = node_text(first, source).trim();
            if text.starts_with("\"\"\"")
                || text.starts_with("'''")
                || text.starts_with('"')
                || text.starts_with('\'')
            {
                if !parts.is_empty() {
                    parts.push('\n');
                }
                parts.push_str(text);
            }
        }
    }
    parts
}

fn extract_python_relation_hints(
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
) -> Result<Vec<CodeRelationHint>> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_python::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no Python syntax tree"))?;
    let mut hints = Vec::new();
    fn walk(
        node: Node<'_>,
        file: &str,
        source: &str,
        items: &[ParsedCodeItem],
        hints: &mut Vec<CodeRelationHint>,
    ) {
        let owner = || {
            items
                .iter()
                .filter(|item| {
                    item.source_start_byte <= node.start_byte()
                        && node.end_byte() <= item.source_end_byte
                })
                .min_by_key(|item| item.source_end_byte - item.source_start_byte)
                .map(|item| item.item_key.clone())
                .unwrap_or_else(|| {
                    blake3::hash(format!("python\0{file}\0module").as_bytes()).to_string()
                })
        };
        if node.kind() == "import_statement" || node.kind() == "import_from_statement" {
            hints.push(CodeRelationHint {
                source_item_key: owner(),
                relation: "uses".to_string(),
                target_identifier: format!("python-import:{}", node_text(node, source).trim()),
            });
        }
        if node.kind() == "call"
            && let Some(function) = node.child_by_field_name("function")
        {
            hints.push(CodeRelationHint {
                source_item_key: owner(),
                relation: "calls_symbol".to_string(),
                target_identifier: format!("python-symbol:{}", node_text(function, source).trim()),
            });
        }
        let mut c = node.walk();
        for child in node.named_children(&mut c) {
            walk(child, file, source, items, hints);
        }
    }
    walk(tree.root_node(), file_path, source, items, &mut hints);
    hints.sort_by(|a, b| {
        (&a.source_item_key, &a.relation, &a.target_identifier).cmp(&(
            &b.source_item_key,
            &b.relation,
            &b.target_identifier,
        ))
    });
    hints.dedup();
    Ok(hints)
}

fn first_web_identifier(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind().contains("identifier"))
        .map(|child| node_text(child, source).to_string())
}

fn extract_native_relation_hints(
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
    language: LanguageId,
) -> Result<Vec<CodeRelationHint>> {
    let mut parser = Parser::new();
    let grammar = match language {
        LanguageId::C => tree_sitter_c::LANGUAGE.into(),
        LanguageId::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        LanguageId::Java => tree_sitter_java::LANGUAGE.into(),
        LanguageId::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        LanguageId::Swift => tree_sitter_swift::LANGUAGE.into(),
        _ => unreachable!("native relation extractor called for non-native language"),
    };
    parser.set_language(&grammar)?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no native syntax tree"))?;
    let mut hints = Vec::new();
    fn walk(
        node: Node<'_>,
        file_path: &str,
        source: &str,
        items: &[ParsedCodeItem],
        language: LanguageId,
        hints: &mut Vec<CodeRelationHint>,
    ) {
        let owner = || {
            items
                .iter()
                .filter(|item| {
                    item.source_start_byte <= node.start_byte()
                        && node.end_byte() <= item.source_end_byte
                })
                .min_by_key(|item| item.source_end_byte - item.source_start_byte)
                .map(|item| item.item_key.clone())
                .unwrap_or_else(|| {
                    blake3::hash(format!("{}\0{file_path}\0module", language.as_str()).as_bytes())
                        .to_string()
                })
        };
        if matches!(node.kind(), "preproc_include" | "import_declaration") {
            hints.push(CodeRelationHint {
                source_item_key: owner(),
                relation: "uses".to_string(),
                target_identifier: format!(
                    "{}-import:{}",
                    language.as_str(),
                    node_text(node, source).trim()
                ),
            });
        }
        if matches!(
            node.kind(),
            "call_expression" | "method_invocation" | "call" | "method_call"
        ) {
            let target = node
                .child_by_field_name("function")
                .or_else(|| node.child_by_field_name("name"))
                .and_then(|value| {
                    let value = node_text(value, source).trim();
                    (!value.is_empty()).then(|| value.to_string())
                })
                .or_else(|| first_web_identifier(node, source));
            if let Some(target) = target {
                hints.push(CodeRelationHint {
                    source_item_key: owner(),
                    relation: "calls_symbol".to_string(),
                    target_identifier: format!("{}-symbol:{target}", language.as_str()),
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            walk(child, file_path, source, items, language, hints);
        }
    }
    walk(
        tree.root_node(),
        file_path,
        source,
        items,
        language,
        &mut hints,
    );
    hints.sort_by(|a, b| {
        (&a.source_item_key, &a.relation, &a.target_identifier).cmp(&(
            &b.source_item_key,
            &b.relation,
            &b.target_identifier,
        ))
    });
    hints.dedup();
    Ok(hints)
}

fn extract_web_relation_hints(
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
    language: LanguageId,
) -> Result<Vec<CodeRelationHint>> {
    let mut parser = Parser::new();
    let grammar = match language {
        LanguageId::JavaScript | LanguageId::Jsx => tree_sitter_javascript::LANGUAGE.into(),
        LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        LanguageId::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        _ => unreachable!(),
    };
    parser
        .set_language(&grammar)
        .map_err(|error| anyhow!("Failed to load {} grammar: {error}", language.as_str()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Tree-sitter returned no syntax tree"))?;
    let mut hints = Vec::new();
    walk_web_relations(
        tree.root_node(),
        file_path,
        source,
        items,
        language,
        &mut hints,
    );
    hints.sort_by(|a, b| {
        (&a.source_item_key, &a.relation, &a.target_identifier).cmp(&(
            &b.source_item_key,
            &b.relation,
            &b.target_identifier,
        ))
    });
    hints.dedup();
    Ok(hints)
}

fn walk_web_relations(
    node: Node<'_>,
    file_path: &str,
    source: &str,
    items: &[ParsedCodeItem],
    language: LanguageId,
    hints: &mut Vec<CodeRelationHint>,
) {
    let owner = || {
        items
            .iter()
            .filter(|item| {
                item.source_start_byte <= node.start_byte()
                    && node.end_byte() <= item.source_end_byte
            })
            .min_by_key(|item| item.source_end_byte - item.source_start_byte)
            .map(|item| item.item_key.clone())
            .unwrap_or_else(|| {
                blake3::hash(format!("{}\0{file_path}\0module", language.as_str()).as_bytes())
                    .to_string()
            })
    };
    if node.kind() == "import_statement" {
        hints.push(CodeRelationHint {
            source_item_key: owner(),
            relation: "uses".to_string(),
            target_identifier: format!(
                "{}-import:{}",
                language.as_str(),
                node_text(node, source).trim()
            ),
        });
    }
    if node.kind() == "call_expression"
        && let Some(function) = node.child_by_field_name("function")
    {
        hints.push(CodeRelationHint {
            source_item_key: owner(),
            relation: "calls_symbol".to_string(),
            target_identifier: format!(
                "{}-symbol:{}",
                language.as_str(),
                node_text(function, source).trim()
            ),
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_web_relations(child, file_path, source, items, language, hints);
    }
}

/// Splits an oversized semantic item while retaining its documentation,
/// attributes, and declaration signature in every resulting segment.
pub fn split_with_preamble(item: &ParsedCodeItem) -> Vec<CodeSegment> {
    split_with_preamble_with_limits(item, CODE_SEGMENT_MAX_CHARS, CODE_SEGMENT_OVERLAP_CHARS)
}

pub fn split_with_preamble_with_limits(
    item: &ParsedCodeItem,
    max_chars: usize,
    overlap_chars: usize,
) -> Vec<CodeSegment> {
    assert!(max_chars > 0, "max_chars must be positive");

    if char_len(&item.content) <= max_chars {
        return vec![make_segment(item, 0, item.content.clone())];
    }

    let repeated_preamble = match (item.preamble.is_empty(), item.signature.is_empty()) {
        (true, true) => String::new(),
        (true, false) => item.signature.clone(),
        (false, true) => item.preamble.clone(),
        (false, false) => format!("{}\n{}", item.preamble, item.signature),
    };
    let body = item
        .body
        .strip_prefix(&item.signature)
        .unwrap_or(&item.body);
    let body_budget = max_chars
        .saturating_sub(char_len(&repeated_preamble))
        .max(1);
    let effective_overlap = overlap_chars.min(body_budget.saturating_sub(1));
    let body_slices = split_body_with_overlap(body, body_budget, effective_overlap);

    body_slices
        .into_iter()
        .enumerate()
        .map(|(index, body_slice)| {
            let content = format!("{repeated_preamble}{body_slice}");
            make_segment(item, index as u32, content)
        })
        .collect()
}

fn make_segment(item: &ParsedCodeItem, segment_index: u32, content: String) -> CodeSegment {
    CodeSegment {
        item_key: item.item_key.clone(),
        segment_index,
        content_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
    }
}

fn split_body_with_overlap(body: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    if body.is_empty() {
        return vec![String::new()];
    }

    let mut slices = Vec::new();
    let mut start = 0;
    while start < body.len() {
        let mut end = advance_chars(body, start, max_chars);
        if end < body.len()
            && let Some(newline) = body[start..end].rfind('\n')
            && newline > 0
        {
            end = start + newline + 1;
        }
        slices.push(body[start..end].to_string());
        if end == body.len() {
            break;
        }

        let next_start = retreat_chars(body, end, overlap_chars);
        start = if next_start <= start { end } else { next_start };
    }
    slices
}

fn advance_chars(text: &str, start: usize, count: usize) -> usize {
    text[start..]
        .char_indices()
        .nth(count)
        .map(|(offset, _)| start + offset)
        .unwrap_or(text.len())
}

fn retreat_chars(text: &str, end: usize, count: usize) -> usize {
    if count == 0 {
        return end;
    }
    text[..end]
        .char_indices()
        .rev()
        .nth(count - 1)
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

fn item_signature(node: Node<'_>, source: &str) -> String {
    let node_start = node.start_byte();
    let Some(body) = node.child_by_field_name("body") else {
        return node_text(node, source).to_string();
    };
    let body_text = node_text(body, source);
    if body_text.starts_with('{') {
        format!("{}{{", &source[node_start..body.start_byte()])
    } else {
        source[node_start..body.start_byte()].to_string()
    }
}

fn impl_symbols(node: Node<'_>, source: &str) -> (String, String) {
    let target = node
        .child_by_field_name("type")
        .map(|node| node_text(node, source).trim().to_string())
        .unwrap_or_default();
    let trait_name = node
        .child_by_field_name("trait")
        .map(|node| node_text(node, source).trim().to_string());
    let display = trait_name
        .as_ref()
        .map(|trait_name| format!("impl {trait_name} for {target}"))
        .unwrap_or_else(|| format!("impl {target}"));

    // Rust permits multiple inherent impl blocks for one type. Method names are
    // a more stable discriminator than source lines and remain unchanged when
    // unrelated items are inserted above the impl.
    let mut members = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() == "function_item"
                && let Some(name) = child.child_by_field_name("name")
            {
                members.push(node_text(name, source).to_string());
            }
        }
    }
    members.sort();
    let identity = if members.is_empty() {
        display.clone()
    } else {
        format!("{display}[{}]", members.join(","))
    };
    (display, identity)
}

fn join_nodes(nodes: &[Node<'_>], source: &str) -> String {
    nodes
        .iter()
        .map(|node| node_text(*node, source).trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn inclusive_end_line(node: Node<'_>) -> u32 {
    let end = node.end_position();
    if end.column == 0 {
        end.row as u32
    } else {
        end.row as u32 + 1
    }
}

fn qualify(module_path: &str, symbol: &str) -> String {
    if module_path.is_empty() {
        symbol.to_string()
    } else {
        format!("{module_path}::{symbol}")
    }
}

fn kind_name(kind: CodeKind) -> &'static str {
    match kind {
        CodeKind::Function => "function",
        CodeKind::Struct => "struct",
        CodeKind::Enum => "enum",
        CodeKind::Trait => "trait",
        CodeKind::Impl => "impl",
        CodeKind::ModuleDoc => "module_doc",
        CodeKind::Interface => "interface",
        CodeKind::TypeAlias => "type_alias",
        CodeKind::Constant => "constant",
        CodeKind::Variable => "variable",
        CodeKind::Class => "class",
    }
}

fn is_outer_doc_comment(text: &str) -> bool {
    text.starts_with("///") && !text.starts_with("////")
}

fn is_inner_doc_comment(text: &str) -> bool {
    text.starts_with("//!")
}

fn is_outer_block_doc_comment(text: &str) -> bool {
    text.starts_with("/**") && !text.starts_with("/***")
}

fn is_inner_block_doc_comment(text: &str) -> bool {
    text.starts_with("/*!")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/rust")
                .join(name),
        )
        .unwrap()
    }

    fn go_fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/go")
                .join(name),
        )
        .unwrap()
    }

    fn assert_identity_and_range(item: &ParsedCodeItem) {
        assert_eq!(item.item_key.len(), 64);
        assert!(
            !item
                .item_key
                .chars()
                .any(|character| !character.is_ascii_hexdigit())
        );
        assert!(item.start_line > 0);
        assert!(item.end_line >= item.start_line);
    }

    #[test]
    fn attaches_outer_doc_comment_to_function() {
        let items = parse_rust_file("src/lib.rs", &fixture("outer_doc_fn.rs")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, CodeKind::Function);
        assert_eq!(items[0].symbol_name, "greet");
        assert_eq!(items[0].preamble, "/// Greets a user.\n/// Keeps context.");
        assert!(items[0].content.starts_with("/// Greets a user."));
        assert_eq!(items[0].start_line, 1);
        assert_eq!(items[0].end_line, 5);
        assert_identity_and_range(&items[0]);
    }

    #[test]
    fn attribute_between_doc_and_item_preserves_attachment() {
        let items = parse_rust_file("src/model.rs", &fixture("doc_attribute_struct.rs")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, CodeKind::Struct);
        assert_eq!(items[0].preamble, "/// Stored user.\n#[derive(Debug)]");
        assert_eq!(items[0].start_line, 1);
        assert_eq!(items[0].end_line, 5);
        assert_identity_and_range(&items[0]);
    }

    #[test]
    fn module_docs_are_a_separate_empty_symbol_item() {
        let items = parse_rust_file("src/lib.rs", &fixture("module_doc.rs")).unwrap();
        let module_doc = items
            .iter()
            .find(|item| item.kind == CodeKind::ModuleDoc)
            .unwrap();
        assert_eq!(module_doc.symbol_name, "");
        assert_eq!(module_doc.qualified_symbol, "<module_doc>");
        assert_eq!(module_doc.start_line, 1);
        assert_eq!(module_doc.end_line, 2);
        assert_identity_and_range(module_doc);
        let root = items
            .iter()
            .find(|item| item.kind == CodeKind::Struct)
            .unwrap();
        assert_eq!(root.preamble, "");
        assert_eq!((root.start_line, root.end_line), (4, 4));
        assert_identity_and_range(root);
    }

    #[test]
    fn nested_modules_produce_qualified_symbols() {
        let items = parse_rust_file("src/lib.rs", &fixture("nested_module.rs")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].module_path, "api::v1");
        assert_eq!(items[0].qualified_symbol, "api::v1::load");
        assert_eq!(items[0].preamble, "/// Loads the API.");
        assert_eq!((items[0].start_line, items[0].end_line), (3, 4));
        assert_identity_and_range(&items[0]);
    }

    #[test]
    fn multiple_inherent_impls_have_distinct_stable_keys() {
        let source = fixture("multiple_impls.rs");
        let items = parse_rust_file("src/model.rs", &source).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].symbol_name, "impl User");
        assert_eq!(items[2].symbol_name, "impl User");
        assert_eq!(items[0].preamble, "");
        assert_eq!(items[1].preamble, "/// Construction methods.");
        assert_eq!(items[2].preamble, "/// Query methods.");
        assert_eq!((items[1].start_line, items[1].end_line), (3, 8));
        assert_eq!((items[2].start_line, items[2].end_line), (10, 15));
        assert_ne!(items[1].item_key, items[2].item_key);
        items.iter().for_each(assert_identity_and_range);

        let shifted = format!("fn unrelated() {{}}\n\n{source}");
        let shifted_items = parse_rust_file("src/model.rs", &shifted).unwrap();
        assert_eq!(items[1].item_key, shifted_items[2].item_key);
        assert_eq!(items[2].item_key, shifted_items[3].item_key);
    }

    #[test]
    fn item_without_doc_comment_has_empty_preamble() {
        let items = parse_rust_file("src/plain.rs", &fixture("no_doc.rs")).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].preamble, "");
        assert_eq!(items[0].symbol_name, "Plain");
        assert_eq!((items[0].start_line, items[0].end_line), (1, 1));
        assert_identity_and_range(&items[0]);
    }

    #[test]
    fn extracts_trait_and_enum_as_separate_items() {
        let items = parse_rust_file("src/types.rs", &fixture("trait_enum.rs")).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, CodeKind::Trait);
        assert_eq!(items[1].kind, CodeKind::Enum);
        assert_eq!(items[0].symbol_name, "Load");
        assert_eq!(items[1].symbol_name, "State");
        assert_eq!(items[0].preamble, "/// Loading behavior.");
        assert_eq!(items[1].preamble, "/// Runtime state.");
        assert_eq!((items[0].start_line, items[0].end_line), (1, 4));
        assert_eq!((items[1].start_line, items[1].end_line), (6, 10));
        items.iter().for_each(assert_identity_and_range);
    }

    #[test]
    fn generic_items_keep_names_and_complete_signatures() {
        let items = parse_rust_file("src/cache.rs", &fixture("generic_items.rs")).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, CodeKind::Struct);
        assert_eq!(items[0].symbol_name, "Cache");
        assert!(items[0].body.contains("Cache<K, V>"));
        assert_eq!(items[0].preamble, "/// A generic cache.");
        assert_eq!((items[0].start_line, items[0].end_line), (1, 4));
        assert_eq!(items[1].kind, CodeKind::Function);
        assert_eq!(items[1].symbol_name, "load");
        assert!(items[1].body.contains("load<K: AsRef<str>, V: Default>"));
        assert_eq!(items[1].preamble, "/// Loads a value by key.");
        assert_eq!((items[1].start_line, items[1].end_line), (6, 10));
        items.iter().for_each(assert_identity_and_range);
    }

    #[test]
    fn outer_docs_on_module_become_module_doc_item() {
        let items = parse_rust_file("src/lib.rs", &fixture("outer_module_doc.rs")).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, CodeKind::ModuleDoc);
        assert_eq!(items[0].module_path, "api");
        assert_eq!(items[0].qualified_symbol, "api::<module_doc>");
        assert_eq!(items[0].content, "/// Public API.");
        assert_eq!((items[0].start_line, items[0].end_line), (1, 1));
        assert_eq!(items[1].qualified_symbol, "api::ping");
        items.iter().for_each(assert_identity_and_range);
    }

    #[test]
    fn oversized_items_repeat_preamble_and_signature_with_stable_segments() {
        let statements = (0..60)
            .map(|index| format!("    let value_{index} = \"payload-{index:02}\";\n"))
            .collect::<String>();
        let source = format!(
            "/// Explains the large operation.\n#[allow(unused_variables)]\npub fn large() {{{statements}}}\n"
        );
        let item = parse_rust_file("src/large.rs", &source)
            .unwrap()
            .pop()
            .unwrap();
        let segments = split_with_preamble_with_limits(&item, 260, 40);
        assert!(segments.len() > 2);

        let repeated = format!("{}\n{}", item.preamble, item.signature);
        for (index, segment) in segments.iter().enumerate() {
            assert_eq!(segment.item_key, item.item_key);
            assert_eq!(segment.segment_index, index as u32);
            assert!(segment.content.starts_with(&repeated));
            assert_eq!(
                segment.content_hash,
                blake3::hash(segment.content.as_bytes()).to_string()
            );
            assert!(char_len(&segment.content) <= 260);
        }

        for pair in segments.windows(2) {
            let left = pair[0].content.strip_prefix(&repeated).unwrap();
            let right = pair[1].content.strip_prefix(&repeated).unwrap();
            assert_eq!(&left[left.len() - 40..], &right[..40]);
        }
    }

    #[test]
    fn extracts_syntactic_use_impl_and_call_hints_without_claiming_resolution() {
        let source = r#"
use crate::shared::Thing;

trait Render { fn render(&self); }
struct Screen;
impl Render for Screen {
    fn render(&self) { helper(); }
}
fn helper() { let _ = Thing::default(); }
"#;
        let items = parse_rust_file("src/ui.rs", source).unwrap();
        let hints = extract_rust_relation_hints("src/ui.rs", source, &items).unwrap();
        assert!(hints.iter().any(|hint| {
            hint.relation == RustRelationKind::Uses
                && hint.target_identifier == "rust-use:crate::shared::Thing"
                && hint.source_item_key == rust_module_item_key("src/ui.rs", &[])
        }));
        assert!(hints.iter().any(|hint| {
            hint.relation == RustRelationKind::Implements
                && hint.target_identifier == "rust-trait:Render"
        }));
        assert!(hints.iter().any(|hint| {
            hint.relation == RustRelationKind::CallsSymbol
                && hint.target_identifier == "rust-symbol:helper"
        }));
        assert!(hints.iter().any(|hint| {
            hint.relation == RustRelationKind::CallsSymbol
                && hint.target_identifier == "rust-symbol:Thing::default"
        }));
    }

    #[test]
    fn go_adapter_extracts_docs_receivers_and_type_shapes() {
        let source = go_fixture("service.go");
        let parsed = parse_code_file("internal/billing/service.go", &source).unwrap();
        assert_eq!(parsed.language, LanguageId::Go);
        assert_eq!(parsed.items.len(), 5);
        let package_doc = parsed
            .items
            .iter()
            .find(|item| item.kind == CodeKind::ModuleDoc)
            .unwrap();
        assert_eq!(package_doc.qualified_symbol, "billing::<package_doc>");
        assert!(
            package_doc
                .content
                .contains("Package billing owns payment operations")
        );
        let service = parsed
            .items
            .iter()
            .find(|item| item.symbol_name == "Service")
            .unwrap();
        assert_eq!(service.kind, CodeKind::Struct);
        assert_eq!(service.module_path, "billing");
        assert!(service.body.starts_with("type Service struct"));
        assert!(service.preamble.contains("Service dispatches payments"));
        let gateway = parsed
            .items
            .iter()
            .find(|item| item.symbol_name == "Gateway")
            .unwrap();
        assert_eq!(gateway.kind, CodeKind::Interface);
        let charge = parsed
            .items
            .iter()
            .find(|item| item.symbol_name == "Charge")
            .unwrap();
        assert_eq!(charge.qualified_symbol, "billing::Service.Charge");
        assert_eq!(charge.kind, CodeKind::Function);
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.target_identifier == "go-import:context")
        );
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.target_identifier == "go-symbol:s.client.Charge")
        );
    }

    #[test]
    fn go_item_identity_survives_unrelated_line_edits() {
        let source = go_fixture("service.go");
        let original = parse_go_file("internal/billing/service.go", &source).unwrap();
        let shifted = parse_go_file(
            "internal/billing/service.go",
            &format!("// unrelated header\n{source}"),
        )
        .unwrap();
        let original_charge = original
            .iter()
            .find(|item| item.qualified_symbol == "billing::Service.Charge")
            .unwrap();
        let shifted_charge = shifted
            .iter()
            .find(|item| item.qualified_symbol == "billing::Service.Charge")
            .unwrap();
        assert_eq!(original_charge.item_key, shifted_charge.item_key);
        assert_eq!(original_charge.start_line + 1, shifted_charge.start_line);
    }

    #[test]
    fn web_adapter_dispatches_dialects_and_extracts_lexical_hints() {
        let source = "import { dep } from './dep';\nexport class Service { run() { return dep(); } }\nexport const handler = () => dep();\n";
        let parsed = parse_code_file("src/service.ts", source).unwrap();
        assert_eq!(parsed.language, LanguageId::TypeScript);
        assert!(
            parsed
                .items
                .iter()
                .any(|item| item.kind == CodeKind::Class && item.symbol_name == "Service")
        );
        assert!(
            parsed
                .items
                .iter()
                .any(|item| item.kind == CodeKind::Function && item.symbol_name == "handler")
        );
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.relation == "uses")
        );
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.target_identifier == "typescript-symbol:dep")
        );
        assert_eq!(
            language_for_path(Path::new("component.tsx")),
            Some(LanguageId::Tsx)
        );
        assert_eq!(
            language_for_path(Path::new("component.jsx")),
            Some(LanguageId::Jsx)
        );
    }

    #[test]
    fn python_adapter_keeps_decorators_docstrings_and_lexical_hints() {
        let source = "from app.client import Client\n\nclass Service:\n    @staticmethod\n    def run():\n        \"\"\"Runs a request.\"\"\"\n        return Client().send()\n";
        let parsed = parse_code_file("app/service.py", source).unwrap();
        assert_eq!(parsed.language, LanguageId::Python);
        let run = parsed
            .items
            .iter()
            .find(|item| item.symbol_name == "run")
            .unwrap();
        assert_eq!(run.qualified_symbol, "app::service::Service.run");
        assert!(run.preamble.contains("@staticmethod"));
        assert!(run.preamble.contains("Runs a request"));
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.relation == "uses")
        );
        assert!(
            parsed
                .relation_hints
                .iter()
                .any(|hint| hint.target_identifier == "python-symbol:Client")
        );
    }

    #[test]
    fn native_adapters_dispatch_c_and_cpp_without_headers() {
        let c = parse_code_file(
            "native/api.c",
            "#include <stdio.h>\ntypedef int Count; struct State { int value; }; int run(void) { return puts(\"ok\"); }",
        )
        .unwrap();
        assert_eq!(c.language, LanguageId::C);
        assert!(c.items.iter().any(|item| item.kind == CodeKind::Function));
        assert!(c.relation_hints.iter().any(|hint| hint.relation == "uses"));
        assert!(
            c.relation_hints
                .iter()
                .any(|hint| hint.target_identifier == "c-symbol:puts")
        );
        let cpp = parse_code_file(
            "native/widget.cpp",
            "class Widget {}; int run() { return 0; }",
        )
        .unwrap();
        assert_eq!(cpp.language, LanguageId::Cpp);
        assert!(cpp.items.iter().any(|item| item.kind == CodeKind::Class));
        assert_eq!(
            language_for_path(Path::new("native/api.h")),
            Some(LanguageId::C)
        );
        assert_eq!(
            language_for_path(Path::new("native/api.hpp")),
            Some(LanguageId::Cpp)
        );
    }

    #[test]
    fn language_configuration_is_validated_and_filters_adapters() {
        assert!(language_is_enabled(LanguageId::Go, &["auto".to_string()]));
        assert!(language_is_enabled(LanguageId::Go, &["go".to_string()]));
        assert!(!language_is_enabled(LanguageId::Rust, &["go".to_string()]));
        assert!(validate_language_config(&["ts".to_string(), "vue".to_string()]).is_ok());
        assert!(validate_language_config(&["fortran".to_string()]).is_err());
    }

    #[test]
    fn native_adapters_dispatch_java_and_ruby() {
        let java = parse_code_file(
            "src/Service.java",
            "import java.util.List; class Service { void run() { work(); } void work() {} }",
        )
        .unwrap();
        assert_eq!(java.language, LanguageId::Java);
        assert!(java.items.iter().any(|item| item.kind == CodeKind::Class));
        assert!(
            java.relation_hints
                .iter()
                .any(|hint| hint.relation == "uses")
        );
        assert!(
            java.relation_hints
                .iter()
                .any(|hint| hint.relation == "calls_symbol")
        );
        let ruby =
            parse_code_file("lib/service.rb", "class Service\n  def run\n  end\nend\n").unwrap();
        assert_eq!(ruby.language, LanguageId::Ruby);
        assert!(ruby.items.iter().any(|item| item.kind == CodeKind::Class));

        let swift = parse_code_file(
            "Sources/App.swift",
            "import Foundation\nfunc run() { print(\"ok\") }",
        )
        .unwrap();
        assert_eq!(swift.language, LanguageId::Swift);
        assert!(
            swift
                .items
                .iter()
                .any(|item| item.kind == CodeKind::Function)
        );
        assert!(
            swift
                .relation_hints
                .iter()
                .any(|hint| hint.relation == "uses")
        );
    }

    #[test]
    fn vue_script_is_reparsed_as_typescript_with_host_ranges() {
        let source = "<template><main /></template>\n<script lang=\"ts\">\nexport function load() { return 1; }\n</script>\n";
        let parsed = parse_code_file("src/App.vue", source).unwrap();
        assert_eq!(parsed.language, LanguageId::Vue);
        let load = parsed
            .items
            .iter()
            .find(|item| item.symbol_name == "load")
            .unwrap();
        assert_eq!(load.file_path, "src/App.vue");
        assert_eq!(load.start_line, 3);
    }

    #[test]
    fn small_items_keep_their_original_content_as_segment_zero() {
        let item = parse_rust_file("src/lib.rs", &fixture("outer_doc_fn.rs"))
            .unwrap()
            .pop()
            .unwrap();
        let segments = split_with_preamble(&item);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].segment_index, 0);
        assert_eq!(segments[0].content, item.content);
    }
}
