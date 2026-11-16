//! Conservative mapping of Rust imports onto source-backed scoped symbols.
//!
//! This layer joins module-prefix evidence with the crate-root-aware symbol
//! catalog. It emits candidates, never compiler-equivalent resolution claims.

use std::path::PathBuf;

use crate::{
    rust_imports::{RustImportResolution, RustImportResolutionReport, RustImportResolutionStatus},
    rust_symbols::{
        RustScopedSymbol, RustScopedSymbolId, RustScopedSymbolKind, RustScopedSymbolReport,
    },
    RustModuleGraphStatus, WorkspaceId,
};

/// Outcome of matching one import against the scoped symbol catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustSymbolImportStatus {
    /// Exactly one source-backed declaration matches the imported member.
    UniqueCandidate,
    /// Multiple source-backed declarations match the imported member.
    AmbiguousCandidates,
    /// A glob import has one or more direct module-scope symbol candidates.
    GlobCandidateSet,
    /// The path imports a module rather than a named symbol.
    ModuleImport,
    /// No matching declaration exists in a complete target scope.
    NoSymbolCandidate,
    /// The target source was not fully parsed, so absence is inconclusive.
    IncompleteTargetSource,
    /// The module graph or symbol catalog is incomplete, so absence is inconclusive.
    IncompleteIndex,
    /// The remaining path has more than one member segment; associated-item and
    /// nested member semantics are not modeled by this source-level resolver.
    UnsupportedMemberPath,
    /// The module resolver did not produce a usable, unique module prefix.
    UnresolvedModule,
}

/// One source-backed candidate for an imported name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustSymbolImportCandidate {
    /// Stable crate-root-aware declaration identity.
    pub symbol_id: RustScopedSymbolId,
    /// Name as declared at the source location.
    pub symbol_name: String,
    /// Name visible at the import site after applying an explicit alias.
    pub local_name: String,
}

/// Candidate mapping for one source-backed Rust import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustSymbolImportResolution {
    /// Crate-root context inherited from module resolution.
    pub crate_root: Option<PathBuf>,
    /// Source file containing the import.
    pub source_path: PathBuf,
    /// Original import path.
    pub import_path: Vec<String>,
    /// Local alias, when explicitly declared.
    pub alias: Option<String>,
    /// Exact source range of the import declaration.
    pub range: crate::SourceRange,
    /// Module-level outcome retained without reinterpretation.
    pub module_status: RustImportResolutionStatus,
    /// Symbol-candidate outcome.
    pub status: RustSymbolImportStatus,
    /// Source-backed declaration candidates.
    pub candidates: Vec<RustSymbolImportCandidate>,
}

/// Results of joining module import mappings with the scoped symbol catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustSymbolImportResolutionReport {
    /// Host-managed workspace identity.
    pub workspace_id: WorkspaceId,
    /// Completeness inherited from the module graph and symbol catalog.
    pub graph_status: RustModuleGraphStatus,
    /// Deterministically ordered import candidate mappings.
    pub imports: Vec<RustSymbolImportResolution>,
}

/// Invalid inputs to scoped symbol import resolution.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RustSymbolImportResolutionError {
    #[error("module import report and symbol catalog have different workspace identities")]
    WorkspaceMismatch,
    #[error("module import report and symbol catalog have different graph completeness states")]
    GraphStatusMismatch,
}

/// Join module-prefix results with crate-root-aware source symbol candidates.
///
/// A unique candidate is not a resolved Rust name: this function does not
/// evaluate visibility, cfg, re-exports, macro expansion, extern-prelude lookup,
/// trait scope, or type-dependent associated-item semantics.
pub fn resolve_rust_import_symbols(
    module_report: &RustImportResolutionReport,
    symbol_report: &RustScopedSymbolReport,
) -> Result<RustSymbolImportResolutionReport, RustSymbolImportResolutionError> {
    if module_report.workspace_id != symbol_report.workspace_id {
        return Err(RustSymbolImportResolutionError::WorkspaceMismatch);
    }
    if module_report.graph_status != symbol_report.graph_status {
        return Err(RustSymbolImportResolutionError::GraphStatusMismatch);
    }

    let mut imports = Vec::with_capacity(module_report.imports.len());
    for import in &module_report.imports {
        imports.push(resolve_one_import(
            import,
            &symbol_report.symbols,
            &symbol_report.incomplete_source_paths,
            symbol_report.graph_status,
        ));
    }

    imports.sort_unstable_by(|left, right| {
        (&left.source_path, left.range, &left.crate_root, &left.import_path)
            .cmp(&(&right.source_path, right.range, &right.crate_root, &right.import_path))
    });

    Ok(RustSymbolImportResolutionReport {
        workspace_id: module_report.workspace_id,
        graph_status: symbol_report.graph_status,
        imports,
    })
}

fn resolve_one_import(
    import: &RustImportResolution,
    symbols: &[RustScopedSymbol],
    incomplete_source_paths: &[PathBuf],
    graph_status: RustModuleGraphStatus,
) -> RustSymbolImportResolution {
    let mut result = RustSymbolImportResolution {
        crate_root: import.crate_root.clone(),
        source_path: import.source_path.clone(),
        import_path: import.import_path.clone(),
        alias: import.alias.clone(),
        range: import.range,
        module_status: import.status,
        status: RustSymbolImportStatus::UnresolvedModule,
        candidates: Vec::new(),
    };

    if import.status == RustImportResolutionStatus::ModuleTarget && !import.is_glob {
        result.status = RustSymbolImportStatus::ModuleImport;
        return result;
    }

    if import.status != RustImportResolutionStatus::ModulePrefixForMember
        && !(import.status == RustImportResolutionStatus::ModuleTarget && import.is_glob)
    {
        return result;
    }

    let (Some(crate_root), Some(module_path)) =
        (import.crate_root.as_ref(), import.target_module_path.as_ref())
    else {
        return result;
    };

    if !import.is_glob && import.member_path.len() != 1 {
        result.status = RustSymbolImportStatus::UnsupportedMemberPath;
        return result;
    }

    let direct_symbols = symbols.iter().filter(|symbol| {
        symbol.id.crate_root == *crate_root
            && symbol.id.module_path == *module_path
            // Methods live in impl/type namespaces, which this first catalog
            // does not model. Do not expose them as module-level import targets.
            && symbol.id.kind != RustScopedSymbolKind::Method
    });

    if import.is_glob {
        result.candidates = direct_symbols
            .map(|symbol| make_candidate(symbol, None))
            .collect();
        if result.candidates.is_empty() {
            result.status = absence_status(
                import.target_source_path.as_ref(),
                incomplete_source_paths,
                graph_status,
            );
        } else {
            result.status = RustSymbolImportStatus::GlobCandidateSet;
        }
        return result;
    }

    let member = &import.member_path[0];
    result.candidates = direct_symbols
        .filter(|symbol| symbol.name == *member)
        .map(|symbol| make_candidate(symbol, import.alias.as_deref()))
        .collect();

    result.status = match result.candidates.len() {
        0 => absence_status(
            import.target_source_path.as_ref(),
            incomplete_source_paths,
            graph_status,
        ),
        1 => RustSymbolImportStatus::UniqueCandidate,
        _ => RustSymbolImportStatus::AmbiguousCandidates,
    };
    result
}

fn absence_status(
    target_source_path: Option<&PathBuf>,
    incomplete_source_paths: &[PathBuf],
    graph_status: RustModuleGraphStatus,
) -> RustSymbolImportStatus {
    if target_source_path.is_some_and(|path| incomplete_source_paths.contains(path)) {
        RustSymbolImportStatus::IncompleteTargetSource
    } else if graph_status != RustModuleGraphStatus::Complete {
        RustSymbolImportStatus::IncompleteIndex
    } else {
        RustSymbolImportStatus::NoSymbolCandidate
    }
}

fn make_candidate(
    symbol: &RustScopedSymbol,
    alias: Option<&str>,
) -> RustSymbolImportCandidate {
    RustSymbolImportCandidate {
        symbol_id: symbol.id.clone(),
        symbol_name: symbol.name.clone(),
        local_name: alias.unwrap_or(&symbol.name).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        rust_imports::RustImportResolution,
        rust_symbols::{RustScopedSymbol, RustScopedSymbolId},
        SourceRange,
    };

    fn symbol(
        crate_root: &str,
        module_path: &[&str],
        source_path: &str,
        name: &str,
        start: usize,
    ) -> RustScopedSymbol {
        RustScopedSymbol {
            id: RustScopedSymbolId {
                workspace_id: WorkspaceId::new(),
                crate_root: PathBuf::from(crate_root),
                module_path: module_path.iter().map(|part| (*part).to_owned()).collect(),
                source_path: PathBuf::from(source_path),
                range: SourceRange { start_byte: start, end_byte: start + name.len() },
                kind: RustScopedSymbolKind::Struct,
            },
            name: name.to_owned(),
        }
    }

    fn module_import(
        id: WorkspaceId,
        member_path: &[&str],
        is_glob: bool,
        alias: Option<&str>,
    ) -> RustImportResolution {
        RustImportResolution {
            crate_root: Some(PathBuf::from("src/lib.rs")),
            source_path: PathBuf::from("src/lib.rs"),
            scope_path: Vec::new(),
            import_path: std::iter::once("crate".to_owned())
                .chain(["model".to_owned()])
                .chain(member_path.iter().map(|part| (*part).to_owned()))
                .collect(),
            alias: alias.map(str::to_owned),
            is_glob,
            target_module_path: Some(vec!["model".to_owned()]),
            target_source_path: Some(PathBuf::from("src/model.rs")),
            candidate_source_paths: Vec::new(),
            member_path: member_path.iter().map(|part| (*part).to_owned()).collect(),
            range: SourceRange { start_byte: 4, end_byte: 24 },
            status: if member_path.is_empty() {
                RustImportResolutionStatus::ModuleTarget
            } else {
                RustImportResolutionStatus::ModulePrefixForMember
            },
        }
    }

    fn resolve(
        imports: Vec<RustImportResolution>,
        symbols: Vec<RustScopedSymbol>,
        graph_status: RustModuleGraphStatus,
        incomplete: Vec<PathBuf>,
    ) -> RustSymbolImportResolutionReport {
        let workspace_id = WorkspaceId::new();
        // Keep fixture identities aligned with the report identity.
        let mut symbols = symbols;
        for symbol in &mut symbols {
            symbol.id.workspace_id = workspace_id;
        }
        resolve_rust_import_symbols(
            &RustImportResolutionReport {
                workspace_id,
                graph_status,
                imports,
            },
            &RustScopedSymbolReport {
                workspace_id,
                graph_status,
                symbols,
                incomplete_source_paths: incomplete,
            },
        ).expect("symbol import resolution")
    }

    #[test]
    fn maps_unique_member_candidate_and_applies_alias() {
        let id = WorkspaceId::new();
        let mut item = symbol("src/lib.rs", &["model"], "src/model.rs", "Widget", 10);
        item.id.workspace_id = id;
        let report = resolve(
            vec![module_import(id, &["Widget"], false, Some("LocalWidget"))],
            vec![item],
            RustModuleGraphStatus::Complete,
            Vec::new(),
        );
        assert_eq!(report.imports[0].status, RustSymbolImportStatus::UniqueCandidate);
        assert_eq!(report.imports[0].candidates[0].local_name, "LocalWidget");
    }

    #[test]
    fn retains_ambiguity_for_duplicate_names_in_one_module() {
        let id = WorkspaceId::new();
        let mut first = symbol("src/lib.rs", &["model"], "src/model.rs", "Widget", 10);
        let mut second = symbol("src/lib.rs", &["model"], "src/model.rs", "Widget", 40);
        first.id.workspace_id = id;
        second.id.workspace_id = id;
        let report = resolve(
            vec![module_import(id, &["Widget"], false, None)],
            vec![first, second],
            RustModuleGraphStatus::Complete,
            Vec::new(),
        );
        assert_eq!(report.imports[0].status, RustSymbolImportStatus::AmbiguousCandidates);
        assert_eq!(report.imports[0].candidates.len(), 2);
    }

    #[test]
    fn glob_import_returns_only_direct_module_candidates() {
        let id = WorkspaceId::new();
        let mut direct = symbol("src/lib.rs", &["model"], "src/model.rs", "Widget", 10);
        let mut nested = symbol("src/lib.rs", &["model", "inner"], "src/model.rs", "Hidden", 40);
        direct.id.workspace_id = id;
        nested.id.workspace_id = id;
        let report = resolve(
            vec![module_import(id, &[], true, None)],
            vec![direct, nested],
            RustModuleGraphStatus::Complete,
            Vec::new(),
        );
        assert_eq!(report.imports[0].status, RustSymbolImportStatus::GlobCandidateSet);
        assert_eq!(report.imports[0].candidates.len(), 1);
    }

    #[test]
    fn does_not_call_absence_resolved_when_target_parse_is_incomplete() {
        let id = WorkspaceId::new();
        let report = resolve(
            vec![module_import(id, &["Missing"], false, None)],
            Vec::new(),
            RustModuleGraphStatus::Partial,
            vec![PathBuf::from("src/model.rs")],
        );
        assert_eq!(report.imports[0].status, RustSymbolImportStatus::IncompleteTargetSource);
    }

    #[test]
    fn preserves_unresolved_module_status() {
        let id = WorkspaceId::new();
        let mut import = module_import(id, &["Widget"], false, None);
        import.status = RustImportResolutionStatus::UncertainModuleTarget;
        import.target_module_path = None;
        let report = resolve(vec![import], Vec::new(), RustModuleGraphStatus::Partial, Vec::new());
        assert_eq!(report.imports[0].status, RustSymbolImportStatus::UnresolvedModule);
        assert_eq!(report.imports[0].module_status, RustImportResolutionStatus::UncertainModuleTarget);
    }
}
