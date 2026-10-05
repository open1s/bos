//! Session state types, serialization, and the session manager.

use react::llm::LlmMessage as Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A lightweight view of one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Identifier of the owning agent.
    pub agent_id: String,
    /// Unix timestamp when the session was created.
    pub created_at: u64,
    /// Unix timestamp of the last update.
    pub updated_at: u64,
    /// Number of messages recorded so far.
    pub message_count: usize,
}

/// The full persisted state of one agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Identifier of the owning agent.
    pub agent_id: String,
    /// Conversation history.
    pub message_log: Vec<Message>,
    /// Opaque per-agent context.
    pub context: serde_json::Value,
    /// Timestamps and counters for the session.
    pub metadata: SessionMetadata,
}

/// Timestamps and counters stored alongside an [`AgentState`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unix timestamp when the session was created.
    pub created_at: u64,
    /// Unix timestamp of the last update.
    pub updated_at: u64,
    /// Number of messages recorded so far.
    pub message_count: usize,
}

/// Where session files are stored.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Directory used for sessions, `.bos/sessions` by default.
    pub base_dir: PathBuf,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from(".bos/sessions"),
        }
    }
}

impl AgentState {
    /// Build an empty state for `agent_id`, stamped with the current time.
    pub fn new(agent_id: String) -> Self {
        let now = current_timestamp();
        Self {
            agent_id,
            message_log: Vec::new(),
            context: serde_json::Value::Null,
            metadata: SessionMetadata {
                created_at: now,
                updated_at: now,
                message_count: 0,
            },
        }
    }
}

/// Helpers for building and (de)serializing [`AgentState`] values.
pub struct SessionSerializer;

impl SessionSerializer {
    /// Build a fresh state for `agent_id`.
    pub fn new_state(agent_id: String, _workspace: Option<String>) -> AgentState {
        AgentState::new(agent_id)
    }

    /// Refresh the timestamp and message count on `state`.
    pub fn update_metadata(state: &mut AgentState) {
        state.metadata.updated_at = current_timestamp();
        state.metadata.message_count = state.message_log.len();
    }

    /// Serialize `state` to JSON bytes.
    pub fn serialize(state: &AgentState) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(state)
    }

    /// Deserialize an [`AgentState`] from JSON bytes.
    pub fn deserialize(bytes: &[u8]) -> Result<AgentState, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The in-memory session manager and its error type.
pub mod manager;

pub use manager::SessionError;
pub use manager::SessionManager;
