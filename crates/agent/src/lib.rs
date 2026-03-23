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
pub mod scheduler;
pub mod session;

pub use error::{AgentError, LlmError, ToolError};
pub use agent::{Agent, AgentConfig, AgentOutput, Message, MessageLog};
pub use agent::config::TomlToolRef;
pub use agent::config::{AgentBuilder, TomlAgentConfig};
pub use llm::{LlmClient, LlmRequest, LlmResponse, OpenAiMessage, OpenAiClient, StreamToken};
pub use tools::{Tool, ToolDescription, ToolRegistry};
pub use tools::{
    UnifiedToolRegistry, UnifiedRegistryConfig,
    A2AToolClient,
    ToolDiscovery, DiscoveredTool, ToolSource, LocalDiscovery,
    ZenohRpcDiscovery, McpDiscovery, A2AToolDiscovery,
};
pub use streaming::{
    SseDecoder, SseEvent, TokenStream, TokenPublisher,
    TokenBatch, SerializedToken, TokenType,
    RateLimiter, BackpressureController,
};
pub use skills::{SkillLoader, SkillMetadata, SkillContent, SkillError, SkillInjector};
pub use mcp::{McpClient, McpToolAdapter, McpError, StdioTransport, ServerCapabilities, ToolDefinition,
    McpResource, McpPrompt, McpPromptArgument, ReadResourceResult, ResourceContents};
pub use a2a::{A2AMessage, A2AContent, AgentIdentity, Task, TaskState, AgentCard, A2AClient};
pub use scheduler::BackoffStrategy;
pub use scheduler::StepType;
pub use scheduler::ConditionType;
pub use scheduler::{Workflow, Step, WorkflowResult, StepResult, WorkflowStatus, StepStatus};
pub use scheduler::dsl::{WorkflowBuilder, StepBuilder};
pub use scheduler::executor::Scheduler;
pub use session::{AgentState, SessionConfig, SessionError, SessionMetadata, SessionSummary};
pub use session::manager::SessionManager;
pub use session::serializer::SessionSerializer;
