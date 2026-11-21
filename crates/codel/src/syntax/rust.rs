//! Rust syntax provider backed by the upstream Tree-sitter Rust grammar.
//!
//! Tree-sitter owns lexical and syntactic parsing. This module classifies
//! declarations, imports, and calls with one native Tree-sitter query, then
//! reads names, scopes, and visibility through the native `Node` API. It builds
//! no Rust AST; the flat `SyntaxFact` projection is a compatibility output for
//! the current resolver.

use std::{
    ops::ControlFlow,
    sync::{Arc, OnceLock},
};

use tree_sitter::{Node, ParseOptions, ParseState, Parser, Query, QueryCursor, StreamingIterator};

use crate::classification::LanguageId;

use super::{
    ImportFact, LanguageProvider, ParseControl, ParseDiagnostic, ParseInput, ParseLimitKind,
    ParseLimits, ParseResult, ParseStatus, ParsedDocument, ParsedDocumentStatus, ProviderError,
    RelationFact, RelationKind, RustModuleFact, SourceRange, SyntaxFact, SyntaxFactKind,
    valid_relative_path,
};

/// Rust provider using the upstream Tree-sitter Rust grammar.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustProvider;

impl RustProvider {
    /// Stable provider identifier.
    pub const ID: &'static str = "codel.rust.tree-sitter";

    /// Parser and grammar versions used by this provider.
    pub const VERSION: &'static str = "tree-sitter-0.26.13/tree-sitter-rust-0.24.2/facts-v3";
}

/// Built-in native Tree-sitter query for Rust syntax facts.
///
/// The query engine owns node-kind matching. Rust code maps the declared
/// capture names to fact kinds and reads names/fields through the native node
/// API; it never walks the tree with its own kind switch.
const SYNTAX_QUERY: &str = r#"
(mod_item) @definition.module
(function_item) @definition.function
(function_signature_item) @definition.function
(struct_item) @definition.struct
(enum_item) @definition.enum
(union_item) @definition.union
(trait_item) @definition.trait
(impl_item) @definition.impl
(type_item) @definition.type_alias
(associated_type) @definition.type_alias
(const_item) @definition.constant
(static_item) @definition.static
(use_declaration) @definition.import
(call_expression) @reference.call
(field_declaration type: (type_identifier) @reference.type)
(parameter type: (type_identifier) @reference.type)
(function_item return_type: (type_identifier) @reference.type)
(function_signature_item return_type: (type_identifier) @reference.type)
(static_item type: (type_identifier) @reference.type)
(const_item type: (type_identifier) @reference.type)
(type_item type: (type_identifier) @reference.type)
(ERROR) @error
"#;

fn syntax_query() -> &'static Query {
    static QUERY: OnceLock<Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        Query::new(&tree_sitter_rust::LANGUAGE.into(), SYNTAX_QUERY)
            .expect("built-in Rust syntax query must compile")
    })
}

impl LanguageProvider for RustProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Rust
    }

    fn provider_id(&self) -> &'static str {
        Self::ID
    }

    fn provider_version(&self) -> &'static str {
        Self::VERSION
    }

    fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
        Some(tree_sitter_rust::LANGUAGE.into())
    }

    fn parse(&self, input: ParseInput<'_>) -> Result<ParseResult, ProviderError> {
        self.parse_bounded(input, None)
    }

    fn parse_with_control(
        &self,
        input: ParseInput<'_>,
        control: &ParseControl<'_>,
    ) -> Result<ParseResult, ProviderError> {
        self.parse_bounded(input, Some(control))
    }
}

impl RustProvider {
    /// Parse a bounded UTF-8 source snapshot and retain the native Tree-sitter tree.
    ///
    /// Unlike the compatibility fact-projection API, this returns no custom
    /// syntax representation. Consumers query and traverse the native tree.
    pub fn parse_document(&self, input: ParseInput<'_>) -> Result<ParsedDocument, ProviderError> {
        self.parse_document_bounded(input, None)
    }

    /// Parse a bounded source snapshot while observing shared cancellation.
    pub fn parse_document_with_control(
        &self,
        input: ParseInput<'_>,
        control: &ParseControl<'_>,
    ) -> Result<ParsedDocument, ProviderError> {
        self.parse_document_bounded(input, Some(control))
    }

    fn parse_document_bounded(
        &self,
        input: ParseInput<'_>,
        control: Option<&ParseControl<'_>>,
    ) -> Result<ParsedDocument, ProviderError> {
        if !valid_relative_path(input.relative_path) {
            return Err(ProviderError::InvalidPath);
        }
        std::str::from_utf8(input.source)?;
        if let Some(control) = control {
            control.check()?;
        }

        let source: Arc<[u8]> = Arc::from(input.source);
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE.into();
        parser
            .set_language(&language)
            .map_err(|error| ProviderError::ParserInitialization(error.to_string()))?;

        let mut budget_error = None;
        let mut progress = |_: &ParseState| match control.map(ParseControl::check) {
            Some(Err(error)) => {
                budget_error = Some(error);
                ControlFlow::Break(())
            }
            _ => ControlFlow::Continue(()),
        };
        let options = ParseOptions::new().progress_callback(&mut progress);
        let tree = parser.parse_with_options(
            &mut |offset, _| source.get(offset..).unwrap_or(&[]),
            None,
            Some(options),
        );

        let Some(tree) = tree else {
            parser.reset();
            if let Some(error) = budget_error {
                return Err(ProviderError::Budget(error));
            }
            if let Some(control) = control {
                control.check()?;
            }
            return Err(ProviderError::ParseAborted);
        };
        if let Some(control) = control {
            control.check()?;
        }

        Ok(ParsedDocument::new(
            input.workspace_id,
            input.relative_path.to_path_buf(),
            LanguageId::Rust,
            Self::ID.to_owned(),
            Self::VERSION.to_owned(),
            source,
            tree,
        ))
    }

    fn parse_bounded(
        &self,
        input: ParseInput<'_>,
        control: Option<&ParseControl<'_>>,
    ) -> Result<ParseResult, ProviderError> {
        // Parse once into the native document, then read facts from the same
        // Tree-sitter tree. There is no second parser and no custom AST.
        let document = self.parse_document_bounded(input, control)?;
        self.facts_from_document(&document, control)
    }

    fn facts_from_document(
        &self,
        document: &ParsedDocument,
        control: Option<&ParseControl<'_>>,
    ) -> Result<ParseResult, ProviderError> {
        let source = std::str::from_utf8(document.source())
            .expect("ParsedDocument validates UTF-8 before construction");
        let default_limits = ParseLimits::default();
        let limits = control.map_or(&default_limits, ParseControl::limits);
        if let Some(control) = control {
            control.check()?;
        }
        let root = document.root_node();
        let mut facts = Vec::new();
        let mut imports = Vec::new();
        let mut modules = Vec::new();
        let mut diagnostics = Vec::new();
        let mut limit_reason = None;

        // Bound the native tree with Tree-sitter's own traversal before running
        // a query. No parallel node graph or recursive walker is built here.
        if root.descendant_count() > limits.max_nodes {
            limit_reason = Some(ParseLimitKind::SyntaxNodes);
        }

        let query = syntax_query();
        let mut errors: Vec<Node<'_>> = Vec::new();
        let mut declarations: Vec<(SyntaxFactKind, Node<'_>)> = Vec::new();
        let mut impl_nodes: Vec<Node<'_>> = Vec::new();
        if limit_reason.is_none() {
            let mut cursor = QueryCursor::new();
            let mut matches = cursor.matches(query, root, source.as_bytes());
            while let Some(query_match) = matches.next() {
                if let Some(control) = control {
                    control.check()?;
                }
                for capture in query_match.captures {
                    let Some(capture_name) = query.capture_names().get(capture.index as usize)
                    else {
                        continue;
                    };
                    if *capture_name == "error" {
                        errors.push(capture.node);
                    } else if let Some(kind) = fact_kind(capture_name, capture.node) {
                        if kind == SyntaxFactKind::Impl {
                            impl_nodes.push(capture.node);
                        }
                        declarations.push((kind, capture.node));
                    }
                }
            }
        }

        errors.sort_unstable_by_key(|node| (node.start_byte(), node.end_byte()));
        errors.dedup_by_key(|node| (node.start_byte(), node.end_byte()));
        if !errors.is_empty() {
            if limits.max_diagnostics == 0 {
                limit_reason = Some(ParseLimitKind::Diagnostics);
            } else {
                for node in errors.iter().take(limits.max_diagnostics) {
                    diagnostics.push(ParseDiagnostic {
                        range: source_range(*node, source.len()),
                        message: "syntax error".to_owned(),
                    });
                }
                if errors.len() > limits.max_diagnostics {
                    limit_reason = Some(ParseLimitKind::Diagnostics);
                }
            }
        } else if root.has_error() {
            // Missing nodes are recoverable syntax but are not a queryable node
            // kind; keep one generic diagnostic rather than inventing a range.
            if limits.max_diagnostics == 0 {
                limit_reason = Some(ParseLimitKind::Diagnostics);
            } else {
                diagnostics.push(ParseDiagnostic {
                    range: None,
                    message: "syntax error".to_owned(),
                });
            }
        }
        diagnostics.sort_unstable_by_key(|diagnostic| diagnostic.range);

        declarations
            .sort_unstable_by_key(|(kind, node)| (node.start_byte(), node.end_byte(), *kind));
        for (kind, node) in declarations {
            if let Some(control) = control {
                control.check()?;
            }
            let Some(range) = source_range(node, source.len()) else {
                continue;
            };

            if kind == SyntaxFactKind::Module
                && let Some(name_node) = node.child_by_field_name("name")
                && let Some(name) = text_for_node(name_node, source)
            {
                if fact_limit_reached(&facts, &imports, &modules, limits) {
                    limit_reason = Some(ParseLimitKind::Facts);
                    break;
                }
                modules.push(RustModuleFact {
                    scope_path: enclosing_module_path(node, source),
                    name: name.trim_start_matches("r#").to_owned(),
                    is_inline: node.child_by_field_name("body").is_some(),
                    has_resolution_attributes: has_resolution_attributes(node, source),
                    range,
                });
            }

            if kind == SyntaxFactKind::Import {
                let scope_path = enclosing_module_path(node, source);
                let argument = node.child_by_field_name("argument").or_else(|| {
                    node.named_child(node.named_child_count().saturating_sub(1) as u32)
                });
                if let Some(argument) = argument {
                    let mut expanded = Vec::new();
                    flatten_use_tree(argument, &[], source, &mut expanded);
                    for (path, alias, is_glob) in expanded {
                        if fact_limit_reached(&facts, &imports, &modules, limits) {
                            limit_reason = Some(ParseLimitKind::Facts);
                            break;
                        }
                        imports.push(ImportFact {
                            scope_path: scope_path.clone(),
                            path,
                            alias,
                            is_glob,
                            range,
                        });
                    }
                }
                if limit_reason.is_some() {
                    break;
                }
            }

            if fact_limit_reached(&facts, &imports, &modules, limits) {
                limit_reason = Some(ParseLimitKind::Facts);
                break;
            }
            facts.push(SyntaxFact {
                kind,
                name: fact_name(kind, node, source),
                scope_path: enclosing_module_path(node, source),
                is_module_scope: is_module_scope(node),
                visibility: rust_visibility(node, source),
                range,
            });
        }

        facts.sort_unstable_by(|left, right| {
            (left.range, left.kind, &left.name).cmp(&(right.range, right.kind, &right.name))
        });
        diagnostics.sort_unstable_by_key(|diagnostic| diagnostic.range);
        imports.sort_unstable_by(|left, right| {
            (
                &left.scope_path,
                &left.path,
                &left.alias,
                left.is_glob,
                left.range,
            )
                .cmp(&(
                    &right.scope_path,
                    &right.path,
                    &right.alias,
                    right.is_glob,
                    right.range,
                ))
        });
        modules.sort_unstable_by(|left, right| {
            (&left.scope_path, &left.name, left.is_inline, left.range).cmp(&(
                &right.scope_path,
                &right.name,
                right.is_inline,
                right.range,
            ))
        });

        let status = if limit_reason.is_some() {
            ParseStatus::Limited
        } else {
            match document.status() {
                ParsedDocumentStatus::Complete => ParseStatus::Complete,
                ParsedDocumentStatus::Recovered => ParseStatus::Recovered,
            }
        };

        let relations = impl_nodes
            .into_iter()
            .filter_map(|node| {
                let source_name = node
                    .child_by_field_name("type")
                    .and_then(|value| text_for_node(value, source))
                    .map(|value| value.trim_start_matches("r#").to_owned())?;
                let target_name = node
                    .child_by_field_name("trait")
                    .and_then(|value| text_for_node(value, source))
                    .map(|value| value.trim_start_matches("r#").to_owned())?;
                Some(RelationFact {
                    kind: RelationKind::Implements,
                    source_name,
                    target_name,
                    range: source_range(node, source.len())?,
                })
            })
            .collect();

        Ok(ParseResult {
            workspace_id: document.workspace_id(),
            relative_path: document.relative_path().to_path_buf(),
            language: document.language(),
            provider_id: document.provider_id().to_owned(),
            provider_version: document.provider_version().to_owned(),
            schema_version: 3,
            status,
            limit_reason,
            facts,
            imports,
            relations,
            modules,
            diagnostics,
        })
    }
}

fn has_resolution_attributes(node: Node<'_>, source: &str) -> bool {
    // Outer attribute_item nodes are siblings immediately preceding mod_item.
    // Only attributes that can change module inclusion or file selection make
    // conventional path mapping unsafe; doc/lint/derive-style attributes do not.
    let mut previous = node.prev_named_sibling();
    while let Some(sibling) = previous {
        if sibling.kind() == "attribute_item" {
            let attribute = source
                .get(sibling.byte_range())
                .unwrap_or_default()
                .trim_start();
            if attribute.starts_with("#[cfg") || attribute.starts_with("#[path") {
                return true;
            }
            previous = sibling.prev_named_sibling();
        } else if sibling.kind().contains("comment") {
            previous = sibling.prev_named_sibling();
        } else {
            break;
        }
    }
    false
}

fn enclosing_module_path(node: Node<'_>, source: &str) -> Vec<String> {
    let mut modules = Vec::new();
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == "mod_item"
            && let Some(name) = current
                .child_by_field_name("name")
                .and_then(|name| text_for_node(name, source))
        {
            modules.push(name.trim_start_matches("r#").to_owned());
        }
        ancestor = current.parent();
    }
    modules.reverse();
    modules
}

fn is_module_scope(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if matches!(
            current.kind(),
            "function_item" | "function_signature_item" | "closure_expression" | "block"
        ) {
            return false;
        }
        ancestor = current.parent();
    }
    true
}

fn flatten_use_tree(
    node: Node<'_>,
    prefix: &[String],
    source: &str,
    output: &mut Vec<(Vec<String>, Option<String>, bool)>,
) {
    match node.kind() {
        "use_list" => {
            for index in 0..node.named_child_count() {
                if let Some(child) = node.named_child(index as u32) {
                    flatten_use_tree(child, prefix, source, output);
                }
            }
        }
        "scoped_use_list" => {
            let mut named =
                (0..node.named_child_count()).filter_map(|index| node.named_child(index as u32));
            if let (Some(path_node), Some(list_node)) = (named.next(), named.next()) {
                let mut path = prefix.to_vec();
                path.extend(path_segments(path_node, source));
                flatten_use_tree(list_node, &path, source, output);
            }
        }
        "use_as_clause" => {
            let mut named =
                (0..node.named_child_count()).filter_map(|index| node.named_child(index as u32));
            if let (Some(path_node), Some(alias_node)) = (named.next(), named.next()) {
                let mut path = prefix.to_vec();
                path.extend(path_segments(path_node, source));
                let alias = text_for_node(alias_node, source)
                    .map(|value| value.trim_start_matches("r#").to_owned());
                output.push((path, alias, false));
            }
        }
        "use_wildcard" => {
            let mut path = prefix.to_vec();
            if let Some(path_node) = node.named_child(0) {
                path.extend(path_segments(path_node, source));
            }
            output.push((path, None, true));
        }
        "self" => {
            output.push((prefix.to_vec(), None, false));
        }
        _ => {
            let mut path = prefix.to_vec();
            path.extend(path_segments(node, source));
            if !path.is_empty() {
                output.push((path, None, false));
            }
        }
    }
}

fn path_segments(node: Node<'_>, source: &str) -> Vec<String> {
    text_for_node(node, source)
        .unwrap_or_default()
        .split("::")
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.trim_start_matches("r#").to_owned())
        .collect()
}

fn rust_visibility(node: Node<'_>, source: &str) -> Option<super::RustVisibility> {
    if !matches!(
        node.kind(),
        "mod_item"
            | "function_item"
            | "function_signature_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "trait_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "use_declaration"
    ) {
        return None;
    }

    // Trait members are part of the trait's public interface and do not
    // declare an independent visibility modifier.
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == "trait_item" {
            return None;
        }
        ancestor = current.parent();
    }

    let modifier = (0..node.named_child_count())
        .filter_map(|index| node.named_child(index as u32))
        .find(|child| child.kind() == "visibility_modifier");
    let Some(modifier) = modifier else {
        return Some(super::RustVisibility::Private);
    };
    let spelling = text_for_node(modifier, source).unwrap_or_default();
    Some(match spelling.as_str() {
        "pub" => super::RustVisibility::Public,
        "pub(crate)" => super::RustVisibility::Crate,
        "pub(super)" => super::RustVisibility::Super,
        "pub(self)" => super::RustVisibility::SelfScope,
        value if value.starts_with("pub(in ") && value.ends_with(')') => {
            let path = value[7..value.len() - 1]
                .split("::")
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.trim_start_matches("r#").to_owned())
                .collect();
            super::RustVisibility::InPath(path)
        }
        _ => super::RustVisibility::Unknown(spelling),
    })
}

fn fact_kind(capture_name: &str, node: Node<'_>) -> Option<SyntaxFactKind> {
    Some(match capture_name {
        "definition.module" => SyntaxFactKind::Module,
        "definition.function" => {
            if is_associated_method(node) {
                SyntaxFactKind::Method
            } else {
                SyntaxFactKind::Function
            }
        }
        "definition.struct" => SyntaxFactKind::Struct,
        "definition.enum" => SyntaxFactKind::Enum,
        "definition.union" => SyntaxFactKind::Union,
        "definition.trait" => SyntaxFactKind::Trait,
        "definition.impl" => SyntaxFactKind::Impl,
        "definition.type_alias" => SyntaxFactKind::TypeAlias,
        "definition.constant" => SyntaxFactKind::Constant,
        "definition.static" => SyntaxFactKind::Static,
        "definition.import" => SyntaxFactKind::Import,
        "reference.call" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "field_expression" {
                SyntaxFactKind::MethodCall
            } else {
                SyntaxFactKind::Call
            }
        }
        "reference.type" => SyntaxFactKind::TypeReference,
        _ => return None,
    })
}

fn fact_name(kind: SyntaxFactKind, node: Node<'_>, source: &str) -> Option<String> {
    match kind {
        SyntaxFactKind::Impl => node
            .child_by_field_name("type")
            .and_then(|value| text_for_node(value, source)),
        SyntaxFactKind::Import => text_for_node(node, source),
        SyntaxFactKind::TypeReference => text_for_node(node, source),
        SyntaxFactKind::Call | SyntaxFactKind::MethodCall => {
            let function = node.child_by_field_name("function")?;
            let target = if kind == SyntaxFactKind::MethodCall {
                function.child_by_field_name("field").unwrap_or(function)
            } else {
                function
            };
            text_for_node(target, source)
        }
        _ => node
            .child_by_field_name("name")
            .and_then(|value| text_for_node(value, source)),
    }
}

fn fact_limit_reached(
    facts: &[SyntaxFact],
    imports: &[ImportFact],
    modules: &[RustModuleFact],
    limits: &ParseLimits,
) -> bool {
    facts
        .len()
        .saturating_add(imports.len())
        .saturating_add(modules.len())
        >= limits.max_facts
}

fn is_associated_method(node: Node<'_>) -> bool {
    node.parent()
        .and_then(|parent| parent.parent())
        .is_some_and(|ancestor| matches!(ancestor.kind(), "impl_item" | "trait_item"))
}

fn text_for_node(node: Node<'_>, source: &str) -> Option<String> {
    source
        .get(node.byte_range())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn source_range(node: Node<'_>, source_len: usize) -> Option<SourceRange> {
    let range = node.byte_range();
    SourceRange::new(range.start, range.end, source_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParseControl, ParseLimits, ScanBudget, ScanLimits, WorkspaceId};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn parse(source: &str) -> ParseResult {
        RustProvider
            .parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source: source.as_bytes(),
            })
            .expect("provider accepts UTF-8 source")
    }

    #[test]
    fn native_syntax_query_compiles_for_the_pinned_grammar() {
        let query = syntax_query();
        for capture in [
            "definition.module",
            "definition.function",
            "definition.struct",
            "definition.import",
            "reference.call",
            "error",
        ] {
            assert!(
                query.capture_names().contains(&capture),
                "missing capture {capture}"
            );
        }
    }

    #[test]
    fn parsed_document_exposes_native_tree_and_matching_source_snapshot() {
        let source = b"pub fn greet() {}";
        let document = RustProvider
            .parse_document(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source,
            })
            .expect("parse native document");

        assert_eq!(
            document.status(),
            super::super::ParsedDocumentStatus::Complete
        );
        assert_eq!(document.source(), source);
        assert_eq!(document.root_node().kind(), "source_file");

        let function = document.root_node().named_child(0).expect("function node");
        assert_eq!(function.kind(), "function_item");
        let name = function.child_by_field_name("name").expect("function name");
        assert_eq!(document.text_for_node(&name), Some("greet"));
        assert_eq!(document.relative_path(), Path::new("src/lib.rs"));
    }

    #[test]
    fn parsed_document_preserves_tree_sitter_recovery() {
        let document = RustProvider
            .parse_document(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source: b"fn broken( {",
            })
            .expect("tree-sitter should return a recovered tree");

        assert_eq!(
            document.status(),
            super::super::ParsedDocumentStatus::Recovered
        );
        assert!(document.root_node().has_error());
    }

    #[test]
    fn parsed_document_observes_shared_cancellation() {
        let budget = ScanBudget::new(ScanLimits::default());
        budget.cancel();
        let limits = ParseLimits::default();
        let error = match RustProvider.parse_document_with_control(
            ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source: b"fn main() {}",
            },
            &ParseControl::new(&budget, &limits),
        ) {
            Err(error) => error,
            Ok(_) => panic!("cancelled budget must stop native parsing"),
        };

        assert!(matches!(
            error,
            ProviderError::Budget(crate::ScanBudgetError::Cancelled)
        ));
    }

    #[test]
    fn extracts_explicit_and_implicit_visibility_from_grammar_nodes() {
        let result = parse(
            r#"
mod private_mod {}
pub mod public_mod {}
pub(crate) struct CrateType;
pub(super) fn parent_visible() {}
pub(self) const SELF_VALUE: u8 = 1;
pub(in crate::outer) type ScopedAlias = u8;
pub use crate::public_mod::Thing as PublicThing;
pub trait Api { fn trait_method(&self); const TRAIT_CONST: u8; type Output; }
fn private_fn() {}
"#,
        );
        let visibility = |name: &str| {
            result
                .facts
                .iter()
                .find(|fact| fact.name.as_deref() == Some(name))
                .and_then(|fact| fact.visibility.clone())
                .expect("declaration visibility fact")
        };
        assert_eq!(
            visibility("private_mod"),
            super::super::RustVisibility::Private
        );
        assert_eq!(
            visibility("public_mod"),
            super::super::RustVisibility::Public
        );
        assert_eq!(visibility("CrateType"), super::super::RustVisibility::Crate);
        assert_eq!(
            visibility("parent_visible"),
            super::super::RustVisibility::Super
        );
        assert_eq!(
            visibility("SELF_VALUE"),
            super::super::RustVisibility::SelfScope
        );
        assert_eq!(
            visibility("ScopedAlias"),
            super::super::RustVisibility::InPath(vec!["crate".to_owned(), "outer".to_owned()])
        );
        assert_eq!(
            visibility("private_fn"),
            super::super::RustVisibility::Private
        );
        for trait_member in ["trait_method", "TRAIT_CONST", "Output"] {
            let fact = result
                .facts
                .iter()
                .find(|fact| fact.name.as_deref() == Some(trait_member))
                .expect("trait member syntax fact");
            assert_eq!(fact.visibility, None);
        }
        let import = result
            .facts
            .iter()
            .find(|fact| fact.kind == SyntaxFactKind::Import)
            .expect("pub use syntax fact");
        assert_eq!(
            import.visibility,
            Some(super::super::RustVisibility::Public)
        );
    }

    #[test]
    fn extracts_declarations_imports_and_calls_with_ranges() {
        let source = r#"
use std::fmt::Debug;

pub mod nested {}
pub struct Widget;
pub enum State { Ready, Busy }
pub union Bits { value: u32 }
pub trait Render { fn render(&self); }
impl Widget {
    pub fn draw(&self) { self.render(); helper(); }
}
type ResultAlias = Result<(), ()>;
const LIMIT: usize = 10;
static ENABLED: bool = true;
fn helper() {}
"#;
        let result = parse(source);
        assert_eq!(result.status, ParseStatus::Complete);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, vec!["std", "fmt", "Debug"]);
        assert!(
            result
                .modules
                .iter()
                .any(|item| item.name == "nested" && item.is_inline)
        );
        for kind in [
            SyntaxFactKind::Module,
            SyntaxFactKind::Import,
            SyntaxFactKind::Struct,
            SyntaxFactKind::Enum,
            SyntaxFactKind::Union,
            SyntaxFactKind::Trait,
            SyntaxFactKind::Impl,
            SyntaxFactKind::Method,
            SyntaxFactKind::Function,
            SyntaxFactKind::TypeAlias,
            SyntaxFactKind::Constant,
            SyntaxFactKind::Static,
            SyntaxFactKind::Call,
            SyntaxFactKind::MethodCall,
        ] {
            assert!(
                result.facts.iter().any(|fact| fact.kind == kind),
                "missing {kind:?}"
            );
        }
        for fact in &result.facts {
            assert!(fact.range.end_byte <= source.len());
            assert!(source.is_char_boundary(fact.range.start_byte));
            assert!(source.is_char_boundary(fact.range.end_byte));
        }
    }

    #[test]
    fn records_lexical_module_scope_and_excludes_block_local_items() {
        let result = parse(
            r#"
mod nested {
    fn visible_in_module() {}
    fn outer() {
        fn local_only() {}
    }
}
"#,
        );
        let module_fn = result
            .facts
            .iter()
            .find(|fact| fact.name.as_deref() == Some("visible_in_module"))
            .expect("module function");
        assert_eq!(module_fn.scope_path, vec!["nested".to_owned()]);
        assert!(module_fn.is_module_scope);

        let local_fn = result
            .facts
            .iter()
            .find(|fact| fact.name.as_deref() == Some("local_only"))
            .expect("local function");
        assert_eq!(local_fn.scope_path, vec!["nested".to_owned()]);
        assert!(!local_fn.is_module_scope);
    }

    #[test]
    fn extracts_grouped_alias_glob_self_and_nested_module_imports() {
        let result = parse(
            r#"
use crate::model::{Widget, self, State as CurrentState, nested::*};
mod nested {
    use super::Widget;
}
"#,
        );
        assert!(
            result.imports.iter().any(|item| item.path
                == vec!["crate".to_owned(), "model".to_owned(), "Widget".to_owned()])
        );
        assert!(result.imports.iter().any(|item| item.path
            == vec!["crate".to_owned(), "model".to_owned()]
            && item.alias.is_none()
            && !item.is_glob));
        assert!(result.imports.iter().any(|item| item.path
            == vec!["crate".to_owned(), "model".to_owned(), "State".to_owned()]
            && item.alias.as_deref() == Some("CurrentState")));
        assert!(result.imports.iter().any(|item| item.path
            == vec!["crate".to_owned(), "model".to_owned(), "nested".to_owned()]
            && item.is_glob));
        assert!(result.imports.iter().any(|item| item.path
            == vec!["super".to_owned(), "Widget".to_owned()]
            && item.scope_path == vec!["nested".to_owned()]));
    }

    #[test]
    fn distinguishes_inline_and_external_module_declarations() {
        let result = parse(
            r#"
mod inline_child { mod nested; }
mod external_child;
"#,
        );
        assert!(result.modules.iter().any(|item| item.name == "inline_child"
            && item.is_inline
            && item.scope_path.is_empty()));
        assert!(result.modules.iter().any(|item| item.name == "nested"
            && !item.is_inline
            && item.scope_path == vec!["inline_child".to_owned()]));
        assert!(
            result
                .modules
                .iter()
                .any(|item| item.name == "external_child"
                    && !item.is_inline
                    && item.scope_path.is_empty())
        );
    }

    #[test]
    fn marks_attributed_module_declarations_as_uncertain() {
        let result = parse(
            r#"#[cfg(feature = "optional")]
mod optional;
#[path = "custom/location.rs"]
mod custom;
#[doc = "documented module"]
mod documented;
"#,
        );
        assert!(
            result
                .modules
                .iter()
                .any(|item| item.name == "optional" && item.has_resolution_attributes)
        );
        assert!(
            result
                .modules
                .iter()
                .any(|item| item.name == "custom" && item.has_resolution_attributes)
        );
        assert!(
            result
                .modules
                .iter()
                .any(|item| item.name == "documented" && !item.has_resolution_attributes)
        );
    }

    #[test]
    fn recovers_facts_and_reports_malformed_syntax() {
        let result = parse("fn good() {}\nfn broken( {\nfn also_good() {}");
        assert_eq!(result.status, ParseStatus::Recovered);
        assert!(!result.diagnostics.is_empty());
    }

    #[test]
    fn preserves_workspace_identity_and_relative_path() {
        let id = WorkspaceId::new();
        let result = RustProvider
            .parse(ParseInput {
                workspace_id: id,
                relative_path: Path::new("src/main.rs"),
                source: b"fn main() {}",
            })
            .expect("parse source");
        assert_eq!(result.workspace_id, id);
        assert_eq!(result.relative_path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn cooperatively_rejects_an_already_cancelled_scan() {
        let budget = ScanBudget::new(ScanLimits::default());
        budget.cancel();
        let limits = ParseLimits::default();
        let error = RustProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/lib.rs"),
                    source: b"fn main() {}",
                },
                &ParseControl::new(&budget, &limits),
            )
            .expect_err("cancelled budget must stop parsing");

        assert!(matches!(
            error,
            ProviderError::Budget(crate::ScanBudgetError::Cancelled)
        ));
    }

    #[test]
    fn bounds_fact_output_and_preserves_limited_status() {
        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::from_secs(10),
            ..ScanLimits::default()
        });
        let limits = ParseLimits {
            max_nodes: 1000,
            max_facts: 1,
            max_diagnostics: 10,
        };
        let result = RustProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/lib.rs"),
                    source: b"fn first() {} fn second() {}",
                },
                &ParseControl::new(&budget, &limits),
            )
            .expect("parse within limits");

        assert_eq!(result.status, ParseStatus::Limited);
        assert_eq!(result.limit_reason, Some(ParseLimitKind::Facts));
        assert_eq!(result.facts.len(), 1);
    }

    #[test]
    fn aggregate_fact_limit_covers_syntax_import_and_module_records() {
        let budget = ScanBudget::new(ScanLimits::default());
        let limits = ParseLimits {
            max_nodes: 1000,
            max_facts: 2,
            max_diagnostics: 10,
        };
        let result = RustProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/lib.rs"),
                    source: b"mod child {} use crate::Thing; fn run() {}",
                },
                &ParseControl::new(&budget, &limits),
            )
            .expect("parse within aggregate fact limit");

        assert_eq!(result.status, ParseStatus::Limited);
        assert_eq!(result.limit_reason, Some(ParseLimitKind::Facts));
        assert!(result.facts.len() + result.imports.len() + result.modules.len() <= 2);
    }

    #[test]
    fn bounds_syntax_tree_traversal() {
        let budget = ScanBudget::new(ScanLimits::default());
        let limits = ParseLimits {
            max_nodes: 1,
            max_facts: 100,
            max_diagnostics: 10,
        };
        let result = RustProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/lib.rs"),
                    source: b"fn main() {}",
                },
                &ParseControl::new(&budget, &limits),
            )
            .expect("parse within node limit");

        assert_eq!(result.status, ParseStatus::Limited);
        assert_eq!(result.limit_reason, Some(ParseLimitKind::SyntaxNodes));
    }

    #[test]
    fn bounds_diagnostic_output() {
        let budget = ScanBudget::new(ScanLimits::default());
        let limits = ParseLimits {
            max_nodes: 1000,
            max_facts: 100,
            max_diagnostics: 0,
        };
        let result = RustProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/lib.rs"),
                    source: b"fn broken( {",
                },
                &ParseControl::new(&budget, &limits),
            )
            .expect("parse within diagnostic limit");

        assert_eq!(result.status, ParseStatus::Limited);
        assert_eq!(result.limit_reason, Some(ParseLimitKind::Diagnostics));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn rejects_invalid_utf8_and_non_relative_paths() {
        assert!(matches!(
            RustProvider.parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/lib.rs"),
                source: &[0xff],
            }),
            Err(ProviderError::InvalidUtf8(_))
        ));
        assert!(matches!(
            RustProvider.parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("../outside.rs"),
                source: b"fn main() {}",
            }),
            Err(ProviderError::InvalidPath)
        ));
    }
    #[test]
    fn extracts_impl_trait_relations() {
        let result = parse(
            "trait Api {}
struct Foo;
impl Api for Foo {}
",
        );
        assert!(
            result.relations.iter().any(|relation| {
                relation.kind == RelationKind::Implements
                    && relation.source_name == "Foo"
                    && relation.target_name == "Api"
            }),
            "relations: {:?}",
            result.relations
        );
    }
}
