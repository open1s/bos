use super::Task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub message_id: String,
    pub task_id: String,
    pub context_id: Option<String>,
    pub idempotency_key: String,
    pub timestamp: u64,
    pub sender: AgentIdentity,
    pub recipient: AgentIdentity,
    pub content: A2AContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum A2AContent {
    TaskRequest {
        task: Task,
    },
    TaskResponse {
        task: Task,
    },
    TaskStatus {
        task_id: String,
        state: super::TaskState,
    },
    InputRequired {
        task_id: String,
        prompt: String,
    },
}
