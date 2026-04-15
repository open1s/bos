//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;
pub mod hooks;

pub use agentic::{Agent, AgentBuilder, AgentConfig, AgentOutput};
pub use hooks::{AgentHook, HookContext, HookEvent, HookRegistry};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};
