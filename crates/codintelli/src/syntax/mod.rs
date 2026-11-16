//! Host-neutral syntax facts and language-provider contracts.

use std::path::{Path, PathBuf};

use crate::{classification::LanguageId, ScanBudget, ScanBudgetError, WorkspaceId};

pub mod document;
pub mod rust;

pub use document::{ParsedDocument, ParsedDocumentStatus};

/// Half-open byte range in the original UTF-8 source file.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceRange {
    /// First byte, inclusive.
    pub start_byte: usize,
    /// First byte after the range.
    pub end_byte: usize,
}

impl SourceRange {
    /// Construct a range when its bounds are valid for the supplied source.
    #[must_use]
    pub fn new(start_byte: usize, end_byte: usize, source_len: usize) -> Option<Self> {
        (start_byte <= end_byte && end_byte <= source_len).then_some(Self {
            start_byte,
            end_byte,
        })
    }
}

/// Syntax entity or relation extracted directly from a source file.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SyntaxFactKind {
    /// A module declaration.
    Module,
    /// A free function declaration.
    Function,
    /// A method declaration in an impl block.
    Method,
    /// A struct declaration.
    Struct,
    /// An enum declaration.
    Enum,
    /// A union declaration.
    Union,
    /// A trait declaration.
    Trait,
    /// An implementation block.
    Impl,
    /// A type alias declaration.
    TypeAlias,
    /// A constant declaration.
    Constant,
    /// A static declaration.
    Static,
    /// A use/import declaration.
    Import,
    /// A function-call expression; target is not resolved.
    Call,
    /// A method-call expression; target is not resolved.
    MethodCall,
}

/// Rust visibility written on a declaration or import.
///
/// This records the syntactic visibility modifier only. It does not calculate
/// effective accessibility through ancestor modules, cfg selection, or re-exports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RustVisibility {
    /// No explicit visibility modifier was present.
    Private,
    /// `pub`.
    Public,
    /// `pub(crate)`.
    Crate,
    /// `pub(super)`.
    Super,
    /// `pub(self)`.
    SelfScope,
    /// `pub(in path)`, with source path segments and raw-identifier prefixes removed.
    InPath(Vec<String>),
    /// A visibility form the provider did not recognize; source spelling is retained.
    Unknown(String),
}

/// One syntax fact with a source-backed location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxFact {
    /// Fact category.
    pub kind: SyntaxFactKind,
    /// Declared name or source spelling of a reference, when available.
    pub name: Option<String>,
    /// Lexical inline-module scope, outermost first. This is source syntax context,
    /// not a compiler-resolved crate/module identity.
    pub scope_path: Vec<String>,
    /// Whether this fact is in module scope rather than a function/block-local scope.
    pub is_module_scope: bool,
    /// Explicit or implicit Rust visibility for declarations/imports; absent for references.
    pub visibility: Option<RustVisibility>,
    /// Exact source byte range.
    pub range: SourceRange,
}

/// Parser outcome for one source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseStatus {
    /// The provider parsed the complete file without syntax errors.
    Complete,
    /// The parser recovered from one or more syntax errors.
    Recovered,
    /// Parsing could not produce a syntax tree.
    Failed,
    /// Parsing stopped at a configured fact, diagnostic, or node limit.
    Limited,
}

/// A parser diagnostic tied to the original source when a range is available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseDiagnostic {
    /// Optional source location.
    pub range: Option<SourceRange>,
    /// Human-readable diagnostic; must not contain source excerpts by default.
    pub message: String,
}

/// Which per-file extraction limit stopped syntax-fact projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseLimitKind {
    /// Maximum visited syntax nodes was reached.
    SyntaxNodes,
    /// Maximum retained syntax facts was reached.
    Facts,
    /// Maximum retained diagnostics was reached.
    Diagnostics,
}

/// One structured Rust-style import path extracted from grammar nodes.
///
/// The path is syntactic evidence, not a resolved module or symbol identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportFact {
    /// Lexical module scope containing the import, from outermost to innermost.
    pub scope_path: Vec<String>,
    /// Import path segments, with raw-identifier prefixes removed.
    pub path: Vec<String>,
    /// Optional local alias introduced by an `as` clause.
    pub alias: Option<String>,
    /// Whether the import ends in a glob (`*`).
    pub is_glob: bool,
    /// Exact source byte range of the complete import declaration.
    pub range: SourceRange,
}

/// One Rust module declaration extracted from the syntax tree.
///
/// `is_inline` distinguishes a module body in this file from an external
/// `mod name;` declaration. This is syntactic evidence only; it does not
/// establish that a corresponding module file exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleFact {
    /// Lexical module scope containing the declaration, outermost first.
    pub scope_path: Vec<String>,
    /// Declared module name, with raw-identifier prefix removed.
    pub name: String,
    /// Whether the declaration has an inline module body.
    pub is_inline: bool,
    /// Whether module-resolution-affecting attributes such as `cfg`, `cfg_attr`,
    /// or `path` are present.
    ///
    /// Documentation and lint attributes do not set this flag. The resolver
    /// preserves uncertainty until relevant attributes are configuration-aware.
    pub has_resolution_attributes: bool,
    /// Exact source byte range of the module declaration.
    pub range: SourceRange,
}

/// Result of parsing one source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseResult {
    /// Host-supplied identity of the checkout being indexed.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative path; never an absolute machine path.
    pub relative_path: PathBuf,
    /// Classified language.
    pub language: LanguageId,
    /// Stable provider identifier.
    pub provider_id: String,
    /// Provider/parser version used for extraction.
    pub provider_version: String,
    /// Version of the language-neutral fact schema.
    pub schema_version: u32,
    /// Parse completion state.
    pub status: ParseStatus,
    /// Limit that stopped extraction, when status is Limited.
    pub limit_reason: Option<ParseLimitKind>,
    /// Deterministically ordered language-neutral syntax facts.
    pub facts: Vec<SyntaxFact>,
    /// Structured import paths when the provider can extract them.
    pub imports: Vec<ImportFact>,
    /// Rust module declarations when the provider can extract them.
    pub modules: Vec<RustModuleFact>,
    /// Parse diagnostics.
    pub diagnostics: Vec<ParseDiagnostic>,
}

/// Input to a language provider.
///
/// The caller is responsible for reading bytes through `WorkspaceFs` and
/// reserving their size through the shared `ScanBudget` before constructing
/// this input. Providers never access the filesystem or execute project code.
pub struct ParseInput<'a> {
    /// Host-supplied workspace identity.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative source path.
    pub relative_path: &'a Path,
    /// Bounded UTF-8 source bytes read through the workspace capability.
    pub source: &'a [u8],
}

/// Errors that prevent a provider from processing the input at all.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Input bytes are not valid UTF-8.
    #[error("source file is not valid UTF-8")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    /// The supplied path is not a non-empty, normal relative path.
    #[error("provider input path must be a non-empty workspace-relative path")]
    InvalidPath,
    /// The Tree-sitter parser could not be configured.
    #[error("failed to initialize language parser: {0}")]
    ParserInitialization(String),
    /// Tree-sitter aborted parsing before producing a syntax tree.
    #[error("language parser aborted before producing a syntax tree")]
    ParseAborted,
    /// Shared scan cancellation or deadline interrupted parsing.
    #[error("parser interrupted by scan budget: {0}")]
    Budget(#[from] ScanBudgetError),
}

/// Hard limits for syntax-tree projection into language-neutral facts.
#[derive(Clone, Debug)]
pub struct ParseLimits {
    /// Maximum syntax nodes visited while extracting facts.
    pub max_nodes: usize,
    /// Maximum facts retained for one source file.
    pub max_facts: usize,
    /// Maximum diagnostics retained for one source file.
    pub max_diagnostics: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_nodes: 1_000_000,
            max_facts: 100_000,
            max_diagnostics: 10_000,
        }
    }
}

/// Cooperative parse control shared with the workspace indexing operation.
pub struct ParseControl<'a> {
    budget: &'a ScanBudget,
    limits: &'a ParseLimits,
}

impl<'a> ParseControl<'a> {
    /// Bind parser execution to the shared workspace budget and per-file limits.
    #[must_use]
    pub const fn new(budget: &'a ScanBudget, limits: &'a ParseLimits) -> Self {
        Self { budget, limits }
    }

    /// Check cancellation and the shared workspace deadline.
    pub fn check(&self) -> Result<(), ScanBudgetError> {
        self.budget.check()
    }

    /// Return the per-file parser output limits.
    #[must_use]
    pub const fn limits(&self) -> &ParseLimits {
        self.limits
    }
}

/// Pluggable syntax parser for one language.
pub trait LanguageProvider: Send + Sync {
    /// Language handled by this provider.
    fn language(&self) -> LanguageId;

    /// Stable provider identifier.
    fn provider_id(&self) -> &'static str;

    /// Version string that invalidates prior extraction results when changed.
    fn provider_version(&self) -> &'static str;

    /// Parse one already-bounded source file.
    fn parse(&self, input: ParseInput<'_>) -> Result<ParseResult, ProviderError>;

    /// Parse with cooperative cancellation and per-file output limits.
    ///
    /// Providers should override this method to observe progress while parsing.
    /// The default preserves compatibility for simple providers but can only
    /// check the budget before and after their non-interruptible parse call.
    fn parse_with_control(
        &self,
        input: ParseInput<'_>,
        control: &ParseControl<'_>,
    ) -> Result<ParseResult, ProviderError> {
        control.check()?;
        let result = self.parse(input)?;
        control.check()?;
        Ok(result)
    }
}

pub(crate) fn valid_relative_path(path: &Path) -> bool {
    use std::path::Component;

    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => return false,
        }
    }
    saw_normal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ranges_are_half_open_and_bounded() {
        assert_eq!(
            SourceRange::new(2, 5, 5),
            Some(SourceRange {
                start_byte: 2,
                end_byte: 5,
            })
        );
        assert_eq!(SourceRange::new(5, 2, 5), None);
        assert_eq!(SourceRange::new(0, 6, 5), None);
    }

    #[test]
    fn rejects_absolute_and_traversal_paths() {
        assert!(!valid_relative_path(Path::new("/etc/passwd")));
        assert!(!valid_relative_path(Path::new("../outside.rs")));
        assert!(!valid_relative_path(Path::new("")));
        assert!(valid_relative_path(Path::new("src/lib.rs")));
    }
}
