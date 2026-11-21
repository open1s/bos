//! Raw Tree-sitter syntax trees for one file.

use std::path::{Path, PathBuf};

use codel::classification::classify_path;
use codel::syntax::LanguageProvider;
use codel::{GrammarProvider, PythonProvider, RustProvider, TypeScriptProvider};
use serde_json::{Value, json};
use tree_sitter::Node;

/// Parse a file and return its S-expression and a bounded JSON tree.
///
/// # Errors
///
/// Returns a message when the path has no provider, the file cannot be read, or
/// the parser rejects the source.
pub fn parse(root: &Path, relative: &Path, depth: usize) -> Result<Value, String> {
    let (_, _, tree) = parse_file(root, relative)?;
    Ok(json!({
        "path": relative.display().to_string(),
        "language": format!("{:?}", classify_path(relative).language),
        "sexp": tree.root_node().to_sexp(),
        "tree": json_node(tree.root_node(), depth),
    }))
}

fn provider_language(path: &Path) -> Option<tree_sitter::Language> {
    let language = classify_path(path).language?;
    let rust = RustProvider;
    let typescript = TypeScriptProvider;
    let python = PythonProvider;
    let providers: [&dyn LanguageProvider; 3] = [&rust, &typescript, &python];
    if let Some(language) = providers
        .iter()
        .find(|provider| provider.language() == language)
        .and_then(|provider| provider.tree_sitter_language())
    {
        return Some(language);
    }
    GrammarProvider::ALL
        .iter()
        .find(|provider| provider.language() == language)
        .and_then(|provider| provider.tree_sitter_language())
}

/// Run a Tree-sitter query against one file.
///
/// # Errors
///
/// Returns a message when the path has no provider, the file cannot be read,
/// the parser rejects the source, or the query is invalid.
pub fn query(
    root: &Path,
    relative: &Path,
    query_text: &str,
    captures: &[String],
    max: usize,
) -> Result<Value, String> {
    use tree_sitter::{Query, QueryCursor, StreamingIterator};

    let (language, source, tree) = parse_file(root, relative)?;
    let query = Query::new(&language, query_text).map_err(|error| error.to_string())?;
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_slice());
    let mut output = Vec::new();
    while let Some(query_match) = matches.next() {
        let mut captured = Vec::new();
        for capture in query_match.captures {
            let name = names
                .get(capture.index as usize)
                .copied()
                .unwrap_or_default()
                .to_owned();
            if !captures.is_empty() && !captures.iter().any(|wanted| wanted == &name) {
                continue;
            }
            let node = capture.node;
            let start = node.start_byte();
            let end = node.end_byte();
            let text = source.get(start..end).unwrap_or_default();
            captured.push(json!({
                "name": name,
                "kind": node.kind(),
                "startByte": start,
                "endByte": end,
                "startLine": line_number(&source, start),
                "text": String::from_utf8_lossy(text),
            }));
        }
        if !captured.is_empty() {
            output.push(json!({"captures": captured}));
            if output.len() >= max {
                break;
            }
        }
    }
    Ok(json!({"path": relative.display().to_string(), "matches": output}))
}

fn parse_file(
    root: &Path,
    relative: &Path,
) -> Result<(tree_sitter::Language, Vec<u8>, tree_sitter::Tree), String> {
    let language = provider_language(relative)
        .ok_or_else(|| format!("no Tree-sitter provider for {}", relative.display()))?;
    let source = std::fs::read(root.join(relative)).map_err(|error| error.to_string())?;
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| error.to_string())?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| "parser returned no tree".to_owned())?;
    Ok((language, source, tree))
}

fn line_number(source: &[u8], byte: usize) -> usize {
    source[..byte.min(source.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}
fn json_node(node: Node<'_>, depth: usize) -> Value {
    let mut value = json!({
        "kind": node.kind(),
        "named": node.is_named(),
        "startByte": node.start_byte(),
        "endByte": node.end_byte(),
    });
    if depth > 0 {
        let children = (0..node.child_count())
            .filter_map(|index| node.child(index as u32))
            .map(|child| json_node(child, depth - 1))
            .collect::<Vec<_>>();
        value["children"] = Value::Array(children);
    }
    value
}

/// Resolve a workspace-relative path with basic traversal protection.
#[must_use]
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    (!path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_))))
    .then(|| path.to_path_buf())
}
