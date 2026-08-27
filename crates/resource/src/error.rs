//! Error types for the resource layer.

use thiserror::Error;

/// Errors that can arise while managing or invoking a resource.
#[derive(Debug, Error)]
pub enum ResourceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("policy denied: agent={agent} uri={uri} action={action}")]
    PolicyDenied {
        agent: String,
        uri: String,
        action: String,
    },

    #[error("resource already exists: {0}")]
    AlreadyExists(String),

    /// Advisory lock conflict (`flock`-style): the resource is already locked
    /// by another holder.
    #[error("resource is locked: {0}")]
    Locked(String),

    #[error("operation not supported for this resource kind: {0}")]
    Unsupported(String),

    #[error("resource is closed")]
    Closed,

    #[error("transport error: {0}")]
    Transport(String),

    #[error("serialization error: {0}")]
    Codec(String),

    #[error("{0}")]
    Other(String),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, ResourceError>;