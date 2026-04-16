//! BrainOS Agent crate
//!
//! Provides the core agent infrastructure for distributed AI agents.

pub mod agent;
pub mod bus_rpc;
pub mod error;
pub mod mcp;
pub mod security;
pub mod session;
pub mod skills;
pub mod tools;

// NOTE: logging is a dependency and used via the `log` crate macros
// The `logging` crate is initialized in binary builds

pub use agent::config::TomlAgentBuilder as AgentBuilder;
pub use agent::config::TomlAgentConfig;
pub use agent::config::TomlToolRef;
pub use agent::hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use agent::{Agent, AgentBuilder as SimpleAgentBuilder, AgentConfig, AgentOutput};
pub use bus_rpc::{AgentCallableServer, AgentCallerTool, AgentRpcClient};
pub use error::{AgentError, LlmError, ToolError};
pub use mcp::{
    McpClient, McpError, McpPrompt, McpPromptArgument, McpResource, McpToolAdapter,
    ReadResourceResult, ResourceContents, ServerCapabilities, StdioTransport, ToolDefinition,
};
pub use react::llm::vendor::OpenAiVendor;
pub use react::llm::{LlmClient, LlmMessage, LlmRequest, LlmResponse, OpenAiMessage, StreamToken};
pub use react::{CircuitBreakerConfig, RateLimiterConfig};
pub use security::{SecurityError, WorkspaceValidator};
pub use session::manager::SessionManager;
pub use session::serializer::SessionSerializer;
pub use session::{AgentState, SessionConfig, SessionError, SessionMetadata, SessionSummary};
pub use skills::{SkillContent, SkillError, SkillInjector, SkillLoader, SkillMetadata};
pub use tools::bash::BashExecutionResult;
pub use tools::{BashTool, Tool, ToolDescription, ToolRegistry};
