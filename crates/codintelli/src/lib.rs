#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Host-independent code intelligence primitives for the BOS workspace.
//!
//! # Architectural boundary
//!
//! `codintelli` is an engine crate, not a BSH or Ferris subsystem. It must not
//! depend on BSH, Ferris, GPUI, or a host runtime. Host-specific integration
//! belongs in a separate adapter crate or in the host that embeds this engine.
//!
//! # Security boundary
//!
//! Code intelligence operates on source files as data. The engine must not
//! execute workspace code, build scripts, tests, package-manager scripts, or
//! other project-controlled commands as part of indexing. Filesystem
//! confinement and resource limits are required before workspace scanning is
//! implemented.
//!
//! # API stability
//!
//! This initial crate skeleton intentionally exports no operational API.
//! Workspace identity, filesystem capabilities, snapshots, and index
//! generations must be specified before their public types are frozen. Those
//! contracts will be introduced in the corresponding implementation stories.
