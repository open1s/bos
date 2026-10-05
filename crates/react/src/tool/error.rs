use thiserror::Error;

/// Tool execution error.
///
/// This is the canonical tool error for the workspace. `agent` re-exports it
/// rather than defining its own, so a tool author only handles one type.
#[derive(Debug, Error, Clone)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Tool execution failed: {0}")]
    Failed(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Schema mismatch: {message}")]
    SchemaMismatch { message: String },
    #[error("Tool execution timed out")]
    Timeout,
}
