use thiserror::Error;

/// Tool execution error.
///
/// This is the canonical tool error for the workspace. `agent` re-exports it
/// rather than defining its own, so a tool author only handles one type.
#[derive(Debug, Error, Clone)]
pub enum ToolError {
    /// No tool with that name is registered.
    #[error("Tool not found: {0}")]
    NotFound(String),
    /// The tool ran but failed.
    #[error("Tool execution failed: {0}")]
    Failed(String),
    /// The arguments were rejected.
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    /// The arguments did not match the tool schema.
    #[error("Schema mismatch: {message}")]
    SchemaMismatch {
        /// Description of the mismatch.
        message: String,
    },
    /// The tool did not finish in time.
    #[error("Tool execution timed out")]
    Timeout,
}
