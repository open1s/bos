//! Zenoh error types

use thiserror::Error;

use serde_json;

/// Errors produced by the bus layer.
#[derive(Error, Debug)]
pub enum ZenohError {
    /// The underlying Zenoh session failed.
    #[error("Session error: {0}")]
    Session(String),

    /// A publish failed.
    #[error("Publisher error: {0}")]
    Publisher(String),

    /// A subscription failed.
    #[error("Subscriber error: {0}")]
    Subscriber(String),

    /// A query or reply failed.
    #[error("Query error: {0}")]
    Query(String),

    /// A value could not be encoded or decoded.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// The operation needs a session, but none is attached.
    #[error("Not connected")]
    NotConnected,

    /// The session is already connected.
    #[error("Already connected")]
    AlreadyConnected,

    /// The queryable or callable was started twice.
    #[error("Already started")]
    AlreadyStarted,

    /// The operation exceeded its timeout.
    #[error("Operation timed out")]
    Timeout,
}

impl From<zenoh::Error> for ZenohError {
    fn from(err: zenoh::Error) -> Self {
        ZenohError::Session(err.to_string())
    }
}

impl From<serde_json::Error> for ZenohError {
    fn from(err: serde_json::Error) -> Self {
        ZenohError::Serialization(err.to_string())
    }
}

impl From<tokio::time::error::Elapsed> for ZenohError {
    fn from(_err: tokio::time::error::Elapsed) -> Self {
        ZenohError::Timeout
    }
}

impl AsRef<str> for ZenohError {
    fn as_ref(&self) -> &str {
        match self {
            ZenohError::Session(s) => s,
            ZenohError::Publisher(s) => s,
            ZenohError::Subscriber(s) => s,
            ZenohError::Query(s) => s,
            ZenohError::Serialization(s) => s,
            ZenohError::NotConnected => "Not connected",
            ZenohError::AlreadyConnected => "Already connected",
            ZenohError::AlreadyStarted => "Already started",
            ZenohError::Timeout => "Operation timed out",
        }
    }
}
