use react::llm::LlmMessage as Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub agent_id: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
}

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
    pub message_count: usize,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
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

pub struct SessionSerializer;

impl SessionSerializer {
    pub fn new_state(agent_id: String, _workspace: Option<String>) -> AgentState {
        AgentState::new(agent_id)
    }

    pub fn update_metadata(state: &mut AgentState) {
        state.metadata.updated_at = current_timestamp();
        state.metadata.message_count = state.message_log.len();
    }

    pub fn serialize(state: &AgentState) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(state)
    }

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

pub mod manager;

pub use manager::SessionError;
pub use manager::SessionManager;
