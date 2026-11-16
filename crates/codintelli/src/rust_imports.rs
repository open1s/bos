//! Conservative mapping of Rust `use` paths onto known module graph nodes.
//!
//! This resolves only module prefixes supported by the crate-root module graph.
//! Remaining path segments are reported as member-path candidates, not symbol
//! resolutions. Unqualified paths are intentionally left unresolved because
//! they may refer to the extern prelude or language-specific scope rules.

use std::path::PathBuf;

use crate::{
    classification::LanguageId,
    module_graph::{RustModuleGraph, RustModuleNodeKind},
    syntax::valid_relative_path,
    ImportFact, ParseResult, SourceRange, WorkspaceId,
};

/// Outcome of mapping an import path onto the known Rust module graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustImportResolutionStatus {
    /// The complete path names a known module.
    ModuleTarget,
    /// A known module prefix exists; remaining segments may name an item.
    ModulePrefixForMember,
    /// Multiple source nodes share the longest matching module path.
    AmbiguousModuleTarget,
    /// No non-root module prefix matches the path.
    NoModuleCandidate,
    /// The path uses an unqualified first segment; extern-prelude and lexical
    /// lookup are not inferred.
    UnsupportedUnqualifiedPath,
    /// The number of `super` segments would move above the crate root.
    InvalidSuperTraversal,
    /// The source file/scope is not represented in the supplied module graph.
    MissingModuleContext,
    /// The import path contains an invalid identifier segment.
    InvalidImportPath,
}

/// One source-backed import mapping result for one crate-root context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustImportResolution {
    /// Crate root whose logical namespace was used, if a context was found.
    pub crate_root: Option<PathBuf>,
    /// Source file containing the import.
    pub source_path: PathBuf,
    /// Lexical module scope from the syntax provider.
    pub scope_path: Vec<String>,
    /// Original syntactic import path.
    pub import_path: Vec<String>,
    /// Optional local alias.
    pub alias: Option<String>,
    /// Whether the import is a glob.
    pub is_glob: bool,
    /// Longest matching module path, when one was found.
    pub target_module_path: Option<Vec<String>>,
    /// Source file for the target module, when uniquely identified.
    pub target_source_path: Option<PathBuf>,
    /// Remaining path segments after the module prefix.
    pub member_path: Vec<String>,
    /// Source evidence for the import declaration.
    pub range: SourceRange,
    /// Conservative mapping outcome.
    pub status: RustImportResolutionStatus,
}

/// Results for all Rust imports against one crate-root module graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustImportResolutionReport {
    /// Workspace identity shared by parse results and graph.
    pub workspace_id: WorkspaceId,
    /// Completeness of the underlying module graph.
    pub graph_status: crate::RustModuleGraphStatus,
    /// Deterministically ordered import mappings.
    pub imports: Vec<RustImportResolution>,
}

/// Invalid input to Rust import-to-module mapping.
#[derive(Debug, thiserror::Error)]
pub enum RustImportResolutionError {
    /// No parse results were supplied.
    #[error("at least one parse result is required")]
    EmptyInput,
    /// Parse results and module graph belong to different workspaces.
    #[error("parse results and module graph have different workspace identities")]
    WorkspaceMismatch,
    /// A source path is not a normal workspace-relative path.
    #[error("import source paths must be normal workspace-relative paths")]
    InvalidPath,
}

/// Map structured Rust import paths onto known module nodes.
///
/// Supports explicit `crate::`, `self::`, and repeated `super::` roots.
/// Bare paths are left unresolved. The longest known module prefix is returned;
/// any remaining segments are merely a member-path candidate and are not
/// asserted to identify a real symbol. Aliases and glob markers are preserved.
pub fn resolve_rust_import_module_candidates(
    parse_results: &[ParseResult],
    graph: &RustModuleGraph,
) -> Result<RustImportResolutionReport, RustImportResolutionError> {
    let first = parse_results.first().ok_or(RustImportResolutionError::EmptyInput)?;
    let workspace_id = first.workspace_id;
    if workspace_id != graph.workspace_id
        || parse_results.iter().any(|result| result.workspace_id != workspace_id)
    {
        return Err(RustImportResolutionError::WorkspaceMismatch);
    }

    for result in parse_results {
        if !valid_relative_path(&result.relative_path) {
            return Err(RustImportResolutionError::InvalidPath);
        }
    }

    let mut imports = Vec::new();
    for result in parse_results.iter().filter(|result| result.language == LanguageId::Rust) {
        for fact in &result.imports {
            if fact.import_path_is_invalid() {
                imports.push(make_unresolved(
                    None,
                    result,
                    fact,
                    RustImportResolutionStatus::InvalidImportPath,
                ));
                continue;
            }
            let contexts: Vec<_> = graph.nodes.iter().filter(|base| {
                base.source_path == result.relative_path
                    && base.kind != RustModuleNodeKind::Inline
            }).filter_map(|base| {
                let mut expected = base.module_path.clone();
                expected.extend(fact.scope_path.iter().cloned());
                graph.nodes.iter().find(|node| {
                    node.crate_root == base.crate_root
                        && node.source_path == result.relative_path
                        && node.module_path == expected
                }).map(|_| (base.crate_root.clone(), expected))
            }).collect();

            if contexts.is_empty() {
                imports.push(make_unresolved(
                    None,
                    result,
                    fact,
                    RustImportResolutionStatus::MissingModuleContext,
                ));
                continue;
            }

            for (crate_root, context_path) in contexts {
                imports.push(resolve_one_import(
                    &graph.nodes,
                    crate_root,
                    context_path,
                    result,
                    fact,
                ));
            }
        }
    }

    imports.sort_unstable_by(|left, right| {
        (
            &left.source_path,
            left.range,
            &left.crate_root,
            &left.import_path,
        )
            .cmp(&(
                &right.source_path,
                right.range,
                &right.crate_root,
                &right.import_path,
            ))
    });

    Ok(RustImportResolutionReport {
        workspace_id,
        graph_status: graph.status,
        imports,
    })
}

fn resolve_one_import(
    nodes: &[crate::RustModuleNode],
    crate_root: PathBuf,
    context_path: Vec<String>,
    source: &ParseResult,
    fact: &ImportFact,
) -> RustImportResolution {
    if fact.import_path_is_invalid() {
        return make_unresolved(
            Some(crate_root),
            source,
            fact,
            RustImportResolutionStatus::InvalidImportPath,
        );
    }

    let mut path = fact.path.clone();
    let path_root = path.first().cloned();
    let target_path = match path_root.as_deref() {
        Some("crate") => {
            path.remove(0);
            path
        }
        Some("self") => {
            path.remove(0);
            let mut target = context_path;
            target.extend(path);
            target
        }
        Some("super") => {
            let mut target = context_path;
            while path.first().is_some_and(|segment| segment == "super") {
                path.remove(0);
                if target.pop().is_none() {
                    return make_unresolved(
                        Some(crate_root),
                        source,
                        fact,
                        RustImportResolutionStatus::InvalidSuperTraversal,
                    );
                }
            }
            target.extend(path);
            target
        }
        _ => {
            return make_unresolved(
                Some(crate_root),
                source,
                fact,
                RustImportResolutionStatus::UnsupportedUnqualifiedPath,
            );
        }
    };

    let mut matches: Vec<_> = nodes.iter().filter(|node| {
        node.crate_root == crate_root
            && is_prefix(&node.module_path, &target_path)
            && (!node.module_path.is_empty() || target_path.is_empty())
    }).collect();
    if matches.is_empty() {
        return make_unresolved(
            Some(crate_root),
            source,
            fact,
            RustImportResolutionStatus::NoModuleCandidate,
        );
    }

    let longest = matches.iter().map(|node| node.module_path.len()).max().unwrap_or(0);
    matches.retain(|node| node.module_path.len() == longest);
    if matches.len() != 1 {
        return make_unresolved(
            Some(crate_root),
            source,
            fact,
            RustImportResolutionStatus::AmbiguousModuleTarget,
        );
    }

    let target = matches[0];
    let member_path = target_path[target.module_path.len()..].to_vec();
    let status = if member_path.is_empty() {
        RustImportResolutionStatus::ModuleTarget
    } else {
        RustImportResolutionStatus::ModulePrefixForMember
    };

    RustImportResolution {
        crate_root: Some(crate_root),
        source_path: source.relative_path.clone(),
        scope_path: fact.scope_path.clone(),
        import_path: fact.path.clone(),
        alias: fact.alias.clone(),
        is_glob: fact.is_glob,
        target_module_path: Some(target.module_path.clone()),
        target_source_path: Some(target.source_path.clone()),
        member_path,
        range: fact.range,
        status,
    }
}

fn is_prefix(prefix: &[String], path: &[String]) -> bool {
    path.starts_with(prefix)
}

fn make_unresolved(
    crate_root: Option<PathBuf>,
    source: &ParseResult,
    fact: &ImportFact,
    status: RustImportResolutionStatus,
) -> RustImportResolution {
    RustImportResolution {
        crate_root,
        source_path: source.relative_path.clone(),
        scope_path: fact.scope_path.clone(),
        import_path: fact.path.clone(),
        alias: fact.alias.clone(),
        is_glob: fact.is_glob,
        target_module_path: None,
        target_source_path: None,
        member_path: Vec::new(),
        range: fact.range,
        status,
    }
}

fn valid_import_identifier(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.chars().any(|character| {
            matches!(character, '/' | '\\\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\\0')
        })
}

trait ImportFactValidation {
    fn import_path_is_invalid(&self) -> bool;
}

impl ImportFactValidation for ImportFact {
    fn import_path_is_invalid(&self) -> bool {
        self.path.iter().any(|segment| !valid_import_identifier(segment))
            || self.scope_path.iter().any(|segment| !valid_import_identifier(segment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ParseStatus, RustModuleEdge, RustModuleFact, RustModuleNode, RustModuleNodeKind,
        SyntaxFact, SyntaxFactKind,
    };

    fn parse_result(
        path: &str,
        imports: Vec<ImportFact>,
        modules: Vec<RustModuleFact>,
        id: WorkspaceId,
    ) -> ParseResult {
        ParseResult {
            workspace_id: id,
            relative_path: PathBuf::from(path),
            language: LanguageId::Rust,
            provider_id: "test".to_owned(),
            provider_version: "test".to_owned(),
            schema_version: 1,
            status: ParseStatus::Complete,
            limit_reason: None,
            facts: vec![SyntaxFact {
                kind: SyntaxFactKind::Module,
                name: None,
                range: SourceRange { start_byte: 0, end_byte: 1 },
            }],
            imports,
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn import(path: &[&str], scope: &[&str]) -> ImportFact {
        ImportFact {
            scope_path: scope.iter().map(|item| (*item).to_owned()).collect(),
            path: path.iter().map(|item| (*item).to_owned()).collect(),
            alias: Some("Local".to_owned()),
            is_glob: false,
            range: SourceRange { start_byte: 10, end_byte: 20 },
        }
    }

    fn make_graph(id: WorkspaceId, nodes: Vec<RustModuleNode>) -> RustModuleGraph {
        RustModuleGraph {
            workspace_id: id,
            crate_roots: vec![PathBuf::from("src/lib.rs")],
            nodes,
            edges: Vec::<RustModuleEdge>::new(),
            status: crate::RustModuleGraphStatus::Complete,
        }
    }

    fn node(
        module_path: &[&str],
        source: &str,
        kind: RustModuleNodeKind,
    ) -> RustModuleNode {
        RustModuleNode {
            crate_root: PathBuf::from("src/lib.rs"),
            module_path: module_path.iter().map(|item| (*item).to_owned()).collect(),
            source_path: PathBuf::from(source),
            kind,
        }
    }

    #[test]
    fn resolves_crate_root_module_and_member_prefixes() {
        let id = WorkspaceId::new();
        let parsed = parse_result("src/lib.rs", vec![
            import(&["crate", "model"], &[]),
            import(&["crate", "model", "Widget"], &[]),
        ], Vec::new(), id);
        let graph = make_graph(id, vec![
            node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot),
            node(&["model"], "src/model.rs", RustModuleNodeKind::External),
        ]);
        let report = resolve_rust_import_module_candidates(&[parsed], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::ModuleTarget);
        assert_eq!(report.imports[1].status, RustImportResolutionStatus::ModulePrefixForMember);
        assert_eq!(report.imports[1].member_path, vec!["Widget".to_owned()]);
    }

    #[test]
    fn resolves_self_and_super_against_inline_scope() {
        let id = WorkspaceId::new();
        let parsed = parse_result("src/parent.rs", vec![
            import(&["self", "Item"], &["inner"]),
            import(&["super", "sibling"], &["inner"]),
        ], Vec::new(), id);
        let graph = make_graph(id, vec![
            node(&["parent"], "src/parent.rs", RustModuleNodeKind::External),
            node(&["parent", "inner"], "src/parent.rs", RustModuleNodeKind::Inline),
            node(&["parent", "sibling"], "src/parent/sibling.rs", RustModuleNodeKind::External),
        ]);
        let report = resolve_rust_import_module_candidates(&[parsed], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::ModulePrefixForMember);
        assert_eq!(report.imports[0].target_module_path, Some(vec!["parent".to_owned(), "inner".to_owned()]));
        assert_eq!(report.imports[1].status, RustImportResolutionStatus::ModuleTarget);
        assert_eq!(report.imports[1].target_module_path, Some(vec!["parent".to_owned(), "sibling".to_owned()]));
    }

    #[test]
    fn preserves_ambiguous_module_paths() {
        let id = WorkspaceId::new();
        let parsed = parse_result("src/lib.rs", vec![import(&["crate", "shared"], &[])], Vec::new(), id);
        let graph = make_graph(id, vec![
            node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot),
            node(&["shared"], "src/shared.rs", RustModuleNodeKind::External),
            node(&["shared"], "src/shared/mod.rs", RustModuleNodeKind::External),
        ]);
        let report = resolve_rust_import_module_candidates(&[parsed], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::AmbiguousModuleTarget);
    }

    #[test]
    fn leaves_bare_external_paths_unresolved() {
        let id = WorkspaceId::new();
        let parsed = parse_result("src/lib.rs", vec![import(&["serde", "Serialize"], &[])], Vec::new(), id);
        let graph = make_graph(id, vec![node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot)]);
        let report = resolve_rust_import_module_candidates(&[parsed], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::UnsupportedUnqualifiedPath);
    }

    #[test]
    fn rejects_invalid_import_path_segments() {
        let id = WorkspaceId::new();
        let parsed = parse_result(
            "src/lib.rs",
            vec![import(&["crate", "..", "outside"], &[])],
            Vec::new(),
            id,
        );
        let graph = make_graph(id, vec![node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot)]);
        let report = resolve_rust_import_module_candidates(&[parsed], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::InvalidImportPath);
    }

    #[test]
    fn reports_invalid_super_traversal_and_workspace_mismatch() {
        let id = WorkspaceId::new();
        let parsed = parse_result("src/lib.rs", vec![import(&["super", "Thing"], &[])], Vec::new(), id);
        let graph = make_graph(id, vec![node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot)]);
        let report = resolve_rust_import_module_candidates(&[parsed.clone()], &graph).expect("imports");
        assert_eq!(report.imports[0].status, RustImportResolutionStatus::InvalidSuperTraversal);

        let other = make_graph(WorkspaceId::new(), vec![node(&[], "src/lib.rs", RustModuleNodeKind::CrateRoot)]);
        assert!(matches!(
            resolve_rust_import_module_candidates(&[parsed], &other),
            Err(RustImportResolutionError::WorkspaceMismatch)
        ));
    }
}
