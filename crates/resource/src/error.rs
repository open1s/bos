//! Error types for the resource layer.

use thiserror::Error;

/// Errors that can arise while managing or invoking a resource.
#[derive(Debug, Error)]
pub enum ResourceError {
    /// An underlying I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The resource was not found.
    #[error("resource not found: {0}")]
    NotFound(String),

    /// The caller is not permitted to perform the action.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// The policy engine denied the action.
    #[error("policy denied: agent={agent} uri={uri} action={action}")]
    PolicyDenied {
        /// Agent identity.
        agent: String,
        /// Target resource URI.
        uri: String,
        /// Attempted action.
        action: String,
    },

    /// The resource already exists.
    #[error("resource already exists: {0}")]
    AlreadyExists(String),

    /// Advisory lock conflict (`flock`-style): the resource is already locked
    /// by another holder.
    #[error("resource is locked: {0}")]
    Locked(String),

    /// The operation is not supported by this resource kind.
    #[error("operation not supported for this resource kind: {0}")]
    Unsupported(String),

    /// The resource is closed.
    #[error("resource is closed")]
    Closed,

    /// A transport-level error.
    #[error("transport error: {0}")]
    Transport(String),

    /// A serialization or deserialization error.
    #[error("serialization error: {0}")]
    Codec(String),

    /// Any other error.
    #[error("{0}")]
    Other(String),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, ResourceError>;
