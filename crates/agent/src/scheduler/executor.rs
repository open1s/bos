use super::{Workflow, WorkflowResult, StepResult, WorkflowStatus, StepStatus, ConditionType, StepType, Step};
use crate::a2a::AgentIdentity;
use crate::a2a::client::A2AClient;
use crate::a2a::task::Task;
use std::time::Duration;
use tokio::time::timeout;

pub struct Scheduler {
    a2a_client: Option<A2AClient>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            a2a_client: None,
        }
    }

    pub fn with_a2a_client(mut self, client: A2AClient) -> Self {
        self.a2a_client = Some(client);
        self
    }

    pub async fn execute_workflow(&self, workflow: &Workflow) -> WorkflowResult {
        let start = std::time::Instant::now();
        let mut step_results = Vec::new();
        let errors = Vec::new();
        let mut prev_output: Option<serde_json::Value> = None;

        for step in &workflow.steps {
            let step_result = self.execute_step(step, prev_output.clone()).await;

            match &step_result.status {
                StepStatus::Completed => {
                    prev_output = step_result.output.clone();
                    step_results.push(step_result);
                }
                StepStatus::Failed { error: _ } | StepStatus::TimedOut => {
                    step_results.push(step_result);
                    return WorkflowResult {
                        workflow_name: workflow.name.clone(),
                        status: WorkflowStatus::Failed {
                            failed_step: step.name.clone(),
                        },
                        step_results,
                        duration: start.elapsed(),
                        errors,
                    };
                }
                StepStatus::Skipped => {
                    prev_output = step_result.output.clone();
                    step_results.push(step_result);
                }
                StepStatus::Retrying => {
                    step_results.push(step_result);
                }
            }
        }

        WorkflowResult {
            workflow_name: workflow.name.clone(),
            status: WorkflowStatus::Completed,
            step_results,
            duration: start.elapsed(),
            errors,
        }
    }

    async fn execute_step(
        &self,
        step: &Step,
        input: Option<serde_json::Value>,
    ) -> StepResult {
        let start = std::time::Instant::now();
        let mut attempts = 0u32;

        loop {
            attempts += 1;

            let step_output = timeout(step.timeout, async {
                match &step.step_type {
                    StepType::Sequential => {
                        self.execute_sequential(step, input.clone()).await
                    }
                    StepType::Parallel => {
                        self.execute_parallel(step, input.clone()).await
                    }
                    StepType::Conditional { condition } => {
                        self.execute_conditional(step, input.clone(), condition).await
                    }
                }
            })
            .await;

            match step_output {
                Ok(Ok((output, duration))) => {
                    return StepResult {
                        step_name: step.name.clone(),
                        status: StepStatus::Completed,
                        output,
                        duration: start.elapsed() + duration,
                        retry_count: attempts - 1,
                    };
                }
                Ok(Err(e)) => {
                    if attempts < step.max_retries {
                        let backoff = step.backoff.calculate_backoff(attempts - 1);
                        tokio::time::sleep(backoff).await;
                        continue;
                    } else {
                        return StepResult {
                            step_name: step.name.clone(),
                            status: StepStatus::Failed {
                                error: e.to_string(),
                            },
                            output: None,
                            duration: start.elapsed(),
                            retry_count: attempts - 1,
                        };
                    }
                }
                Err(_) => {
                    return StepResult {
                        step_name: step.name.clone(),
                        status: StepStatus::TimedOut,
                        output: None,
                        duration: start.elapsed(),
                        retry_count: attempts - 1,
                    };
                }
            }
        }
    }

    async fn execute_sequential(
        &self,
        _step: &Step,
        input: Option<serde_json::Value>,
    ) -> Result<(Option<serde_json::Value>, Duration), anyhow::Error> {
        let start = std::time::Instant::now();

        let value = input
            .and_then(|v| v.get("value").and_then(|v| v.as_i64()))
            .unwrap_or(0);

        let result = value + 1;

        let output = Some(serde_json::json!({ "value": result }));

        Ok((output, start.elapsed()))
    }

    async fn execute_parallel(
        &self,
        _step: &Step,
        input: Option<serde_json::Value>,
    ) -> Result<(Option<serde_json::Value>, Duration), anyhow::Error> {
        let start = std::time::Instant::now();

        let value = input
            .and_then(|v| v.get("value").and_then(|v| v.as_i64()))
            .unwrap_or(0);

        let results = futures::future::join_all(vec![
            tokio::spawn(async move { value + 1 }),
            tokio::spawn(async move { value * 2 }),
            tokio::spawn(async move { value - 1 }),
        ])
        .await;

        let r1 = results[0].as_ref().map_err(|e| anyhow::anyhow!("Task 1 failed: {}", e))?;
        let r2 = results[1].as_ref().map_err(|e| anyhow::anyhow!("Task 2 failed: {}", e))?;
        let r3 = results[2].as_ref().map_err(|e| anyhow::anyhow!("Task 3 failed: {}", e))?;

        let output = Some(serde_json::json!({
            "results": [*r1, *r2, *r3],
            "count": 3
        }));

        Ok((output, start.elapsed()))
    }

    async fn execute_conditional(
        &self,
        _step: &Step,
        input: Option<serde_json::Value>,
        condition: &ConditionType,
    ) -> Result<(Option<serde_json::Value>, Duration), anyhow::Error> {
        let start = std::time::Instant::now();

        let default_value = serde_json::json!({});
        let input_value = input.as_ref().unwrap_or(&default_value);
        let should_take_high_branch = Self::evaluate_condition(condition, input_value);

        let value = input
            .and_then(|v| v.get("value").and_then(|v| v.as_i64()))
            .unwrap_or(0);

        if should_take_high_branch {
            let output = Some(serde_json::json!({ "value": value * 10, "branch": "high" }));
            Ok((output, start.elapsed()))
        } else {
            let output = Some(serde_json::json!({ "value": value + 5, "branch": "low" }));
            Ok((output, start.elapsed()))
        }
    }

    #[allow(dead_code)]
    async fn execute_remotely(
        &self,
        agent_id: &str,
        input: Option<serde_json::Value>,
    ) -> Result<(Option<serde_json::Value>, Duration), anyhow::Error> {
        let start = std::time::Instant::now();

        let client = self.a2a_client.as_ref()
            .ok_or_else(|| anyhow::anyhow!("A2A client not configured"))?;

        let task = Task::new(
            uuid::Uuid::new_v4().to_string(),
            input.unwrap_or(serde_json::json!({})),
        );

        let recipient_identity = AgentIdentity::new(
            agent_id.to_string(),
            agent_id.to_string(),
            "1.0.0".to_string(),
        );

        match client.delegate_task(&recipient_identity, task).await {
            Ok(response_task) => {
                let output = response_task.output;
                Ok((output, start.elapsed()))
            }
            Err(e) => Err(anyhow::anyhow!("Remote execution failed: {}", e)),
        }
    }

    pub fn evaluate_condition(condition: &ConditionType, output: &serde_json::Value) -> bool {
        match condition {
            ConditionType::JsonPath { path, expected } => {
                output.get(path) == Some(expected)
            }
            ConditionType::Script { expression: _ } => {
                false
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
