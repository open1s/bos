//! Error types for qserde operations

use crate::Error as QserdeLibError;
use thiserror::Error;

/// Unified error enum for qserde operations
#[derive(Debug, Error)]
pub enum QserdeError {
    #[error("serialization failed: {0}")]
    Serialize(String),

    #[error("deserialization failed: {0}")]
    Deserialize(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("backend not supported for this operation: {0}")]
    UnsupportedBackend(String),
}

impl From<QserdeLibError> for QserdeError {
    fn from(e: QserdeLibError) -> Self {
        match e {
            QserdeLibError::Serialize(rkyv_err) => QserdeError::Serialize(rkyv_err.to_string()),
            QserdeLibError::Deserialize(rkyv_err) => QserdeError::Deserialize(rkyv_err.to_string()),
        }
    }
}

impl QserdeError {
    pub fn serialize(msg: impl Into<String>) -> Self {
        Self::Serialize(msg.into())
    }

    pub fn deserialize(msg: impl Into<String>) -> Self {
        Self::Deserialize(msg.into())
    }

    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend(msg.into())
    }

    pub fn is_serialize_error(&self) -> bool {
        matches!(self, QserdeError::Serialize(_))
    }

    pub fn is_deserialize_error(&self) -> bool {
        matches!(self, QserdeError::Deserialize(_))
    }
}

/// Result type using QserdeError
pub type Result<T> = core::result::Result<T, QserdeError>;

// Convenience From implementations for backend errors
#[cfg(feature = "rkyv-backend")]
impl From<rkyv::rancor::Error> for QserdeError {
    fn from(e: rkyv::rancor::Error) -> Self {
        QserdeError::backend(e.to_string())
    }
}

#[cfg(feature = "bincode-backend")]
impl From<bincode::error::EncodeError> for QserdeError {
    fn from(e: bincode::error::EncodeError) -> Self {
        QserdeError::backend(e.to_string())
    }
}

#[cfg(feature = "serde-backend")]
impl From<serde_json::Error> for QserdeError {
    fn from(e: serde_json::Error) -> Self {
        QserdeError::backend(e.to_string())
    }
}
