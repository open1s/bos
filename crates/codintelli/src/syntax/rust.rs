//! Rust syntax provider backed by the upstream Tree-sitter Rust grammar.
//!
//! Tree-sitter owns lexical and syntactic parsing. This module only projects
//! grammar nodes into codintelli's language-neutral syntax-fact contract.

use std::ops::ControlFlow;

use tree_sitter::{Node, ParseOptions, Parser, ParseState};

use crate::classification::LanguageId;

use super::{
    valid_relative_path, LanguageProvider, ParseDiagnostic, ParseInput, ParseResult, ParseStatus,
    ParseControl, ParseLimits, ProviderError, SourceRange, SyntaxFact, SyntaxFactKind,
};

/// Rust provider using the upstream Tree-sitter Rust grammar.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustProvider;

impl RustProvider {
    /// Stable provider identifier.
    pub const ID: &'static str = "codintelli.rust.tree-sitter";

    /// Parser and grammar versions used by this provider.
    pub const VERSION: &'static str = "tree-sitter-0.26.13/tree-sitter-rust-0.24.2";
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
    fn parse_bounded(
        &self,
        input: ParseInput<'_>,
        control: Option<&ParseControl<'_>>,
    ) -> Result<ParseResult, ProviderError> {
        if !valid_relative_path(input.relative_path) {
            return Err(ProviderError::InvalidPath);
        }
        if let Some(control) = control {
            control.check()?;
        }
        let source = std::str::from_utf8(input.source)?;
        let default_limits = ParseLimits::default();
        let limits = control.map_or(&default_limits, ParseControl::limits);
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE.into();
        parser
            .set_language(&language)
            .map_err(|error| ProviderError::ParserInitialization(error.to_string()))?;

        // Tree-sitter invokes this callback during parsing. Returning Break
        // aborts the native parser cooperatively; reset prevents a later parse
        // from resuming the cancelled parser's internal state.
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
            &mut |offset, _| &input.source[offset..],
            None,
            Some(options),
        );
        drop(progress);
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

        let root = tree.root_node();
        let mut facts = Vec::new();
        let mut diagnostics = Vec::new();
        let mut pending = vec![root];
        let mut visited_nodes = 0usize;
        let mut limited = false;

        // Iterative traversal avoids adding another recursive walk over
        // potentially adversarially nested source syntax. The pending stack,
        // visited nodes, facts, and diagnostics all have explicit bounds.
        while let Some(node) = pending.pop() {
            if let Some(control) = control {
                control.check()?;
            }
            visited_nodes = visited_nodes.saturating_add(1);
            if visited_nodes > limits.max_nodes {
                limited = true;
                break;
            }

            if node.is_error() || node.is_missing() {
                if diagnostics.len() >= limits.max_diagnostics {
                    limited = true;
                    break;
                }
                diagnostics.push(ParseDiagnostic {
                    range: source_range(node, source.len()),
                    message: if node.is_missing() {
                        format!("missing syntax node: {}", node.kind())
                    } else {
                        "syntax error".to_owned()
                    },
                });
            }

            if let Some((kind, name)) = classify_node(node, source) {
                if let Some(range) = source_range(node, source.len()) {
                    if facts.len() >= limits.max_facts {
                        limited = true;
                        break;
                    }
                    facts.push(SyntaxFact { kind, name, range });
                }
            }

            let child_count = node.child_count();
            if pending.len().saturating_add(child_count) > limits.max_nodes {
                limited = true;
                break;
            }
            for index in (0..child_count).rev() {
                if let Some(child) = node.child(index) {
                    pending.push(child);
                }
            }
        }

        facts.sort_unstable_by(|left, right| {
            (left.range, left.kind, &left.name).cmp(&(right.range, right.kind, &right.name))
        });
        diagnostics.sort_unstable_by_key(|diagnostic| diagnostic.range);

        let status = if limited {
            ParseStatus::Limited
        } else if diagnostics.is_empty() {
            ParseStatus::Complete
        } else {
            ParseStatus::Recovered
        };

        Ok(ParseResult {
            workspace_id: input.workspace_id,
            relative_path: input.relative_path.to_path_buf(),
            language: LanguageId::Rust,
            provider_id: Self::ID.to_owned(),
            provider_version: Self::VERSION.to_owned(),
            schema_version: 1,
            status,
            facts,
            diagnostics,
        })
    }
}

fn classify_node(node: Node<'_>, source: &str) -> Option<(SyntaxFactKind, Option<String>)> {
    let kind = match node.kind() {
        "mod_item" => SyntaxFactKind::Module,
        "function_item" if is_associated_method(node) => SyntaxFactKind::Method,
        "function_item" => SyntaxFactKind::Function,
        "function_signature_item" if is_associated_method(node) => SyntaxFactKind::Method,
        "function_signature_item" => SyntaxFactKind::Function,
        "struct_item" => SyntaxFactKind::Struct,
        "enum_item" => SyntaxFactKind::Enum,
        "union_item" => SyntaxFactKind::Union,
        "trait_item" => SyntaxFactKind::Trait,
        "impl_item" => SyntaxFactKind::Impl,
        "type_item" => SyntaxFactKind::TypeAlias,
        "const_item" => SyntaxFactKind::Constant,
        "static_item" => SyntaxFactKind::Static,
        "use_declaration" => SyntaxFactKind::Import,
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "field_expression" {
                return Some((
                    SyntaxFactKind::MethodCall,
                    text_for_node(
                        function.child_by_field_name("field").unwrap_or(function),
                        source,
                    ),
                ));
            }
            return Some((SyntaxFactKind::Call, text_for_node(function, source)));
        }
        _ => return None,
    };

    let name = match kind {
        SyntaxFactKind::Impl => node
            .child_by_field_name("type")
            .and_then(|value| text_for_node(value, source)),
        SyntaxFactKind::Import => text_for_node(node, source),
        _ => node
            .child_by_field_name("name")
            .and_then(|value| text_for_node(value, source)),
    };
    Some((kind, name))
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
    use std::time::Duration;
    use std::path::{Path, PathBuf};

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
        assert_eq!(result.facts.len(), 1);
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
}
