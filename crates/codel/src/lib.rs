#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Host-independent code intelligence primitives for the BOS workspace.
//!
//! codel is an engine crate, not a BSH or Ferris subsystem. It must not
//! depend on BSH, Ferris, GPUI, or a host runtime. Host-specific integration
//! belongs in a separate adapter crate or in the host that embeds this engine.
//!
//! Code intelligence operates on source files as data. The engine must not
//! execute workspace code, build scripts, tests, package-manager scripts, or
//! other project-controlled commands as part of indexing. Filesystem
//! confinement and resource limits are required before workspace scanning.
//!
//! A workspace ID identifies one logical checkout (including one Git worktree),
//! not its Git repository. The ID is separate from its canonical filesystem
//! root. Callers that need identity to survive restarts must persist the ID in
//! host-managed metadata and supply it again when opening the workspace.

pub mod classification;
pub mod context;
pub mod evidence;
pub mod evidence_index;
pub mod export;
pub mod export_formats;
pub mod fs;
pub mod graph;
pub mod ignore;
pub mod index;
pub mod language_resolver;
pub mod leiden;
pub mod lexical;
pub mod model;
pub mod module_graph;
pub mod resolution;
pub mod retrieve;
pub mod rust_imports;
pub mod rust_modules;
pub mod rust_symbol_imports;
pub mod rust_symbols;
pub mod scan_budget;
pub mod scanner;
pub mod scip_semantics;
pub mod scope;
pub mod store;
pub mod symbol_resolver;
pub mod syntax;
pub mod workspace;

pub use context::{
    ContextBudget, ContextBundle, ContextOmission, ContextSegment, DEFAULT_MAX_CONTEXT_BYTES,
    DEFAULT_MAX_CONTEXT_SEGMENTS, OmissionReason, SourceError, SourceReader, assemble_context,
};
pub use evidence::{
    CaptureIdentity, EvidenceBatch, EvidenceCoverage, EvidenceDiagnostic, EvidenceDiagnosticKind,
    EvidenceRecord, FingerprintAlgorithm, ParseCoverage, ProviderIdentity, QueryCoverage,
    QueryIdentity, SourceFingerprint,
};
pub use evidence_index::{
    ChangeKind, ChangeSet, EvidenceIndex, FileChange, FileEvidence, IndexError, IndexLimits,
    IndexSnapshot, IndexUpdate, Publication, QueryEvidence, SourceObservation, StaleMarks,
    StaleReason,
};
pub use export::{Community, ExportedDocument, GraphBenchmark};
pub use fs::{DirectoryListing, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFs, WorkspaceFsError};
pub use graph::{
    Affected, AffectedNode, AffectedOptions, AnswerBudget, BYTES_PER_TOKEN, Budgeted, CodeGraph,
    DEFAULT_ANSWER_TOKENS, EdgeKind, EvidenceClass, Explanation, GodNode, GraphCoverage, GraphEdge,
    GraphError, GraphNode, GraphPath, MAX_AFFECTED_NODES, MAX_ANSWER_TOKENS, MAX_EXPLAIN_NEIGHBORS,
    MAX_PATH_DEPTH, MAX_PATH_NODES, MAX_QUERY_RESULTS, Neighbor, NodeId, NodeKind, PathOptions,
    QueryHit, ReferenceEvidence,
};
pub use ignore::{IgnoreAction, IgnorePolicy, IgnorePolicyError, IgnoreRule};
pub use index::{
    SourceFileResult, SourceFileStatus, SourceIndexDiagnostic, SourceIndexDiagnosticKind,
    SourceIndexReport, SourceIndexStatus, index_rust_workspace, index_workspace,
    index_workspace_with_limits,
};
pub use language_resolver::{
    CIncludeResolver, CSharpResolver, GoResolver, ImportEdge, JavaResolver, JavaScriptResolver,
    KotlinResolver, LanguageResolver, PhpResolver, RubyResolver, ScalaResolver,
};
pub use module_graph::{
    RustModuleEdge, RustModuleEdgeStatus, RustModuleGraph, RustModuleGraphError,
    RustModuleGraphLimits, RustModuleGraphStatus, RustModuleNode, RustModuleNodeKind,
    build_rust_module_graph,
};
pub use resolution::{
    CandidateState, ReferenceCandidates, ReferenceKind, SymbolCandidateError,
    SymbolCandidateReport, SymbolId, SymbolKind, SymbolRecord, build_symbol_candidates,
};
pub use retrieve::{
    DEFAULT_LEXICAL_WEIGHT, DEFAULT_REMOTE_WEIGHT, DEFAULT_RRF_K, DEFAULT_VECTOR_WEIGHT,
    EXACT_NAME_BONUS, Escalation, HybridIndex, RemoteError, RemoteHit, RemoteQuery, RemoteSearch,
    RemoteStatus, RetrievalCandidate, RetrievalConfig, RetrievalCoverage, RetrievalOutcome,
    RetrievalQuery, RetrievalTier, VectorError, VectorIndex, rrf_contribution,
};
pub use rust_imports::{
    RustImportResolution, RustImportResolutionError, RustImportResolutionReport,
    RustImportResolutionStatus, resolve_rust_import_module_candidates,
};
pub use rust_modules::{
    RustModuleResolution, RustModuleResolutionError, RustModuleResolutionReport,
    RustModuleResolutionStatus, resolve_rust_module_candidates,
};
pub use rust_symbol_imports::{
    RustSymbolImportCandidate, RustSymbolImportResolution, RustSymbolImportResolutionError,
    RustSymbolImportResolutionReport, RustSymbolImportStatus, resolve_rust_import_symbols,
};
pub use rust_symbols::{
    RustScopedSymbol, RustScopedSymbolError, RustScopedSymbolId, RustScopedSymbolKind,
    RustScopedSymbolReport, build_rust_scoped_symbols,
};
pub use scan_budget::{ScanBudget, ScanBudgetError, ScanLimits, ScanUsage};
pub use scanner::{
    ScanDiagnostic, ScanDiagnosticKind, ScanStatus, WorkspaceScanReport, content_manifest_digest,
    scan_manifest_digest, scan_workspace, scan_workspace_with_policy,
};
pub use scip_semantics::{
    ScipExternalReference, ScipExternalSymbol, ScipReference,
    external_references as scip_external_references, external_symbols as scip_external_symbols,
    load_external_references, load_external_symbols, load_index as load_scip_index,
    resolve_index as resolve_scip_index, to_symbol_edges as scip_symbol_edges,
};
pub use store::{
    GraphStore, IndexStore, LoadedIndex, MAX_STORE_BYTES, Recovery, STORE_SCHEMA_VERSION,
    StoreError,
};
pub use symbol_resolver::{
    CIncludeSymbolResolver, CSharpReceiverResolver, CSharpSymbolResolver, CppSymbolResolver,
    GoReceiverResolver, GoSymbolResolver, JavaReceiverResolver, JavaScriptSymbolResolver,
    JavaSymbolResolver, KotlinReceiverResolver, KotlinSymbolResolver, PhpSymbolResolver,
    RubySymbolResolver, ScalaReceiverResolver, ScalaSymbolResolver, ScopedSymbolEdge,
    SymbolResolver, TypeScriptReceiverResolver,
};
pub use syntax::generic::GrammarProvider;
pub use syntax::python::PythonProvider;
pub use syntax::query::{
    MAX_MATCH_LIMIT, MAX_QUERY_ID_BYTES, MAX_QUERY_VERSION_BYTES, MIN_MATCH_LIMIT,
    QueryCaptureRecord, QueryCompileErrorKind, QueryControl, QueryExecution, QueryExecutionError,
    QueryExecutionLimits, QueryExecutionOptions, QueryExecutionStatus, QueryMatchRecord,
    QueryOutputLimit, QueryRange, QueryRangeMode, QueryRegistrationError, QueryRegistry,
    RegisteredQuery, execute_query,
};
pub use syntax::rust::RustProvider;
pub use syntax::typescript::TypeScriptProvider;
pub use syntax::{
    ImportFact, LanguageProvider, ParseControl, ParseDiagnostic, ParseInput, ParseLimitKind,
    ParseLimits, ParseResult, ParseStatus, ParsedDocument, ParsedDocumentStatus, ProviderError,
    RelationFact, RelationKind, RustModuleFact, RustVisibility, SourceRange, SyntaxFact,
    SyntaxFactKind,
};
pub use workspace::{WorkspaceDescriptor, WorkspaceError, WorkspaceId, WorkspaceRegistry};
