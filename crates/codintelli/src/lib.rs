#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Host-independent code intelligence primitives for the BOS workspace.
//!
//! codintelli is an engine crate, not a BSH or Ferris subsystem. It must not
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
pub mod fs;
pub mod ignore;
pub mod index;
pub mod scan_budget;
pub mod scanner;
pub mod resolution;
pub mod rust_modules;
pub mod module_graph;
pub mod rust_imports;
pub mod rust_symbols;
pub mod rust_symbol_imports;
pub mod syntax;
pub mod workspace;

pub use ignore::{IgnoreAction, IgnorePolicy, IgnorePolicyError, IgnoreRule};
pub use fs::{
    DirectoryListing, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFs, WorkspaceFsError,
};
pub use index::{
    index_rust_workspace, index_workspace, index_workspace_with_limits, SourceFileResult, SourceFileStatus,
    SourceIndexDiagnostic, SourceIndexDiagnosticKind, SourceIndexReport, SourceIndexStatus,
};
pub use module_graph::{
    build_rust_module_graph, RustModuleEdge, RustModuleEdgeStatus, RustModuleGraph,
    RustModuleGraphError, RustModuleGraphLimits, RustModuleGraphStatus, RustModuleNode,
    RustModuleNodeKind,
};
pub use rust_imports::{
    resolve_rust_import_module_candidates, RustImportResolution, RustImportResolutionError,
    RustImportResolutionReport, RustImportResolutionStatus,
};
pub use rust_symbols::{
    build_rust_scoped_symbols, RustScopedSymbol, RustScopedSymbolError,
    RustScopedSymbolId, RustScopedSymbolKind, RustScopedSymbolReport,
};
pub use rust_symbol_imports::{
    resolve_rust_import_symbols, RustSymbolImportCandidate, RustSymbolImportResolution,
    RustSymbolImportResolutionError, RustSymbolImportResolutionReport, RustSymbolImportStatus,
};
pub use rust_modules::{
    resolve_rust_module_candidates, RustModuleResolution, RustModuleResolutionError,
    RustModuleResolutionReport, RustModuleResolutionStatus,
};
pub use resolution::{
    build_symbol_candidates, CandidateState, ReferenceCandidates, ReferenceKind,
    SymbolCandidateError, SymbolCandidateReport, SymbolId, SymbolKind, SymbolRecord,
};
pub use scan_budget::{ScanBudget, ScanBudgetError, ScanLimits, ScanUsage};
pub use scanner::{
    scan_workspace, scan_workspace_with_policy, ScanDiagnostic, ScanDiagnosticKind, ScanStatus,
    WorkspaceScanReport,
};
pub use syntax::{
    ImportFact, RustModuleFact, LanguageProvider, ParseControl, ParseDiagnostic, ParseInput, ParseLimitKind, ParseLimits, ParseResult, ParseStatus, ProviderError,
    ParsedDocument, ParsedDocumentStatus, RustVisibility, SourceRange, SyntaxFact, SyntaxFactKind,
};
pub use syntax::rust::RustProvider;
pub use workspace::{WorkspaceDescriptor, WorkspaceError, WorkspaceId, WorkspaceRegistry};
