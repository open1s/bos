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

impl Task {
    pub fn new(task_id: String, input: serde_json::Value) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            task_id,
            context_id: None,
            state: TaskState::Submitted,
            created_at: now,
            updated_at: now,
            input,
            output: None,
            error: None,
        }
    }

    pub fn with_context(mut self, context_id: String) -> Self {
        self.context_id = Some(context_id);
        self
    }

    pub fn with_state(mut self, state: TaskState) -> Self {
        if self.state == TaskState::Submitted && state != TaskState::Submitted {
            self.update_timestamp();
            self.state = state;
        }
        self
    }

    pub fn with_output(mut self, output: serde_json::Value) -> Self {
        self.output = Some(output);
        self
    }

    pub fn with_error(mut self, error: String) -> Self {
        self.error = Some(error);
        self.state = TaskState::Failed;
        self.update_timestamp();
        self
    }

    fn update_timestamp(&mut self) {
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }

    pub fn transition_to(&mut self, new_state: TaskState) -> Result<(), String> {
        if !self.state.can_transition_to(new_state) {
            return Err(format!(
                "Invalid transition from {:?} to {:?}",
                self.state, new_state
            ));
        }
        self.state = new_state;
        self.update_timestamp();
        Ok(())
    }

    pub fn complete(&mut self, output: serde_json::Value) {
        self.state = TaskState::Completed;
        self.output = Some(output);
        self.error = None;
        self.update_timestamp();
    }

    pub fn fail(&mut self, error: String) {
        self.state = TaskState::Failed;
        self.error = Some(error);
        self.output = None;
        self.update_timestamp();
    }

    pub fn require_input(&mut self, prompt: String) {
        self.state = TaskState::InputRequired;
        self.error = Some(prompt);
        self.update_timestamp();
    }

    pub fn cancel(&mut self) {
        if !self.state.is_terminal() {
            self.state = TaskState::Canceled;
            self.update_timestamp();
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, TaskState::Submitted | TaskState::InputRequired)
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, TaskState::Working)
    }

    pub fn duration(&self) -> u64 {
        self.updated_at - self.created_at
    }

    pub fn clone_for_retry(&self) -> Self {
        let mut task = self.clone();
        task.task_id = format!("{}-retry", self.task_id);
        task.state = TaskState::Submitted;
        task.error = None;
        task.output = None;
        task.update_timestamp();
        task
    }

    pub fn can_retry(&self) -> bool {
        matches!(self.state, TaskState::Failed)
    }

    pub fn valid_actions(&self) -> Vec<&'static str> {
        match self.state {
            TaskState::Submitted => vec!["start", "cancel"],
            TaskState::Working => vec!["complete", "fail", "cancel", "request_input"],
            TaskState::InputRequired => vec!["provide_input", "cancel"],
            TaskState::Completed | TaskState::Failed | TaskState::Canceled => vec!["retry"],
        }
    }

    pub fn apply_action(&mut self, action: &str) -> Result<(), String> {
        if !self.valid_actions().contains(&action) {
            return Err(format!(
                "Invalid action '{}' for state {:?}",
                action, self.state
            ));
        }

        match action {
            "start" => self.transition_to(TaskState::Working)?,
            "retry" => *self = self.clone_for_retry(),
            "cancel" => self.cancel(),
            _ => return Err(format!("Action '{}' not implemented", action)),
        }

        Ok(())
    }
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

impl TaskStatus {
    pub fn new(task_id: String, state: TaskState) -> Self {
        Self {
            task_id,
            state,
            message: None,
        }
    }

    pub fn with_message(mut self, message: String) -> Self {
        self.message = Some(message);
        self
    }

    pub fn started(task_id: String) -> Self {
        Self::new(task_id, TaskState::Working).with_message("Task started".to_string())
    }

    pub fn awaiting_input(task_id: String, prompt: String) -> Self {
        Self::new(task_id, TaskState::InputRequired).with_message(prompt)
    }

    pub fn success(task_id: String) -> Self {
        Self::new(task_id, TaskState::Completed)
            .with_message("Task completed successfully".to_string())
    }

    pub fn failed(task_id: String, error: String) -> Self {
        Self::new(task_id, TaskState::Failed).with_message(error)
    }

    pub fn canceled(task_id: String) -> Self {
        Self::new(task_id, TaskState::Canceled).with_message("Task was canceled".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: TaskState,
    pub message: Option<String>,
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submitted => write!(f, "Submitted"),
            Self::Working => write!(f, "Working"),
            Self::InputRequired => write!(f, "InputRequired"),
            Self::Completed => write!(f, "Completed"),
            Self::Failed => write!(f, "Failed"),
            Self::Canceled => write!(f, "Canceled"),
        }
    }
}

impl std::fmt::Display for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Task[{}] state={} input={} output={} error={:?}",
            self.task_id,
            self.state,
            self.input,
            self.output
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "empty".to_string()),
            self.error
        )
    }
}
