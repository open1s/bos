//! Source context, documentation, and metrics for graph answers.

use std::path::Path;

use codel::graph::GraphNode;
use serde_json::{Value, json};

/// Enrich one node with a source snippet, preceding documentation, and simple
/// size/complexity metrics.
#[must_use]
pub fn enrich(node: &GraphNode, root: &Path) -> Option<Value> {
    let source = std::fs::read_to_string(root.join(&node.path)).ok()?;
    let range = node.range?;
    let start = range.start_byte.min(source.len());
    let end = range.end_byte.min(source.len());
    let end = end.max(start);
    Some(json!({
        "snippet": snippet(&source, start, end),
        "documentation": documentation(&source, start, &node.path),
        "metrics": metrics(&source, start, end),
    }))
}

fn snippet(source: &str, start: usize, end: usize) -> Value {
    let first = line_number(source, start).saturating_sub(2);
    let last = line_number(source, end).saturating_add(2);
    let lines: Vec<&str> = source.lines().collect();
    let from = first.min(lines.len());
    let to = last.min(lines.len());
    let text = lines[from..to].join("\n");
    json!({
        "startLine": from + 1,
        "endLine": to,
        "text": text,
    })
}

fn documentation(source: &str, start: usize, path: &Path) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let line = line_number(source, start);
    let prefix_len = source[..start.min(source.len())]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .len();
    let mut comments = Vec::new();
    if let Some(current) = lines.get(line) {
        let prefix = current.get(..prefix_len).unwrap_or("").trim();
        if prefix.contains("/*") || prefix.contains("//") {
            comments.push(prefix);
        }
    }
    let mut index = line.saturating_sub(1);
    while index < lines.len() {
        let line = lines[index].trim();
        if is_comment(line) {
            comments.push(line);
            if index == 0 {
                break;
            }
            index -= 1;
        } else if line.is_empty() && comments.is_empty() {
            if index == 0 {
                break;
            }
            index -= 1;
        } else {
            break;
        }
    }
    comments.reverse();
    let comments = comments.join("\n");
    if !comments.is_empty() {
        return comments.chars().take(4000).collect();
    }
    if path.extension().is_some_and(|extension| extension == "py") {
        return python_docstring(source, start);
    }
    comments
}

fn python_docstring(source: &str, start: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut index = line_number(source, start) + 1;
    while index < lines.len() {
        let line = lines[index].trim();
        if line.is_empty() {
            index += 1;
            continue;
        }
        let Some(quote) = ["\"\"\"", "'''"]
            .into_iter()
            .find(|quote| line.starts_with(quote))
        else {
            return String::new();
        };
        let mut doc = line.trim_start_matches(quote).to_owned();
        if doc.contains(quote) {
            return doc.split(quote).next().unwrap_or("").trim().to_owned();
        }
        index += 1;
        while index < lines.len() {
            let line = lines[index];
            if line.trim().contains(quote) {
                doc.push('\n');
                doc.push_str(line.split(quote).next().unwrap_or(""));
                return doc.trim().chars().take(4000).collect();
            }
            doc.push('\n');
            doc.push_str(line);
            index += 1;
        }
        return doc.trim().chars().take(4000).collect();
    }
    String::new()
}

fn is_comment(line: &str) -> bool {
    line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with("/*")
        || line.starts_with('*')
        || line.starts_with("--")
}

fn metrics(source: &str, start: usize, end: usize) -> Value {
    let body = &source[start.min(source.len())..end.min(source.len())];
    let lines = body.lines().count();
    let complexity = ["if ", "for ", "while ", "case ", "catch ", "&&", "||", "?"]
        .iter()
        .map(|token| body.matches(token).count())
        .sum::<usize>()
        + 1;
    json!({
        "lines": lines,
        "bytes": body.len(),
        "complexity": complexity,
    })
}

fn line_number(source: &str, byte: usize) -> usize {
    source[..byte.min(source.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn extracts_python_docstring() {
        let source = r#"def f():
    """Doc."""
    return 1
"#;
        assert_eq!(documentation(source, 0, Path::new("x.py")), "Doc.");
    }

    #[test]
    fn extracts_jsdoc_block() {
        let source = r#"/**
 * Adds.
 */
function add() {}
"#;
        assert!(
            documentation(
                source,
                source.find("function").expect("function"),
                Path::new("x.js")
            )
            .contains("Adds.")
        );
    }
}
