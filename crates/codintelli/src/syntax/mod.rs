//! Host-neutral syntax facts and language-provider contracts.

use std::path::{Path, PathBuf};

use crate::{classification::LanguageId, WorkspaceId};

pub mod rust;

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

/// One syntax fact with a source-backed location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxFact {
    /// Fact category.
    pub kind: SyntaxFactKind,
    /// Declared name or source spelling of a reference, when available.
    pub name: Option<String>,
    /// Exact source byte range.
    pub range: SourceRange,
}

/// Parser outcome for one source file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseStatus {
    /// The provider parsed the complete file without syntax errors.
    Complete,
    /// Parsing failed; no partial facts are claimed as complete.
    Failed,
}

/// A parser diagnostic tied to the original source when a range is available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseDiagnostic {
    /// Optional source location.
    pub range: Option<SourceRange>,
    /// Human-readable diagnostic; must not contain source excerpts by default.
    pub message: String,
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
    /// Deterministically ordered facts.
    pub facts: Vec<SyntaxFact>,
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
