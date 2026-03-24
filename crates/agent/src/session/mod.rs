pub mod serializer;
pub mod storage;
pub mod manager;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub agent_id: String,
    pub message_log: Vec<Message>,
    pub context: serde_json::Value,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub created_at: u64,
    pub updated_at: u64,
    pub expires_at: Option<u64>,
    pub message_count: usize,
    pub agent_version: String,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub agent_id: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub labels: Vec<String>,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub base_dir: PathBuf,
    pub default_ttl_secs: Option<u64>,
    pub compression_enabled: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from(".bos/sessions"),
            default_ttl_secs: None,
            compression_enabled: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Session already exists: {0}")]
    AlreadyExists(String),

    #[error("Session expired: {0}")]
    Expired(String),
}

pub use manager::SessionManager;
pub use serializer::SessionSerializer;
use crate::agent::message::Message;
