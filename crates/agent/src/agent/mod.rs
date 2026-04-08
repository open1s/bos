//! Core agent types: Message, MessageLog, Agent, AgentConfig.

pub mod agentic;
pub mod config;
pub mod context;

pub use agentic::{Agent, AgentBuilder, AgentConfig, AgentOutput};
pub use react::llm::{LlmMessage as Message, LlmRequest, LlmResponse, StreamToken};

fn format_tool_result_content(result: serde_json::Value) -> String {
    match result {
        serde_json::Value::String(content) => content,
        other => serde_json::to_string(&other).unwrap_or_else(|_| other.to_string()),
    }
}