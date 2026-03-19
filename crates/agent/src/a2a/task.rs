use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub context_id: Option<String>,
    pub state: TaskState,
    pub created_at: u64,
    pub updated_at: u64,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Canceled,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }

    pub fn can_transition_to(self, next: TaskState) -> bool {
        match (self, next) {
            (Self::Submitted, Self::Working) => true,
            (Self::Submitted, Self::InputRequired) => true,
            (Self::Submitted, Self::Failed) => true,
            (Self::Submitted, Self::Canceled) => true,
            (Self::Working, Self::Completed) => true,
            (Self::Working, Self::Failed) => true,
            (Self::Working, Self::InputRequired) => true,
            (Self::Working, Self::Canceled) => true,
            (Self::InputRequired, Self::Working) => true,
            (Self::InputRequired, Self::Canceled) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: TaskState,
    pub message: Option<String>,
}
