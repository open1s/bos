use super::Task;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentIdentity {
    pub id: String,
    pub name: String,
    pub version: String,
}

impl AgentIdentity {
    /// Create a new agent identity
    pub fn new(id: String, name: String, version: String) -> Self {
        Self { id, name, version }
    }

    /// Create a default identity for testing
    pub fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "default-agent".to_string(),
            version: "0.1.0".to_string(),
        }
    }
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

impl A2AMessage {
    /// Create a new message with auto-generated message_id and timestamp
    pub fn new(
        task_id: String,
        sender: AgentIdentity,
        recipient: AgentIdentity,
        content: A2AContent,
    ) -> Self {
        Self {
            message_id: uuid::Uuid::new_v4().to_string(),
            task_id,
            context_id: None,
            idempotency_key: String::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            sender,
            recipient,
            content,
        }
    }

    /// Set the context_id for grouping related tasks
    pub fn with_context(mut self, context_id: String) -> Self {
        self.context_id = Some(context_id);
        self
    }

    /// Set the idempotency key for deduplication
    pub fn with_idempotency_key(mut self, key: String) -> Self {
        self.idempotency_key = key;
        self
    }

    /// Validate required fields are present
    pub fn validate(&self) -> Result<(), String> {
        if self.message_id.is_empty() {
            return Err("message_id cannot be empty".to_string());
        }
        if self.task_id.is_empty() {
            return Err("task_id cannot be empty".to_string());
        }
        if self.sender.id.is_empty() || self.recipient.id.is_empty() {
            return Err("sender and recipient ids cannot be empty".to_string());
        }
        Ok(())
    }

    /// Get a correlation ID for response routing (message_id)
    pub fn correlation_id(&self) -> &str {
        &self.message_id
    }

    /// Create a TaskRequest message
    pub fn task_request(task: Task, sender: AgentIdentity, recipient: AgentIdentity) -> Self {
        Self::new(
            task.task_id.clone(),
            sender,
            recipient,
            A2AContent::TaskRequest { task },
        )
    }

    /// Create a TaskResponse message
    pub fn task_response(task: Task, sender: AgentIdentity, recipient: AgentIdentity) -> Self {
        Self::new(
            task.task_id.clone(),
            sender,
            recipient,
            A2AContent::TaskResponse { task },
        )
    }

    /// Create a TaskStatus update message
    pub fn task_status(
        task_id: String,
        state: super::TaskState,
        sender: AgentIdentity,
        recipient: AgentIdentity,
    ) -> Self {
        Self::new(
            task_id.clone(),
            sender,
            recipient,
            A2AContent::TaskStatus { task_id, state },
        )
    }

    /// Create an InputRequired message when task needs human input
    pub fn input_required(
        task_id: String,
        prompt: String,
        sender: AgentIdentity,
        recipient: AgentIdentity,
    ) -> Self {
        Self::new(
            task_id.clone(),
            sender,
            recipient,
            A2AContent::InputRequired { task_id, prompt },
        )
    }
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

impl super::TaskState {
    pub fn with_message(self, task_id: String, message: Option<String>) -> super::TaskStatus {
        super::TaskStatus {
            task_id,
            state: self,
            message,
        }
    }

    /// All possible transitions from this state
    pub fn valid_transitions(&self) -> Vec<super::TaskState> {
        match self {
            Self::Submitted => vec![
                Self::Working,
                Self::InputRequired,
                Self::Failed,
                Self::Canceled,
            ],
            Self::Working => vec![
                Self::Completed,
                Self::Failed,
                Self::InputRequired,
                Self::Canceled,
            ],
            Self::InputRequired => vec![Self::Working, Self::Canceled],
            Self::Completed | Self::Failed | Self::Canceled => vec![],
        }
    }
}
