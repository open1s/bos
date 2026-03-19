use std::sync::Arc;
use std::time::Duration;
use zenoh::Session;

use super::{A2AMessage, A2AContent, Task, TaskState, IdempotencyStore, AgentIdentity, ProcessedResult};

pub struct A2AClient {
    session: Arc<Session>,
    identity: AgentIdentity,
    idempotency: IdempotencyStore,
    timeout: Duration,
}

impl A2AClient {
    pub fn new(session: Arc<Session>, identity: AgentIdentity) -> Self {
        Self {
            session,
            identity,
            idempotency: IdempotencyStore::new(),
            timeout: Duration::from_secs(60),
        }
    }

    pub async fn delegate_task(
        &self,
        recipient: &AgentIdentity,
        mut task: Task,
    ) -> Result<Task, crate::error::AgentError> {
        let key = format!("{}:{}", task.task_id, task.created_at);
        if let Some(cached) = self.idempotency.check(&key).await {
            return Ok(Task {
                state: cached.state,
                output: cached.result,
                ..task
            });
        }

        let message_id = uuid::Uuid::new_v4().to_string();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| crate::error::AgentError::Config(e.to_string()))?
            .as_secs();

        let message = A2AMessage {
            message_id: message_id.clone(),
            task_id: task.task_id.clone(),
            context_id: task.context_id.clone(),
            idempotency_key: key.clone(),
            timestamp,
            sender: self.identity.clone(),
            recipient: recipient.clone(),
            content: A2AContent::TaskRequest { task: task.clone() },
        };

        let topic = format!("agent/{}/tasks", recipient.id);
        let publisher = self.session.declare_publisher(&topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let data = serde_json::to_vec(&message)
            .map_err(|e| crate::error::AgentError::Config(e.to_string()))?;

        publisher.put(data).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let response_topic = format!("agent/{}/responses/{}", self.identity.id, message_id);
        let subscriber = self.session.declare_subscriber(&response_topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let deadline = std::time::Instant::now() + self.timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(sample) = subscriber.recv() {
                if let Ok(response) = serde_json::from_slice::<A2AMessage>(&sample.payload().to_bytes()) {
                    if let A2AContent::TaskResponse { task: response_task } = response.content {
                        let result = ProcessedResult {
                            task_id: response_task.task_id.clone(),
                            state: response_task.state,
                            result: response_task.output.clone(),
                            timestamp: response.timestamp,
                        };
                        self.idempotency.record(key, result).await;
                        return Ok(response_task);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        task.state = TaskState::Failed;
        task.error = Some("Task delegation timed out".to_string());
        Ok(task)
    }

    pub async fn poll_status(&self, task_id: &str) -> Result<TaskState, crate::error::AgentError> {
        let status_topic = format!("agent/{}/status/{}", self.identity.id, task_id);
        let subscriber = self.session.declare_subscriber(&status_topic).await
            .map_err(|e| crate::error::AgentError::Bus(e.to_string()))?;

        let deadline = std::time::Instant::now() + self.timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(sample) = subscriber.recv() {
                if let Ok(status) = serde_json::from_slice::<super::TaskStatus>(&sample.payload().to_bytes()) {
                    return Ok(status.state);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Err(crate::error::AgentError::Session("Status poll timed out".to_string()))
    }
}