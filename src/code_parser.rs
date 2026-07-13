use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    ModuleDoc,
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
    pub body: String,
    pub content: String,
    pub item_hash: String,
}

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
        body,
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
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
        body: content.clone(),
        item_hash: blake3::hash(content.as_bytes()).to_string(),
        content,
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
}
