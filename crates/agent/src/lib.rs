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
pub mod sandbox;
pub mod security;
pub mod session;
mod shell_env;
pub mod skills;
pub mod tools;

pub use prelude::*;

// The prompt corpus is public API: a host composes its persona into the same
// identity and workflow framework the default prompt uses.
pub use agent::prompts;
pub use agent::prompts::{
    compose_system_prompt, guidance_for_tool, DEFAULT_PERSONA, HARNESS_IDENTITY, TOOL_GUIDANCE,
    WORKFLOW_GUIDANCE,
};
