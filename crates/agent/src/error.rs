//! Agent error types.
//!
//! The LLM and tool error types are the canonical ones defined in `react`; this
//! crate re-exports them instead of maintaining parallel enums with lossy
//! conversions. `AgentError` is the composition point.

use crate::agent::context::CompactionFailure;
use crate::skills::SkillError;
use thiserror::Error;

pub use react::llm::LlmError;
pub use react::tool::ToolError;

/// Top-level agent errors.
#[derive(Error, Debug, Clone)]
pub enum AgentError {
    /// An LLM call failed.
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    /// A tool call failed.
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    /// Configuration was invalid.
    #[error("Configuration error: {0}")]
    Config(String),

    /// A session or skill operation failed.
    #[error("Session error: {0}")]
    Session(String),

    /// A compaction attempt failed for a classified reason.
    ///
    /// Transparent so the failure keeps its own real wording: a caller that
    /// matches this variant can report the provider's text, an empty answer, or
    /// a shrink-gate refusal instead of one generic sentence.
    #[error(transparent)]
    Compaction(#[from] CompactionFailure),

    /// A bus RPC operation failed.
    #[error("Bus error: {0}")]
    Bus(String),

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serde(String),

    /// The caller cancelled the operation.
    #[error("operation stopped by the caller")]
    Stopped,
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
