use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Tool execution failed: {0}")]
    Failed(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}
