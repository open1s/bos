//! Rust syntax provider based on Syn.
//!
//! Syn is a pure-Rust parser and avoids introducing a C/C++ parser build
//! toolchain into the codintelli dependency graph. It parses complete Rust
//! syntax trees; unlike an error-recovering parser, malformed files produce a
//! failed result with a diagnostic and no partial facts.

use std::path::PathBuf;

use proc_macro2::Span;
use syn::{
    spanned::Spanned,
    visit::{self, Visit},
};

use crate::classification::LanguageId;

use super::{
    valid_relative_path, LanguageProvider, ParseDiagnostic, ParseInput, ParseResult, ParseStatus,
    ProviderError, SourceRange, SyntaxFact, SyntaxFactKind,
};

/// Rust provider using the Syn parser.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustProvider;

impl RustProvider {
    /// Stable provider identifier.
    pub const ID: &'static str = "codintelli.rust.syn";

    /// Provider/parser version included in persisted extraction metadata.
    pub const VERSION: &'static str = "syn-2.0.117";
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
        if !valid_relative_path(input.relative_path) {
            return Err(ProviderError::InvalidPath);
        }
        let source = std::str::from_utf8(input.source)?;
        let mut result = ParseResult {
            workspace_id: input.workspace_id,
            relative_path: input.relative_path.to_path_buf(),
            language: LanguageId::Rust,
            provider_id: Self::ID.to_owned(),
            provider_version: Self::VERSION.to_owned(),
            schema_version: 1,
            status: ParseStatus::Failed,
            facts: Vec::new(),
            diagnostics: Vec::new(),
        };

        match syn::parse_file(source) {
            Ok(file) => {
                let mut extractor = RustFactExtractor {
                    source,
                    facts: Vec::new(),
                };
                extractor.visit_file(&file);
                extractor.facts.sort_unstable_by(|left, right| {
                    (left.range, left.kind, &left.name).cmp(&(right.range, right.kind, &right.name))
                });
                result.facts = extractor.facts;
                result.status = ParseStatus::Complete;
            }
            Err(error) => {
                let range = span_range(error.span(), source.len());
                result.diagnostics.push(ParseDiagnostic {
                    range,
                    message: error.to_string(),
                });
            }
        }
        Ok(result)
    }
}

struct RustFactExtractor<'a> {
    source: &'a str,
    facts: Vec<SyntaxFact>,
}

impl RustFactExtractor<'_> {
    fn push(&mut self, kind: SyntaxFactKind, name: Option<String>, span: Span) {
        let Some(range) = span_range(span, self.source.len()) else {
            return;
        };
        self.facts.push(SyntaxFact { kind, name, range });
    }

    fn source_spelling(&self, span: Span) -> Option<String> {
        let range = span_range(span, self.source.len())?;
        self.source.get(range.start_byte..range.end_byte).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
    }
}

fn span_range(span: Span, source_len: usize) -> Option<SourceRange> {
    let range = span.byte_range();
    SourceRange::new(range.start, range.end, source_len)
}

impl<'ast> Visit<'ast> for RustFactExtractor<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.push(SyntaxFactKind::Module, Some(node.ident.to_string()), node.span());
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.push(SyntaxFactKind::Function, Some(node.sig.ident.to_string()), node.span());
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.push(SyntaxFactKind::Method, Some(node.sig.ident.to_string()), node.span());
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.push(SyntaxFactKind::Method, Some(node.sig.ident.to_string()), node.span());
        visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        self.push(SyntaxFactKind::Struct, Some(node.ident.to_string()), node.span());
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        self.push(SyntaxFactKind::Enum, Some(node.ident.to_string()), node.span());
        visit::visit_item_enum(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast syn::ItemUnion) {
        self.push(SyntaxFactKind::Union, Some(node.ident.to_string()), node.span());
        visit::visit_item_union(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.push(SyntaxFactKind::Trait, Some(node.ident.to_string()), node.span());
        visit::visit_item_trait(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let name = self.source_spelling(node.self_ty.span());
        self.push(SyntaxFactKind::Impl, name, node.span());
        visit::visit_item_impl(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        self.push(SyntaxFactKind::TypeAlias, Some(node.ident.to_string()), node.span());
        visit::visit_item_type(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.push(SyntaxFactKind::Constant, Some(node.ident.to_string()), node.span());
        visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.push(SyntaxFactKind::Static, Some(node.ident.to_string()), node.span());
        visit::visit_item_static(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.push(SyntaxFactKind::Import, self.source_spelling(node.span()), node.span());
        visit::visit_item_use(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        self.push(SyntaxFactKind::Call, self.source_spelling(node.func.span()), node.func.span());
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.push(SyntaxFactKind::MethodCall, Some(node.method.to_string()), node.span());
        visit::visit_expr_method_call(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorkspaceId;
    use std::path::Path;

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

pub struct Widget;
pub enum State { Ready, Busy }
pub trait Render { fn render(&self); }
impl Widget {
    pub fn draw(&self) { self.render(); helper(); }
}
fn helper() {}
"#;
        let result = parse(source);
        assert_eq!(result.status, ParseStatus::Complete);
        for kind in [
            SyntaxFactKind::Import,
            SyntaxFactKind::Struct,
            SyntaxFactKind::Enum,
            SyntaxFactKind::Trait,
            SyntaxFactKind::Impl,
            SyntaxFactKind::Method,
            SyntaxFactKind::Function,
            SyntaxFactKind::Call,
            SyntaxFactKind::MethodCall,
        ] {
            assert!(result.facts.iter().any(|fact| fact.kind == kind), "missing {kind:?}");
        }
        for fact in &result.facts {
            assert!(fact.range.end_byte <= source.len());
            assert!(source.is_char_boundary(fact.range.start_byte));
            assert!(source.is_char_boundary(fact.range.end_byte));
        }
    }

    #[test]
    fn malformed_source_is_failed_not_silently_complete() {
        let result = parse("pub fn broken( {");
        assert_eq!(result.status, ParseStatus::Failed);
        assert!(result.facts.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
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
