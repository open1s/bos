//! Crate-root-aware Rust symbol catalog built from syntax facts and the module graph.
//!
//! This catalog preserves source scope and crate-root context. It deliberately
//! does not perform compiler-equivalent name resolution, visibility checking,
//! cfg evaluation, re-export expansion, or type-based method resolution.

use std::path::PathBuf;

use crate::{
    module_graph::{RustModuleGraph, RustModuleGraphStatus, RustModuleNodeKind},
    syntax::{ParseResult, ParseStatus, RustVisibility, SourceRange, SyntaxFactKind},
    WorkspaceId,
};

/// Declaration category retained by the crate-root-aware Rust symbol catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustScopedSymbolKind {
    /// A free function in module scope.
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

/// Identity of a declaration in one crate-root-specific module namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustScopedSymbolId {
    /// Workspace identity shared by the catalog.
    pub workspace_id: WorkspaceId,
    /// Host-supplied crate root owning this namespace.
    pub crate_root: PathBuf,
    /// Logical module path relative to the crate root.
    pub module_path: Vec<String>,
    /// Workspace-relative source file.
    pub source_path: PathBuf,
    /// Exact declaration source range.
    pub range: SourceRange,
    /// Declaration category.
    pub kind: RustScopedSymbolKind,
}

/// Source-backed Rust declaration with its logical crate/module path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustScopedSymbol {
    /// Deterministic crate-root-scoped declaration identity.
    pub id: RustScopedSymbolId,
    /// Declared name.
    pub name: String,
    /// Visibility modifier written at the declaration; not effective accessibility.
    pub visibility: Option<RustVisibility>,
}

/// Catalog coverage and diagnostics for one workspace module graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustScopedSymbolReport {
    /// Workspace identity shared by the catalog.
    pub workspace_id: WorkspaceId,
    /// Completeness of the module graph used to scope declarations.
    pub graph_status: RustModuleGraphStatus,
    /// Deterministically ordered scoped symbols.
    pub symbols: Vec<RustScopedSymbol>,
    /// Files whose parse output is recovered or limited.
    pub incomplete_source_paths: Vec<PathBuf>,
}

/// Invalid input to scoped Rust symbol catalog construction.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RustScopedSymbolError {
    /// Parse results and the module graph mix workspace identities.
    #[error("parse results and module graph have different workspace identities")]
    WorkspaceMismatch,
    /// Two parse results describe the same relative path.
    #[error("duplicate parse result path: {0}")]
    DuplicateParsePath(PathBuf),
}

/// Build symbols in crate-root-specific module contexts.
///
/// A source file shared by multiple crate roots intentionally produces distinct
/// symbol identities. Facts in function/block-local scopes are excluded because
/// this module-level catalog cannot model their lexical visibility accurately.
pub fn build_rust_scoped_symbols(
    parse_results: &[ParseResult],
    graph: &RustModuleGraph,
) -> Result<RustScopedSymbolReport, RustScopedSymbolError> {
    if parse_results
        .iter()
        .any(|result| result.workspace_id != graph.workspace_id)
    {
        return Err(RustScopedSymbolError::WorkspaceMismatch);
    }

    let mut by_path = std::collections::HashMap::new();
    let mut incomplete_source_paths = Vec::new();
    for result in parse_results {
        if by_path
            .insert(result.relative_path.clone(), result)
            .is_some()
        {
            return Err(RustScopedSymbolError::DuplicateParsePath(
                result.relative_path.clone(),
            ));
        }
        if matches!(
            result.status,
            ParseStatus::Recovered | ParseStatus::Limited | ParseStatus::Failed
        ) {
            incomplete_source_paths.push(result.relative_path.clone());
        }
    }

    let mut symbols = Vec::new();
    for node in graph
        .nodes
        .iter()
        .filter(|node| node.kind != RustModuleNodeKind::Inline)
    {
        let Some(parse) = by_path.get(&node.source_path) else {
            continue;
        };
        if parse.language != crate::classification::LanguageId::Rust
            || parse.status == ParseStatus::Failed
        {
            continue;
        }
        for fact in &parse.facts {
            if !fact.is_module_scope {
                continue;
            }
            let Some(name) = fact.name.as_deref().filter(|name| !name.is_empty()) else {
                continue;
            };
            let kind = match fact.kind {
                SyntaxFactKind::Function => RustScopedSymbolKind::Function,
                SyntaxFactKind::Method => RustScopedSymbolKind::Method,
                SyntaxFactKind::Struct => RustScopedSymbolKind::Struct,
                SyntaxFactKind::Enum => RustScopedSymbolKind::Enum,
                SyntaxFactKind::Union => RustScopedSymbolKind::Union,
                SyntaxFactKind::Trait => RustScopedSymbolKind::Trait,
                SyntaxFactKind::TypeAlias => RustScopedSymbolKind::TypeAlias,
                SyntaxFactKind::Constant => RustScopedSymbolKind::Constant,
                SyntaxFactKind::Static => RustScopedSymbolKind::Static,
                SyntaxFactKind::Module
                | SyntaxFactKind::Impl
                | SyntaxFactKind::Import
                | SyntaxFactKind::Call
                | SyntaxFactKind::MethodCall => continue,
            };
            let mut module_path = node.module_path.clone();
            module_path.extend(fact.scope_path.iter().cloned());
            symbols.push(RustScopedSymbol {
                id: RustScopedSymbolId {
                    workspace_id: graph.workspace_id,
                    crate_root: node.crate_root.clone(),
                    module_path,
                    source_path: node.source_path.clone(),
                    range: fact.range,
                    kind,
                },
                name: name.to_owned(),
                visibility: fact.visibility.clone(),
            });
        }
    }

    symbols.sort_unstable_by(|left, right| {
        (
            &left.id.crate_root,
            &left.id.module_path,
            &left.name,
            &left.id.source_path,
            left.id.range,
        )
            .cmp(&(
                &right.id.crate_root,
                &right.id.module_path,
                &right.name,
                &right.id.source_path,
                right.id.range,
            ))
    });
    incomplete_source_paths.sort();
    incomplete_source_paths.dedup();

    Ok(RustScopedSymbolReport {
        workspace_id: graph.workspace_id,
        graph_status: graph.status,
        symbols,
        incomplete_source_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        classification::LanguageId,
        module_graph::{RustModuleNode, RustModuleNodeKind},
        syntax::{ParseLimitKind, ParseStatus, SyntaxFact, SyntaxFactKind},
    };

    #[test]
    fn keeps_shared_source_in_distinct_crate_root_namespaces() {
        let workspace_id = WorkspaceId::new();
        let source_path = PathBuf::from("src/shared.rs");
        let parse = ParseResult {
            workspace_id,
            relative_path: source_path.clone(),
            language: LanguageId::Rust,
            provider_id: "test".to_owned(),
            provider_version: "test".to_owned(),
            schema_version: 2,
            status: ParseStatus::Complete,
            limit_reason: None::<ParseLimitKind>,
            facts: vec![SyntaxFact {
                kind: SyntaxFactKind::Function,
                name: Some("run".to_owned()),
                scope_path: vec!["inner".to_owned()],
                is_module_scope: true,
                visibility: Some(RustVisibility::Public),
                range: SourceRange {
                    start_byte: 0,
                    end_byte: 12,
                },
            }],
            imports: Vec::new(),
            modules: Vec::new(),
            diagnostics: Vec::new(),
        };
        let graph = RustModuleGraph {
            workspace_id,
            crate_roots: vec![PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")],
            nodes: vec![
                RustModuleNode {
                    crate_root: PathBuf::from("src/lib.rs"),
                    module_path: Vec::new(),
                    source_path: source_path.clone(),
                    kind: RustModuleNodeKind::CrateRoot,
                },
                RustModuleNode {
                    crate_root: PathBuf::from("src/main.rs"),
                    module_path: vec!["shared".to_owned()],
                    source_path: source_path.clone(),
                    kind: RustModuleNodeKind::External,
                },
            ],
            edges: Vec::new(),
            status: RustModuleGraphStatus::Complete,
        };

        let report = build_rust_scoped_symbols(&[parse], &graph).expect("scoped symbol catalog");
        assert_eq!(report.symbols.len(), 2);
        assert_eq!(report.symbols[0].id.module_path, vec!["inner".to_owned()]);
        assert_eq!(
            report.symbols[1].id.module_path,
            vec!["shared".to_owned(), "inner".to_owned()]
        );
        assert_ne!(
            report.symbols[0].id.crate_root,
            report.symbols[1].id.crate_root
        );
    }
}
