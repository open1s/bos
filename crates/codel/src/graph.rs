//! Evidence-backed candidate graph and bounded graph queries.
//!
//! The graph is built from [`SymbolCandidateReport`]. It is deliberately a
//! candidate graph, not a compiler-resolved call graph: a unique same-name
//! match is [`EvidenceClass::Inferred`], a multi-target match is
//! [`EvidenceClass::Ambiguous`], and containment is
//! [`EvidenceClass::Extracted`]. Every call/import edge retains the source
//! range, spelling, and candidate state that produced it.
//!
//! Queries are deterministic and bounded: a label query returns ranked nodes,
//! `explain` lists neighbours, `path` runs breadth-first search, and
//! `affected` walks reverse call/import edges to find dependents. Nothing here
//! reads the filesystem or executes project code.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::WorkspaceId;
use crate::language_resolver::ImportEdge;
use crate::model::tokenize;
use crate::resolution::{
    CandidateState, ReferenceCandidates, ReferenceKind, SymbolCandidateReport, SymbolId, SymbolKind,
};
use crate::symbol_resolver::ScopedSymbolEdge;
use crate::syntax::{RelationFact, RelationKind, SourceRange};

/// Maximum number of nodes a label query may return.
pub const MAX_QUERY_RESULTS: usize = 100;

/// Maximum number of neighbours `explain` may return per direction.
pub const MAX_EXPLAIN_NEIGHBORS: usize = 200;

/// Maximum accepted path depth.
pub const MAX_PATH_DEPTH: usize = 64;

/// Maximum nodes visited by one path search.
pub const MAX_PATH_NODES: usize = 100_000;

/// Maximum nodes returned by one `affected` walk.
pub const MAX_AFFECTED_NODES: usize = 10_000;

/// Default token budget for one graph answer.
pub const DEFAULT_ANSWER_TOKENS: usize = 2_000;

/// Maximum accepted token budget for one graph answer.
pub const MAX_ANSWER_TOKENS: usize = 32_000;

/// Conservative bytes-per-token divisor used for budgets.
pub const BYTES_PER_TOKEN: usize = 4;

/// Stable identity of one graph node.
///
/// The string is opaque to callers but stable for one workspace identity: file
/// nodes use the workspace-relative path, symbol nodes add the declaration
/// range and kind. It is not a filesystem path.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    /// Identity of one workspace-relative file.
    #[must_use]
    pub fn file(path: &Path) -> Self {
        Self(format!("file:{}", path.display()))
    }

    /// Identity of one declaration.
    #[must_use]
    pub fn symbol(id: &SymbolId) -> Self {
        Self(format!(
            "symbol:{}:{}:{}:{}",
            id.path.display(),
            id.start_byte,
            id.end_byte,
            id.kind.as_str()
        ))
    }

    /// Opaque stable spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a graph node represents.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// One workspace-relative source file.
    File,
    /// One declaration.
    Symbol(SymbolKind),
}

impl NodeKind {
    /// Whether this is a declaration node.
    #[must_use]
    pub const fn is_symbol(self) -> bool {
        matches!(self, Self::Symbol(_))
    }
}

/// One graph node with its source identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphNode {
    /// Stable identity.
    pub id: NodeId,
    /// Node category.
    pub kind: NodeKind,
    /// Declared name, or the relative path for a file node.
    pub name: String,
    /// Workspace-relative source path.
    pub path: PathBuf,
    /// Declaration range; absent for file nodes.
    pub range: Option<SourceRange>,
}

/// Why an edge exists.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A file contains a declaration.
    Contains,
    /// A call-like reference points at candidate declarations.
    Calls,
    /// An import-like reference points at candidate declarations.
    Imports,
    /// A class or interface extends another type.
    Inherits,
    /// A class or impl implements a trait or interface.
    Implements,
    /// A declaration names another type without claiming resolution.
    ReferencesType,
    /// A field read.
    Accesses,
    /// A field write.
    Writes,
    /// A reference imported from a SCIP index.
    Scip,
}

impl EdgeKind {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Calls => "calls",
            Self::Imports => "imports",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::ReferencesType => "references_type",
            Self::Accesses => "accesses",
            Self::Writes => "writes",
            Self::Scip => "scip",
        }
    }
}

/// How strongly the source establishes an edge.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceClass {
    /// Read directly from the source structure; no resolution was involved.
    Extracted,
    /// A unique candidate produced by a conservative resolver.
    Inferred,
    /// Several candidate targets remain possible.
    Ambiguous,
}

impl EvidenceClass {
    /// Stable machine-readable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "extracted",
            Self::Inferred => "inferred",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// The reference evidence that produced an edge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReferenceEvidence {
    /// File containing the reference.
    pub path: PathBuf,
    /// Reference byte range.
    pub range: SourceRange,
    /// Source spelling of the target.
    pub spelling: String,
    /// Candidate outcome recorded by the resolver.
    pub state: CandidateState,
}

/// One directed graph edge.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphEdge {
    /// Source node.
    pub from: NodeId,
    /// Target node.
    pub to: NodeId,
    /// Relationship category.
    pub kind: EdgeKind,
    /// Evidence class for the weakest contributing reference.
    pub class: EvidenceClass,
    /// Source references that produced or support this edge.
    pub references: Vec<ReferenceEvidence>,
}

/// Aggregate facts about the graph's completeness.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct GraphCoverage {
    /// Declaration nodes.
    pub symbols: usize,
    /// File nodes.
    pub files: usize,
    /// Reference records considered.
    pub references: usize,
    /// Edges emitted.
    pub edges: usize,
    /// Edges backed by a unique candidate.
    pub inferred_edges: usize,
    /// Edges with more than one candidate target.
    pub ambiguous_edges: usize,
    /// References with no same-name declaration.
    pub unresolved_references: u64,
    /// References outside the supported syntactic subset.
    pub unsupported_references: u64,
    /// Relationship facts considered.
    pub relations: usize,
    /// Relationship facts without a source or target candidate.
    pub unresolved_relations: u64,
}

impl GraphCoverage {
    /// Whether no ambiguity or unresolved reference remains.
    ///
    /// Inferred edges are still candidates: this returns true for a graph whose
    /// only uncertainty is the conservative "unique name match" class.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.ambiguous_edges == 0
            && self.unresolved_references == 0
            && self.unsupported_references == 0
            && self.unresolved_relations == 0
    }
}

/// The graph could not be built consistently.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GraphError {
    /// Input reports mix workspace identities.
    #[error("symbol candidate report mixes workspace identities")]
    MixedWorkspaces,
    /// A declaration appeared twice with the same identity.
    #[error("duplicate symbol node '{0}'")]
    DuplicateSymbol(String),
    /// A reference candidate names a declaration that is not in the catalog.
    #[error("reference candidate targets unknown symbol '{0}'")]
    UnknownCandidate(String),
    /// A snapshot carries a workspace identity that is not a UUID.
    #[error("snapshot workspace identity '{0}' is not a UUID")]
    InvalidWorkspaceId(String),
    /// Snapshot counts disagree with the nodes and edges it carries.
    #[error("snapshot coverage does not match its nodes/edges: {0}")]
    CoverageMismatch(String),
}

/// One candidate graph over a workspace snapshot.
#[derive(Clone, Debug)]
pub struct CodeGraph {
    workspace_id: WorkspaceId,
    nodes: BTreeMap<NodeId, GraphNode>,
    edges: Vec<GraphEdge>,
    outgoing: BTreeMap<NodeId, Vec<usize>>,
    incoming: BTreeMap<NodeId, Vec<usize>>,
    coverage: GraphCoverage,
}

/// Serializable snapshot of one graph.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphSnapshot {
    /// Workspace identity as a UUID string.
    pub workspace_id: String,
    /// Nodes in identity order.
    pub nodes: Vec<GraphNode>,
    /// Edges in deterministic order.
    pub edges: Vec<GraphEdge>,
    /// Coverage recorded when the graph was built.
    pub coverage: GraphCoverage,
    /// Optional source-inventory digest used by the durable store.
    #[serde(default)]
    pub source_digest: String,
    /// Optional enrichment metadata (types, SCIP digest, ...).
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl CodeGraph {
    /// Build a candidate graph from one symbol-candidate report.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::MixedWorkspaces`] for mixed identities,
    /// [`GraphError::DuplicateSymbol`] for a repeated declaration identity,
    /// and [`GraphError::UnknownCandidate`] when an edge targets a symbol that
    /// is absent from the catalog.
    pub fn build(report: &SymbolCandidateReport) -> Result<Self, GraphError> {
        Self::build_with_relations(report, &[])
    }

    /// Build a candidate graph with inheritance and type relationships.
    ///
    /// Each relation is a candidate: the source and target are matched by name,
    /// so a unique target is [`EvidenceClass::Inferred`] and multiple targets
    /// are [`EvidenceClass::Ambiguous`]. A relation without both endpoints is
    /// counted in [`GraphCoverage::unresolved_relations`].
    ///
    /// # Errors
    ///
    /// Returns the same validation errors as [`Self::build`].
    pub fn build_with_relations(
        report: &SymbolCandidateReport,
        relations: &[(PathBuf, RelationFact)],
    ) -> Result<Self, GraphError> {
        Self::build_internal(report, relations, &[], &[])
    }

    /// Build a candidate graph with resolved import edges.
    pub fn build_with_imports(
        report: &SymbolCandidateReport,
        relations: &[(PathBuf, RelationFact)],
        imports: &[ImportEdge],
    ) -> Result<Self, GraphError> {
        Self::build_with_symbols(report, relations, imports, &[])
    }

    /// Build a candidate graph with scoped symbol bindings from imports.
    pub fn build_with_symbols(
        report: &SymbolCandidateReport,
        relations: &[(PathBuf, RelationFact)],
        imports: &[ImportEdge],
        symbols: &[ScopedSymbolEdge],
    ) -> Result<Self, GraphError> {
        Self::build_internal(report, relations, imports, symbols)
    }

    fn build_internal(
        report: &SymbolCandidateReport,
        relations: &[(PathBuf, RelationFact)],
        imports: &[ImportEdge],
        symbol_edges: &[ScopedSymbolEdge],
    ) -> Result<Self, GraphError> {
        let mut coverage = GraphCoverage {
            symbols: report.symbols.len(),
            references: report.references.len(),
            relations: relations.len(),
            unresolved_references: report.unresolved_references,
            unsupported_references: report.unsupported_references,
            ..GraphCoverage::default()
        };

        let mut file_paths: BTreeSet<PathBuf> = report
            .symbols
            .iter()
            .map(|symbol| symbol.id.path.clone())
            .collect();
        file_paths.extend(
            report
                .references
                .iter()
                .map(|reference| reference.path.clone()),
        );
        for import in imports {
            file_paths.insert(import.from.clone());
            file_paths.insert(import.to.clone());
        }

        let mut nodes = BTreeMap::new();
        for path in &file_paths {
            let id = NodeId::file(path);
            nodes.insert(
                id.clone(),
                GraphNode {
                    id,
                    kind: NodeKind::File,
                    name: path.display().to_string(),
                    path: path.clone(),
                    range: None,
                },
            );
            coverage.files += 1;
        }

        let mut by_path: BTreeMap<PathBuf, Vec<(SourceRange, NodeId)>> = BTreeMap::new();
        let mut edge_map: BTreeMap<
            (NodeId, NodeId, EdgeKind),
            (EvidenceClass, Vec<ReferenceEvidence>),
        > = BTreeMap::new();

        for symbol in &report.symbols {
            if symbol.id.workspace_id != report.workspace_id {
                return Err(GraphError::MixedWorkspaces);
            }
            let id = NodeId::symbol(&symbol.id);
            if nodes.contains_key(&id) {
                return Err(GraphError::DuplicateSymbol(id.to_string()));
            }
            let range = SourceRange {
                start_byte: symbol.id.start_byte,
                end_byte: symbol.id.end_byte,
            };
            nodes.insert(
                id.clone(),
                GraphNode {
                    id: id.clone(),
                    kind: NodeKind::Symbol(symbol.id.kind),
                    name: symbol.name.clone(),
                    path: symbol.id.path.clone(),
                    range: Some(range),
                },
            );
            by_path
                .entry(symbol.id.path.clone())
                .or_default()
                .push((range, id.clone()));

            let file = NodeId::file(&symbol.id.path);
            insert_edge(
                &mut edge_map,
                file,
                id,
                EdgeKind::Contains,
                EvidenceClass::Extracted,
                None,
            );
        }
        for symbols in by_path.values_mut() {
            symbols.sort_by(|left, right| {
                left.0
                    .start_byte
                    .cmp(&right.0.start_byte)
                    .then_with(|| left.0.end_byte.cmp(&right.0.end_byte))
                    .then_with(|| left.1.cmp(&right.1))
            });
        }

        // Member index: a declaration contained by a larger declaration gets a
        // type/member containment edge. This is the first layer of method/field
        // resolution; receiver typing remains separate.
        for symbols in by_path.values() {
            let mut stack: Vec<(SourceRange, NodeId)> = Vec::new();
            for (range, id) in symbols {
                while stack
                    .last()
                    .is_some_and(|(parent, _)| parent.end_byte <= range.start_byte)
                {
                    stack.pop();
                }
                if let Some((_, parent)) = stack.last() {
                    insert_edge(
                        &mut edge_map,
                        parent.clone(),
                        id.clone(),
                        EdgeKind::Contains,
                        EvidenceClass::Extracted,
                        None,
                    );
                }
                stack.push((*range, id.clone()));
            }
        }

        for (path, relation) in relations {
            let source = nodes
                .values()
                .find(|node| {
                    node.kind.is_symbol()
                        && node.path.as_path() == path.as_path()
                        && node.name == relation.source_name
                })
                .map(|node| node.id.clone());
            let Some(source) = source else {
                coverage.unresolved_relations += 1;
                continue;
            };
            let targets: Vec<NodeId> = nodes
                .values()
                .filter(|node| node.kind.is_symbol() && node.name == relation.target_name)
                .map(|node| node.id.clone())
                .collect();
            if targets.is_empty() {
                coverage.unresolved_relations += 1;
                continue;
            }
            let class = if targets.len() == 1 {
                EvidenceClass::Inferred
            } else {
                EvidenceClass::Ambiguous
            };
            let state = if targets.len() == 1 {
                CandidateState::UniqueCandidate
            } else {
                CandidateState::AmbiguousCandidates
            };
            let kind = match relation.kind {
                RelationKind::Inherits => EdgeKind::Inherits,
                RelationKind::Implements => EdgeKind::Implements,
                RelationKind::ReferencesType => EdgeKind::ReferencesType,
            };
            for target in targets {
                insert_edge(
                    &mut edge_map,
                    source.clone(),
                    target,
                    kind,
                    class,
                    Some(ReferenceEvidence {
                        path: path.clone(),
                        range: relation.range,
                        spelling: relation.target_name.clone(),
                        state,
                    }),
                );
            }
        }

        for edge in symbol_edges {
            let from = NodeId::symbol(&edge.from);
            let to = NodeId::symbol(&edge.to);
            if !nodes.contains_key(&from) || !nodes.contains_key(&to) {
                continue;
            }
            let kind = match edge.kind {
                ReferenceKind::FunctionCall | ReferenceKind::MethodCall => EdgeKind::Calls,
                ReferenceKind::Import => EdgeKind::Imports,
                ReferenceKind::TypeReference => EdgeKind::ReferencesType,
                ReferenceKind::FieldAccess => EdgeKind::Accesses,
                ReferenceKind::FieldWrite => EdgeKind::Writes,
                ReferenceKind::ScipRead => EdgeKind::Accesses,
                ReferenceKind::ScipWrite => EdgeKind::Writes,
                ReferenceKind::Scip => EdgeKind::Scip,
            };
            insert_edge(
                &mut edge_map,
                from,
                to,
                kind,
                EvidenceClass::Inferred,
                Some(ReferenceEvidence {
                    path: edge.from.path.clone(),
                    range: edge.range,
                    spelling: edge.spelling.clone(),
                    state: CandidateState::UniqueCandidate,
                }),
            );
        }

        for import in imports {
            let from = NodeId::file(&import.from);
            let to = NodeId::file(&import.to);
            insert_edge(
                &mut edge_map,
                from,
                to,
                EdgeKind::Imports,
                EvidenceClass::Inferred,
                Some(ReferenceEvidence {
                    path: import.from.clone(),
                    range: import.range,
                    spelling: import.spelling.clone(),
                    state: CandidateState::UniqueCandidate,
                }),
            );
        }

        for reference in &report.references {
            let Some(class) = reference_class(reference) else {
                continue;
            };
            let source = enclosing_symbol(&by_path, reference)
                .unwrap_or_else(|| NodeId::file(&reference.path));
            let edge_kind = match reference.kind {
                ReferenceKind::Import => EdgeKind::Imports,
                ReferenceKind::TypeReference => EdgeKind::ReferencesType,
                ReferenceKind::FunctionCall | ReferenceKind::MethodCall => EdgeKind::Calls,
                ReferenceKind::FieldAccess => EdgeKind::Accesses,
                ReferenceKind::FieldWrite => EdgeKind::Writes,
                ReferenceKind::ScipRead => EdgeKind::Accesses,
                ReferenceKind::ScipWrite => EdgeKind::Writes,
                ReferenceKind::Scip => EdgeKind::Scip,
            };
            for candidate in &reference.candidates {
                if candidate.workspace_id != report.workspace_id {
                    return Err(GraphError::MixedWorkspaces);
                }
                let target = NodeId::symbol(candidate);
                if !nodes.contains_key(&target) {
                    return Err(GraphError::UnknownCandidate(target.to_string()));
                }
                insert_edge(
                    &mut edge_map,
                    source.clone(),
                    target,
                    edge_kind,
                    class,
                    Some(ReferenceEvidence {
                        path: reference.path.clone(),
                        range: SourceRange {
                            start_byte: reference.start_byte,
                            end_byte: reference.end_byte,
                        },
                        spelling: reference.spelling.clone(),
                        state: reference.state,
                    }),
                );
            }
        }

        let mut edges: Vec<GraphEdge> = edge_map
            .into_iter()
            .map(|((from, to, kind), (class, mut references))| {
                references.sort_by(|left, right| {
                    left.path
                        .cmp(&right.path)
                        .then_with(|| left.range.start_byte.cmp(&right.range.start_byte))
                        .then_with(|| left.range.end_byte.cmp(&right.range.end_byte))
                        .then_with(|| left.spelling.cmp(&right.spelling))
                });
                GraphEdge {
                    from,
                    to,
                    kind,
                    class,
                    references,
                }
            })
            .collect();
        edges.sort_by(|left, right| {
            left.from
                .cmp(&right.from)
                .then_with(|| left.to.cmp(&right.to))
                .then_with(|| left.kind.cmp(&right.kind))
        });

        let mut outgoing: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut incoming: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        for (index, edge) in edges.iter().enumerate() {
            outgoing.entry(edge.from.clone()).or_default().push(index);
            incoming.entry(edge.to.clone()).or_default().push(index);
            match edge.class {
                EvidenceClass::Extracted => {}
                EvidenceClass::Inferred => coverage.inferred_edges += 1,
                EvidenceClass::Ambiguous => coverage.ambiguous_edges += 1,
            }
        }
        coverage.edges = edges.len();

        Ok(Self {
            workspace_id: report.workspace_id,
            nodes,
            edges,
            outgoing,
            incoming,
            coverage,
        })
    }

    /// Copy the graph into a serializable snapshot.
    #[must_use]
    pub fn to_snapshot(&self) -> GraphSnapshot {
        GraphSnapshot {
            workspace_id: self.workspace_id.to_string(),
            nodes: self.nodes.values().cloned().collect(),
            edges: self.edges.clone(),
            coverage: self.coverage,
            source_digest: String::new(),
            metadata: BTreeMap::new(),
        }
    }

    /// Rebuild a graph from a validated snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::InvalidWorkspaceId`] for a malformed identity,
    /// [`GraphError::DuplicateSymbol`] for a repeated node id,
    /// [`GraphError::UnknownCandidate`] for an edge endpoint that is missing,
    /// and [`GraphError::CoverageMismatch`] when counts disagree.
    pub fn from_snapshot(snapshot: GraphSnapshot) -> Result<Self, GraphError> {
        let workspace_id: WorkspaceId = snapshot
            .workspace_id
            .parse()
            .map_err(|_| GraphError::InvalidWorkspaceId(snapshot.workspace_id.clone()))?;
        let mut nodes = BTreeMap::new();
        let mut symbols = 0usize;
        let mut files = 0usize;
        for node in snapshot.nodes {
            match node.kind {
                NodeKind::Symbol(_) => symbols += 1,
                NodeKind::File => files += 1,
            }
            if nodes.insert(node.id.clone(), node).is_some() {
                return Err(GraphError::DuplicateSymbol(
                    "duplicate node id in snapshot".to_owned(),
                ));
            }
        }

        let mut edges = snapshot.edges;
        edges.sort_by(|left, right| {
            left.from
                .cmp(&right.from)
                .then_with(|| left.to.cmp(&right.to))
                .then_with(|| left.kind.cmp(&right.kind))
        });
        let mut outgoing: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut incoming: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
        let mut inferred = 0usize;
        let mut ambiguous = 0usize;
        for (index, edge) in edges.iter().enumerate() {
            if !nodes.contains_key(&edge.from) {
                return Err(GraphError::UnknownCandidate(edge.from.to_string()));
            }
            if !nodes.contains_key(&edge.to) {
                return Err(GraphError::UnknownCandidate(edge.to.to_string()));
            }
            outgoing.entry(edge.from.clone()).or_default().push(index);
            incoming.entry(edge.to.clone()).or_default().push(index);
            match edge.class {
                EvidenceClass::Extracted => {}
                EvidenceClass::Inferred => inferred += 1,
                EvidenceClass::Ambiguous => ambiguous += 1,
            }
        }

        let coverage = snapshot.coverage;
        if coverage.symbols != symbols
            || coverage.files != files
            || coverage.edges != edges.len()
            || coverage.inferred_edges != inferred
            || coverage.ambiguous_edges != ambiguous
        {
            return Err(GraphError::CoverageMismatch(format!(
                "snapshot coverage does not match: symbols {symbols}/{}, files {files}/{}, edges {}/{}, inferred {inferred}/{}, ambiguous {ambiguous}/{}",
                coverage.symbols,
                coverage.files,
                edges.len(),
                coverage.edges,
                coverage.inferred_edges,
                coverage.ambiguous_edges
            )));
        }

        Ok(Self {
            workspace_id,
            nodes,
            edges,
            outgoing,
            incoming,
            coverage,
        })
    }

    /// Workspace identity this graph was built for.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Graph completeness and ambiguity counts.
    #[must_use]
    pub const fn coverage(&self) -> GraphCoverage {
        self.coverage
    }

    /// One node by identity.
    #[must_use]
    pub fn node(&self, id: &NodeId) -> Option<&GraphNode> {
        self.nodes.get(id)
    }

    /// All nodes in identity order.
    pub fn nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.values()
    }

    /// All edges in deterministic order.
    #[must_use]
    pub fn edges(&self) -> &[GraphEdge] {
        &self.edges
    }

    /// Rank nodes whose name or path matches the query words.
    ///
    /// Matching uses the same identifier tokenizer as retrieval; it is a label
    /// search, not a natural-language embedding query.
    #[must_use]
    pub fn query(&self, text: &str, limit: usize) -> Vec<QueryHit> {
        let terms = tokenize(text);
        if terms.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, MAX_QUERY_RESULTS);
        let exact = text.trim().to_ascii_lowercase();
        let mut hits = Vec::new();
        for node in self.nodes.values() {
            let name = node.name.to_ascii_lowercase();
            let name_tokens = tokenize(&node.name);
            let path_tokens = tokenize(&node.path.to_string_lossy());
            let mut score = 0u32;
            if !exact.is_empty() && name == exact {
                score += 10;
            }
            for term in &terms {
                if name_tokens.iter().any(|token| token == term) {
                    score += 3;
                }
                if path_tokens.iter().any(|token| token == term) {
                    score += 1;
                }
            }
            if score == 0 {
                continue;
            }
            let degree = self.outgoing.get(&node.id).map_or(0, Vec::len)
                + self.incoming.get(&node.id).map_or(0, Vec::len);
            hits.push(QueryHit {
                node: node.clone(),
                score,
                degree,
            });
        }
        hits.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.degree.cmp(&left.degree))
                .then_with(|| left.node.name.cmp(&right.node.name))
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        hits.truncate(limit);
        hits
    }

    /// Summarize one node and its direct neighbours.
    #[must_use]
    pub fn explain(&self, id: &NodeId, limit: usize) -> Option<Explanation> {
        let node = self.nodes.get(id)?.clone();
        let limit = limit.clamp(1, MAX_EXPLAIN_NEIGHBORS);
        let mut incoming = self.neighbors(id, Direction::Incoming);
        let mut outgoing = self.neighbors(id, Direction::Outgoing);
        let incoming_total = incoming.len();
        let outgoing_total = outgoing.len();
        incoming.truncate(limit);
        outgoing.truncate(limit);
        Some(Explanation {
            node,
            incoming,
            outgoing,
            truncated: incoming_total > limit || outgoing_total > limit,
        })
    }

    /// Shortest path between two nodes.
    ///
    /// The search follows edge direction unless [`PathOptions::undirected`] is
    /// set. Ties are broken by node identity so repeated runs return the same
    /// path.
    #[must_use]
    pub fn path(&self, from: &NodeId, to: &NodeId, options: PathOptions) -> Option<GraphPath> {
        let from_node = self.nodes.get(from)?.clone();
        self.nodes.get(to)?;
        if from == to {
            return Some(GraphPath {
                nodes: vec![from_node],
                edges: Vec::new(),
            });
        }

        let options = options.normalized();
        let mut depth: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut previous: BTreeMap<NodeId, (NodeId, usize)> = BTreeMap::new();
        let mut queue = VecDeque::new();
        depth.insert(from.clone(), 0);
        queue.push_back(from.clone());
        let mut visited = 0usize;

        while let Some(current) = queue.pop_front() {
            visited = visited.saturating_add(1);
            if visited > MAX_PATH_NODES {
                break;
            }
            let current_depth = depth.get(&current).copied().unwrap_or_default();
            if current_depth >= options.max_depth {
                continue;
            }
            let mut neighbors = self.path_neighbors(&current, options.undirected);
            neighbors
                .sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            neighbors.dedup_by(|left, right| left.0 == right.0);
            for (next, edge_index) in neighbors {
                if depth.contains_key(&next) {
                    continue;
                }
                depth.insert(next.clone(), current_depth + 1);
                previous.insert(next.clone(), (current.clone(), edge_index));
                if next == *to {
                    return self.reconstruct_path(from, to, &previous);
                }
                queue.push_back(next);
            }
        }
        None
    }

    /// Find nodes that depend on `id` through call/import edges.
    ///
    /// The result includes `id` at depth 0. Traversal follows edges in reverse,
    /// so a changed function lists its callers. Only call/import edges are
    /// followed; containment edges never make a file a dependent.
    #[must_use]
    pub fn affected(&self, id: &NodeId, options: AffectedOptions) -> Option<Affected> {
        let root = self.nodes.get(id)?.clone();
        let options = options.normalized();
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::new();
        let mut nodes = vec![AffectedNode {
            node: root.clone(),
            depth: 0,
        }];
        let mut edge_indices = BTreeSet::new();
        let mut truncated = false;
        seen.insert(id.clone());
        queue.push_back((id.clone(), 0usize));

        while let Some((current, current_depth)) = queue.pop_front() {
            let mut incoming = self.incoming.get(&current).cloned().unwrap_or_default();
            incoming.retain(|index| {
                let edge = &self.edges[*index];
                edge.kind != EdgeKind::Contains
                    && options
                        .kinds
                        .as_ref()
                        .is_none_or(|kinds| kinds.contains(&edge.kind))
            });
            if current_depth >= options.max_depth {
                // Only report truncation when the depth limit actually hides a
                // node that has not already been reached by a shorter path.
                truncated |= incoming
                    .iter()
                    .any(|index| !seen.contains(&self.edges[*index].from));
                continue;
            }
            incoming.sort_by(|left, right| {
                self.edges[*left]
                    .from
                    .cmp(&self.edges[*right].from)
                    .then_with(|| self.edges[*left].to.cmp(&self.edges[*right].to))
            });
            for index in incoming {
                edge_indices.insert(index);
                let source = self.edges[index].from.clone();
                if seen.insert(source.clone()) {
                    if nodes.len() >= MAX_AFFECTED_NODES {
                        truncated = true;
                        continue;
                    }
                    if let Some(node) = self.nodes.get(&source) {
                        nodes.push(AffectedNode {
                            node: node.clone(),
                            depth: current_depth + 1,
                        });
                        queue.push_back((source, current_depth + 1));
                    }
                }
            }
        }

        nodes.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        let edges = edge_indices
            .into_iter()
            .map(|index| self.edges[index].clone())
            .collect();
        Some(Affected {
            root,
            nodes,
            edges,
            truncated,
        })
    }

    /// Rank nodes by incident edge count.
    #[must_use]
    pub fn god_nodes(&self, limit: usize) -> Vec<GodNode> {
        let limit = limit.clamp(1, MAX_QUERY_RESULTS);
        let mut nodes: Vec<GodNode> = self
            .nodes
            .values()
            .map(|node| {
                let incoming = self.incoming.get(&node.id).map_or(0, Vec::len);
                let outgoing = self.outgoing.get(&node.id).map_or(0, Vec::len);
                GodNode {
                    node: node.clone(),
                    degree: incoming.saturating_add(outgoing),
                    incoming,
                    outgoing,
                }
            })
            .collect();
        nodes.sort_by(|left, right| {
            right
                .degree
                .cmp(&left.degree)
                .then_with(|| right.incoming.cmp(&left.incoming))
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        nodes.truncate(limit);
        nodes
    }

    /// Rank nodes by degree and trim to a token budget.
    #[must_use]
    pub fn god_nodes_budgeted(&self, limit: usize, budget: AnswerBudget) -> Budgeted<Vec<GodNode>> {
        let budget = budget.normalized();
        let nodes = self.god_nodes(limit);
        let mut kept = Vec::new();
        let mut tokens = 8usize;
        let mut truncated = false;
        for node in nodes {
            let cost = estimate_node(&node.node) + 24;
            if !kept.is_empty() && tokens.saturating_add(cost) > budget.max_tokens {
                truncated = true;
                break;
            }
            tokens = tokens.saturating_add(cost);
            kept.push(node);
        }
        Budgeted {
            value: kept,
            estimated_tokens: tokens,
            truncated,
        }
    }

    /// Rank nodes and trim the result to a token budget.
    #[must_use]
    pub fn query_budgeted(
        &self,
        text: &str,
        limit: usize,
        budget: AnswerBudget,
    ) -> Budgeted<Vec<QueryHit>> {
        let budget = budget.normalized();
        let hits = self.query(text, limit);
        let mut kept = Vec::new();
        let mut tokens = 8usize;
        let mut truncated = false;
        for hit in hits {
            let cost = estimate_hit(&hit);
            if !kept.is_empty() && tokens.saturating_add(cost) > budget.max_tokens {
                truncated = true;
                break;
            }
            tokens = tokens.saturating_add(cost);
            kept.push(hit);
        }
        Budgeted {
            value: kept,
            estimated_tokens: tokens,
            truncated,
        }
    }

    /// Explain one node and trim neighbours to a token budget.
    #[must_use]
    pub fn explain_budgeted(
        &self,
        id: &NodeId,
        limit: usize,
        budget: AnswerBudget,
    ) -> Option<Budgeted<Explanation>> {
        let budget = budget.normalized();
        let explanation = self.explain(id, limit)?;
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();
        let mut tokens = estimate_node(&explanation.node) + 16;
        let mut truncated = explanation.truncated;

        for neighbor in explanation.incoming {
            let cost = estimate_neighbor(&neighbor);
            if (!incoming.is_empty() || !outgoing.is_empty())
                && tokens.saturating_add(cost) > budget.max_tokens
            {
                truncated = true;
                break;
            }
            tokens = tokens.saturating_add(cost);
            incoming.push(neighbor);
        }
        if !truncated {
            for neighbor in explanation.outgoing {
                let cost = estimate_neighbor(&neighbor);
                if (!incoming.is_empty() || !outgoing.is_empty())
                    && tokens.saturating_add(cost) > budget.max_tokens
                {
                    truncated = true;
                    break;
                }
                tokens = tokens.saturating_add(cost);
                outgoing.push(neighbor);
            }
        }

        Some(Budgeted {
            value: Explanation {
                node: explanation.node,
                incoming,
                outgoing,
                truncated,
            },
            estimated_tokens: tokens,
            truncated,
        })
    }

    /// Shortest path, trimmed to a token budget.
    ///
    /// A trimmed path remains a connected prefix; the last retained edge still
    /// connects the last two nodes.
    #[must_use]
    pub fn path_budgeted(
        &self,
        from: &NodeId,
        to: &NodeId,
        options: PathOptions,
        budget: AnswerBudget,
    ) -> Option<Budgeted<GraphPath>> {
        let budget = budget.normalized();
        let path = self.path(from, to, options)?;
        let first = path.nodes.first()?.clone();
        let mut nodes = vec![first];
        let mut edges = Vec::new();
        let mut tokens = estimate_node(&nodes[0]);
        let mut truncated = false;

        for (index, edge) in path.edges.iter().enumerate() {
            let Some(next) = path.nodes.get(index + 1) else {
                break;
            };
            let cost = estimate_edge(edge) + estimate_node(next);
            if !edges.is_empty() && tokens.saturating_add(cost) > budget.max_tokens {
                truncated = true;
                break;
            }
            tokens = tokens.saturating_add(cost);
            edges.push(edge.clone());
            nodes.push(next.clone());
        }

        Some(Budgeted {
            value: GraphPath { nodes, edges },
            estimated_tokens: tokens,
            truncated,
        })
    }

    /// Reverse dependency walk, trimmed to a token budget.
    #[must_use]
    pub fn affected_budgeted(
        &self,
        id: &NodeId,
        options: AffectedOptions,
        budget: AnswerBudget,
    ) -> Option<Budgeted<Affected>> {
        let budget = budget.normalized();
        let affected = self.affected(id, options)?;
        let Affected {
            root,
            nodes: all_nodes,
            edges: all_edges,
            truncated: depth_truncated,
        } = affected;

        let mut nodes = Vec::new();
        let mut tokens = estimate_node(&root) + 16;
        nodes.push(AffectedNode {
            node: root.clone(),
            depth: 0,
        });
        let mut truncated = depth_truncated;

        for node in all_nodes.into_iter().skip(1) {
            let cost = estimate_node(&node.node) + 8;
            if !nodes.is_empty() && tokens.saturating_add(cost) > budget.max_tokens {
                truncated = true;
                break;
            }
            tokens = tokens.saturating_add(cost);
            nodes.push(node);
        }

        let kept: BTreeSet<&NodeId> = nodes.iter().map(|node| &node.node.id).collect();
        let edges: Vec<GraphEdge> = all_edges
            .into_iter()
            .filter(|edge| kept.contains(&edge.from) && kept.contains(&edge.to))
            .collect();
        let edge_tokens: usize = edges.iter().map(estimate_edge).sum();

        Some(Budgeted {
            value: Affected {
                root,
                nodes,
                edges,
                truncated,
            },
            estimated_tokens: tokens.saturating_add(edge_tokens),
            truncated,
        })
    }

    fn reconstruct_path(
        &self,
        from: &NodeId,
        to: &NodeId,
        previous: &BTreeMap<NodeId, (NodeId, usize)>,
    ) -> Option<GraphPath> {
        let mut node_ids = vec![to.clone()];
        let mut edge_indices = Vec::new();
        let mut cursor = to.clone();
        while cursor != *from {
            let (parent, edge_index) = previous.get(&cursor)?.clone();
            edge_indices.push(edge_index);
            node_ids.push(parent.clone());
            cursor = parent;
        }
        node_ids.reverse();
        edge_indices.reverse();
        let nodes = node_ids
            .iter()
            .map(|id| self.nodes.get(id).cloned())
            .collect::<Option<Vec<_>>>()?;
        let edges = edge_indices
            .iter()
            .map(|index| self.edges.get(*index).cloned())
            .collect::<Option<Vec<_>>>()?;
        Some(GraphPath { nodes, edges })
    }

    fn path_neighbors(&self, id: &NodeId, undirected: bool) -> Vec<(NodeId, usize)> {
        let mut neighbors = Vec::new();
        if let Some(indices) = self.outgoing.get(id) {
            for index in indices {
                neighbors.push((self.edges[*index].to.clone(), *index));
            }
        }
        if undirected && let Some(indices) = self.incoming.get(id) {
            for index in indices {
                neighbors.push((self.edges[*index].from.clone(), *index));
            }
        }
        neighbors
    }

    fn neighbors(&self, id: &NodeId, direction: Direction) -> Vec<Neighbor> {
        let mut neighbors = Vec::new();
        let indices = match direction {
            Direction::Incoming => self.incoming.get(id),
            Direction::Outgoing => self.outgoing.get(id),
        };
        if let Some(indices) = indices {
            for index in indices {
                let edge = &self.edges[*index];
                let other = match direction {
                    Direction::Incoming => &edge.from,
                    Direction::Outgoing => &edge.to,
                };
                if let Some(node) = self.nodes.get(other) {
                    neighbors.push(Neighbor {
                        edge: edge.clone(),
                        node: node.clone(),
                    });
                }
            }
        }
        neighbors.sort_by(|left, right| {
            left.edge
                .kind
                .cmp(&right.edge.kind)
                .then_with(|| left.node.name.cmp(&right.node.name))
                .then_with(|| left.node.id.cmp(&right.node.id))
        });
        neighbors
    }
}

fn insert_edge(
    edges: &mut BTreeMap<(NodeId, NodeId, EdgeKind), (EvidenceClass, Vec<ReferenceEvidence>)>,
    from: NodeId,
    to: NodeId,
    kind: EdgeKind,
    class: EvidenceClass,
    reference: Option<ReferenceEvidence>,
) {
    let entry = edges.entry((from, to, kind)).or_insert((class, Vec::new()));
    if class > entry.0 {
        entry.0 = class;
    }
    if let Some(reference) = reference {
        entry.1.push(reference);
    }
}

fn reference_class(reference: &ReferenceCandidates) -> Option<EvidenceClass> {
    match reference.state {
        CandidateState::UniqueCandidate if reference.candidates.len() == 1 => {
            Some(EvidenceClass::Inferred)
        }
        CandidateState::UniqueCandidate | CandidateState::AmbiguousCandidates => {
            Some(EvidenceClass::Ambiguous)
        }
        CandidateState::NoCandidates | CandidateState::UnsupportedSyntax => None,
    }
}

fn enclosing_symbol(
    by_path: &BTreeMap<PathBuf, Vec<(SourceRange, NodeId)>>,
    reference: &ReferenceCandidates,
) -> Option<NodeId> {
    by_path
        .get(&reference.path)?
        .iter()
        .filter(|(range, _)| {
            range.start_byte <= reference.start_byte && reference.end_byte <= range.end_byte
        })
        .min_by(|left, right| {
            let left_span = left.0.end_byte.saturating_sub(left.0.start_byte);
            let right_span = right.0.end_byte.saturating_sub(right.0.start_byte);
            left_span
                .cmp(&right_span)
                .then_with(|| left.1.cmp(&right.1))
        })
        .map(|(_, id)| id.clone())
}

/// Token budget for one graph answer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnswerBudget {
    /// Maximum estimated tokens.
    pub max_tokens: usize,
}

impl AnswerBudget {
    /// Budget with an explicit token ceiling.
    #[must_use]
    pub const fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// Clamp into the supported range.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            max_tokens: if self.max_tokens == 0 {
                1
            } else if self.max_tokens > MAX_ANSWER_TOKENS {
                MAX_ANSWER_TOKENS
            } else {
                self.max_tokens
            },
        }
    }
}

impl Default for AnswerBudget {
    fn default() -> Self {
        Self::new(DEFAULT_ANSWER_TOKENS)
    }
}

/// One graph answer with its deterministic size estimate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Budgeted<T> {
    /// Answer payload after trimming.
    pub value: T,
    /// Estimated tokens in the payload.
    pub estimated_tokens: usize,
    /// Whether a budget cut the answer short.
    pub truncated: bool,
}

/// One node ranked by incident-edge count.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GodNode {
    /// Ranked node.
    pub node: GraphNode,
    /// Total incident edges.
    pub degree: usize,
    /// Incoming edges.
    pub incoming: usize,
    /// Outgoing edges.
    pub outgoing: usize,
}

/// One ranked label-query result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueryHit {
    /// Matching node.
    pub node: GraphNode,
    /// Match score; higher is better.
    pub score: u32,
    /// Number of incident edges.
    pub degree: usize,
}

/// One neighbour of an explained node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Neighbor {
    /// Edge connecting the nodes.
    pub edge: GraphEdge,
    /// Other endpoint.
    pub node: GraphNode,
}

/// Result of [`CodeGraph::explain`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Explanation {
    /// Explained node.
    pub node: GraphNode,
    /// Edges arriving at the node, after the limit.
    pub incoming: Vec<Neighbor>,
    /// Edges leaving the node, after the limit.
    pub outgoing: Vec<Neighbor>,
    /// Whether either side was truncated.
    pub truncated: bool,
}

/// Options for [`CodeGraph::path`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathOptions {
    /// Maximum edge count.
    pub max_depth: usize,
    /// Follow edges against their direction.
    pub undirected: bool,
}

impl PathOptions {
    /// Normalize the depth into the supported range.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            max_depth: if self.max_depth == 0 {
                1
            } else if self.max_depth > MAX_PATH_DEPTH {
                MAX_PATH_DEPTH
            } else {
                self.max_depth
            },
            undirected: self.undirected,
        }
    }
}

impl Default for PathOptions {
    fn default() -> Self {
        Self {
            max_depth: 8,
            undirected: false,
        }
    }
}

/// One shortest path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphPath {
    /// Nodes in traversal order.
    pub nodes: Vec<GraphNode>,
    /// Edges connecting consecutive nodes.
    ///
    /// Edges keep their original direction; an undirected search may traverse
    /// one from its target back to its source.
    pub edges: Vec<GraphEdge>,
}

/// Options for [`CodeGraph::affected`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AffectedOptions {
    /// Maximum reverse-edge depth.
    pub max_depth: usize,
    /// Restrict traversal to these edge kinds; `None` means call/import.
    pub kinds: Option<Vec<EdgeKind>>,
}

impl AffectedOptions {
    /// Normalize the depth into the supported range.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            max_depth: if self.max_depth > MAX_PATH_DEPTH {
                MAX_PATH_DEPTH
            } else {
                self.max_depth
            },
            kinds: self.kinds,
        }
    }
}

impl Default for AffectedOptions {
    fn default() -> Self {
        Self {
            max_depth: 2,
            kinds: None,
        }
    }
}

/// One node in an `affected` result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AffectedNode {
    /// Dependent node.
    pub node: GraphNode,
    /// Reverse-edge depth from the root.
    pub depth: usize,
}

/// Result of [`CodeGraph::affected`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Affected {
    /// Queried node, included at depth 0.
    pub root: GraphNode,
    /// Nodes in deterministic `(depth, identity)` order.
    pub nodes: Vec<AffectedNode>,
    /// Edges followed by the traversal.
    pub edges: Vec<GraphEdge>,
    /// Whether a depth or node limit cut the result short.
    pub truncated: bool,
}

fn tokens_for(bytes: usize) -> usize {
    bytes.saturating_add(BYTES_PER_TOKEN - 1) / BYTES_PER_TOKEN
}

fn estimate_node(node: &GraphNode) -> usize {
    let range = node.range.map_or(0, |_| 24);
    12 + tokens_for(node.name.len()) + tokens_for(node.path.to_string_lossy().len()) + range
}

fn estimate_hit(hit: &QueryHit) -> usize {
    estimate_node(&hit.node) + 8
}

fn estimate_edge(edge: &GraphEdge) -> usize {
    let references: usize = edge
        .references
        .iter()
        .map(|reference| {
            tokens_for(reference.spelling.len())
                + tokens_for(reference.path.to_string_lossy().len())
                + 12
        })
        .sum();
    24 + tokens_for(edge.from.as_str().len()) + tokens_for(edge.to.as_str().len()) + references
}

fn estimate_neighbor(neighbor: &Neighbor) -> usize {
    estimate_node(&neighbor.node) + estimate_edge(&neighbor.edge) + 4
}

enum Direction {
    Incoming,
    Outgoing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolution::SymbolRecord;

    fn symbol(
        workspace: WorkspaceId,
        path: &str,
        start: usize,
        end: usize,
        kind: SymbolKind,
        name: &str,
    ) -> SymbolRecord {
        SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from(path),
                start_byte: start,
                end_byte: end,
                kind,
            },
            name: name.to_owned(),
        }
    }

    fn reference(
        path: &str,
        start: usize,
        end: usize,
        kind: ReferenceKind,
        spelling: &str,
        state: CandidateState,
        candidates: Vec<SymbolId>,
    ) -> ReferenceCandidates {
        ReferenceCandidates {
            path: PathBuf::from(path),
            start_byte: start,
            end_byte: end,
            kind,
            spelling: spelling.to_owned(),
            state,
            candidates,
        }
    }

    struct Fixture {
        workspace: WorkspaceId,
        alpha: SymbolId,
        beta: SymbolId,
        gamma: SymbolId,
        report: SymbolCandidateReport,
    }

    fn fixture() -> Fixture {
        let workspace = WorkspaceId::new();
        let alpha = symbol(
            workspace,
            "src/lib.rs",
            0,
            20,
            SymbolKind::Function,
            "alpha",
        );
        let beta = symbol(
            workspace,
            "src/lib.rs",
            30,
            50,
            SymbolKind::Function,
            "beta",
        );
        let gamma = symbol(
            workspace,
            "src/other.rs",
            0,
            20,
            SymbolKind::Function,
            "gamma",
        );
        let report = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![alpha.clone(), beta.clone(), gamma.clone()],
            references: vec![
                reference(
                    "src/lib.rs",
                    10,
                    14,
                    ReferenceKind::FunctionCall,
                    "beta",
                    CandidateState::UniqueCandidate,
                    vec![beta.id.clone()],
                ),
                reference(
                    "src/other.rs",
                    10,
                    15,
                    ReferenceKind::FunctionCall,
                    "alpha",
                    CandidateState::AmbiguousCandidates,
                    vec![alpha.id.clone(), beta.id.clone()],
                ),
                reference(
                    "src/lib.rs",
                    55,
                    60,
                    ReferenceKind::FunctionCall,
                    "missing",
                    CandidateState::NoCandidates,
                    Vec::new(),
                ),
                reference(
                    "src/lib.rs",
                    5,
                    8,
                    ReferenceKind::Import,
                    "gamma",
                    CandidateState::UniqueCandidate,
                    vec![gamma.id.clone()],
                ),
            ],
            unique_candidate_references: 2,
            ambiguous_references: 1,
            unresolved_references: 1,
            unsupported_references: 0,
        };
        Fixture {
            workspace,
            alpha: alpha.id,
            beta: beta.id,
            gamma: gamma.id,
            report,
        }
    }

    fn edge<'a>(
        graph: &'a CodeGraph,
        from: &SymbolId,
        to: &SymbolId,
        kind: EdgeKind,
    ) -> &'a GraphEdge {
        graph
            .edges()
            .iter()
            .find(|edge| {
                edge.from == NodeId::symbol(from)
                    && edge.to == NodeId::symbol(to)
                    && edge.kind == kind
            })
            .expect("edge")
    }

    #[test]
    fn builds_candidate_edges_with_explicit_evidence_classes() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");

        assert_eq!(graph.workspace_id(), fixture.workspace);
        assert_eq!(graph.nodes().count(), 5);
        assert_eq!(graph.coverage().symbols, 3);
        assert_eq!(graph.coverage().files, 2);
        assert_eq!(graph.coverage().references, 4);
        assert_eq!(graph.coverage().inferred_edges, 2);
        assert_eq!(graph.coverage().ambiguous_edges, 2);
        assert_eq!(graph.coverage().unresolved_references, 1);
        assert!(!graph.coverage().is_complete());

        let inferred = edge(&graph, &fixture.alpha, &fixture.beta, EdgeKind::Calls);
        assert_eq!(inferred.class, EvidenceClass::Inferred);
        assert_eq!(inferred.references.len(), 1);
        assert_eq!(inferred.references[0].spelling, "beta");

        let ambiguous = edge(&graph, &fixture.gamma, &fixture.alpha, EdgeKind::Calls);
        assert_eq!(ambiguous.class, EvidenceClass::Ambiguous);

        let containment = graph
            .edges()
            .iter()
            .find(|edge| {
                edge.from == NodeId::file(Path::new("src/lib.rs"))
                    && edge.to == NodeId::symbol(&fixture.alpha)
                    && edge.kind == EdgeKind::Contains
            })
            .expect("containment edge");
        assert_eq!(containment.class, EvidenceClass::Extracted);
    }

    #[test]
    fn query_ranks_symbol_names_over_path_tokens() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");

        let hits = graph.query("beta", 10);
        assert_eq!(hits[0].node.name, "beta");
        assert!(hits[0].score > 0);

        let hits = graph.query("src other", 10);
        assert!(
            hits.iter()
                .any(|hit| hit.node.path == Path::new("src/other.rs"))
        );
    }

    #[test]
    fn explain_lists_incoming_and_outgoing_evidence() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        let explanation = graph
            .explain(&NodeId::symbol(&fixture.beta), 10)
            .expect("explained");

        assert_eq!(explanation.node.name, "beta");
        assert_eq!(explanation.outgoing.len(), 0);
        assert!(
            explanation
                .incoming
                .iter()
                .any(|neighbor| neighbor.node.name == "alpha"
                    && neighbor.edge.class == EvidenceClass::Inferred)
        );
        assert!(
            explanation
                .incoming
                .iter()
                .any(|neighbor| neighbor.node.name == "gamma"
                    && neighbor.edge.class == EvidenceClass::Ambiguous)
        );
        assert!(!explanation.truncated);
    }

    #[test]
    fn path_follows_edge_direction_unless_undirected() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        let alpha = NodeId::symbol(&fixture.alpha);
        let beta = NodeId::symbol(&fixture.beta);
        let gamma = NodeId::symbol(&fixture.gamma);

        let forward = graph
            .path(&alpha, &beta, PathOptions::default())
            .expect("alpha calls beta");
        assert_eq!(forward.nodes.len(), 2);
        assert_eq!(forward.edges[0].class, EvidenceClass::Inferred);

        assert!(
            graph.path(&beta, &alpha, PathOptions::default()).is_none(),
            "directed search must not follow edges backwards"
        );
        let backward = graph
            .path(
                &beta,
                &alpha,
                PathOptions {
                    undirected: true,
                    ..PathOptions::default()
                },
            )
            .expect("undirected search finds the caller");
        assert_eq!(backward.nodes.len(), 2);

        let ambiguous = graph
            .path(&gamma, &beta, PathOptions::default())
            .expect("gamma reaches beta through an ambiguous edge");
        assert!(
            ambiguous
                .edges
                .iter()
                .any(|edge| edge.class == EvidenceClass::Ambiguous)
        );
    }

    #[test]
    fn affected_walks_call_edges_in_reverse() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        let affected = graph
            .affected(
                &NodeId::symbol(&fixture.beta),
                AffectedOptions {
                    max_depth: 1,
                    kinds: None,
                },
            )
            .expect("affected");

        assert_eq!(affected.root.name, "beta");
        assert_eq!(affected.nodes[0].depth, 0);
        assert!(
            affected
                .nodes
                .iter()
                .any(|node| node.node.name == "alpha" && node.depth == 1)
        );
        assert!(
            affected
                .nodes
                .iter()
                .any(|node| node.node.name == "gamma" && node.depth == 1)
        );
        assert_eq!(affected.edges.len(), 2);
        assert!(!affected.truncated);
    }

    #[test]
    fn query_limit_is_enforced() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        assert_eq!(graph.query("src", 1).len(), 1);
    }

    #[test]
    fn unknown_candidates_are_rejected() {
        let fixture = fixture();
        let mut report = fixture.report;
        report.references[0].candidates = vec![SymbolId {
            workspace_id: fixture.workspace,
            path: PathBuf::from("src/missing.rs"),
            start_byte: 0,
            end_byte: 1,
            kind: SymbolKind::Function,
        }];

        let error = CodeGraph::build(&report).expect_err("unknown candidate");
        assert!(matches!(error, GraphError::UnknownCandidate(_)));
    }
    #[test]
    fn token_budget_trims_query_and_marks_truncation() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");

        let full = graph.query_budgeted("src", 10, AnswerBudget::new(2_000));
        assert!(full.value.len() > 1);
        assert!(!full.truncated);

        let trimmed = graph.query_budgeted("src", 10, AnswerBudget::new(10));
        assert_eq!(trimmed.value.len(), 1);
        assert!(trimmed.truncated);
        assert!(trimmed.estimated_tokens >= 10);
    }

    #[test]
    fn token_budget_trims_explain_and_affected() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        let beta = NodeId::symbol(&fixture.beta);

        let explain = graph
            .explain_budgeted(&beta, 10, AnswerBudget::new(10))
            .expect("explain");
        assert!(explain.truncated);
        assert!(!explain.value.incoming.is_empty());
        assert!(explain.value.incoming.len() < 2);

        let affected = graph
            .affected_budgeted(&beta, AffectedOptions::default(), AnswerBudget::new(10))
            .expect("affected");
        assert!(affected.truncated);
        assert!(affected.value.nodes.len() < 3);
        assert!(affected.value.nodes.iter().any(|node| node.depth == 0));
    }

    #[test]
    fn token_budget_keeps_a_connected_path_prefix() {
        let fixture = fixture();
        let graph = CodeGraph::build(&fixture.report).expect("graph");
        let alpha = NodeId::symbol(&fixture.alpha);
        let beta = NodeId::symbol(&fixture.beta);

        let path = graph
            .path_budgeted(&alpha, &beta, PathOptions::default(), AnswerBudget::new(1))
            .expect("path");
        assert!(!path.truncated);
        assert_eq!(path.value.nodes.len(), 2);
        assert_eq!(path.value.edges.len(), 1);
    }

    #[test]
    fn relation_facts_become_candidate_edges() {
        let fixture = fixture();
        let relations = vec![(
            PathBuf::from("src/lib.rs"),
            RelationFact {
                kind: RelationKind::Inherits,
                source_name: "alpha".to_owned(),
                target_name: "beta".to_owned(),
                range: SourceRange {
                    start_byte: 0,
                    end_byte: 20,
                },
            },
        )];
        let graph = CodeGraph::build_with_relations(&fixture.report, &relations).expect("graph");

        let edge = edge(&graph, &fixture.alpha, &fixture.beta, EdgeKind::Inherits);
        assert_eq!(edge.class, EvidenceClass::Inferred);
        assert_eq!(edge.references[0].spelling, "beta");
        assert_eq!(graph.coverage().relations, 1);
        assert_eq!(graph.coverage().unresolved_relations, 0);
    }
    #[test]
    fn resolved_imports_add_file_edges() {
        let fixture = fixture();
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/lib.rs"),
            to: PathBuf::from("src/other.rs"),
            spelling: "import com.example.Foo;".to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 7,
            },
        }];
        let graph = CodeGraph::build_with_imports(&fixture.report, &[], &imports).expect("graph");

        let edge = graph
            .edges()
            .iter()
            .find(|edge| {
                edge.from == NodeId::file(Path::new("src/lib.rs"))
                    && edge.to == NodeId::file(Path::new("src/other.rs"))
                    && edge.kind == EdgeKind::Imports
            })
            .expect("import edge");
        assert_eq!(edge.class, EvidenceClass::Inferred);
        assert_eq!(edge.references[0].spelling, "import com.example.Foo;");
    }
    #[test]
    fn owner_contains_member_symbols() {
        let workspace = WorkspaceId::new();
        let owner = symbol(workspace, "src/lib.rs", 0, 100, SymbolKind::Struct, "Owner");
        let method = symbol(workspace, "src/lib.rs", 10, 20, SymbolKind::Method, "run");
        let report = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![owner.clone(), method.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let graph = CodeGraph::build(&report).expect("graph");

        let edge = graph
            .edges()
            .iter()
            .find(|edge| {
                edge.from == NodeId::symbol(&owner.id)
                    && edge.to == NodeId::symbol(&method.id)
                    && edge.kind == EdgeKind::Contains
            })
            .expect("member edge");
        assert_eq!(edge.class, EvidenceClass::Extracted);
    }
}
