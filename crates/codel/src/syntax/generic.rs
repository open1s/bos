//! Generic Tree-sitter provider for additional grammars.
//!
//! Each grammar reuses one native parse path and a conservative structural
//! walker. The walker recognizes declaration, call, and import node families by
//! grammar node-kind names; it does not define a second AST. Language-specific
//! providers remain preferred where their queries are more precise.

use std::{ops::ControlFlow, sync::Arc};

use tree_sitter::{
    Language, Node, ParseOptions, ParseState, Parser, Query, QueryCursor, StreamingIterator,
};

use crate::classification::LanguageId;

use super::{
    ImportFact, LanguageProvider, ParseControl, ParseDiagnostic, ParseInput, ParseLimitKind,
    ParseLimits, ParseResult, ParseStatus, ParsedDocument, ParsedDocumentStatus, ProviderError,
    RelationFact, RelationKind, RustModuleFact, SyntaxFact, SyntaxFactKind, valid_relative_path,
};

/// One configured generic grammar provider.
#[derive(Clone, Copy, Debug)]
pub struct GrammarProvider {
    language: LanguageId,
    provider_id: &'static str,
    provider_version: &'static str,
    grammar: fn() -> Language,
}

impl GrammarProvider {
    const fn new(
        language: LanguageId,
        provider_id: &'static str,
        provider_version: &'static str,
        grammar: fn() -> Language,
    ) -> Self {
        Self {
            language,
            provider_id,
            provider_version,
            grammar,
        }
    }

    /// JavaScript.
    pub const JAVASCRIPT: Self = Self::new(
        LanguageId::JavaScript,
        "codel.javascript.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-javascript-0.23.1/facts-v1",
        javascript,
    );
    /// Go.
    pub const GO: Self = Self::new(
        LanguageId::Go,
        "codel.go.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-go-0.23.4/facts-v1",
        go,
    );
    /// Java.
    pub const JAVA: Self = Self::new(
        LanguageId::Java,
        "codel.java.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-java-0.23.5/facts-v1",
        java,
    );
    /// C.
    pub const C: Self = Self::new(
        LanguageId::C,
        "codel.c.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-c-0.24.2/facts-v1",
        c,
    );
    /// C or C++ header parsed with the C grammar.
    pub const C_HEADER: Self = Self::new(
        LanguageId::CHeader,
        "codel.c-header.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-c-0.24.2/facts-v1",
        c,
    );
    /// C++.
    pub const CPP: Self = Self::new(
        LanguageId::Cpp,
        "codel.cpp.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-cpp-0.23.4/facts-v1",
        cpp,
    );
    /// Ruby.
    pub const RUBY: Self = Self::new(
        LanguageId::Ruby,
        "codel.ruby.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-ruby-0.23.1/facts-v1",
        ruby,
    );
    /// C#.
    pub const CSHARP: Self = Self::new(
        LanguageId::CSharp,
        "codel.csharp.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-c-sharp-0.23.5/facts-v1",
        csharp,
    );
    /// Kotlin.
    pub const KOTLIN: Self = Self::new(
        LanguageId::Kotlin,
        "codel.kotlin.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-kotlin-sg-0.4.1/facts-v1",
        kotlin,
    );
    /// Scala.
    pub const SCALA: Self = Self::new(
        LanguageId::Scala,
        "codel.scala.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-scala-0.23.4/facts-v1",
        scala,
    );
    /// PHP.
    pub const PHP: Self = Self::new(
        LanguageId::Php,
        "codel.php.tree-sitter",
        "tree-sitter-0.26.13/tree-sitter-php-0.24.2/facts-v1",
        php,
    );

    /// Every generic grammar provider.
    pub const ALL: [Self; 11] = [
        Self::JAVASCRIPT,
        Self::GO,
        Self::JAVA,
        Self::C,
        Self::C_HEADER,
        Self::CPP,
        Self::RUBY,
        Self::CSHARP,
        Self::KOTLIN,
        Self::SCALA,
        Self::PHP,
    ];

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
        parser
            .set_language(&(self.grammar)())
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
            self.language,
            self.provider_id.to_owned(),
            self.provider_version.to_owned(),
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
            return Ok(limited_result(
                document,
                self.language,
                ParseLimitKind::SyntaxNodes,
            ));
        }

        let mut facts = Vec::new();
        let mut relations = Vec::new();
        let mut diagnostics = Vec::new();
        let mut limit_reason = None;
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if let Some(control) = control {
                control.check()?;
            }
            if node.is_error() || node.is_missing() {
                if diagnostics.len() >= limits.max_diagnostics {
                    limit_reason = Some(ParseLimitKind::Diagnostics);
                    break;
                }
                diagnostics.push(ParseDiagnostic {
                    range: Some(range_of(node)),
                    message: if node.is_missing() {
                        format!("missing syntax node: {}", node.kind())
                    } else {
                        "syntax error".to_owned()
                    },
                });
            }

            if let Some(kind) = declaration_kind(node.kind(), node)
                && let Some(name) = declaration_name(node, source)
            {
                if facts.len() >= limits.max_facts {
                    limit_reason = Some(ParseLimitKind::Facts);
                    break;
                }
                facts.push(SyntaxFact {
                    kind,
                    name: Some(name.clone()),
                    scope_path: Vec::new(),
                    is_module_scope: is_module_scope(node),
                    visibility: None,
                    range: range_of(node),
                });
                collect_relations(node, &name, source, &mut relations);
            }

            if let Some((kind, name)) = call_fact(node, source) {
                if facts.len() >= limits.max_facts {
                    limit_reason = Some(ParseLimitKind::Facts);
                    break;
                }
                facts.push(SyntaxFact {
                    kind,
                    name: Some(name),
                    scope_path: Vec::new(),
                    is_module_scope: is_module_scope(node),
                    visibility: None,
                    range: range_of(node),
                });
            }

            if (is_import(node.kind()) || is_ruby_require(node, source))
                && let Some(name) = text_for_node(node, source)
            {
                if facts.len() >= limits.max_facts {
                    limit_reason = Some(ParseLimitKind::Facts);
                    break;
                }
                facts.push(SyntaxFact {
                    kind: SyntaxFactKind::Import,
                    name: Some(name.to_owned()),
                    scope_path: Vec::new(),
                    is_module_scope: true,
                    visibility: None,
                    range: range_of(node),
                });
            }

            for index in (0..node.child_count()).rev() {
                if let Ok(index) = u32::try_from(index)
                    && let Some(child) = node.child(index)
                {
                    pending.push(child);
                }
            }
        }

        if self.language == LanguageId::Java {
            java_semantic_facts_query(root, source, &mut facts);
        } else if self.language == LanguageId::CSharp {
            csharp_semantic_facts_query(root, source, &mut facts);
        } else if self.language == LanguageId::Kotlin {
            kotlin_semantic_facts_query(root, source, &mut facts);
        } else if self.language == LanguageId::Scala {
            scala_semantic_facts_query(root, source, &mut facts);
        } else if self.language == LanguageId::Go {
            go_semantic_facts_query(root, source, &mut facts);
        }

        facts.sort_unstable_by(|left, right| {
            (left.range, left.kind, &left.name).cmp(&(right.range, right.kind, &right.name))
        });
        diagnostics.sort_unstable_by_key(|diagnostic| diagnostic.range);

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
            language: self.language,
            provider_id: document.provider_id().to_owned(),
            provider_version: document.provider_version().to_owned(),
            schema_version: 1,
            status,
            limit_reason,
            facts,
            imports: Vec::<ImportFact>::new(),
            relations,
            modules: Vec::<RustModuleFact>::new(),
            diagnostics,
        })
    }
}

impl LanguageProvider for GrammarProvider {
    fn language(&self) -> LanguageId {
        self.language
    }

    fn provider_id(&self) -> &'static str {
        self.provider_id
    }

    fn provider_version(&self) -> &'static str {
        self.provider_version
    }

    fn tree_sitter_language(&self) -> Option<Language> {
        Some((self.grammar)())
    }

    fn parse(&self, input: ParseInput<'_>) -> Result<ParseResult, ProviderError> {
        let document = self.parse_document_bounded(input, None)?;
        self.facts_from_document(&document, None)
    }

    fn parse_with_control(
        &self,
        input: ParseInput<'_>,
        control: &ParseControl<'_>,
    ) -> Result<ParseResult, ProviderError> {
        let document = self.parse_document_bounded(input, Some(control))?;
        self.facts_from_document(&document, Some(control))
    }
}

fn declaration_kind(kind: &str, node: Node<'_>) -> Option<SyntaxFactKind> {
    if matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_item"
            | "method_definition"
            | "method_declaration"
            | "constructor_declaration"
            | "method"
            | "singleton_method"
    ) {
        return Some(if is_inside_type(node) {
            SyntaxFactKind::Method
        } else {
            SyntaxFactKind::Function
        });
    }
    match kind {
        "class_declaration" | "class_definition" | "class_specifier" | "object_definition"
        | "struct_specifier" | "struct_item" | "struct_declaration" => Some(SyntaxFactKind::Struct),
        "interface_declaration" | "trait_declaration" | "trait_definition" => {
            Some(SyntaxFactKind::Trait)
        }
        "enum_declaration" | "enum_item" | "enum_specifier" => Some(SyntaxFactKind::Enum),
        "type_alias_declaration" | "type_declaration" => Some(SyntaxFactKind::TypeAlias),
        _ => None,
    }
}

fn collect_relations(
    node: Node<'_>,
    source_name: &str,
    source: &str,
    relations: &mut Vec<RelationFact>,
) {
    for field in [
        "superclass",
        "superclasses",
        "interfaces",
        "super_interfaces",
        "base",
        "bases",
        "extends",
    ] {
        if let Some(value) = node.child_by_field_name(field) {
            push_relation(value, source_name, source, relations);
        }
    }
    for child in named_children(node) {
        let kind = child.kind();
        if kind.contains("super")
            || kind.contains("base")
            || kind.contains("extends")
            || kind.contains("implements")
            || kind.contains("heritage")
        {
            push_relation(child, source_name, source, relations);
        }
    }
    for field in ["return_type", "type"] {
        if let Some(value) = node.child_by_field_name(field)
            && !value.kind().contains("function")
            && let Some(target) = text_for_node(value, source)
        {
            relations.push(RelationFact {
                kind: RelationKind::ReferencesType,
                source_name: source_name.to_owned(),
                target_name: bare_type(target),
                range: range_of(value),
            });
        }
    }
}

fn push_relation(
    node: Node<'_>,
    source_name: &str,
    source: &str,
    relations: &mut Vec<RelationFact>,
) {
    let kind = if node.kind().contains("implements") || node.kind().contains("interface") {
        RelationKind::Implements
    } else {
        RelationKind::Inherits
    };
    let children = named_children(node);
    if children.is_empty() {
        if let Some(target) = text_for_node(node, source) {
            relations.push(RelationFact {
                kind,
                source_name: source_name.to_owned(),
                target_name: bare_type(target),
                range: range_of(node),
            });
        }
        return;
    }
    for child in children {
        if child.kind().contains("comment") {
            continue;
        }
        if let Some(target) = text_for_node(child, source) {
            relations.push(RelationFact {
                kind,
                source_name: source_name.to_owned(),
                target_name: bare_type(target),
                range: range_of(child),
            });
        }
    }
}

fn bare_type(value: &str) -> String {
    value
        .split(['<', '(', ' '])
        .next()
        .unwrap_or(value)
        .trim()
        .to_owned()
}

fn declaration_name(node: Node<'_>, source: &str) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        return text_for_node(name, source).map(str::to_owned);
    }
    if matches!(node.kind(), "function_definition" | "function_declarator")
        && let Some(declarator) = node.child_by_field_name("declarator")
    {
        return first_name(declarator, source);
    }
    if node.kind() == "type_declaration" {
        for child in named_children(node) {
            if child.kind() == "type_spec"
                && let Some(name) = child.child_by_field_name("name")
            {
                return text_for_node(name, source).map(str::to_owned);
            }
        }
    }
    first_name(node, source)
}

fn first_name(node: Node<'_>, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "identifier"
            | "field_identifier"
            | "type_identifier"
            | "property_identifier"
            | "name"
            | "constant"
    ) {
        return text_for_node(node, source).map(str::to_owned);
    }
    for child in named_children(node) {
        if let Some(name) = first_name(child, source) {
            return Some(name);
        }
    }
    None
}

fn go_semantic_facts_query(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>) {
    let language: Language = tree_sitter_go::LANGUAGE.into();
    let queries = [
        (
            "variable",
            "(var_spec (identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "parameter",
            "(parameter_declaration (identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "field",
            "(field_declaration (field_identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "method",
            "(method_declaration receiver: (parameter_list (parameter_declaration (type_identifier) @receiver)) name: (field_identifier) @name parameters: (parameter_list) @params result: (type_identifier) @ret) @decl",
        ),
        (
            "call",
            "(call_expression (selector_expression (identifier) @receiver (field_identifier) @name) (argument_list) @args) @expr",
        ),
        (
            "access",
            "(selector_expression (identifier) @receiver (field_identifier) @name) @expr",
        ),
    ];
    for (kind, query_text) in queries {
        let Ok(query) = Query::new(&language, query_text) else {
            continue;
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            match kind {
                "variable" | "field" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                    if kind == "field" {
                        facts.push(SyntaxFact {
                            kind: SyntaxFactKind::Field,
                            name: Some(name.to_owned()),
                            scope_path: Vec::new(),
                            is_module_scope: false,
                            visibility: None,
                            range: range_of(decl),
                        });
                    }
                }
                "parameter" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "method" => {
                    let (Some(decl), Some(name), Some(params), Some(ret), Some(receiver)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "params"),
                        capture(query_match, &query, "ret"),
                        capture(query_match, &query, "receiver"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ret), Some(receiver)) = (
                        text_for_node(name, source),
                        text_for_node(ret, source),
                        text_for_node(receiver, source),
                    ) else {
                        continue;
                    };
                    let parameters = named_children(params)
                        .into_iter()
                        .filter(|child| child.kind() == "parameter_declaration")
                        .collect::<Vec<_>>();
                    let mut signature = vec![bare_type(ret), parameters.len().to_string()];
                    for parameter in parameters {
                        if let Some(ty) = parameter
                            .child_by_field_name("type")
                            .and_then(|node| text_for_node(node, source))
                        {
                            signature.push(bare_type(ty));
                        }
                    }
                    signature.push(bare_type(receiver));
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::MethodSignature,
                        name: Some(name.to_owned()),
                        scope_path: signature,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "call" => {
                    let (Some(expr), Some(receiver), Some(name), Some(args)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "args"),
                    ) else {
                        continue;
                    };
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    let arguments = named_children(args);
                    let mut call = vec![receiver.to_owned(), arguments.len().to_string()];
                    for argument in arguments {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::ReceiverCall,
                        name: Some(name.to_owned()),
                        scope_path: call,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                "access" => {
                    let (Some(expr), Some(receiver), Some(name)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                    ) else {
                        continue;
                    };
                    if ancestor_is(expr, "call_expression") {
                        continue;
                    }
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::FieldAccess,
                        name: Some(name.to_owned()),
                        scope_path: vec![receiver.to_owned(), "read".to_owned(), "read".to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                _ => {}
            }
        }
    }
}

fn kotlin_semantic_facts_query(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>) {
    let language: Language = tree_sitter_kotlin_sg::LANGUAGE.into();
    let queries = [
        (
            "property",
            "(property_declaration (variable_declaration (simple_identifier) @name (user_type (type_identifier) @type))) @decl",
        ),
        (
            "function",
            "(function_declaration (simple_identifier) @name (function_value_parameters) @params (user_type (type_identifier) @ret)) @decl",
        ),
        (
            "parameter",
            "(parameter (simple_identifier) @name (user_type (type_identifier) @type)) @decl",
        ),
        (
            "call",
            "(call_expression (navigation_expression (simple_identifier) @receiver (navigation_suffix (simple_identifier) @name)) (call_suffix (value_arguments) @args)) @expr",
        ),
        (
            "field",
            "(navigation_expression (simple_identifier) @receiver (navigation_suffix (simple_identifier) @name)) @expr",
        ),
    ];
    for (kind, query_text) in queries {
        let Ok(query) = Query::new(&language, query_text) else {
            continue;
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            match kind {
                "property" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![bare_type(ty)],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                    if !ancestor_is(decl, "function_declaration") {
                        facts.push(SyntaxFact {
                            kind: SyntaxFactKind::Field,
                            name: Some(name.to_owned()),
                            scope_path: Vec::new(),
                            is_module_scope: false,
                            visibility: None,
                            range: range_of(decl),
                        });
                    }
                }
                "function" => {
                    let (Some(decl), Some(name), Some(params), Some(ret)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "params"),
                        capture(query_match, &query, "ret"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ret)) =
                        (text_for_node(name, source), text_for_node(ret, source))
                    else {
                        continue;
                    };
                    let parameters = named_children(params)
                        .into_iter()
                        .filter(|child| child.kind() == "parameter")
                        .collect::<Vec<_>>();
                    let mut signature = vec![bare_type(ret), parameters.len().to_string()];
                    for parameter in parameters {
                        if let Some(ty) = named_children(parameter)
                            .into_iter()
                            .find(|child| child.kind() == "user_type")
                            .and_then(|child| text_for_node(child, source))
                        {
                            signature.push(bare_type(ty));
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::MethodSignature,
                        name: Some(name.to_owned()),
                        scope_path: signature,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "parameter" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![bare_type(ty)],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "call" => {
                    let (Some(expr), Some(receiver), Some(name), Some(args)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "args"),
                    ) else {
                        continue;
                    };
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    let arguments = named_children(args)
                        .into_iter()
                        .filter(|child| child.kind() == "value_argument")
                        .collect::<Vec<_>>();
                    let mut call = vec![receiver.to_owned(), arguments.len().to_string()];
                    for argument in arguments {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::ReceiverCall,
                        name: Some(name.to_owned()),
                        scope_path: call,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                "field" => {
                    let (Some(expr), Some(receiver), Some(name)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                    ) else {
                        continue;
                    };
                    if ancestor_is(expr, "call_expression") {
                        continue;
                    }
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::FieldAccess,
                        name: Some(name.to_owned()),
                        scope_path: vec![receiver.to_owned(), "read".to_owned(), "read".to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                _ => {}
            }
        }
    }
}

fn capture<'tree>(
    query_match: &tree_sitter::QueryMatch<'tree, '_>,
    query: &Query,
    name: &str,
) -> Option<Node<'tree>> {
    query_match
        .captures
        .iter()
        .find(|capture| {
            query
                .capture_names()
                .get(capture.index as usize)
                .is_some_and(|capture_name| *capture_name == name)
        })
        .map(|capture| capture.node)
}

fn scala_semantic_facts_query(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>) {
    let language: Language = tree_sitter_scala::LANGUAGE.into();
    let queries = [
        (
            "val",
            "(val_definition (identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "var",
            "(var_definition (identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "function",
            "(function_definition (identifier) @name (parameters) @params (type_identifier)? @ret) @decl",
        ),
        (
            "parameter",
            "(parameter (identifier) @name (type_identifier) @type) @decl",
        ),
        (
            "call",
            "(call_expression (field_expression (identifier) @receiver (identifier) @name) (arguments) @args) @expr",
        ),
        (
            "field",
            "(field_expression (identifier) @receiver (identifier) @name) @expr",
        ),
    ];
    for (kind, query_text) in queries {
        let Ok(query) = Query::new(&language, query_text) else {
            continue;
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            match kind {
                "val" | "var" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![bare_type(ty)],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                    if !ancestor_is(decl, "function_definition") && !ancestor_is(decl, "block") {
                        facts.push(SyntaxFact {
                            kind: SyntaxFactKind::Field,
                            name: Some(name.to_owned()),
                            scope_path: Vec::new(),
                            is_module_scope: false,
                            visibility: None,
                            range: range_of(decl),
                        });
                    }
                }
                "function" => {
                    let (Some(decl), Some(name), Some(params)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "params"),
                    ) else {
                        continue;
                    };
                    let Some(name) = text_for_node(name, source) else {
                        continue;
                    };
                    let return_type = capture(query_match, &query, "ret")
                        .and_then(|node| text_for_node(node, source))
                        .map(bare_type)
                        .unwrap_or_else(|| "Unit".to_owned());
                    let parameters = named_children(params)
                        .into_iter()
                        .filter(|child| child.kind() == "parameter")
                        .collect::<Vec<_>>();
                    let mut signature = vec![return_type, parameters.len().to_string()];
                    for parameter in parameters {
                        if let Some(ty) = named_children(parameter)
                            .into_iter()
                            .find(|child| child.kind() == "type_identifier")
                            .and_then(|child| text_for_node(child, source))
                        {
                            signature.push(bare_type(ty));
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::MethodSignature,
                        name: Some(name.to_owned()),
                        scope_path: signature,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "parameter" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![bare_type(ty)],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "call" => {
                    let (Some(expr), Some(receiver), Some(name), Some(args)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "args"),
                    ) else {
                        continue;
                    };
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    let arguments = named_children(args);
                    let mut call = vec![receiver.to_owned(), arguments.len().to_string()];
                    for argument in arguments {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::ReceiverCall,
                        name: Some(name.to_owned()),
                        scope_path: call,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                "field" => {
                    let (Some(expr), Some(receiver), Some(name)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                    ) else {
                        continue;
                    };
                    if ancestor_is(expr, "call_expression") {
                        continue;
                    }
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::FieldAccess,
                        name: Some(name.to_owned()),
                        scope_path: vec![receiver.to_owned(), "read".to_owned(), "read".to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                _ => {}
            }
        }
    }
}

fn ancestor_is(node: Node<'_>, kind: &str) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if current.kind() == kind {
            return true;
        }
        ancestor = current.parent();
    }
    false
}

fn csharp_semantic_facts_query(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>) {
    let language: Language = tree_sitter_c_sharp::LANGUAGE.into();
    let queries = [
        (
            "variable",
            "(variable_declaration type: (_) @type (variable_declarator name: (identifier) @name)) @decl",
        ),
        (
            "parameter",
            "(parameter type: (_) @type name: (identifier) @name) @decl",
        ),
        (
            "method",
            "(method_declaration returns: (_) @ret name: (identifier) @name parameters: (parameter_list) @params) @decl",
        ),
        (
            "call",
            "(invocation_expression (member_access_expression) @member (argument_list) @args) @expr",
        ),
        ("field", "(member_access_expression) @expr"),
    ];
    for (kind, query_text) in queries {
        let Ok(query) = Query::new(&language, query_text) else {
            continue;
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            match kind {
                "variable" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                    if decl
                        .parent()
                        .is_some_and(|parent| parent.kind() == "field_declaration")
                    {
                        facts.push(SyntaxFact {
                            kind: SyntaxFactKind::Field,
                            name: Some(name.to_owned()),
                            scope_path: Vec::new(),
                            is_module_scope: false,
                            visibility: None,
                            range: range_of(decl),
                        });
                    }
                }
                "parameter" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "method" => {
                    let (Some(decl), Some(name), Some(params), Some(ret)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "params"),
                        capture(query_match, &query, "ret"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ret)) =
                        (text_for_node(name, source), text_for_node(ret, source))
                    else {
                        continue;
                    };
                    let parameters = named_children(params)
                        .into_iter()
                        .filter(|child| child.kind() == "parameter")
                        .collect::<Vec<_>>();
                    let mut signature = vec![ret.to_owned(), parameters.len().to_string()];
                    for parameter in parameters {
                        if let Some(ty) = parameter
                            .child_by_field_name("type")
                            .and_then(|node| text_for_node(node, source))
                        {
                            signature.push(bare_type(ty));
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::MethodSignature,
                        name: Some(name.to_owned()),
                        scope_path: signature,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "call" => {
                    let (Some(expr), Some(member), Some(args)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "member"),
                        capture(query_match, &query, "args"),
                    ) else {
                        continue;
                    };
                    let (Some(member), Some(args)) = (text_for_node(member, source), Some(args))
                    else {
                        continue;
                    };
                    let Some((receiver, name)) = split_member(member) else {
                        continue;
                    };
                    let arguments = named_children(args)
                        .into_iter()
                        .filter(|child| child.kind() == "argument")
                        .collect::<Vec<_>>();
                    let mut call = vec![receiver.to_owned(), arguments.len().to_string()];
                    for argument in arguments {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::ReceiverCall,
                        name: Some(name.to_owned()),
                        scope_path: call,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                "field" => {
                    let Some(expr) = capture(query_match, &query, "expr") else {
                        continue;
                    };
                    if ancestor_is(expr, "invocation_expression") {
                        continue;
                    }
                    let Some((receiver, name)) = text_for_node(expr, source).and_then(split_member)
                    else {
                        continue;
                    };
                    let (write, operator) = csharp_field_mode(expr, source);
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::FieldAccess,
                        name: Some(name.to_owned()),
                        scope_path: vec![
                            receiver.to_owned(),
                            if write { "write" } else { "read" }.to_owned(),
                            operator,
                        ],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                _ => {}
            }
        }
    }
}

fn split_member(text: &str) -> Option<(&str, &str)> {
    let index = text.rfind('.')?;
    Some((&text[..index], &text[index + 1..]))
}

fn csharp_field_mode(node: Node<'_>, source: &str) -> (bool, String) {
    let parent = node.parent();
    if parent.is_some_and(|parent| {
        parent.kind() == "assignment_expression"
            && parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id())
    }) {
        return (
            true,
            parent
                .and_then(|parent| parent.child_by_field_name("operator"))
                .and_then(|operator| text_for_node(operator, source))
                .unwrap_or("=")
                .to_owned(),
        );
    }
    if let Some(parent) = parent.filter(|parent| {
        parent.kind() == "postfix_unary_expression" || parent.kind() == "prefix_unary_expression"
    }) {
        let text = text_for_node(parent, source).unwrap_or("");
        return (
            true,
            if text.contains("++") { "++" } else { "--" }.to_owned(),
        );
    }
    (false, "read".to_owned())
}

fn java_semantic_facts_query(root: Node<'_>, source: &str, facts: &mut Vec<SyntaxFact>) {
    let language: Language = tree_sitter_java::LANGUAGE.into();
    let queries = [
        (
            "field",
            "(field_declaration type: (_) @type declarator: (variable_declarator name: (identifier) @name)) @decl",
        ),
        (
            "local",
            "(local_variable_declaration type: (_) @type declarator: (variable_declarator name: (identifier) @name)) @decl",
        ),
        (
            "parameter",
            "(formal_parameter type: (_) @type name: (identifier) @name) @decl",
        ),
        (
            "method",
            "(method_declaration type: (_) @ret name: (identifier) @name parameters: (formal_parameters) @params) @decl",
        ),
        (
            "call",
            "(method_invocation object: (_) @receiver name: (identifier) @name arguments: (argument_list) @args) @expr",
        ),
        (
            "access",
            "(field_access object: (_) @receiver field: (identifier) @name) @expr",
        ),
        (
            "typeparam",
            "(class_declaration name: (identifier) @owner type_parameters: (type_parameters (type_parameter (type_identifier) @param))) @decl",
        ),
    ];
    for (kind, query_text) in queries {
        let Ok(query) = Query::new(&language, query_text) else {
            continue;
        };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, root, source.as_bytes());
        while let Some(query_match) = matches.next() {
            match kind {
                "field" | "local" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                    if kind == "field" {
                        facts.push(SyntaxFact {
                            kind: SyntaxFactKind::Field,
                            name: Some(name.to_owned()),
                            scope_path: Vec::new(),
                            is_module_scope: false,
                            visibility: None,
                            range: range_of(decl),
                        });
                    }
                }
                "parameter" => {
                    let (Some(decl), Some(name), Some(ty)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "type"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ty)) =
                        (text_for_node(name, source), text_for_node(ty, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::Variable,
                        name: Some(name.to_owned()),
                        scope_path: vec![ty.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "method" => {
                    let (Some(decl), Some(name), Some(params), Some(ret)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "params"),
                        capture(query_match, &query, "ret"),
                    ) else {
                        continue;
                    };
                    let (Some(name), Some(ret)) =
                        (text_for_node(name, source), text_for_node(ret, source))
                    else {
                        continue;
                    };
                    let parameters = named_children(params)
                        .into_iter()
                        .filter(|child| child.kind() == "formal_parameter")
                        .collect::<Vec<_>>();
                    let mut signature = vec![ret.to_owned(), parameters.len().to_string()];
                    for parameter in parameters {
                        if let Some(ty) = parameter
                            .child_by_field_name("type")
                            .and_then(|node| text_for_node(node, source))
                        {
                            signature.push(bare_type(ty));
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::MethodSignature,
                        name: Some(name.to_owned()),
                        scope_path: signature,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "call" => {
                    let (Some(expr), Some(receiver), Some(name), Some(args)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                        capture(query_match, &query, "args"),
                    ) else {
                        continue;
                    };
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    let arguments = named_children(args);
                    let mut call = vec![receiver.to_owned(), arguments.len().to_string()];
                    for argument in arguments {
                        if let Some(text) = text_for_node(argument, source) {
                            call.push(text.to_owned());
                        }
                    }
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::ReceiverCall,
                        name: Some(name.to_owned()),
                        scope_path: call,
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                "typeparam" => {
                    let (Some(decl), Some(owner), Some(param)) = (
                        capture(query_match, &query, "decl"),
                        capture(query_match, &query, "owner"),
                        capture(query_match, &query, "param"),
                    ) else {
                        continue;
                    };
                    let (Some(owner), Some(param)) =
                        (text_for_node(owner, source), text_for_node(param, source))
                    else {
                        continue;
                    };
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::TypeParameter,
                        name: Some(param.to_owned()),
                        scope_path: vec![owner.to_owned()],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(decl),
                    });
                }
                "access" => {
                    let (Some(expr), Some(receiver), Some(name)) = (
                        capture(query_match, &query, "expr"),
                        capture(query_match, &query, "receiver"),
                        capture(query_match, &query, "name"),
                    ) else {
                        continue;
                    };
                    let (Some(receiver), Some(name)) =
                        (text_for_node(receiver, source), text_for_node(name, source))
                    else {
                        continue;
                    };
                    let (write, operator) = java_field_mode(expr, source);
                    facts.push(SyntaxFact {
                        kind: SyntaxFactKind::FieldAccess,
                        name: Some(name.to_owned()),
                        scope_path: vec![
                            receiver.to_owned(),
                            if write { "write" } else { "read" }.to_owned(),
                            operator,
                        ],
                        is_module_scope: false,
                        visibility: None,
                        range: range_of(expr),
                    });
                }
                _ => {}
            }
        }
    }
}

fn java_field_mode(node: Node<'_>, source: &str) -> (bool, String) {
    let parent = node.parent();
    if parent.is_some_and(|parent| {
        parent.kind() == "assignment_expression"
            && parent
                .child_by_field_name("left")
                .is_some_and(|left| left.id() == node.id())
    }) {
        return (
            true,
            parent
                .and_then(|parent| parent.child_by_field_name("operator"))
                .and_then(|operator| text_for_node(operator, source))
                .unwrap_or("=")
                .to_owned(),
        );
    }
    if let Some(parent) = parent.filter(|parent| parent.kind() == "update_expression") {
        let operator = text_for_node(parent, source)
            .map(|text| if text.contains("++") { "++" } else { "--" }.to_owned())
            .unwrap_or_else(|| "++".to_owned());
        return (true, operator);
    }
    (false, "read".to_owned())
}

fn call_fact(node: Node<'_>, source: &str) -> Option<(SyntaxFactKind, String)> {
    let kind = node.kind();
    if !(kind.contains("call") || kind == "method_invocation" || kind == "invocation_expression") {
        return None;
    }
    let function = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("method"))
        .or_else(|| node.child_by_field_name("name"))?;
    let method = kind == "method_invocation"
        || kind == "invocation_expression"
        || node.child_by_field_name("object").is_some()
        || function.kind().contains("member")
        || function.kind().contains("selector")
        || function.kind().contains("attribute");
    let target = function
        .child_by_field_name("field")
        .or_else(|| function.child_by_field_name("property"))
        .or_else(|| function.child_by_field_name("attribute"))
        .unwrap_or(function);
    let name = text_for_node(target, source)
        .map(str::to_owned)
        .or_else(|| first_name(target, source))?;
    Some((
        if method {
            SyntaxFactKind::MethodCall
        } else {
            SyntaxFactKind::Call
        },
        name,
    ))
}

fn is_inside_type(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if matches!(
            current.kind(),
            "class_declaration"
                | "class_definition"
                | "class_specifier"
                | "struct_specifier"
                | "struct_item"
                | "interface_declaration"
                | "trait_declaration"
                | "trait_definition"
                | "impl_item"
                | "object_definition"
        ) {
            return true;
        }
        ancestor = current.parent();
    }
    false
}

fn is_module_scope(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if matches!(
            current.kind(),
            "function_definition"
                | "function_declaration"
                | "function_item"
                | "method_definition"
                | "method_declaration"
                | "method"
        ) {
            return false;
        }
        ancestor = current.parent();
    }
    true
}

fn is_import(kind: &str) -> bool {
    kind.contains("import")
        || kind.contains("include")
        || kind.contains("require")
        || kind.contains("namespace_use")
        || kind == "using_directive"
}

fn is_ruby_require(node: Node<'_>, source: &str) -> bool {
    if node.kind() != "call" {
        return false;
    }
    node.child_by_field_name("method")
        .and_then(|method| text_for_node(method, source))
        .is_some_and(|method| matches!(method, "require" | "require_relative" | "load"))
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    (0..node.named_child_count())
        .filter_map(|index| node.named_child(index as u32))
        .collect()
}

fn range_of(node: Node<'_>) -> super::SourceRange {
    super::SourceRange {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

fn text_for_node<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.byte_range())
}

fn limited_result(
    document: &ParsedDocument,
    language: LanguageId,
    reason: ParseLimitKind,
) -> ParseResult {
    ParseResult {
        workspace_id: document.workspace_id(),
        relative_path: document.relative_path().to_path_buf(),
        language,
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

fn javascript() -> Language {
    tree_sitter_javascript::LANGUAGE.into()
}
fn go() -> Language {
    tree_sitter_go::LANGUAGE.into()
}
fn java() -> Language {
    tree_sitter_java::LANGUAGE.into()
}
fn c() -> Language {
    tree_sitter_c::LANGUAGE.into()
}
fn cpp() -> Language {
    tree_sitter_cpp::LANGUAGE.into()
}
fn ruby() -> Language {
    tree_sitter_ruby::LANGUAGE.into()
}
fn csharp() -> Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}
fn kotlin() -> Language {
    tree_sitter_kotlin_sg::LANGUAGE.into()
}
fn scala() -> Language {
    tree_sitter_scala::LANGUAGE.into()
}
fn php() -> Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::WorkspaceId;

    fn parse(provider: &GrammarProvider, source: &str) -> ParseResult {
        provider
            .parse(ParseInput {
                workspace_id: WorkspaceId::new(),
                relative_path: Path::new("src/input"),
                source: source.as_bytes(),
            })
            .expect("source")
    }

    #[test]
    fn recognizes_core_declarations_across_generic_grammars() {
        let cases: &[(&GrammarProvider, &str, &str)] = &[
            (
                &GrammarProvider::JAVASCRIPT,
                "function hello() { helper(); }",
                "hello",
            ),
            (
                &GrammarProvider::GO,
                "package main\nfunc hello() { helper() }\nfunc helper() {}",
                "hello",
            ),
            (
                &GrammarProvider::JAVA,
                "class Main { void run() { helper(); } void helper() {} }",
                "Main",
            ),
            (
                &GrammarProvider::C,
                "int main(void) { return helper(); }",
                "main",
            ),
            (
                &GrammarProvider::CPP,
                "int main() { return helper(); }",
                "main",
            ),
            (
                &GrammarProvider::RUBY,
                "def hello()\n  helper()\nend",
                "hello",
            ),
            (
                &GrammarProvider::CSHARP,
                "class Main { void Run() { Helper(); } void Helper() {} }",
                "Main",
            ),
            (
                &GrammarProvider::KOTLIN,
                "fun helper() {}\nclass Main { fun run() { helper() } }",
                "Main",
            ),
            (
                &GrammarProvider::SCALA,
                "object Main { def run(): Unit = { helper() } }",
                "Main",
            ),
            (
                &GrammarProvider::PHP,
                "<?php function hello() { helper(); }",
                "hello",
            ),
        ];

        for (provider, source, expected) in cases {
            let result = parse(provider, source);
            assert!(
                result
                    .facts
                    .iter()
                    .any(|fact| fact.name.as_deref() == Some(*expected)),
                "{:?} missing {expected}; facts: {:?}",
                provider.language,
                result.facts
            );
        }
    }
    #[test]
    fn generic_provider_collects_heritage_and_type_relations() {
        let result = parse(
            &GrammarProvider::JAVA,
            "class Base {}
class Child extends Base { Bar run() { return null; } }",
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
                relation.kind == RelationKind::ReferencesType
                    && relation.source_name == "run"
                    && relation.target_name == "Bar"
            }),
            "relations: {:?}",
            result.relations
        );
    }
    #[test]
    fn java_provider_emits_typed_variable_and_receiver_facts() {
        let result = parse(
            &GrammarProvider::JAVA,
            "class Foo {
  Bar field;
  Baz run(Qux q) {
    Bar local = field;
    return local.method(q);
  }
}",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("local")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("run")
                && fact.scope_path == ["Baz", "1", "Qux"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("method")
                && fact.scope_path == ["local", "1", "q"]
        }));
    }
    #[test]
    fn java_provider_emits_field_and_field_access_facts() {
        let result = parse(
            &GrammarProvider::JAVA,
            "class Foo { Bar field; Baz run() { return this.field; } }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Field && fact.name.as_deref() == Some("field")
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::FieldAccess
                && fact.name.as_deref() == Some("field")
                && fact.scope_path == ["this", "read", "read"]
        }));
    }
    #[test]
    fn java_provider_records_field_write_operators() {
        let result = parse(
            &GrammarProvider::JAVA,
            "class Foo { void run() { this.x += 1; this.x++; } }",
        );
        let operators: Vec<&str> = result
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
            .filter_map(|fact| fact.scope_path.get(2).map(String::as_str))
            .collect();
        assert!(operators.contains(&"+="), "operators: {operators:?}");
        assert!(operators.contains(&"++"), "operators: {operators:?}");
    }
    #[test]
    fn csharp_provider_emits_typed_receiver_facts() {
        let result = parse(
            &GrammarProvider::CSHARP,
            "class Foo { Bar bar; Baz Run(Qux q) { return bar.Helper(q); } }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("bar")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("Run")
                && fact.scope_path == ["Baz", "1", "Qux"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("Helper")
                && fact.scope_path.first().map(String::as_str) == Some("bar")
        }));
    }
    #[test]
    fn csharp_provider_emits_write_operators() {
        let result = parse(
            &GrammarProvider::CSHARP,
            "class Foo { void Run() { this.x = 1; this.x += 2; this.x++; } }",
        );
        let writes: Vec<&str> = result
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
            .filter_map(|fact| fact.scope_path.get(2).map(String::as_str))
            .collect();
        assert!(writes.contains(&"="), "writes: {writes:?}");
        assert!(writes.contains(&"+="), "writes: {writes:?}");
        assert!(writes.contains(&"++"), "writes: {writes:?}");
    }
    #[test]
    fn kotlin_provider_emits_typed_receiver_facts() {
        let result = parse(
            &GrammarProvider::KOTLIN,
            "class Foo { val bar: Bar = Bar(); fun run(q: Qux): Baz { val x: Bar = bar; return x.helper(q) } }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("bar")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("run")
                && fact.scope_path == ["Baz", "1", "Qux"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("helper")
                && fact.scope_path.first().map(String::as_str) == Some("x")
        }));
    }
    #[test]
    fn scala_provider_emits_typed_receiver_facts() {
        let result = parse(
            &GrammarProvider::SCALA,
            "class Foo { val bar: Bar = new Bar(); def run(q: Qux): Baz = { val x: Bar = bar; x.helper(q) } }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("bar")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("run")
                && fact.scope_path == ["Baz", "1", "Qux"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("helper")
                && fact.scope_path.first().map(String::as_str) == Some("x")
        }));
    }
    #[test]
    fn go_provider_emits_typed_receiver_facts() {
        let result = parse(
            &GrammarProvider::GO,
            "package main
type Bar struct { x int }
func (b Bar) Helper(q Qux) Baz { return Baz{} }
func run(q Qux) Baz { var y Bar; return y.Helper(q) }",
        );
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::Variable
                && fact.name.as_deref() == Some("y")
                && fact.scope_path == ["Bar"]
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::MethodSignature
                && fact.name.as_deref() == Some("Helper")
                && fact.scope_path.last().map(String::as_str) == Some("Bar")
        }));
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::ReceiverCall
                && fact.name.as_deref() == Some("Helper")
                && fact.scope_path.first().map(String::as_str) == Some("y")
        }));
    }
    #[test]
    fn java_provider_emits_type_parameters() {
        let result = parse(&GrammarProvider::JAVA, "class Box<T> { T value; }");
        assert!(result.facts.iter().any(|fact| {
            fact.kind == SyntaxFactKind::TypeParameter
                && fact.name.as_deref() == Some("T")
                && fact.scope_path == ["Box"]
        }));
    }
}
