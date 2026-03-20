//! Workflow scheduler module
//! 
//! Provides workflow execution engine with sequential, parallel, and conditional execution,
//! configurable timeout and retry logic.

pub mod dsl;
pub mod retry;
pub mod executor;

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Workflow step types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepType {
    Sequential,
    Parallel,
    Conditional { condition: ConditionType },
}

/// Condition types for branching
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConditionType {
    /// JSON path based condition
    JsonPath { path: String, expected: serde_json::Value },
    /// Script expression based condition (optional)
    Script { expression: String },
}

/// Individual step in workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub step_type: StepType,
    pub agent_id: Option<String>, // None = local, Some = A2A delegation
    pub timeout: Duration,
    pub max_retries: u32,
    pub backoff: BackoffStrategy,
}

/// Complete workflow definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
    pub default_timeout: Duration,
    pub default_retries: u32,
}

/// Retry strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    Exponential { base: Duration, max: Duration },
    Linear { interval: Duration },
    Fixed { interval: Duration },
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(10),
        }
    }
}

/// Workflow execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub workflow_name: String,
    pub status: WorkflowStatus,
    pub step_results: Vec<StepResult>,
    pub duration: Duration,
    pub errors: Vec<String>,
}

/// Step-level result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_name: String,
    pub status: StepStatus,
    pub output: Option<serde_json::Value>,
    pub duration: Duration,
    pub retry_count: u32,
}

/// Workflow status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStatus {
    Completed,
    Failed { failed_step: String },
    PartiallyCompleted,
}

/// Step status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepStatus {
    Completed,
    Failed { error: String },
    TimedOut,
    Skipped,
    Retrying,
}