//! The BSH extension platform.
//!
//! This module is the runtime-independent extension system described in
//! docs/BSH_Extension_Framework_Design.md and specified by
//! docs/bsh-extension-standard.md:
//!
//! - a versioned manifest and package format;
//! - a managed store with hashing, staging, atomic install and a lockfile;
//! - a capability policy with explicit grants, scopes and an audit trail;
//! - a lifecycle state machine;
//! - three runtimes - built-in Rust, declarative files and external processes;
//! - a bounded event bus;
//! - agent tools, commands, panels, prompts and contexts;
//! - a versioned declarative UI contract;
//! - diagnostics and a developer CLI.
//!
//! Nothing in this module imports GPUI. The UI is a consumer of the declarative
//! tree and the status rows, never a dependency of the platform.

// The platform is deliberately complete before every consumer lands: the
// storage namespace, the subscription side of the event bus and the UI action
// protocol are implemented and unit-tested here, while the first host build
// consumes tools, prompts, commands and status. The allow stays scoped to this
// module; code outside it still has to be wired to be accepted.
#![allow(dead_code)]

mod agent;
mod capability;
mod cli;
mod events;
mod host;
mod lifecycle;
mod manifest;
mod package;
mod resolver;
mod runtime;
mod storage;
mod types;
mod ui;

pub(crate) use host::install;

/// Run the developer CLI and return the process exit code.
pub(crate) fn cli(args: &[String]) -> i32 {
    cli::run(args)
}
