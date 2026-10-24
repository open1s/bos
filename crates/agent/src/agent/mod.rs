//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;
pub mod hooks;
pub mod plugin;
pub mod prompts;

pub use self::agentic::{Agent, AgentConfig, FallbackProvider, DEFAULT_SYSTEM_PROMPT};
pub use context::{
    AgentReActApp, AgentReactContext, AgentSession, CompactionFailure, CompactionOutcome,
    MessageContext, SteerMailbox, SteerMessage, CHECKPOINT_PREAMBLE, COMPACTION_INSTRUCTION,
    MAX_STEER_DEPTH,
};
pub use hooks::external::{EventMatcher, ExternalHook, HookDispatcher, HookOutcome};
pub use hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper,
};
pub use prompts::{
    compose_system_prompt, compose_system_prompt_with, guidance_for_tool, DEFAULT_PERSONA,
    HARNESS_IDENTITY, TOOL_GUIDANCE, WORKFLOW_GUIDANCE,
};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};
