//! In-process code intelligence for BSH.
//!
//! The MCP binary is only a transport shim over these modules; linking this
//! library lets the shell register codel as a built-in tool without spawning a
//! server.

pub use codel;

pub mod ast;
pub mod context;
pub mod diagnostics;
pub mod workspace;
