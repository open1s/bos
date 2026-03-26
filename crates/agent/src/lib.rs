//! BrainOS Agent crate
//!
//! Provides the core agent infrastructure for distributed AI agents.

pub mod error;
pub mod agent;
pub mod llm;
pub mod tools;
pub mod skills;
pub mod mcp;
pub mod session;
pub mod streaming;
mod react;

#[allow(unused)]
use logging;

pub use error::{AgentError, LlmError, ToolError};
pub use agent::config::TomlToolRef;
pub use agent::config::{AgentBuilder, TomlAgentConfig};
pub use llm::{LlmClient, LlmRequest, LlmResponse, OpenAiMessage, OpenAiClient, StreamToken};
pub use tools::{Tool, ToolDescription, ToolRegistry};
pub use skills::{SkillLoader, SkillMetadata, SkillContent, SkillError, SkillInjector};
pub use mcp::{McpClient, McpToolAdapter, McpError, StdioTransport, ServerCapabilities, ToolDefinition,
    McpResource, McpPrompt, McpPromptArgument, ReadResourceResult, ResourceContents};
pub use session::{AgentState, SessionConfig, SessionError, SessionMetadata, SessionSummary};
pub use session::manager::SessionManager;
pub use session::serializer::SessionSerializer;
