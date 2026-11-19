//! Crate-root-anchored Rust module graph projection.
//!
//! The graph is a conservative view over parser facts and workspace inventory.
//! It does not evaluate cfg, expand macros, interpret `#[path]`, or claim compiler
//! semantic resolution. Ambiguous and unavailable candidates remain explicit.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::{
    ParseResult, ParseStatus, SourceRange, WorkspaceId,
    classification::LanguageId,
    resolve_rust_module_candidates,
    rust_modules::{RustModuleResolutionError, RustModuleResolutionStatus},
    syntax::valid_relative_path,
};

/// Resource limits for building a crate-root-anchored module graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RustModuleGraphLimits {
    /// Maximum retained module nodes across all crate roots.
    pub max_nodes: usize,
    /// Maximum retained declaration edges across all crate roots.
    pub max_edges: usize,
    /// Maximum logical module-path depth below a crate root.
    pub max_depth: usize,
}

impl Default for RustModuleGraphLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_edges: 100_000,
            max_depth: 256,
        }
    }
}

/// Kind of source-backed module node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustModuleNodeKind {
    /// Host-supplied crate root.
    CrateRoot,
    /// Inline module whose body is in its containing source file.
    Inline,
    /// External module backed by a unique inventory file candidate.
    External,
}

/// One module node in the crate-root-specific module namespace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleNode {
    /// Workspace-relative crate-root source path.
    pub crate_root: PathBuf,
    /// Logical module path relative to the crate root.
    pub module_path: Vec<String>,
    /// Source file containing this module's body.
    pub source_path: PathBuf,
    /// Module declaration category.
    pub kind: RustModuleNodeKind,
}

/// Outcome for one module declaration edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustModuleEdgeStatus {
    /// Inline module body is present in the containing file.
    Inline,
    /// Exactly one candidate exists and has a Rust parse result.
    UniqueCandidateParsed,
    /// Exactly one inventory candidate exists but no Rust parse result is available.
    UniqueCandidateUnparsed,
    /// Both conventional file layouts exist.
    AmbiguousCandidates,
    /// No conventional file candidate exists.
    NoCandidate,
    /// Attribute interpretation is required (for example cfg or path).
    SkippedAttributedDeclaration,
    /// Declaration is nested beneath an inline module whose cfg/path is unknown.
    SkippedUncertainParentScope,
    /// Candidate exists but traversal stopped at the configured depth limit.
    DepthLimit,
}

/// Source-backed declaration connecting a parent and child module path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleEdge {
    /// Workspace-relative crate-root source path.
    pub crate_root: PathBuf,
    /// Logical parent module path relative to the crate root.
    pub parent_module_path: Vec<String>,
    /// Logical child module path relative to the crate root.
    pub child_module_path: Vec<String>,
    /// File containing the declaration.
    pub declaration_source: PathBuf,
    /// Exact declaration source range.
    pub range: SourceRange,
    /// Existing workspace-relative file candidates, if any.
    pub candidate_paths: Vec<PathBuf>,
    /// Conservative resolution outcome.
    pub status: RustModuleEdgeStatus,
}

/// Whether graph construction exhausted its configured output bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RustModuleGraphStatus {
    /// All reachable, unambiguous module declarations were visited.
    Complete,
    /// Some parse results were recovered, limited, or failed.
    Partial,
    /// A node or edge limit stopped graph construction.
    Limited,
}

/// Crate-root-anchored module graph for one workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustModuleGraph {
    /// Host-managed workspace identity.
    pub workspace_id: WorkspaceId,
    /// Crate-root source paths supplied by the host.
    pub crate_roots: Vec<PathBuf>,
    /// Deterministically ordered module nodes.
    pub nodes: Vec<RustModuleNode>,
    /// Deterministically ordered declaration edges.
    pub edges: Vec<RustModuleEdge>,
    /// Completeness of this bounded projection.
    pub status: RustModuleGraphStatus,
}

/// Errors in crate-root-anchored module graph construction.
#[derive(Debug, thiserror::Error)]
pub enum RustModuleGraphError {
    /// Candidate resolution failed.
    #[error(transparent)]
    Resolution(#[from] RustModuleResolutionError),
    /// No crate roots were supplied.
    #[error("at least one crate root must be supplied")]
    NoCrateRoots,
    /// A crate root is not present in the source parse results.
    #[error("crate root has no parse result: {0}")]
    MissingCrateRoot(PathBuf),
    /// A crate root is absent from the supplied workspace inventory.
    #[error("crate root is absent from the workspace inventory: {0}")]
    CrateRootNotInInventory(PathBuf),
    /// A crate root does not have Rust language facts.
    #[error("crate root is not a Rust source file: {0}")]
    NonRustCrateRoot(PathBuf),
    /// Multiple parse results describe the same relative path.
    #[error("duplicate parse result path: {0}")]
    DuplicateParsePath(PathBuf),
    /// A module name or scope segment is not a safe identifier segment.
    #[error("invalid Rust module identifier in syntax facts")]
    InvalidModuleName,
}

/// Build a bounded module graph anchored at explicit host-supplied crate roots.
///
/// Each crate root receives its own logical module namespace, even when two
/// roots share source files. Only unique conventional file candidates are
/// traversed. Ambiguous, missing, and attributed declarations remain edges
/// with explicit status and are never promoted to resolved module links.
///
/// The input inventory and parse results are data only. This function does not
/// read the filesystem, inspect Cargo metadata, execute rustc, or run project
/// code. The host must supply the actual crate roots from trusted context.
pub fn build_rust_module_graph(
    parse_results: &[ParseResult],
    inventory_paths: &[PathBuf],
    crate_roots: &[PathBuf],
    limits: RustModuleGraphLimits,
) -> Result<RustModuleGraph, RustModuleGraphError> {
    let first = parse_results
        .first()
        .ok_or(RustModuleGraphError::Resolution(
            RustModuleResolutionError::EmptyInput,
        ))?;
    let workspace_id = first.workspace_id;
    if parse_results
        .iter()
        .any(|result| result.workspace_id != workspace_id)
    {
        return Err(RustModuleGraphError::Resolution(
            RustModuleResolutionError::MixedWorkspace,
        ));
    }
    if crate_roots.is_empty() {
        return Err(RustModuleGraphError::NoCrateRoots);
    }

    let mut by_path = HashMap::new();
    for result in parse_results {
        if !valid_relative_path(&result.relative_path) {
            return Err(RustModuleGraphError::Resolution(
                RustModuleResolutionError::InvalidPath,
            ));
        }
        if by_path
            .insert(result.relative_path.clone(), result)
            .is_some()
        {
            return Err(RustModuleGraphError::DuplicateParsePath(
                result.relative_path.clone(),
            ));
        }
    }

    let mut roots = crate_roots.to_vec();
    roots.sort();
    roots.dedup();
    for root in &roots {
        if !inventory_paths.contains(root) {
            return Err(RustModuleGraphError::CrateRootNotInInventory(root.clone()));
        }
        let result = by_path
            .get(root)
            .ok_or_else(|| RustModuleGraphError::MissingCrateRoot(root.clone()))?;
        if result.language != LanguageId::Rust {
            return Err(RustModuleGraphError::NonRustCrateRoot(root.clone()));
        }
    }

    let mut graph = RustModuleGraph {
        workspace_id,
        crate_roots: roots.clone(),
        nodes: Vec::new(),
        edges: Vec::new(),
        status: RustModuleGraphStatus::Complete,
    };
    let mut node_keys = HashSet::new();
    let mut visited = HashSet::new();

    for root in &roots {
        if graph.nodes.len() >= limits.max_nodes {
            graph.status = RustModuleGraphStatus::Limited;
            break;
        }
        graph.nodes.push(RustModuleNode {
            crate_root: root.clone(),
            module_path: Vec::new(),
            source_path: root.clone(),
            kind: RustModuleNodeKind::CrateRoot,
        });
        node_keys.insert((root.clone(), Vec::<String>::new(), root.clone()));

        let mut queue = VecDeque::new();
        queue.push_back((root.clone(), Vec::<String>::new()));

        while let Some((source_path, current_module_path)) = queue.pop_front() {
            if !visited.insert((
                root.clone(),
                source_path.clone(),
                current_module_path.clone(),
            )) {
                continue;
            }
            let Some(parsed) = by_path.get(&source_path) else {
                graph.status = RustModuleGraphStatus::Partial;
                continue;
            };
            if parsed.status != ParseStatus::Complete
                && graph.status == RustModuleGraphStatus::Complete
            {
                graph.status = RustModuleGraphStatus::Partial;
            }
            let uncertain_inline_scopes: Vec<Vec<String>> = parsed
                .modules
                .iter()
                .filter(|fact| fact.is_inline && fact.has_resolution_attributes)
                .map(|fact| {
                    let mut scope = fact.scope_path.clone();
                    scope.push(fact.name.clone());
                    scope
                })
                .collect();
            // Resolve only the current reachable file, not every file in the
            // workspace. This keeps candidate projection proportional to the
            // bounded graph traversal and preserves root-specific path rules.
            let candidate_report = resolve_rust_module_candidates(
                std::slice::from_ref(*parsed),
                inventory_paths,
                std::slice::from_ref(root),
            )?;
            let candidates_by_range: HashMap<_, _> = candidate_report
                .modules
                .iter()
                .map(|candidate| {
                    (
                        (candidate.range.start_byte, candidate.range.end_byte),
                        candidate,
                    )
                })
                .collect();

            for fact in &parsed.modules {
                if !valid_module_segment(&fact.name)
                    || fact
                        .scope_path
                        .iter()
                        .any(|segment| !valid_module_segment(segment))
                {
                    return Err(RustModuleGraphError::InvalidModuleName);
                }
                let mut parent_path = current_module_path.clone();
                parent_path.extend(fact.scope_path.iter().cloned());
                let mut child_path = parent_path.clone();
                child_path.push(fact.name.clone());

                if uncertain_inline_scopes
                    .iter()
                    .any(|scope| fact.scope_path.starts_with(scope))
                {
                    if graph.status == RustModuleGraphStatus::Complete {
                        graph.status = RustModuleGraphStatus::Partial;
                    }
                    if graph.edges.len() >= limits.max_edges {
                        graph.status = RustModuleGraphStatus::Limited;
                        break;
                    }
                    graph.edges.push(RustModuleEdge {
                        crate_root: root.clone(),
                        parent_module_path: parent_path,
                        child_module_path: child_path,
                        declaration_source: source_path.clone(),
                        range: fact.range,
                        candidate_paths: Vec::new(),
                        status: RustModuleEdgeStatus::SkippedUncertainParentScope,
                    });
                    continue;
                }

                if graph.edges.len() >= limits.max_edges {
                    graph.status = RustModuleGraphStatus::Limited;
                    break;
                }

                if fact.has_resolution_attributes {
                    if graph.status == RustModuleGraphStatus::Complete {
                        graph.status = RustModuleGraphStatus::Partial;
                    }
                    if fact.is_inline {
                        graph.edges.push(RustModuleEdge {
                            crate_root: root.clone(),
                            parent_module_path: parent_path,
                            child_module_path: child_path,
                            declaration_source: source_path.clone(),
                            range: fact.range,
                            candidate_paths: Vec::new(),
                            status: RustModuleEdgeStatus::SkippedAttributedDeclaration,
                        });
                        continue;
                    }
                }

                if fact.is_inline {
                    if child_path.len() > limits.max_depth {
                        graph.edges.push(RustModuleEdge {
                            crate_root: root.clone(),
                            parent_module_path: parent_path,
                            child_module_path: child_path,
                            declaration_source: source_path.clone(),
                            range: fact.range,
                            candidate_paths: Vec::new(),
                            status: RustModuleEdgeStatus::DepthLimit,
                        });
                        continue;
                    }
                    let key = (root.clone(), child_path.clone(), source_path.clone());
                    if !node_keys.contains(&key) && graph.nodes.len() >= limits.max_nodes {
                        graph.status = RustModuleGraphStatus::Limited;
                        break;
                    }
                    graph.edges.push(RustModuleEdge {
                        crate_root: root.clone(),
                        parent_module_path: parent_path,
                        child_module_path: child_path.clone(),
                        declaration_source: source_path.clone(),
                        range: fact.range,
                        candidate_paths: Vec::new(),
                        status: RustModuleEdgeStatus::Inline,
                    });
                    if node_keys.insert(key) {
                        graph.nodes.push(RustModuleNode {
                            crate_root: root.clone(),
                            module_path: child_path,
                            source_path: source_path.clone(),
                            kind: RustModuleNodeKind::Inline,
                        });
                    }
                    continue;
                }

                let candidate =
                    candidates_by_range.get(&(fact.range.start_byte, fact.range.end_byte));
                let Some(candidate) = candidate else {
                    continue;
                };

                let mut edge_status = match candidate.status {
                    RustModuleResolutionStatus::UniqueCandidate => {
                        RustModuleEdgeStatus::UniqueCandidateUnparsed
                    }
                    RustModuleResolutionStatus::AmbiguousCandidates => {
                        RustModuleEdgeStatus::AmbiguousCandidates
                    }
                    RustModuleResolutionStatus::NoCandidate => RustModuleEdgeStatus::NoCandidate,
                    RustModuleResolutionStatus::SkippedAttributedDeclaration => {
                        RustModuleEdgeStatus::SkippedAttributedDeclaration
                    }
                };
                let mut unique_target = None;
                if candidate.status == RustModuleResolutionStatus::UniqueCandidate
                    && let Some(target) = candidate.existing_candidates.first()
                {
                    if let Some(target_parse) = by_path.get(target) {
                        if target_parse.language == LanguageId::Rust
                            && target_parse.status != ParseStatus::Failed
                        {
                            if child_path.len() > limits.max_depth {
                                edge_status = RustModuleEdgeStatus::DepthLimit;
                            } else {
                                edge_status = RustModuleEdgeStatus::UniqueCandidateParsed;
                                unique_target = Some(target.clone());
                            }
                        } else if graph.status == RustModuleGraphStatus::Complete {
                            graph.status = RustModuleGraphStatus::Partial;
                        }
                    } else if graph.status == RustModuleGraphStatus::Complete {
                        graph.status = RustModuleGraphStatus::Partial;
                    }
                }

                if let Some(target) = unique_target.as_ref() {
                    let key = (root.clone(), child_path.clone(), target.clone());
                    if !node_keys.contains(&key) && graph.nodes.len() >= limits.max_nodes {
                        graph.status = RustModuleGraphStatus::Limited;
                        break;
                    }
                }
                graph.edges.push(RustModuleEdge {
                    crate_root: root.clone(),
                    parent_module_path: parent_path,
                    child_module_path: child_path.clone(),
                    declaration_source: source_path.clone(),
                    range: fact.range,
                    candidate_paths: if candidate.status
                        == RustModuleResolutionStatus::SkippedAttributedDeclaration
                    {
                        Vec::new()
                    } else {
                        candidate.existing_candidates.clone()
                    },
                    status: edge_status,
                });

                if let Some(target) = unique_target {
                    let key = (root.clone(), child_path.clone(), target.clone());
                    if node_keys.insert(key) {
                        graph.nodes.push(RustModuleNode {
                            crate_root: root.clone(),
                            module_path: child_path.clone(),
                            source_path: target.clone(),
                            kind: RustModuleNodeKind::External,
                        });
                    }
                    queue.push_back((target, child_path));
                }
            }

            if graph.status == RustModuleGraphStatus::Limited {
                break;
            }
        }
        if graph.status == RustModuleGraphStatus::Limited {
            break;
        }
    }

    graph.nodes.sort_unstable_by(|left, right| {
        (
            &left.crate_root,
            &left.module_path,
            &left.source_path,
            left.kind as u8,
        )
            .cmp(&(
                &right.crate_root,
                &right.module_path,
                &right.source_path,
                right.kind as u8,
            ))
    });
    graph.edges.sort_unstable_by(|left, right| {
        (
            &left.crate_root,
            &left.declaration_source,
            left.range,
            &left.child_module_path,
        )
            .cmp(&(
                &right.crate_root,
                &right.declaration_source,
                right.range,
                &right.child_module_path,
            ))
    });

    Ok(graph)
}

fn valid_module_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.chars().any(|character| {
            matches!(
                character,
                '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\0'
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParseStatus, RustModuleFact, SyntaxFact, SyntaxFactKind};

    fn parsed(path: &str, modules: Vec<RustModuleFact>, workspace_id: WorkspaceId) -> ParseResult {
        ParseResult {
            workspace_id,
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
                scope_path: Vec::new(),
                is_module_scope: true,
                visibility: None,
                range: SourceRange {
                    start_byte: 0,
                    end_byte: 1,
                },
            }],
            imports: Vec::new(),
            modules,
            diagnostics: Vec::new(),
        }
    }

    fn module(name: &str, scope: &[&str], inline: bool) -> RustModuleFact {
        RustModuleFact {
            scope_path: scope.iter().map(|part| (*part).to_owned()).collect(),
            name: name.to_owned(),
            is_inline: inline,
            has_resolution_attributes: false,
            range: SourceRange {
                start_byte: 10,
                end_byte: 20,
            },
        }
    }

    #[test]
    fn traverses_nested_external_modules_and_inline_scopes() {
        let id = WorkspaceId::new();
        let results = vec![
            parsed("src/lib.rs", vec![module("outer", &[], false)], id),
            parsed(
                "src/outer.rs",
                vec![
                    module("inner", &[], true),
                    module("leaf", &["inner"], false),
                ],
                id,
            ),
            parsed("src/outer/inner/leaf.rs", Vec::new(), id),
        ];
        let graph = build_rust_module_graph(
            &results,
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/outer.rs"),
                PathBuf::from("src/outer/inner/leaf.rs"),
            ],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        assert_eq!(graph.status, RustModuleGraphStatus::Complete);
        assert!(graph.nodes.iter().any(|node| node.module_path
            == vec!["outer".to_owned(), "inner".to_owned(), "leaf".to_owned()]));
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.status == RustModuleEdgeStatus::Inline)
        );
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| edge.status == RustModuleEdgeStatus::UniqueCandidateParsed)
        );
    }

    #[test]
    fn keeps_crate_root_namespaces_separate_for_shared_source_files() {
        let id = WorkspaceId::new();
        let results = vec![
            parsed("src/lib.rs", vec![module("shared", &[], false)], id),
            parsed("src/main.rs", vec![module("shared", &[], false)], id),
            parsed("src/shared.rs", Vec::new(), id),
        ];
        let graph = build_rust_module_graph(
            &results,
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/main.rs"),
                PathBuf::from("src/shared.rs"),
            ],
            &[PathBuf::from("src/lib.rs"), PathBuf::from("src/main.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        let shared_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| node.module_path == vec!["shared".to_owned()])
            .collect();
        assert_eq!(shared_nodes.len(), 2);
        assert_ne!(shared_nodes[0].crate_root, shared_nodes[1].crate_root);
    }

    #[test]
    fn applies_file_directory_rules_per_crate_root_context() {
        let id = WorkspaceId::new();
        let results = vec![
            parsed("src/lib.rs", vec![module("foo", &[], false)], id),
            parsed("src/foo.rs", vec![module("child", &[], false)], id),
            parsed("src/foo/child.rs", Vec::new(), id),
            parsed("src/child.rs", Vec::new(), id),
        ];
        let graph = build_rust_module_graph(
            &results,
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/foo.rs"),
                PathBuf::from("src/foo/child.rs"),
                PathBuf::from("src/child.rs"),
            ],
            &[PathBuf::from("src/lib.rs"), PathBuf::from("src/foo.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("multi-root module graph");

        assert!(graph.nodes.iter().any(|node| {
            node.crate_root == std::path::Path::new("src/lib.rs")
                && node.module_path == vec!["foo".to_owned(), "child".to_owned()]
                && node.source_path == std::path::Path::new("src/foo/child.rs")
        }));
        assert!(graph.nodes.iter().any(|node| {
            node.crate_root == std::path::Path::new("src/foo.rs")
                && node.module_path == vec!["child".to_owned()]
                && node.source_path == std::path::Path::new("src/child.rs")
        }));
    }

    #[test]
    fn preserves_ambiguous_candidates_without_traversing_them() {
        let id = WorkspaceId::new();
        let results = vec![parsed("src/lib.rs", vec![module("model", &[], false)], id)];
        let graph = build_rust_module_graph(
            &results,
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/model.rs"),
                PathBuf::from("src/model/mod.rs"),
            ],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        assert_eq!(
            graph.edges[0].status,
            RustModuleEdgeStatus::AmbiguousCandidates
        );
        assert_eq!(graph.edges[0].candidate_paths.len(), 2);
    }

    #[test]
    fn does_not_assume_attributed_inline_modules_are_active() {
        let id = WorkspaceId::new();
        let mut optional = module("optional", &[], true);
        optional.has_resolution_attributes = true;
        let graph = build_rust_module_graph(
            &[parsed("src/lib.rs", vec![optional], id)],
            &[PathBuf::from("src/lib.rs")],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        assert_eq!(graph.status, RustModuleGraphStatus::Partial);
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(
            graph.edges[0].status,
            RustModuleEdgeStatus::SkippedAttributedDeclaration
        );
    }

    #[test]
    fn does_not_publish_conventional_candidates_for_path_or_cfg_modules() {
        let id = WorkspaceId::new();
        let mut optional = module("optional", &[], false);
        optional.has_resolution_attributes = true;
        let graph = build_rust_module_graph(
            &[parsed("src/lib.rs", vec![optional], id)],
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/optional.rs"),
            ],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        assert_eq!(
            graph.edges[0].status,
            RustModuleEdgeStatus::SkippedAttributedDeclaration
        );
        assert!(graph.edges[0].candidate_paths.is_empty());
    }

    #[test]
    fn does_not_traverse_children_of_cfg_gated_inline_modules() {
        let id = WorkspaceId::new();
        let mut optional = module("optional", &[], true);
        optional.has_resolution_attributes = true;
        let child = module("child", &["optional"], false);
        let results = vec![
            parsed("src/lib.rs", vec![optional, child], id),
            parsed("src/optional/child.rs", Vec::new(), id),
        ];
        let graph = build_rust_module_graph(
            &results,
            &[
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/optional/child.rs"),
            ],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("module graph");
        assert_eq!(graph.status, RustModuleGraphStatus::Partial);
        assert_eq!(graph.nodes.len(), 1);
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| { edge.status == RustModuleEdgeStatus::SkippedUncertainParentScope })
        );
    }

    #[test]
    fn reports_partial_when_candidate_file_was_not_parsed() {
        let id = WorkspaceId::new();
        let results = vec![parsed("src/lib.rs", vec![module("child", &[], false)], id)];
        let graph = build_rust_module_graph(
            &results,
            &[PathBuf::from("src/lib.rs"), PathBuf::from("src/child.rs")],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits::default(),
        )
        .expect("partial module graph");
        assert_eq!(graph.status, RustModuleGraphStatus::Partial);
        assert_eq!(
            graph.edges[0].status,
            RustModuleEdgeStatus::UniqueCandidateUnparsed
        );
    }

    #[test]
    fn enforces_output_limits() {
        let id = WorkspaceId::new();
        let results = vec![parsed("src/lib.rs", vec![module("child", &[], true)], id)];
        let graph = build_rust_module_graph(
            &results,
            &[PathBuf::from("src/lib.rs")],
            &[PathBuf::from("src/lib.rs")],
            RustModuleGraphLimits {
                max_nodes: 1,
                max_edges: 10,
                max_depth: 10,
            },
        )
        .expect("limited graph");
        assert_eq!(graph.status, RustModuleGraphStatus::Limited);
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.edges.is_empty());
    }
}
