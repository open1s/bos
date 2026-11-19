//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;
pub mod hooks;
pub mod plugin;
pub mod prompts;

pub use self::agentic::{
    Agent, AgentConfig, ConditionalPrompt, DEFAULT_SYSTEM_PROMPT, FallbackProvider,
};
pub use context::{
    AgentReActApp, AgentReactContext, AgentSession, CHECKPOINT_PREAMBLE, COMPACTION_INSTRUCTION,
    CompactionFailure, CompactionOutcome, MAX_STEER_DEPTH, MessageContext, SteerMailbox,
    SteerMessage,
};
pub use hooks::external::{EventMatcher, ExternalHook, HookDispatcher, HookOutcome};
pub use hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper, TurnEndWrapper, TurnStartWrapper,
};
pub use prompts::{
    DEFAULT_PERSONA, HARNESS_IDENTITY, TOOL_GUIDANCE, WORKFLOW_GUIDANCE, compose_system_prompt,
    compose_system_prompt_with, guidance_for_tool,
};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};
