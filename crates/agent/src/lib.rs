//! Core agent runtime: tools, skills, hooks, plugins, sessions, and MCP.
#![warn(missing_docs)]

pub mod agent;
pub mod bus;
pub mod error;
pub mod extensions;
pub mod goal;
pub mod jobs;
pub mod mcp;
pub mod memory;
pub mod metrics;
pub mod plugins;
pub mod policy;
pub mod prelude;
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;

pub use prelude::*;
