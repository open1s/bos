//! Core agent runtime: tools, skills, hooks, plugins, sessions, and MCP.
#![warn(missing_docs)]

pub mod agent;
pub mod bus;
pub mod error;
pub mod mcp;
pub mod memory;
pub mod metrics;
pub mod prelude;
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;

pub use prelude::*;
