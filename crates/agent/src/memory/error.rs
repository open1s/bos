//! Typed memory failures (issue #342).
//!
//! A memory operation reports success only after its durable commit succeeded.
//! Every failure mode a repository can hit has a variant here, so callers tell
//! "persisted" from "rejected" from "failed" without parsing log lines.

/// A typed memory repository failure.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// The durable commit kernel failed; the stage and retryability are kept.
    #[error(transparent)]
    Storage(#[from] hostkit::durable::DurableError),
    /// A record could not be serialized; nothing was committed.
    #[error("memory serialization failed: {0}")]
    Serialization(String),
    /// The store file exists but cannot be read as records; it was preserved.
    #[error("memory store {path} is corrupt: {detail}")]
    Corrupt {
        /// The store path.
        path: String,
        /// What made it unreadable.
        detail: String,
    },
    /// The addressed record does not exist (or is already tombstoned).
    #[error("memory item {id} was not found")]
    NotFound {
        /// The record id.
        id: String,
    },
    /// The caller's scope does not permit the operation.
    #[error("memory scope denied: {reason}")]
    ScopeDenied {
        /// Why the scope rule refused.
        reason: String,
    },
    /// A concurrent commit changed the record under the prepared update.
    #[error("memory conflict: {0}")]
    Conflict(String),
    /// Memory is configured but not usable right now.
    #[error("memory unavailable: {0}")]
    Unavailable(String),
}

impl MemoryError {
    /// Whether retrying the same operation later can succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            MemoryError::Storage(error) => error.is_retryable(),
            MemoryError::Unavailable(_) => true,
            MemoryError::Serialization(_)
            | MemoryError::Corrupt { .. }
            | MemoryError::NotFound { .. }
            | MemoryError::ScopeDenied { .. }
            | MemoryError::Conflict(_) => false,
        }
    }

    /// A short, stable label for diagnostics (never carries content).
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            MemoryError::Storage(_) => "storage",
            MemoryError::Serialization(_) => "serialization",
            MemoryError::Corrupt { .. } => "corrupt",
            MemoryError::NotFound { .. } => "not-found",
            MemoryError::ScopeDenied { .. } => "scope-denied",
            MemoryError::Conflict(_) => "conflict",
            MemoryError::Unavailable(_) => "unavailable",
        }
    }
}

impl From<serde_json::Error> for MemoryError {
    fn from(error: serde_json::Error) -> Self {
        MemoryError::Serialization(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_follows_the_kernel() {
        let io = MemoryError::Storage(hostkit::durable::DurableError::Io {
            stage: hostkit::durable::IoStage::Write,
            path: "m.jsonl".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x"),
        });
        let corrupt = MemoryError::Corrupt {
            path: "m.jsonl".into(),
            detail: "line 3".into(),
        };
        assert!(io.is_retryable());
        assert!(!corrupt.is_retryable());
        assert_eq!(io.label(), "storage");
        assert_eq!(corrupt.label(), "corrupt");
    }
}
