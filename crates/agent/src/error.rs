//! Agent error types
//!
//! Provides typed errors for the agent crate following the pattern from `brainos-bus`.

use crate::skills::SkillError;
use react::llm::LlmError as ReactLlmError;
use thiserror::Error;

/// Errors from LLM client operations.
#[derive(Error, Debug, Clone)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Request timed out")]
    Timeout,

    #[error("API key is missing")]
    ApiKeyMissing,

    #[error("Rate limited")]
    RateLimited,
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            LlmError::Timeout
        } else {
            LlmError::Http(e.to_string())
        }
    }
}

impl From<ReactLlmError> for LlmError {
    fn from(e: ReactLlmError) -> Self {
        match e {
            ReactLlmError::Http(s) => LlmError::Http(s),
            ReactLlmError::Parse(s) => LlmError::Parse(s),
            ReactLlmError::Timeout => LlmError::Timeout,
            ReactLlmError::ApiKeyMissing => LlmError::ApiKeyMissing,
            ReactLlmError::RateLimited => LlmError::RateLimited,
            ReactLlmError::Other(s) => LlmError::Http(s),
        }
    }
}

/// Errors from tool execution.
#[derive(Error, Debug, Clone)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Schema mismatch: {message}")]
    SchemaMismatch { message: String },

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Tool execution timed out")]
    Timeout,
}

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
}

impl From<ReactLlmError> for AgentError {
    fn from(e: ReactLlmError) -> Self {
        AgentError::Llm(e.into())
    }
}

impl From<SkillError> for AgentError {
    fn from(e: SkillError) -> Self {
        AgentError::Session(e.to_string())
    }
}
