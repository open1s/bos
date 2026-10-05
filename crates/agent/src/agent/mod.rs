//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
#[warn(missing_docs)]
pub mod config;
#[warn(missing_docs)]
pub mod context;
#[warn(missing_docs)]
pub mod hooks;
#[warn(missing_docs)]
pub mod plugin;

pub use self::agentic::{Agent, AgentConfig};
pub use context::{AgentReActApp, AgentReactContext, AgentSession, MessageContext};
pub use hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use plugin::{
    AgentPlugin, LlmRequestWrapper, LlmResponseWrapper, PluginRegistry, StreamTokenWrapper,
    ToolCallWrapper, ToolResultWrapper,
};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};
