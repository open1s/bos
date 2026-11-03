//! Error helpers shared by SDK-authored extensions.

pub use ferris_core::{ErrorCode, ExtError};

/// SDK result alias.
pub type Result<T> = std::result::Result<T, ExtError>;

fn error(code: ErrorCode, message: impl Into<String>) -> ExtError {
    ExtError {
        code,
        message: message.into(),
        details: serde_json::Value::Null,
    }
}

/// A runtime failure.
pub fn runtime(message: impl Into<String>) -> ExtError {
    error(ErrorCode::Runtime, message)
}

/// A contribution/host failure.
pub fn contribution(message: impl Into<String>) -> ExtError {
    error(ErrorCode::Contribution, message)
}

/// Invalid author input.
pub fn invalid_input(message: impl Into<String>) -> ExtError {
    error(ErrorCode::Manifest, message)
}

/// Attach structured details to an error.
#[must_use]
pub fn with_details(mut error: ExtError, details: serde_json::Value) -> ExtError {
    error.details = details;
    error
}
