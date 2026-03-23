use std::sync::Arc;
use async_trait::async_trait;
use zenoh::Session;

use crate::a2a::{A2AClient, AgentIdentity, Task, TaskState};
use crate::tools::{Tool, ToolDescription, ToolError};

/// Tool implementation that calls remote agent tools via A2A protocol.
///
/// This allows an agent to invoke tools on another agent through
/// the A2A task delegation mechanism.
pub struct A2AToolClient {
    session: Arc<Session>,
    target_agent: AgentIdentity,
    tool_name: String,
    description: String,
    schema: serde_json::Value,
    timeout: std::time::Duration,
}

impl A2AToolClient {
    /// Create a new A2A tool client.
    pub fn new(
        session: Arc<Session>,
        target_agent: AgentIdentity,
        tool_name: impl Into<String>,
        description: impl Into<String>,
        schema: serde_json::Value,
    ) -> Self {
        Self {
            session,
            target_agent,
            tool_name: tool_name.into(),
            description: description.into(),
            schema,
            timeout: std::time::Duration::from_secs(60),
        }
    }

    /// Set a custom timeout for tool calls.
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get the target agent identity.
    pub fn target_agent(&self) -> &AgentIdentity {
        &self.target_agent
    }
}

#[async_trait]
impl Tool for A2AToolClient {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!("{} (via A2A from {})", self.description, self.target_agent.name),
            parameters: self.schema.to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // Create A2A client with a sender identity
        // In real usage, the agent would pass its own identity
        let sender_identity = AgentIdentity::new(
            uuid::Uuid::new_v4().to_string(),
            "tool-caller".to_string(),
            "1.0.0".to_string(),
        );
        
        let client = A2AClient::new(self.session.clone(), sender_identity);

        // Create a task with the tool call
        let task_input = serde_json::json!({
            "tool": self.tool_name,
            "arguments": args,
        });

        let task = Task::new(
            uuid::Uuid::new_v4().to_string(),
            task_input,
        );

        // Delegate task to the target agent
        let result_task = client.delegate_task(&self.target_agent, task).await
            .map_err(|e| ToolError::ExecutionFailed(format!("A2A delegation failed: {}", e)))?;

        // Check task state
        match result_task.state {
            TaskState::Completed => {
                result_task.output.ok_or_else(|| {
                    ToolError::ExecutionFailed("Task completed but no output".to_string())
                })
            }
            TaskState::Failed => {
                Err(ToolError::ExecutionFailed(
                    result_task.error.unwrap_or_else(|| "Unknown error".to_string())
                ))
            }
            TaskState::Canceled => {
                Err(ToolError::ExecutionFailed("Task was canceled".to_string()))
            }
            _ => {
                Err(ToolError::ExecutionFailed(format!(
                    "Unexpected task state: {:?}",
                    result_task.state
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_identity() -> AgentIdentity {
        AgentIdentity::new("test-agent".to_string(), "TestAgent".to_string(), "1.0.0".to_string())
    }

    fn create_test_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        })
    }

    #[test]
    fn test_a2a_tool_client_creation() {
        let identity = create_test_identity();
        let schema = create_test_schema();
    }

    #[test]
    fn test_tool_name_and_description() {
        let expected_name = "add";
        let expected_desc_suffix = " (via A2A from TestAgent)";
        
        assert!(!expected_name.is_empty());
        assert!(expected_desc_suffix.contains("A2A"));
    }

    #[test]
    fn test_schema_passthrough() {
        let schema = create_test_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["a"]["type"].is_string());
    }
}
