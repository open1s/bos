//! RPC error types

use thiserror::Error;

/// Client-side RPC errors.
///
/// These errors occur on the client side during RPC operations,
/// distinct from service-side errors which are carried in `RpcResponse::Err`.
#[derive(Error, Debug, Clone)]
pub enum RpcError {
    #[error("RPC call timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("No service responded at {topic}")]
    NotFound { topic: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Network error: {0}")]
    Network(String),
}

impl From<serde_json::Error> for RpcError {
    fn from(e: serde_json::Error) -> Self {
        RpcError::Serialization(e.to_string())
    }
}

impl From<zenoh::Error> for RpcError {
    fn from(e: zenoh::Error) -> Self {
        RpcError::Network(e.to_string())
    }
}

/// Service-side error carried in `RpcResponse::Err`.
///
/// This represents errors that occur within the service itself,
/// as opposed to `RpcError` which represents client-side errors.
#[derive(Error, Debug, Clone)]
pub enum RpcServiceError {
    #[error("Service error [{code}]: {message}")]
    Business { code: u32, message: String },

    #[error("Internal error: {0}")]
    Internal(String),
}
