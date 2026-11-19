//! Conservative symbol candidate indexing over source-backed syntax facts.
//!
//! This layer deliberately does not claim semantic name resolution. The Rust
//! provider currently emits declaration names, call spellings, and raw import
//! declarations without full lexical scope, module paths, or type information.
//! Matching names therefore produces candidates, never resolved edges.

use std::path::PathBuf;

use crate::{
    WorkspaceId,
    index::{SourceFileStatus, SourceIndexReport},
    syntax::SyntaxFactKind,
};

/// Kind of declaration represented in the candidate symbol catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    /// A module declaration.
    Module,
    /// A free function.
    Function,
    /// A method or associated function.
    Method,
    /// A struct.
    Struct,
    /// An enum.
    Enum,
    /// A union.
    Union,
    /// A trait.
    Trait,
    /// A type alias.
    TypeAlias,
    /// A constant.
    Constant,
    /// A static item.
    Static,
}

/// Stable identity of one declaration within one workspace snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolId {
    /// Workspace identity shared by the indexed snapshot.
    pub workspace_id: WorkspaceId,
    /// Workspace-relative source path.
    pub path: PathBuf,
    /// First declaration byte.
    pub start_byte: usize,
    /// First byte after the declaration.
    pub end_byte: usize,
    /// Declaration category.
    pub kind: SymbolKind,
}

/// A declaration indexed for conservative candidate lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolRecord {
    /// Deterministic declaration identity.
    pub id: SymbolId,
    /// Declared name.
    pub name: String,
}

/// Kind of source reference collected from syntax facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceKind {
    /// A direct function-call expression.
    FunctionCall,
    /// A method-call expression; the receiver type is unknown.
    MethodCall,
    /// A `use` declaration.
    Import,
}

/// Why a reference did not produce candidate declarations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateState {
    /// No declaration with a matching name exists in this workspace snapshot.
    NoCandidates,
    /// Exactly one same-name declaration exists; this is not semantic proof.
    UniqueCandidate,
    /// Multiple same-name declarations exist; scope/type information is needed.
    AmbiguousCandidates,
    /// The import syntax is outside the deliberately narrow supported subset.
    UnsupportedSyntax,
}

/// A source reference and its possible declaration targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceCandidates {
    /// Workspace-relative source path containing the reference.
    pub path: PathBuf,
    /// First reference byte.
    pub start_byte: usize,
    /// First byte after the reference.
    pub end_byte: usize,
    /// Reference category.
    pub kind: ReferenceKind,
    /// Source spelling of the reference target.
    pub spelling: String,
    /// Candidate outcome; never a resolved target.
    pub state: CandidateState,
    /// Same-name declaration candidates.
    pub candidates: Vec<SymbolId>,
}

/// Candidate catalog and references for one workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolCandidateReport {
    /// Workspace identity shared by the catalog.
    pub workspace_id: WorkspaceId,
    /// Deterministically ordered declaration catalog.
    pub symbols: Vec<SymbolRecord>,
    /// Deterministically ordered reference candidates.
    pub references: Vec<ReferenceCandidates>,
    /// References with exactly one same-name candidate.
    pub unique_candidate_references: u64,
    /// References with multiple same-name candidates.
    pub ambiguous_references: u64,
    /// References with no same-name candidate.
    pub unresolved_references: u64,
    /// References whose syntax is outside the supported subset.
    pub unsupported_references: u64,
}

/// Errors that make candidate indexing unsafe or internally inconsistent.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SymbolCandidateError {
    /// Input reports mix workspace identities.
    #[error("source index contains facts from more than one workspace")]
    MixedWorkspaces,
}

/// Build a deterministic declaration catalog and conservative reference candidates.
///
/// Only facts from complete, recovered, or limited parse results are consumed.
/// Failed and unsupported files remain represented by the source-index report
/// and do not contribute declarations. A unique name match is explicitly only
/// a candidate: this function does not know Rust lexical scopes, visibility,
/// module resolution, macro expansion, traits in scope, or receiver types.
pub fn build_symbol_candidates(
    workspace_id: WorkspaceId,
    source: &SourceIndexReport,
) -> Result<SymbolCandidateReport, SymbolCandidateError> {
    let mut symbols = Vec::new();
    let mut raw_references = Vec::new();

    for file in &source.files {
        let Some(parse) = &file.parse else {
            continue;
        };
        if parse.workspace_id != workspace_id {
            return Err(SymbolCandidateError::MixedWorkspaces);
        }
        if !matches!(
            file.status,
            SourceFileStatus::Parsed | SourceFileStatus::Recovered | SourceFileStatus::Limited
        ) {
            continue;
        }

        for fact in &parse.facts {
            let Some(name) = fact.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let symbol_kind = match fact.kind {
                SyntaxFactKind::Module => Some(SymbolKind::Module),
                SyntaxFactKind::Function => Some(SymbolKind::Function),
                SyntaxFactKind::Method => Some(SymbolKind::Method),
                SyntaxFactKind::Struct => Some(SymbolKind::Struct),
                SyntaxFactKind::Enum => Some(SymbolKind::Enum),
                SyntaxFactKind::Union => Some(SymbolKind::Union),
                SyntaxFactKind::Trait => Some(SymbolKind::Trait),
                SyntaxFactKind::TypeAlias => Some(SymbolKind::TypeAlias),
                SyntaxFactKind::Constant => Some(SymbolKind::Constant),
                SyntaxFactKind::Static => Some(SymbolKind::Static),
                SyntaxFactKind::Impl
                | SyntaxFactKind::Import
                | SyntaxFactKind::Call
                | SyntaxFactKind::MethodCall => None,
            };
            if let Some(kind) = symbol_kind {
                symbols.push(SymbolRecord {
                    id: SymbolId {
                        workspace_id,
                        path: file.path.clone(),
                        start_byte: fact.range.start_byte,
                        end_byte: fact.range.end_byte,
                        kind,
                    },
                    name: name.to_owned(),
                });
                continue;
            }

            let reference_kind = match fact.kind {
                SyntaxFactKind::Call => Some(ReferenceKind::FunctionCall),
                SyntaxFactKind::MethodCall => Some(ReferenceKind::MethodCall),
                SyntaxFactKind::Import => Some(ReferenceKind::Import),
                _ => None,
            };
            if let Some(kind) = reference_kind {
                raw_references.push((
                    file.path.clone(),
                    fact.range.start_byte,
                    fact.range.end_byte,
                    kind,
                    name.to_owned(),
                ));
            }
        }
    }

    symbols.sort_by(|left, right| {
        (
            &left.name,
            &left.id.path,
            left.id.start_byte,
            left.id.kind as u8,
        )
            .cmp(&(
                &right.name,
                &right.id.path,
                right.id.start_byte,
                right.id.kind as u8,
            ))
    });

    let mut references = Vec::with_capacity(raw_references.len());
    let mut unique_candidate_references = 0u64;
    let mut ambiguous_references = 0u64;
    let mut unresolved_references = 0u64;
    let mut unsupported_references = 0u64;

    for (path, start_byte, end_byte, kind, spelling) in raw_references {
        let lookup_name = match kind {
            ReferenceKind::Import => match import_leaf(&spelling) {
                Some(name) => name,
                None => {
                    unsupported_references = unsupported_references.saturating_add(1);
                    references.push(ReferenceCandidates {
                        path,
                        start_byte,
                        end_byte,
                        kind,
                        spelling,
                        state: CandidateState::UnsupportedSyntax,
                        candidates: Vec::new(),
                    });
                    continue;
                }
            },
            ReferenceKind::FunctionCall | ReferenceKind::MethodCall => {
                spelling.rsplit("::").next().unwrap_or(&spelling).trim()
            }
        };

        let candidates: Vec<_> = symbols
            .iter()
            .filter(|symbol| {
                symbol.name == lookup_name
                    && match kind {
                        ReferenceKind::FunctionCall => symbol.id.kind == SymbolKind::Function,
                        ReferenceKind::MethodCall => symbol.id.kind == SymbolKind::Method,
                        ReferenceKind::Import => true,
                    }
            })
            .map(|symbol| symbol.id.clone())
            .collect();

        let state = match candidates.len() {
            0 => {
                unresolved_references = unresolved_references.saturating_add(1);
                CandidateState::NoCandidates
            }
            1 => {
                unique_candidate_references = unique_candidate_references.saturating_add(1);
                CandidateState::UniqueCandidate
            }
            _ => {
                ambiguous_references = ambiguous_references.saturating_add(1);
                CandidateState::AmbiguousCandidates
            }
        };
        references.push(ReferenceCandidates {
            path,
            start_byte,
            end_byte,
            kind,
            spelling,
            state,
            candidates,
        });
    }

    references.sort_by(|left, right| {
        (&left.path, left.start_byte, left.end_byte, left.kind as u8).cmp(&(
            &right.path,
            right.start_byte,
            right.end_byte,
            right.kind as u8,
        ))
    });

    Ok(SymbolCandidateReport {
        workspace_id,
        symbols,
        references,
        unique_candidate_references,
        ambiguous_references,
        unresolved_references,
        unsupported_references,
    })
}

/// Extract the final identifier from a simple Rust use path.
///
/// Grouped imports, globs, aliases, and malformed declarations are deliberately
/// left unsupported until the provider emits structured Tree-sitter import facts.
fn import_leaf(import: &str) -> Option<&str> {
    let declaration = import
        .trim()
        .strip_prefix("pub use ")
        .or_else(|| import.trim().strip_prefix("use "))?
        .strip_suffix(';')?
        .trim();
    if declaration.is_empty()
        || declaration
            .chars()
            .any(|character| matches!(character, '{' | '}' | '*' | ','))
        || declaration.contains(" as ")
    {
        return None;
    }
    let leaf = declaration.rsplit("::").next()?.trim();
    let leaf = leaf.strip_prefix("r#").unwrap_or(leaf);
    (!leaf.is_empty()
        && leaf
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric()))
    .then_some(leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_import_leaf_is_extracted_without_claiming_resolution() {
        assert_eq!(import_leaf("use crate::model::Widget;"), Some("Widget"));
        assert_eq!(import_leaf("pub use crate::model::r#type;"), Some("type"));
        assert_eq!(import_leaf("use crate::model::{A, B};"), None);
        assert_eq!(import_leaf("use crate::model::*;"), None);
        assert_eq!(import_leaf("use crate::model::A as B;"), None);
    }

    #[test]
    fn builds_unique_and_ambiguous_candidates_from_real_rust_facts() {
        use crate::{
            IgnorePolicy, ScanBudget, ScanLimits, WorkspaceDescriptor, WorkspaceFs,
            index_rust_workspace,
        };
        use std::{fs, time::Duration};
        use tempfile::tempdir;

        let root = tempdir().expect("create workspace");
        fs::write(
            root.path().join("lib.rs"),
            "fn helper() {} fn caller() { helper(); }",
        )
        .expect("write source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::from_secs(10),
            ..ScanLimits::default()
        });
        let source = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());
        let report = build_symbol_candidates(workspace.workspace_id(), &source)
            .expect("same workspace identity");

        assert_eq!(report.symbols.len(), 2);
        assert_eq!(report.unique_candidate_references, 1);
        assert_eq!(report.references[0].state, CandidateState::UniqueCandidate);
        assert_eq!(report.references[0].candidates.len(), 1);
    }

    #[test]
    fn duplicate_names_remain_ambiguous_instead_of_selecting_arbitrarily() {
        use crate::{
            IgnorePolicy, ScanBudget, ScanLimits, WorkspaceDescriptor, WorkspaceFs,
            index_rust_workspace,
        };
        use std::{fs, time::Duration};
        use tempfile::tempdir;

        let root = tempdir().expect("create workspace");
        fs::write(
            root.path().join("lib.rs"),
            "fn helper() {} mod nested { fn helper() {} } fn caller() { helper(); }",
        )
        .expect("write source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::from_secs(10),
            ..ScanLimits::default()
        });
        let source = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());
        let report = build_symbol_candidates(workspace.workspace_id(), &source)
            .expect("same workspace identity");

        assert_eq!(report.ambiguous_references, 1);
        assert_eq!(
            report.references[0].state,
            CandidateState::AmbiguousCandidates
        );
        assert_eq!(report.references[0].candidates.len(), 2);
    }

    #[test]
    fn rejects_facts_from_another_workspace() {
        use crate::{
            IgnorePolicy, ScanBudget, ScanLimits, WorkspaceDescriptor, WorkspaceFs,
            index_rust_workspace,
        };
        use std::{fs, time::Duration};
        use tempfile::tempdir;

        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("lib.rs"), "fn helper() {}").expect("write source");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), root.path()).expect("create descriptor");
        let workspace = WorkspaceFs::open(&descriptor).expect("open workspace");
        let budget = ScanBudget::new(ScanLimits {
            max_duration: Duration::from_secs(10),
            ..ScanLimits::default()
        });
        let source = index_rust_workspace(&workspace, &budget, &IgnorePolicy::default());

        assert_eq!(
            build_symbol_candidates(WorkspaceId::new(), &source),
            Err(SymbolCandidateError::MixedWorkspaces)
        );
    }
}
