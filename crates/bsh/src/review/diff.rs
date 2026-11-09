//! Text and structural diffs derived from stored snapshots.

use std::collections::BTreeMap;
use std::path::Path;

use similar::{ChangeTag, TextDiff};
use tree_sitter::{Language, Node, Parser};

use super::domain::{Change, DiffLine, FileDiff, ResourceFamily, SnapshotState, StructuralNode};
use super::store::{snapshot_text, ReviewStore};

/// The most text this module will diff in one change.
const MAX_DIFF_TEXT: usize = 4 * 1024 * 1024;
/// The most rows a prepared diff carries.
const MAX_DIFF_LINES: usize = 4_000;

/// Build the prepared diff for one change.
pub(crate) fn build(store: &ReviewStore, change: &Change) -> FileDiff {
    let Some(first) = change.revisions.first() else {
        return FileDiff::unavailable(change, "the change carries no revision");
    };
    let Some(last) = change.revisions.last() else {
        return FileDiff::unavailable(change, "the change carries no revision");
    };
    let old = side_text(store, &first.before);
    let new = side_text(store, &last.after);
    let old_bytes = state_bytes(&first.before);
    let new_bytes = state_bytes(&last.after);
    let binary = is_binary(&first.before) || is_binary(&last.after);
    if binary {
        let mut diff = FileDiff::unavailable(change, "");
        diff.old_bytes = old_bytes;
        diff.new_bytes = new_bytes;
        diff.binary = true;
        diff.note = Some(binary_note(
            store,
            &first.before,
            &last.after,
            old_bytes,
            new_bytes,
        ));
        return diff;
    }
    let Some(old) = old else {
        let mut diff = FileDiff::unavailable(change, unavailable_note(&first.before));
        diff.old_bytes = old_bytes;
        diff.new_bytes = new_bytes;
        return diff;
    };
    let Some(new) = new else {
        let mut diff = FileDiff::unavailable(change, unavailable_note(&last.after));
        diff.old_bytes = old_bytes;
        diff.new_bytes = new_bytes;
        return diff;
    };
    if old.len() > MAX_DIFF_TEXT || new.len() > MAX_DIFF_TEXT {
        let mut diff = FileDiff::unavailable(
            change,
            "the file is larger than the review diff ceiling; open it in the Files pane",
        );
        diff.old_bytes = old_bytes;
        diff.new_bytes = new_bytes;
        return diff;
    }
    let mut lines = Vec::new();
    let mut added = 0usize;
    let mut removed = 0usize;
    let diff = TextDiff::from_lines(&old, &new);
    for group in diff.grouped_ops(3) {
        for op in group {
            for change in diff.iter_changes(&op) {
                if lines.len() >= MAX_DIFF_LINES {
                    break;
                }
                let mark = match change.tag() {
                    ChangeTag::Delete => {
                        removed += 1;
                        "-"
                    }
                    ChangeTag::Insert => {
                        added += 1;
                        "+"
                    }
                    ChangeTag::Equal => " ",
                };
                lines.push(DiffLine {
                    mark: mark.to_string(),
                    old_line: change.old_index().map(|index| index as u32 + 1),
                    new_line: change.new_index().map(|index| index as u32 + 1),
                    text: change.value().trim_end_matches('\n').to_string(),
                });
            }
        }
    }
    let structural = structural_diff(&change.path.workspace_relative, &old, &new);
    FileDiff {
        change_id: change.id.clone(),
        path: change.path.workspace_relative.clone(),
        kind: kind_label(change),
        binary: false,
        old_bytes,
        new_bytes,
        added,
        removed,
        structural,
        lines,
        note: None,
    }
}

fn kind_label(change: &Change) -> String {
    match &change.kind {
        super::domain::ChangeKind::Created => "created",
        super::domain::ChangeKind::Modified => "modified",
        super::domain::ChangeKind::Deleted => "deleted",
        super::domain::ChangeKind::Renamed { .. } => "renamed",
        super::domain::ChangeKind::ModeChanged { .. } => "mode",
        super::domain::ChangeKind::Unchanged => "unchanged",
    }
    .to_string()
}

fn is_binary(state: &SnapshotState) -> bool {
    matches!(
        state,
        SnapshotState::Present(snapshot) if !snapshot.family.is_text()
    )
}

fn binary_note(
    store: &ReviewStore,
    before: &SnapshotState,
    after: &SnapshotState,
    old_bytes: u64,
    new_bytes: u64,
) -> String {
    let mut note = format!("binary or document format; {old_bytes} -> {new_bytes} bytes");
    let old = image_dimensions(store, before);
    let new = image_dimensions(store, after);
    match (old, new) {
        (Some((width, height)), Some((new_width, new_height))) => {
            note.push_str(&format!(
                "; image {width}x{height} -> {new_width}x{new_height}"
            ));
        }
        (None, Some((width, height))) => {
            note.push_str(&format!("; image {width}x{height}"));
        }
        _ => {}
    }
    note
}

fn image_dimensions(store: &ReviewStore, state: &SnapshotState) -> Option<(u32, u32)> {
    let SnapshotState::Present(snapshot) = state else {
        return None;
    };
    if snapshot.family != ResourceFamily::Image {
        return None;
    }
    let bytes = store.read_blob(&snapshot.hash).ok()?;
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.into_dimensions().ok()
}

fn side_text(store: &ReviewStore, state: &SnapshotState) -> Option<String> {
    match state {
        SnapshotState::Absent => Some(String::new()),
        SnapshotState::Present(_) => snapshot_text(store, state),
        SnapshotState::Unavailable(_) => None,
    }
}

fn state_bytes(state: &SnapshotState) -> u64 {
    match state {
        SnapshotState::Present(snapshot) => snapshot.bytes,
        _ => 0,
    }
}

fn unavailable_note(state: &SnapshotState) -> String {
    match state {
        SnapshotState::Absent => "the side did not exist".to_string(),
        SnapshotState::Unavailable(reason) => format!("snapshot unavailable: {reason:?}"),
        SnapshotState::Present(_) => "the snapshot is not readable as text".to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
}

fn language_for(path: &str) -> Option<Lang> {
    let ext = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Lang::Rust),
        "ts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
        "py" => Some(Lang::Python),
        _ => None,
    }
}

fn ts_language(lang: Lang) -> Language {
    match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
    }
}

#[derive(Debug, Clone)]
struct Definition {
    kind: &'static str,
    name: String,
    line: u32,
    text: String,
}

fn structural_diff(path: &str, old: &str, new: &str) -> Vec<StructuralNode> {
    if let Some(lang) = language_for(path) {
        let before = definitions(lang, old);
        let after = definitions(lang, new);
        return compare_definitions(before, after);
    }
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "json" => config_diff(old, new, ConfigKind::Json),
        "toml" => config_diff(old, new, ConfigKind::Toml),
        "yaml" | "yml" => config_diff(old, new, ConfigKind::Yaml),
        "md" | "markdown" | "mdx" => markdown_diff(old, new),
        _ => Vec::new(),
    }
}

fn definitions(lang: Lang, source: &str) -> BTreeMap<String, Definition> {
    let mut parser = Parser::new();
    if parser.set_language(&ts_language(lang)).is_err() {
        return BTreeMap::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    walk_definitions(tree.root_node(), source.as_bytes(), lang, &mut out);
    out
}

fn walk_definitions(node: Node, source: &[u8], lang: Lang, out: &mut BTreeMap<String, Definition>) {
    if let Some(kind) = structural_kind(lang, node.kind()) {
        if let Some(name) = definition_name(node, source) {
            let key = format!("{kind}:{name}");
            let line = node.start_position().row as u32 + 1;
            out.entry(key).or_insert_with(|| {
                let text = node
                    .utf8_text(source)
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                Definition {
                    kind,
                    name,
                    line,
                    text,
                }
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_definitions(child, source, lang, out);
    }
}

fn structural_kind(lang: Lang, kind: &str) -> Option<&'static str> {
    match lang {
        Lang::Rust => match kind {
            "function_item" => Some("function"),
            "struct_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "impl_item" => Some("impl"),
            "mod_item" => Some("module"),
            "type_item" => Some("type"),
            "const_item" => Some("const"),
            "static_item" => Some("static"),
            "macro_definition" => Some("macro"),
            _ => None,
        },
        Lang::TypeScript | Lang::Tsx => match kind {
            "function_declaration" => Some("function"),
            "class_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "type_alias_declaration" => Some("type"),
            "enum_declaration" => Some("enum"),
            "method_definition" => Some("method"),
            _ => None,
        },
        Lang::JavaScript => match kind {
            "function_declaration" => Some("function"),
            "class_declaration" => Some("class"),
            "method_definition" => Some("method"),
            _ => None,
        },
        Lang::Python => match kind {
            "function_definition" => Some("function"),
            "class_definition" => Some("class"),
            _ => None,
        },
    }
}

fn definition_name(node: Node, source: &[u8]) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        if let Ok(text) = name.utf8_text(source) {
            return Some(text.to_string());
        }
    }
    if let Some(name) = node.child_by_field_name("type") {
        if let Ok(text) = name.utf8_text(source) {
            return Some(text.to_string());
        }
    }
    None
}

fn compare_definitions(
    before: BTreeMap<String, Definition>,
    after: BTreeMap<String, Definition>,
) -> Vec<StructuralNode> {
    let mut nodes = Vec::new();
    for (key, old) in &before {
        match after.get(key) {
            Some(new) if old.text == new.text && old.line == new.line => {}
            Some(new) if old.text == new.text => nodes.push(node_for(
                key,
                old.kind,
                &old.name,
                "moved",
                Some(old.line),
                Some(new.line),
                format!("line {} to {}", old.line, new.line),
            )),
            Some(new) => nodes.push(node_for(
                key,
                old.kind,
                &old.name,
                "modified",
                Some(old.line),
                Some(new.line),
                format!("line {} to {}", old.line, new.line),
            )),
            None => nodes.push(node_for(
                key,
                old.kind,
                &old.name,
                "removed",
                Some(old.line),
                None,
                format!("line {}", old.line),
            )),
        }
    }
    for (key, new) in &after {
        if !before.contains_key(key) {
            nodes.push(node_for(
                key,
                new.kind,
                &new.name,
                "added",
                None,
                Some(new.line),
                format!("line {}", new.line),
            ));
        }
    }
    nodes.sort_by(|left, right| {
        left.qualified_name
            .cmp(&right.qualified_name)
            .then(left.kind.cmp(&right.kind))
    });
    nodes
}

fn node_for(
    key: &str,
    kind: &'static str,
    name: &str,
    change: &'static str,
    old_line: Option<u32>,
    new_line: Option<u32>,
    detail: String,
) -> StructuralNode {
    StructuralNode {
        id: key.to_string(),
        parent: None,
        kind: kind.to_string(),
        qualified_name: name.to_string(),
        change: change.to_string(),
        old_line,
        new_line,
        detail,
    }
}

#[derive(Debug, Clone, Copy)]
enum ConfigKind {
    Json,
    Toml,
    Yaml,
}

fn config_diff(old: &str, new: &str, kind: ConfigKind) -> Vec<StructuralNode> {
    let Some(old_value) = parse_config(old, kind) else {
        return Vec::new();
    };
    let Some(new_value) = parse_config(new, kind) else {
        return Vec::new();
    };
    let mut old_map = BTreeMap::new();
    let mut new_map = BTreeMap::new();
    flatten_config("", &old_value, &mut old_map);
    flatten_config("", &new_value, &mut new_map);
    let mut nodes = Vec::new();
    for (key, value) in &old_map {
        match new_map.get(key) {
            Some(next) if next == value => {}
            Some(next) => nodes.push(config_node(key, "modified", Some(value), Some(next))),
            None => nodes.push(config_node(key, "removed", Some(value), None)),
        }
    }
    for (key, value) in &new_map {
        if !old_map.contains_key(key) {
            nodes.push(config_node(key, "added", None, Some(value)));
        }
    }
    nodes.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));
    nodes
}

fn parse_config(text: &str, kind: ConfigKind) -> Option<serde_json::Value> {
    match kind {
        ConfigKind::Json => serde_json::from_str(text).ok(),
        ConfigKind::Toml => {
            let value: toml::Value = toml::from_str(text).ok()?;
            serde_json::to_value(value).ok()
        }
        ConfigKind::Yaml => serde_yaml::from_str(text).ok(),
    }
}

fn flatten_config(prefix: &str, value: &serde_json::Value, out: &mut BTreeMap<String, String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_config(&key, value, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let key = if prefix.is_empty() {
                    format!("[{index}]")
                } else {
                    format!("{prefix}[{index}]")
                };
                flatten_config(&key, item, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn config_node(
    key: &str,
    change: &'static str,
    old: Option<&String>,
    new: Option<&String>,
) -> StructuralNode {
    let detail = match (old, new) {
        (Some(old), Some(new)) => format!("{old} -> {new}"),
        (Some(old), None) => format!("was {old}"),
        (None, Some(new)) => format!("now {new}"),
        (None, None) => String::new(),
    };
    StructuralNode {
        id: format!("config:{key}"),
        parent: None,
        kind: "config-key".to_string(),
        qualified_name: key.to_string(),
        change: change.to_string(),
        old_line: None,
        new_line: None,
        detail,
    }
}

fn markdown_diff(old: &str, new: &str) -> Vec<StructuralNode> {
    let old = headings(old);
    let new = headings(new);
    let mut nodes = Vec::new();
    for (name, line) in &old {
        match new.get(name) {
            Some(next) if next == line => {}
            Some(next) => nodes.push(node_for(
                &format!("section:{name}"),
                "section",
                name,
                "moved",
                Some(*line),
                Some(*next),
                format!("line {line} to {next}"),
            )),
            None => nodes.push(node_for(
                &format!("section:{name}"),
                "section",
                name,
                "removed",
                Some(*line),
                None,
                format!("line {line}"),
            )),
        }
    }
    for (name, line) in &new {
        if !old.contains_key(name) {
            nodes.push(node_for(
                &format!("section:{name}"),
                "section",
                name,
                "added",
                None,
                Some(*line),
                format!("line {line}"),
            ));
        }
    }
    nodes
}

fn headings(text: &str) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_start();
        let count = line
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if count == 0 || count > 6 {
            continue;
        }
        let title = line[count..].trim();
        if !title.is_empty() {
            out.insert(title.to_string(), index as u32 + 1);
        }
    }
    out
}

// Keep the family list referenced so a future format adapter can switch on it
// without changing the diff signature.
const _: fn(ResourceFamily) -> bool = ResourceFamily::is_text;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::domain::{
        ChangeKind, ChangeOperation, Revision, SnapshotRef, SnapshotState,
    };

    fn change_with(root: &Path, path: &str, old: &str, new: &str) -> (ReviewStore, Change) {
        let store = ReviewStore::open(root);
        let before = store
            .put_blob(
                old.as_bytes(),
                ResourceFamily::Code,
                0o644,
                crate::review::domain::SnapshotSource::ToolBefore,
            )
            .unwrap();
        let after = store
            .put_blob(
                new.as_bytes(),
                ResourceFamily::Code,
                0o644,
                crate::review::domain::SnapshotSource::ToolAfter,
            )
            .unwrap();
        let change = Change {
            id: "change".to_string(),
            path: crate::review::domain::ResourcePath {
                workspace_relative: path.to_string(),
                display: path.to_string(),
                container: None,
            },
            kind: ChangeKind::Modified,
            revisions: vec![Revision {
                id: "revision".to_string(),
                seq: 1,
                operation: ChangeOperation::Edit,
                before: SnapshotState::Present(before),
                after: SnapshotState::Present(after),
                tool: None,
                captured_at_ms: 0,
            }],
            first_seen_ms: 0,
            last_seen_ms: 0,
        };
        (store, change)
    }

    #[test]
    fn text_diff_counts_a_replaced_line() {
        let root = std::env::temp_dir().join(format!("bsh-diff-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (store, change) = change_with(&root, "src/lib.rs", "a\nb\n", "a\nc\n");
        let diff = build(&store, &change);
        assert_eq!(diff.added, 1);
        assert_eq!(diff.removed, 1);
        assert!(diff
            .lines
            .iter()
            .any(|line| line.mark == "+" && line.text == "c"));
    }

    #[test]
    fn rust_structure_names_the_changed_function() {
        let root = std::env::temp_dir().join(format!("bsh-diff-rust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (store, change) = change_with(
            &root,
            "src/lib.rs",
            "fn keep() {}\nfn changed(a: i32) {}\n",
            "fn keep() {}\nfn changed(a: i32, b: i32) {}\n",
        );
        let diff = build(&store, &change);
        assert!(diff
            .structural
            .iter()
            .any(|node| node.qualified_name == "changed" && node.change == "modified"));
    }

    #[test]
    fn config_structure_names_the_changed_key() {
        let root = std::env::temp_dir().join(format!("bsh-diff-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (store, change) = change_with(
            &root,
            "config.json",
            "{\"a\":1,\"b\":2}",
            "{\"a\":1,\"b\":3}",
        );
        let diff = build(&store, &change);
        assert!(diff
            .structural
            .iter()
            .any(|node| node.qualified_name == "b" && node.change == "modified"));
    }

    #[test]
    fn an_image_change_reports_its_dimensions() {
        let root = std::env::temp_dir().join(format!("bsh-diff-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = ReviewStore::open(&root);
        let old = png(2, 3);
        let new = png(4, 3);
        let before = store
            .put_blob(
                &old,
                ResourceFamily::Image,
                0o644,
                crate::review::domain::SnapshotSource::ToolBefore,
            )
            .unwrap();
        let after = store
            .put_blob(
                &new,
                ResourceFamily::Image,
                0o644,
                crate::review::domain::SnapshotSource::ToolAfter,
            )
            .unwrap();
        let change = Change {
            id: "change".to_string(),
            path: crate::review::domain::ResourcePath {
                workspace_relative: "art/logo.png".to_string(),
                display: "art/logo.png".to_string(),
                container: None,
            },
            kind: ChangeKind::Modified,
            revisions: vec![Revision {
                id: "revision".to_string(),
                seq: 1,
                operation: ChangeOperation::Edit,
                before: SnapshotState::Present(before),
                after: SnapshotState::Present(after),
                tool: None,
                captured_at_ms: 0,
            }],
            first_seen_ms: 0,
            last_seen_ms: 0,
        };
        let diff = build(&store, &change);
        assert!(diff.binary);
        assert!(
            diff.note
                .as_deref()
                .is_some_and(|note| note.contains("2x3 -> 4x3")),
            "dimensions are metadata the review can trust: {:?}",
            diff.note
        );
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::new(width, height);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("png");
        bytes.into_inner()
    }

    #[test]
    fn a_created_file_is_all_added_lines() {
        let root = std::env::temp_dir().join(format!("bsh-diff-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = ReviewStore::open(&root);
        let after = store
            .put_blob(
                b"one\ntwo\n",
                ResourceFamily::Code,
                0o644,
                crate::review::domain::SnapshotSource::ToolAfter,
            )
            .unwrap();
        let change = Change {
            id: "change".to_string(),
            path: crate::review::domain::ResourcePath {
                workspace_relative: "src/new.rs".to_string(),
                display: "src/new.rs".to_string(),
                container: None,
            },
            kind: ChangeKind::Created,
            revisions: vec![Revision {
                id: "revision".to_string(),
                seq: 1,
                operation: ChangeOperation::Create,
                before: SnapshotState::Absent,
                after: SnapshotState::Present(after),
                tool: None,
                captured_at_ms: 0,
            }],
            first_seen_ms: 0,
            last_seen_ms: 0,
        };
        let diff = build(&store, &change);
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
        assert!(diff.lines.iter().all(|line| line.mark == "+"));
    }

    #[test]
    fn a_deleted_side_is_not_an_invented_snapshot() {
        let mut state = SnapshotState::Absent;
        assert!(!state.is_present());
        let snapshot = SnapshotRef {
            hash: "sha256:00".to_string(),
            bytes: 0,
            media_type: "text/plain".to_string(),
            family: ResourceFamily::Text,
            mode: 0o644,
            source: crate::review::domain::SnapshotSource::ToolAfter,
        };
        state = SnapshotState::Present(snapshot);
        assert!(state.is_present());
    }
}
