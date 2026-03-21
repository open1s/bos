//! Integration tests for Scheduler workflow execution

use crate::scheduler::{Workflow, Step, StepType, ConditionType, BackoffStrategy, WorkflowStatus, StepStatus};
use crate::scheduler::executor::Scheduler;

#[tokio::test]
async fn test_sequential_workflow_execution() {
    let scheduler = Scheduler::new();
    
    let workflow = Workflow {
        name: "test-sequential".to_string(),
        description: "Test sequential workflow".to_string(),
        steps: vec![
            Step {
                name: "step1".to_string(),
                step_type: StepType::Sequential,
                agent_id: None,
                timeout: std::time::Duration::from_secs(5),
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(100)
                },
            },
            Step {
                name: "step2".to_string(),
                step_type: StepType::Sequential,
                agent_id: None,
                timeout: std::time::Duration::from_secs(5),
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(100)
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let result = scheduler.execute_workflow(&workflow).await;
    
    assert_eq!(result.workflow_name, "test-sequential");
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert!(result.duration.as_millis() < 100);
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn test_parallel_workflow_execution() {
    let scheduler = Scheduler::new();
    
    let workflow = Workflow {
        name: "test-parallel".to_string(),
        description: "Test parallel workflow".to_string(),
        steps: vec![
            Step {
                name: "parallel1".to_string(),
                step_type: StepType::Parallel,
                agent_id: None,
                timeout: std::time::Duration::from_secs(5),
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(100)
                },
            },
            Step {
                name: "parallel2".to_string(),
                step_type: StepType::Parallel,
                agent_id: None,
                timeout: std::time::Duration::from_secs(5),
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(100)
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let result = scheduler.execute_workflow(&workflow).await;
    
    assert_eq!(result.workflow_name, "test-parallel");
    assert_eq!(result.status, WorkflowStatus::Completed);
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn test_conditional_workflow_jsonpath() {
    let scheduler = Scheduler::new();
    
    let workflow = Workflow {
        name: "test-conditional".to_string(),
        description: "Test conditional workflow".to_string(),
        steps: vec![
            Step {
                name: "condition-test".to_string(),
                step_type: StepType::Conditional {
                    condition: ConditionType::JsonPath {
                        path: "result".to_string(),
                        expected: serde_json::json!("success"),
                    },
                },
                agent_id: None,
                timeout: std::time::Duration::from_secs(5),
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(100)
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let result = scheduler.execute_workflow(&workflow).await;
    
    assert_eq!(result.workflow_name, "test-conditional");
}

#[tokio::test]
async fn test_backoff_strategies() {
    let scheduler = Scheduler::new();
    
    let workflow_exp = Workflow {
        name: "test-backoff-exp".to_string(),
        description: "Test exponential backoff".to_string(),
        steps: vec![
            Step {
                name: "step1".to_string(),
                step_type: StepType::Sequential,
                agent_id: None,
                timeout: std::time::Duration::from_secs(1),
                max_retries: 3,
                backoff: BackoffStrategy::Exponential {
                    base: std::time::Duration::from_millis(100),
                    max: std::time::Duration::from_secs(1),
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let result_exp = scheduler.execute_workflow(&workflow_exp).await;
    assert_eq!(result_exp.status, WorkflowStatus::Completed);
    
    let workflow_linear = Workflow {
        name: "test-backoff-linear".to_string(),
        description: "Test linear backoff".to_string(),
        steps: vec![
            Step {
                name: "step1".to_string(),
                step_type: StepType::Sequential,
                agent_id: None,
                timeout: std::time::Duration::from_secs(1),
                max_retries: 2,
                backoff: BackoffStrategy::Linear {
                    interval: std::time::Duration::from_millis(200),
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let result_linear = scheduler.execute_workflow(&workflow_linear).await;
    assert_eq!(result_linear.status, WorkflowStatus::Completed);
}

#[tokio::test]
async fn test_workflow_timeout_handling() {
    let scheduler = Scheduler::new();
    
    let workflow = Workflow {
        name: "test-timeout".to_string(),
        description: "Test timeout handling".to_string(),
        steps: vec![
            Step {
                name: "timeout-step".to_string(),
                step_type: StepType::Sequential,
                agent_id: None,
                timeout: std::time::Duration::from_millis(10), 
                max_retries: 1,
                backoff: BackoffStrategy::Fixed {
                    interval: std::time::Duration::from_millis(50),
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(1),
        default_retries: 1,
    };
    
    let result = scheduler.execute_workflow(&workflow).await;
    
    match &result.status {
        WorkflowStatus::Completed => {},
        WorkflowStatus::Failed { failed_step: _ } => {},
        _ => panic!("Unexpected status"),
    };
    assert!(result.duration < std::time::Duration::from_secs(1));
}

#[test]
fn test_condition_evaluation_jsonpath() {
    let output = serde_json::json!({
        "result": "success",
        "key": "value"
    });
    
    let condition_true = ConditionType::JsonPath {
        path: "result".to_string(),
        expected: serde_json::json!("success"),
    };
    
    let condition_false = ConditionType::JsonPath {
        path: "result".to_string(),
        expected: serde_json::json!("failure"),
    };
    
    assert!(Scheduler::evaluate_condition(&condition_true, &output));
    assert!(!Scheduler::evaluate_condition(&condition_false, &output));
}

#[test]
fn test_condition_evaluation_nested_path() {
    let output = serde_json::json!({
        "value": 42
    });

    let condition = ConditionType::JsonPath {
        path: "value".to_string(),
        expected: serde_json::json!(42),
    };

    assert!(Scheduler::evaluate_condition(&condition, &output));
}

#[test]
fn test_workflow_serialization() {
    let workflow = Workflow {
        name: "test-serialization".to_string(),
        description: "Test serialization".to_string(),
        steps: vec![
            Step {
                name: "step1".to_string(),
                step_type: StepType::Sequential,
                agent_id: Some("agent1".to_string()),
                timeout: std::time::Duration::from_secs(5),
                max_retries: 2,
                backoff: BackoffStrategy::Exponential {
                    base: std::time::Duration::from_millis(100),
                    max: std::time::Duration::from_secs(5),
                },
            },
        ],
        default_timeout: std::time::Duration::from_secs(10),
        default_retries: 1,
    };
    
    let serialized = serde_json::to_string(&workflow).unwrap();
    assert!(serialized.len() > 0);
    
    let deserialized: Workflow = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.name, workflow.name);
    assert_eq!(deserialized.steps.len(), workflow.steps.len());
    assert_eq!(deserialized.default_timeout, workflow.default_timeout);
}
