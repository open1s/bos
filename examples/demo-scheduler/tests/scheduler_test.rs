use agent::{
    WorkflowBuilder, StepBuilder, Scheduler, BackoffStrategy, ConditionType,
};

#[tokio::test]
async fn demo_sched_seq() {
    let workflow = WorkflowBuilder::new("test-sequential")
        .add_step(StepBuilder::new("step1")
            .sequential()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .add_step(StepBuilder::new("step2")
            .sequential()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .build();

    let scheduler = Scheduler::new();
    let result = scheduler.execute_workflow(&workflow).await;

    assert_eq!(result.status, agent::WorkflowStatus::Completed);
    assert_eq!(result.step_results.len(), 2);

    let output_1 = result.step_results[0].output.as_ref().expect("Expected output for step 1");
    let expected_1 = serde_json::json!({ "value": 1 });
    assert_eq!(output_1, &expected_1, "Unexpected output in step 1");

    let output_2 = result.step_results[1].output.as_ref().expect("Expected output for step 2");
    let expected_2 = serde_json::json!({ "value": 2 });
    assert_eq!(output_2, &expected_2, "Unexpected output in step 2");
}

#[tokio::test]
async fn demo_sched_par() {
    let workflow = WorkflowBuilder::new("test-parallel")
        .add_step(StepBuilder::new("task_a")
            .parallel()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .add_step(StepBuilder::new("task_b")
            .parallel()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .add_step(StepBuilder::new("task_c")
            .parallel()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .build();

    let scheduler = Scheduler::new();
    let result = scheduler.execute_workflow(&workflow).await;

    assert_eq!(result.status, agent::WorkflowStatus::Completed);
    assert_eq!(result.step_results.len(), 3);

    for step in &result.step_results {
        assert_eq!(step.status, agent::StepStatus::Completed);
    }

    let output = result.step_results[0].output.as_ref().expect("Expected output for task_a");
    let expected = serde_json::json!({ "results": [1, 0, -1], "count": 3 });
    assert_eq!(output, &expected, "Unexpected output for parallel execution");
}

#[tokio::test]
async fn demo_sched_cond() {
    let workflow = WorkflowBuilder::new("test-cond-high")
        .add_step(StepBuilder::new("conditional")
            .conditional(ConditionType::JsonPath {
                path: "value".to_string(),
                expected: serde_json::json!(10),
            })
            .timeout(std::time::Duration::from_secs(5))
            .build()
        )
        .build();

    let scheduler = Scheduler::new();
    let result = scheduler.execute_workflow(&workflow).await;

    assert_eq!(result.status, agent::WorkflowStatus::Completed);

    let output = result.step_results[0].output.as_ref().expect("Expected output for conditional");
    assert!(output.is_object(), "Output should be a JSON object");
    assert!(output.get("branch").is_some(), "Output should have 'branch' field");
}

#[tokio::test]
async fn demo_sched_retry() {
    let backoff = BackoffStrategy::Linear {
        interval: std::time::Duration::from_millis(100),
    };

    let delay_0 = backoff.calculate_backoff(0);
    let delay_1 = backoff.calculate_backoff(1);
    let delay_2 = backoff.calculate_backoff(2);

    // Linear: interval * (attempt + 1)
    assert_eq!(delay_0, std::time::Duration::from_millis(100));
    assert_eq!(delay_1, std::time::Duration::from_millis(200));
    assert_eq!(delay_2, std::time::Duration::from_millis(300));
}

#[tokio::test]
async fn demo_sched_timeout() {
    // Create workflow with very short timeout
    let workflow = WorkflowBuilder::new("test-timeout")
        .default_timeout(std::time::Duration::from_millis(10))
        .add_step(StepBuilder::new("slow-step")
            .sequential()
            .timeout(std::time::Duration::from_millis(10))
            .build()
        )
        .build();

    let scheduler = Scheduler::new();
    let result = scheduler.execute_workflow(&workflow).await;

    // Sequential step completes quickly (value + 1), shouldn't timeout
    assert_eq!(result.status, agent::WorkflowStatus::Completed);
    assert_eq!(result.step_results.len(), 1);
    assert_eq!(result.step_results[0].status, agent::StepStatus::Completed);
}
