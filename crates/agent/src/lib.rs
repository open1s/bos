//! BrainOS Agent crate
//!
//! Provides the core agent infrastructure for distributed AI agents.

pub mod error;
pub mod agent;
pub mod llm;
pub mod tools;
pub mod streaming;
pub mod skills;
pub mod mcp;
pub mod a2a;

pub use error::{AgentError, LlmError, ToolError};
pub use agent::{Agent, AgentConfig, AgentOutput, Message, MessageLog};
pub use agent::config::{AgentBuilder, TomlAgentConfig};
pub use llm::{LlmClient, LlmRequest, LlmResponse, OpenAiMessage, OpenAiClient, StreamToken};
pub use tools::{Tool, ToolDescription, ToolRegistry};
pub use streaming::{
    SseDecoder, SseEvent, TokenStream,
    PublisherWrapper, TokenPublisher,
    TokenBatch, SerializedToken, TokenType,
    RateLimiter, BackpressureController,
};
pub use skills::{SkillLoader, SkillMetadata, SkillContent, SkillError, SkillInjector};
pub use mcp::{McpClient, McpToolAdapter, McpError, StdioTransport, ServerCapabilities, ToolDefinition};
pub use a2a::{A2AMessage, A2AContent, AgentIdentity, Task, TaskState, AgentCard, A2AClient};
