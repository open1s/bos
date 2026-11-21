//! Python syntax provider backed by the upstream Tree-sitter grammar.
//!
//! Parses once into [`ParsedDocument`], then one native query captures
//! functions, classes, imports, calls, and simple type annotations. The output
//! is the same flat [`SyntaxFact`] projection as Rust and TypeScript.

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

/// Python provider using the upstream Tree-sitter Python grammar.
#[derive(Clone, Copy, Debug, Default)]
pub struct PythonProvider;

impl PythonProvider {
    /// Stable provider identifier.
    pub const ID: &'static str = "codel.python.tree-sitter";

    /// Parser and grammar versions used by this provider.
    pub const VERSION: &'static str = "tree-sitter-0.26.13/tree-sitter-python-0.23.6/facts-v1";

    /// Parse one bounded source snapshot into a native document.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] for invalid input, parser setup failure, or
    /// cancellation.
    pub fn parse_document(&self, input: ParseInput<'_>) -> Result<ParsedDocument, ProviderError> {
        self.parse_document_bounded(input, None)
    }

    /// Parse one bounded source snapshot while observing shared cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError`] for invalid input, parser setup failure, or
    /// cancellation.
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
        let language = tree_sitter_python::LANGUAGE.into();
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
            LanguageId::Python,
            Self::ID.to_owned(),
            Self::VERSION.to_owned(),
            source,
            tree,
        ))
    }

    fn facts_from_document(
        &self,
        document: &ParsedDocument,
        control: Option<&ParseControl<'_>>,
    ) -> Result<ParseResult, ProviderError> {
        let source = std::str::from_utf8(document.source())
            .expect("ParsedDocument validates UTF-8 before construction");
        if let Some(control) = control {
            control.check()?;
        }
        let default_limits = ParseLimits::default();
        let limits = control.map_or(&default_limits, ParseControl::limits);
        let root = document.root_node();

        if root.descendant_count() > limits.max_nodes {
            return Ok(limited_result(document, ParseLimitKind::SyntaxNodes));
        }

        let query = syntax_query();
        let mut facts = Vec::new();
        let mut errors: Vec<Node<'_>> = Vec::new();
        let mut class_nodes: Vec<Node<'_>> = Vec::new();
        let mut limit_reason = None;
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            if let Some(control) = control {
                control.check()?;
            }
            for capture in query_match.captures {
                let Some(capture_name) = query.capture_names().get(capture.index as usize) else {
                    continue;
                };
                if *capture_name == "error" {
                    errors.push(capture.node);
                    continue;
                }
                if facts.len() >= limits.max_facts {
                    limit_reason = Some(ParseLimitKind::Facts);
                    break;
                }
                if let Some(kind) = fact_kind(capture_name, capture.node) {
                    if kind == SyntaxFactKind::Struct {
                        class_nodes.push(capture.node);
                    }
                    facts.push(SyntaxFact {
                        kind,
                        name: fact_name(kind, capture.node, source),
                        scope_path: Vec::new(),
                        is_module_scope: is_module_scope(capture.node),
                        visibility: None,
                        range: range_of(capture.node),
                    });
                }
            }
            if limit_reason.is_some() {
                break;
            }
        }

        facts.sort_unstable_by(|left, right| {
            (left.range, left.kind, &left.name).cmp(&(right.range, right.kind, &right.name))
        });
        errors.sort_unstable_by_key(|node| (node.start_byte(), node.end_byte()));
        errors.dedup_by_key(|node| (node.start_byte(), node.end_byte()));

        let mut diagnostics = Vec::new();
        if limits.max_diagnostics == 0 && !errors.is_empty() {
            limit_reason = Some(ParseLimitKind::Diagnostics);
        } else {
            for node in errors.iter().take(limits.max_diagnostics) {
                diagnostics.push(ParseDiagnostic {
                    range: Some(range_of(*node)),
                    message: "syntax error".to_owned(),
                });
            }
            if errors.len() > limits.max_diagnostics {
                limit_reason = Some(ParseLimitKind::Diagnostics);
            }
        }

        let status = if limit_reason.is_some() {
            ParseStatus::Limited
        } else {
            match document.status() {
                ParsedDocumentStatus::Complete => ParseStatus::Complete,
                ParsedDocumentStatus::Recovered => ParseStatus::Recovered,
            }
        };

        Ok(ParseResult {
            workspace_id: document.workspace_id(),
            relative_path: document.relative_path().to_path_buf(),
            language: LanguageId::Python,
            provider_id: document.provider_id().to_owned(),
            provider_version: document.provider_version().to_owned(),
            schema_version: 1,
            status,
            limit_reason,
            facts,
            imports: Vec::<ImportFact>::new(),
            relations: relations_for(&class_nodes, source),
            modules: Vec::<RustModuleFact>::new(),
            diagnostics,
        })
    }
}

impl LanguageProvider for PythonProvider {
    fn language(&self) -> LanguageId {
        LanguageId::Python
    }

    fn provider_id(&self) -> &'static str {
        Self::ID
    }

    fn provider_version(&self) -> &'static str {
        Self::VERSION
    }

    fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
        Some(tree_sitter_python::LANGUAGE.into())
    }

    fn parse(&self, input: ParseInput<'_>) -> Result<ParseResult, ProviderError> {
        let document = self.parse_document(input)?;
        self.facts_from_document(&document, None)
    }

    fn parse_with_control(
        &self,
        input: ParseInput<'_>,
        control: &ParseControl<'_>,
    ) -> Result<ParseResult, ProviderError> {
        let document = self.parse_document_with_control(input, control)?;
        self.facts_from_document(&document, Some(control))
    }
}

const SYNTAX_QUERY: &str = r#"
(function_definition name: (identifier)) @definition.function
(class_definition name: (identifier)) @definition.struct
(import_statement) @definition.import
(import_from_statement) @definition.import
(call) @reference.call
(typed_parameter type: (type) @reference.type)
(typed_default_parameter type: (type) @reference.type)
(ERROR) @error
"#;

fn syntax_query() -> &'static Query {
    static QUERY: OnceLock<Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        Query::new(&tree_sitter_python::LANGUAGE.into(), SYNTAX_QUERY)
            .expect("built-in Python syntax query must compile")
    })
}

fn fact_kind(capture_name: &str, node: Node<'_>) -> Option<SyntaxFactKind> {
    Some(match capture_name {
        "definition.function" => {
            if is_inside_class(node) {
                SyntaxFactKind::Method
            } else {
                SyntaxFactKind::Function
            }
        }
        "definition.struct" => SyntaxFactKind::Struct,
        "definition.import" => SyntaxFactKind::Import,
        "reference.call" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "attribute" {
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
        SyntaxFactKind::Import => text_for_node(node, source).map(str::to_owned),
        SyntaxFactKind::TypeReference => text_for_node(node, source).map(str::to_owned),
        SyntaxFactKind::Call | SyntaxFactKind::MethodCall => {
            let function = node.child_by_field_name("function")?;
            let target = if kind == SyntaxFactKind::MethodCall {
                function
                    .child_by_field_name("attribute")
                    .unwrap_or(function)
            } else {
                function
            };
            text_for_node(target, source).map(str::to_owned)
        }
        _ => node
            .child_by_field_name("name")
            .and_then(|value| text_for_node(value, source))
            .map(str::to_owned),
    }
}

fn is_inside_class(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == "class_definition" {
            return true;
        }
        ancestor = current.parent();
    }
    false
}

fn is_module_scope(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == "function_definition" {
            return false;
        }
        ancestor = current.parent();
    }
    true
}

fn relations_for(classes: &[Node<'_>], source: &str) -> Vec<RelationFact> {
    let mut relations = Vec::new();
    for class in classes {
        let Some(source_name) = class
            .child_by_field_name("name")
            .and_then(|node| text_for_node(node, source))
        else {
            continue;
        };
        let Some(superclasses) = class.child_by_field_name("superclasses") else {
            continue;
        };
        for target in named_children(superclasses) {
            if target.kind() == "keyword_argument" {
                continue;
            }
            if let Some(text) = text_for_node(target, source) {
                relations.push(RelationFact {
                    kind: RelationKind::Inherits,
                    source_name: source_name.to_owned(),
                    target_name: text.trim().to_owned(),
                    range: range_of(target),
                });
            }
        }
    }
    relations
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    (0..node.named_child_count())
        .filter_map(|index| node.named_child(index as u32))
        .collect()
}

fn range_of(node: Node<'_>) -> SourceRange {
    SourceRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

fn text_for_node<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range())
}

fn limited_result(document: &ParsedDocument, reason: ParseLimitKind) -> ParseResult {
    ParseResult {
        workspace_id: document.workspace_id(),
        relative_path: document.relative_path().to_path_buf(),
        language: LanguageId::Python,
        provider_id: document.provider_id().to_owned(),
        provider_version: document.provider_version().to_owned(),
        schema_version: 1,
        status: ParseStatus::Limited,
        limit_reason: Some(reason),
        facts: Vec::new(),
        imports: Vec::new(),
        relations: Vec::new(),
        modules: Vec::new(),
        diagnostics: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::WorkspaceId;

    fn parse(source: &str) -> ParseResult {
        PythonProvider
            .parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/app.py"),
                source: source.as_bytes(),
            })
            .expect("Python source")
    }

    #[test]
    fn extracts_declarations_methods_calls_and_imports() {
        let result = parse(
            r#"
import os
class Base:
    pass
class Child(Base):
    def run(self):
        helper()
def helper():
    return 1
"#,
        );

        assert_eq!(result.status, ParseStatus::Complete);
        assert_eq!(result.language, LanguageId::Python);
        for (name, kind) in [
            ("Child", SyntaxFactKind::Struct),
            ("run", SyntaxFactKind::Method),
            ("helper", SyntaxFactKind::Function),
        ] {
            assert!(
                result
                    .facts
                    .iter()
                    .any(|fact| fact.name.as_deref() == Some(name) && fact.kind == kind),
                "missing {name} as {kind:?}"
            );
        }
        assert!(
            result
                .facts
                .iter()
                .any(|fact| fact.kind == SyntaxFactKind::Import)
        );
        assert!(
            result
                .facts
                .iter()
                .any(|fact| fact.kind == SyntaxFactKind::Call)
        );
        assert!(result.relations.iter().any(|relation| {
            relation.kind == RelationKind::Inherits
                && relation.source_name == "Child"
                && relation.target_name == "Base"
        }));
    }

    #[test]
    fn extracts_simple_type_annotations() {
        let result = parse(
            "class Widget:\n    pass\ndef build(value: Widget) -> Widget:\n    return value\n",
        );
        assert!(
            result
                .facts
                .iter()
                .any(|fact| fact.kind == SyntaxFactKind::TypeReference),
            "facts: {:?}",
            result.facts
        );
    }
}
