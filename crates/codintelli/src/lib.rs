#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Host-independent code intelligence primitives for the BOS workspace.
//!
//! `codintelli` is an engine crate, not a BSH or Ferris subsystem. It must not
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
pub mod scan_budget;
pub mod scanner;
pub mod workspace;

pub use ignore::{IgnoreAction, IgnorePolicy, IgnorePolicyError, IgnoreRule};
pub use fs::{
    DirectoryListing, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFs, WorkspaceFsError,
};
pub use scan_budget::{ScanBudget, ScanBudgetError, ScanLimits, ScanUsage};
pub use scanner::{
    scan_workspace, scan_workspace_with_policy, ScanDiagnostic, ScanDiagnosticKind, ScanStatus,
    WorkspaceScanReport,
};
pub use workspace::{WorkspaceDescriptor, WorkspaceError, WorkspaceId, WorkspaceRegistry};
