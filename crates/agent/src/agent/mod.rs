//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;
pub mod hooks;
pub mod plugin;

pub use self::agentic::{Agent, AgentConfig, FallbackProvider};
pub use context::{
    AgentReActApp, AgentReactContext, AgentSession, CompactionOutcome, MessageContext,
    CHECKPOINT_PREAMBLE, COMPACTION_INSTRUCTION,
};
pub use hooks::external::{EventMatcher, ExternalHook, HookDispatcher, HookOutcome};
pub use hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper,
};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};
