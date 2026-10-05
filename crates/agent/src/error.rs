//! Agent error types.
//!
//! The LLM and tool error types are the canonical ones defined in `react`; this
//! crate re-exports them instead of maintaining parallel enums with lossy
//! conversions. `AgentError` is the composition point.

use crate::skills::SkillError;
use thiserror::Error;

pub use react::llm::LlmError;
pub use react::tool::ToolError;

/// Top-level agent errors.
#[derive(Error, Debug, Clone)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Session error: {0}")]
    Session(String),

    #[error("Bus error: {0}")]
    Bus(String),

    #[error("Serialization error: {0}")]
    Serde(String),
}

impl From<SkillError> for AgentError {
    fn from(e: SkillError) -> Self {
        AgentError::Session(e.to_string())
    }
}

impl From<qserde::Error> for AgentError {
    fn from(e: qserde::Error) -> Self {
        AgentError::Serde(e.to_string())
    }
}
