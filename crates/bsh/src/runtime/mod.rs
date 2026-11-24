//! Standalone multi-interface Runtime Host.
//!
//! One Host owns execution, sessions, approvals, recovery and durable state;
//! every interface (today the terminal client, later the GUI) is a *client* of
//! that one authority over a transport-neutral Application Contract:
//!
//! - [`contract`] — the shared Command, Query, Control, Event and Projection
//!   vocabulary. No transport semantics live here.
//! - [`host`] — the authority: journal-backed state, projections, executions,
//!   approvals, shutdown and recovery.
//! - [`journal`] / [`projection`] — durable facts (RFC-013) and the read models
//!   folded from them.
//! - [`server`] / [`client`] — the local IPC adapter (RFC-015) and the blocking
//!   client the terminal interface uses.
//! - [`embed`] — the fallback that lets `bsh tui` start the Host for a data
//!   directory no other process owns, so one command is a whole runtime.
//! - [`authz`] / [`audit`] — who acted, on what, and the record of the decision.
//!
//! The module is crate-private: the binaries reach it through the two entry
//! points in this file, so the runtime's types never become a public API of the
//! shell crate.

// The contract is deliberately complete before every consumer lands: the audit
// trail, the authorization surface, the approval lifecycle and the read models
// are implemented and unit-tested here, while today's consumers are the Host,
// the IPC server and one terminal client. The allow stays scoped to this
// module; code outside it still has to be wired to be accepted.
#![allow(dead_code)]

pub(crate) mod agent_exec;
pub(crate) mod audit;
pub(crate) mod authz;
pub(crate) mod cli;
pub(crate) mod client;
pub(crate) mod config;
pub(crate) mod contract;
pub(crate) mod embed;
pub(crate) mod endpoint;
pub(crate) mod executor;
pub(crate) mod host;
pub(crate) mod ids;
pub(crate) mod journal;
pub(crate) mod projection;
pub(crate) mod review;
pub(crate) mod server;
#[cfg(test)]
pub(crate) mod test_support;

use std::io::Write;

/// The tool whose call replaces the model's working plan.
///
/// Named here because two layers agree on it: the executor reports the plan
/// after this call returns, and the terminal draws the rows the Host journalled
/// from it. A panel row is therefore the plan the model wrote, never a
/// client-side guess about which call produced it.
pub(crate) const PLAN_TOOL: &str = "update_plan";

/// Entry point of `bsh host`, the standalone Runtime Host subcommand.
///
/// Returns the process exit code. The Host owns the data directory for the
/// whole run, so a second Host over the same directory is refused rather than
/// started.
pub fn host_main(args: &[String]) -> i32 {
    cli::host_main(args)
}

/// Entry point of `bsh tui`, the terminal client.
///
/// It connects to whatever Host published `<data-dir>/runtime.json`, and starts
/// one in its own process only when the data directory has no owner. The
/// interface is a client in both cases: the Host it starts is served over the
/// same local IPC endpoint, so a second client sees one authority, not two.
pub fn tui_main(args: &[String]) -> i32 {
    cli::tui_main(args)
}

/// Print one line to stderr; shared by the binaries so both report the same
/// way when the Host is unreachable.
pub(crate) fn report(message: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "bsh: {message}");
}
