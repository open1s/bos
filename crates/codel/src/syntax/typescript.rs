//! TypeScript syntax provider backed by the upstream Tree-sitter grammar.
//!
//! The provider parses once into [`ParsedDocument`], then classifies
//! declarations, imports, and calls with one native Tree-sitter query. It emits
//! the same language-neutral [`SyntaxFact`] projection as the Rust provider, so
//! the candidate index and graph do not need a TypeScript-specific model.

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

/// TypeScript provider using the upstream Tree-sitter TypeScript grammar.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptProvider;

impl TypeScriptProvider {
    /// Stable provider identifier.
    pub const ID: &'static str = "codel.typescript.tree-sitter";

    /// Parser and grammar versions used by this provider.
    pub const VERSION: &'static str = "tree-sitter-0.26.13/tree-sitter-typescript-0.23.2/facts-v1";

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
        let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
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
            LanguageId::TypeScript,
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
        let mut interface_nodes: Vec<Node<'_>> = Vec::new();
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
                    if kind == SyntaxFactKind::Trait {
                        interface_nodes.push(capture.node);
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

        semantic_facts(root, source, &mut facts, limits.max_facts);

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
            language: LanguageId::TypeScript,
            provider_id: document.provider_id().to_owned(),
            provider_version: document.provider_version().to_owned(),
            schema_version: 1,
            status,
            limit_reason,
            facts,
            imports: Vec::<ImportFact>::new(),
            relations: relations_for(&class_nodes, &interface_nodes, source),
            modules: Vec::<RustModuleFact>::new(),
            diagnostics,
        })
    }
}

impl LanguageProvider for TypeScriptProvider {
    fn language(&self) -> LanguageId {
        LanguageId::TypeScript
    }

    fn provider_id(&self) -> &'static str {
        Self::ID
    }

    fn provider_version(&self) -> &'static str {
        Self::VERSION
    }

    fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
        Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
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
(function_declaration name: (identifier)) @definition.function
(function_expression name: (identifier)) @definition.function
(class_declaration name: (type_identifier)) @definition.struct
(interface_declaration name: (type_identifier)) @definition.trait
(type_alias_declaration name: (type_identifier)) @definition.type_alias
(method_definition name: (property_identifier)) @definition.method
(method_signature name: (property_identifier)) @definition.method
(variable_declarator name: (identifier)) @definition.constant
(import_statement) @definition.import
(call_expression) @reference.call
(type_annotation (type_identifier) @reference.type)
(ERROR) @error
"#;

fn syntax_query() -> &'static Query {
    static QUERY: OnceLock<Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        Query::new(
            &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SYNTAX_QUERY,
        )
        .expect("built-in TypeScript syntax query must compile")
    })
}

fn semantic_facts(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>, max_facts: usize) {
    if facts.len() >= max_facts {
        return;
    }
    match root.kind() {
        "variable_declarator" => {
            if let (Some(name), Some(ty)) = (
                root.child_by_field_name("name")
                    .and_then(|node| text_for_node(node, source)),
                type_for_node(root, "type", source),
            ) {
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::Variable,
                    name: Some(name.to_owned()),
                    scope_path: vec![ty],
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        "required_parameter" | "optional_parameter" => {
            if let (Some(name), Some(ty)) = (
                root.child_by_field_name("pattern")
                    .and_then(|node| text_for_node(node, source)),
                type_for_node(root, "type", source),
            ) {
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::Variable,
                    name: Some(name.to_owned()),
                    scope_path: vec![ty],
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        "public_field_definition" => {
            if let (Some(name), Some(ty)) = (
                root.child_by_field_name("name")
                    .and_then(|node| text_for_node(node, source)),
                type_for_node(root, "type", source),
            ) {
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::Variable,
                    name: Some(name.to_owned()),
                    scope_path: vec![ty],
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::Field,
                    name: Some(name.to_owned()),
                    scope_path: Vec::new(),
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        "method_definition" => {
            if let (Some(name), Some(return_type)) = (
                root.child_by_field_name("name")
                    .and_then(|node| text_for_node(node, source)),
                type_for_node(root, "return_type", source),
            ) {
                let parameters = root.child_by_field_name("parameters");
                let arity = parameters
                    .map(|parameters| named_children(parameters).len().to_string())
                    .unwrap_or_default();
                let mut signature = vec![return_type, arity];
                if let Some(parameters) = parameters {
                    for parameter in named_children(parameters) {
                        if let Some(ty) = type_for_node(parameter, "type", source) {
                            signature.push(ty);
                        }
                    }
                }
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::MethodSignature,
                    name: Some(name.to_owned()),
                    scope_path: signature,
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        "call_expression" => {
            let Some(function) = root.child_by_field_name("function") else {
                return;
            };
            if function.kind() == "member_expression"
                && let (Some(receiver), Some(name)) = (
                    function
                        .child_by_field_name("object")
                        .and_then(|node| text_for_node(node, source)),
                    function
                        .child_by_field_name("property")
                        .and_then(|node| text_for_node(node, source)),
                )
            {
                let arguments = root.child_by_field_name("arguments");
                let arity = arguments
                    .map(|arguments| named_children(arguments).len().to_string())
                    .unwrap_or_default();
                let mut call = vec![receiver.to_owned(), arity];
                if let Some(arguments) = arguments {
                    for argument in named_children(arguments) {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                }
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::ReceiverCall,
                    name: Some(name.to_owned()),
                    scope_path: call,
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        "member_expression" => {
            let is_call_function = root.parent().is_some_and(|parent| {
                parent.kind() == "call_expression"
                    && parent
                        .child_by_field_name("function")
                        .is_some_and(|function| function.id() == root.id())
            });
            if !is_call_function
                && let (Some(receiver), Some(field)) = (
                    root.child_by_field_name("object")
                        .and_then(|node| text_for_node(node, source)),
                    root.child_by_field_name("property")
                        .and_then(|node| text_for_node(node, source)),
                )
            {
                let parent = root.parent();
                let (write, operator) = if parent.is_some_and(|parent| {
                    parent.kind() == "assignment_expression"
                        && parent
                            .child_by_field_name("left")
                            .is_some_and(|left| left.id() == root.id())
                }) {
                    (true, "=".to_owned())
                } else if let Some(parent) = parent.filter(|parent| {
                    parent.kind() == "augmented_assignment_expression"
                        && parent
                            .child_by_field_name("left")
                            .is_some_and(|left| left.id() == root.id())
                }) {
                    (
                        true,
                        parent
                            .child_by_field_name("operator")
                            .and_then(|operator| text_for_node(operator, source))
                            .unwrap_or("+=")
                            .to_owned(),
                    )
                } else if let Some(parent) =
                    parent.filter(|parent| parent.kind() == "update_expression")
                {
                    (
                        true,
                        parent
                            .child_by_field_name("operator")
                            .and_then(|operator| text_for_node(operator, source))
                            .unwrap_or("++")
                            .to_owned(),
                    )
                } else {
                    (false, "read".to_owned())
                };
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::FieldAccess,
                    name: Some(field.to_owned()),
                    scope_path: vec![
                        receiver.to_owned(),
                        if write { "write" } else { "read" }.to_owned(),
                        operator,
                    ],
                    is_module_scope: false,
                    visibility: None,
                    range: range_of(root),
                });
            }
        }
        _ => {}
    }
    for child in named_children(root) {
        if facts.len() >= max_facts {
            break;
        }
        semantic_facts(child, source, facts, max_facts);
    }
}

fn bare_type(value: &str) -> String {
    value
        .split(['<', '['])
        .next()
        .unwrap_or(value)
        .trim()
        .to_owned()
}

fn type_for_node(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    let ty = node.child_by_field_name(field)?;
    let ty = if ty.kind() == "type_annotation" {
        named_children(ty).first().copied().unwrap_or(ty)
    } else {
        ty
    };
    text_for_node(ty, source).map(bare_type)
}

fn fact_kind(capture_name: &str, node: Node<'_>) -> Option<SyntaxFactKind> {
    Some(match capture_name {
        "definition.function" => SyntaxFactKind::Function,
        "definition.method" => SyntaxFactKind::Method,
        "definition.struct" => SyntaxFactKind::Struct,
        "definition.trait" => SyntaxFactKind::Trait,
        "definition.type_alias" => SyntaxFactKind::TypeAlias,
        "definition.constant" => SyntaxFactKind::Constant,
        "definition.import" => SyntaxFactKind::Import,
        "reference.call" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "member_expression" {
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
                function.child_by_field_name("property").unwrap_or(function)
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

fn is_module_scope(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if matches!(
            current.kind(),
            "function_declaration"
                | "function_expression"
                | "arrow_function"
                | "method_definition"
                | "method_signature"
                | "statement_block"
        ) {
            return false;
        }
        ancestor = current.parent();
    }
    true
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

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    (0..node.named_child_count())
        .filter_map(|index| node.named_child(index as u32))
        .collect()
}

fn relations_for(classes: &[Node<'_>], interfaces: &[Node<'_>], source: &str) -> Vec<RelationFact> {
    let mut relations = Vec::new();
    for class in classes {
        let Some(source_name) = class
            .child_by_field_name("name")
            .and_then(|node| text_for_node(node, source))
            .map(bare_type_name)
        else {
            continue;
        };
        let Some(heritage) = named_children(*class)
            .into_iter()
            .find(|child| child.kind() == "class_heritage")
        else {
            continue;
        };
        for clause in named_children(heritage) {
            match clause.kind() {
                "extends_clause" => {
                    if let Some(target) = clause
                        .child_by_field_name("value")
                        .and_then(|node| text_for_node(node, source))
                    {
                        relations.push(RelationFact {
                            kind: RelationKind::Inherits,
                            source_name: source_name.clone(),
                            target_name: bare_type_name(target),
                            range: range_of(clause),
                        });
                    }
                }
                "implements_clause" => {
                    for target in named_children(clause) {
                        if let Some(target) = text_for_node(target, source) {
                            relations.push(RelationFact {
                                kind: RelationKind::Implements,
                                source_name: source_name.clone(),
                                target_name: bare_type_name(target),
                                range: range_of(clause),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    for interface in interfaces {
        let Some(source_name) = interface
            .child_by_field_name("name")
            .and_then(|node| text_for_node(node, source))
            .map(bare_type_name)
        else {
            continue;
        };
        let Some(clause) = named_children(*interface)
            .into_iter()
            .find(|child| child.kind() == "extends_type_clause")
        else {
            continue;
        };
        for target in named_children(clause) {
            if let Some(target) = text_for_node(target, source) {
                relations.push(RelationFact {
                    kind: RelationKind::Inherits,
                    source_name: source_name.clone(),
                    target_name: bare_type_name(target),
                    range: range_of(clause),
                });
            }
        }
    }

    relations
}

fn bare_type_name(value: &str) -> String {
    value.split('<').next().unwrap_or(value).trim().to_owned()
}

fn limited_result(document: &ParsedDocument, reason: ParseLimitKind) -> ParseResult {
    ParseResult {
        workspace_id: document.workspace_id(),
        relative_path: document.relative_path().to_path_buf(),
        language: LanguageId::TypeScript,
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
        TypeScriptProvider
            .parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/app.ts"),
                source: source.as_bytes(),
            })
            .expect("TypeScript source")
    }

    #[test]
    fn extracts_declarations_methods_calls_and_imports() {
        let result = parse(
            r#"
import { Foo } from "./foo";
export class Service {
  run() { helper(); this.stop(); }
}
function helper() { return 1; }
const answer = 42;
interface Api { ping(): void; }
type Alias = string;
"#,
        );

        assert_eq!(result.status, ParseStatus::Complete);
        assert_eq!(result.language, LanguageId::TypeScript);
        for (name, kind) in [
            ("Service", SyntaxFactKind::Struct),
            ("run", SyntaxFactKind::Method),
            ("helper", SyntaxFactKind::Function),
            ("answer", SyntaxFactKind::Constant),
            ("Api", SyntaxFactKind::Trait),
            ("Alias", SyntaxFactKind::TypeAlias),
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
        assert!(
            result
                .facts
                .iter()
                .any(|fact| fact.kind == SyntaxFactKind::MethodCall)
        );
    }

    #[test]
    fn native_document_exposes_the_typescript_tree() {
        let document = TypeScriptProvider
            .parse_document(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/app.ts"),
                source: b"function greet(name: string) { return name; }",
            })
            .expect("document");

        assert_eq!(document.language(), LanguageId::TypeScript);
        assert_eq!(document.status(), ParsedDocumentStatus::Complete);
        assert_eq!(document.root_node().kind(), "program");
    }

    #[test]
    fn bounds_fact_output() {
        let control_budget = crate::ScanBudget::new(crate::ScanLimits::default());
        let limits = ParseLimits {
            max_nodes: 10_000,
            max_facts: 1,
            max_diagnostics: 10,
        };
        let result = TypeScriptProvider
            .parse_with_control(
                ParseInput {
                    workspace_id: WorkspaceId::new(),
                    relative_path: Path::new("src/app.ts"),
                    source: b"function one() {}
function two() {}
",
                },
                &ParseControl::new(&control_budget, &limits),
            )
            .expect("parsed");

        assert_eq!(result.status, ParseStatus::Limited);
        assert_eq!(result.limit_reason, Some(ParseLimitKind::Facts));
        assert_eq!(result.facts.len(), 1);
    }
    #[test]
    fn extracts_class_and_interface_heritage_relations() {
        let result = parse(
            "class Base {}
interface Api {}
class Child extends Base implements Api {}
",
        );
        assert!(
            result.relations.iter().any(|relation| {
                relation.kind == RelationKind::Inherits
                    && relation.source_name == "Child"
                    && relation.target_name == "Base"
            }),
            "relations: {:?}",
            result.relations
        );
        assert!(
            result.relations.iter().any(|relation| {
                relation.kind == RelationKind::Implements
                    && relation.source_name == "Child"
                    && relation.target_name == "Api"
            }),
            "relations: {:?}",
            result.relations
        );
    }
    #[test]
    fn extracts_type_reference_facts() {
        let result = parse(
            "class Bar {}
class Foo { value: Bar; }
",
        );
        assert!(
            result.facts.iter().any(|fact| {
                fact.kind == SyntaxFactKind::TypeReference && fact.name.as_deref() == Some("Bar")
            }),
            "facts: {:?}",
            result.facts
        );
    }
    #[test]
    fn emits_typescript_typed_variable_and_receiver_facts() {
        let result = parse(
            "class Foo { bar: Bar; run(s: string): Baz { const x: Bar = this.bar; return this.bar.helper(s); } }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("x")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("run")
                && fact.scope_path == ["Baz", "1", "string"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("helper")
                && fact.scope_path.first().map(String::as_str) == Some("this.bar")
        }));
    }
    #[test]
    fn emits_typescript_field_write_operators() {
        let result = parse("class Foo { x: Bar; run() { this.x = 1; this.x += 2; this.x++; } }");
        let writes: Vec<(&str, &str)> = result
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
            .filter_map(|fact| {
                Some((
                    fact.scope_path.get(1)?.as_str(),
                    fact.scope_path.get(2)?.as_str(),
                ))
            })
            .collect();
        assert!(writes.contains(&("write", "=")), "writes: {writes:?}");
        assert!(writes.contains(&("write", "+=")), "writes: {writes:?}");
        assert!(writes.contains(&("write", "++")), "writes: {writes:?}");
    }
}
