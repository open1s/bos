//! Zenoh error types

use thiserror::Error;

use serde_json;

#[derive(Error, Debug)]
pub enum ZenohError {
    #[error("Session error: {0}")]
    Session(String),

    #[error("Publisher error: {0}")]
    Publisher(String),

    #[error("Subscriber error: {0}")]
    Subscriber(String),

    #[error("Query error: {0}")]
    Query(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Not connected")]
    NotConnected,

    #[error("Already connected")]
    AlreadyConnected,

    #[error("Already started")]
    AlreadyStarted,

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
