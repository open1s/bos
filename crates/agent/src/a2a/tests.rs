//! Integration tests for A2A message flow

use super::{
    AgentIdentity, A2AMessage, A2AContent, Task, TaskState, AgentCard,
};

#[tokio::test]
async fn test_agent_card_serialization() {
    let identity = AgentIdentity::new(
        "test-agent".to_string(),
        "Test Agent".to_string(),
        "1.0.0".to_string(),
    );

    let card = AgentCard::new(
        identity.clone(),
        "Test Agent".to_string(),
        "Test Description".to_string(),
    )
    .with_capability("test-cap".to_string(), "Test capability".to_string())
    .with_skill("test-skill".to_string());

    let serialized = serde_json::to_string(&card).unwrap();
    assert!(!serialized.is_empty());
    assert!(serialized.contains("test-agent"));

    let deserialized: AgentCard = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.agent_id.id, "test-agent");
    assert!(deserialized.capabilities.iter().any(|c| c.name == "test-cap"));
    assert!(deserialized.skills.contains(&"test-skill".to_string()));
}

#[tokio::test]
async fn test_a2a_message_serialization() {
    let sender = AgentIdentity::new(
        "sender".to_string(),
        "Sender".to_string(),
        "1.0.0".to_string(),
    );
    let recipient = AgentIdentity::new(
        "recipient".to_string(),
        "Recipient".to_string(),
        "1.0.0".to_string(),
    );

    let task = Task {
        task_id: "task-123".to_string(),
        context_id: Some("ctx-456".to_string()),
        state: TaskState::Submitted,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        input: serde_json::json!("Test task"),
        output: None,
        error: None,
    };

    let message = A2AMessage::task_request(task.clone(), sender.clone(), recipient.clone());

    let serialized = serde_json::to_string(&message).unwrap();
    assert!(!serialized.is_empty());

    let deserialized: A2AMessage = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.task_id, "task-123");
    assert_eq!(deserialized.sender.id, "sender");
    assert_eq!(deserialized.recipient.id, "recipient");
}

#[tokio::test]
async fn test_task_state_transitions() {
    let task = Task {
        task_id: "task-transitions".to_string(),
        context_id: None,
        state: TaskState::Submitted,
        created_at: 0,
        updated_at: 0,
        input: serde_json::json!("Test"),
        output: None,
        error: None,
    };

    let mut modified = task.clone();
    modified.state = TaskState::Working;
    assert_eq!(modified.state, TaskState::Working);

    let mut with_output = modified;
    with_output.state = TaskState::Completed;
    with_output.output = Some(serde_json::json!({"result": "success"}));
    assert_eq!(with_output.state, TaskState::Completed);
    assert!(with_output.output.is_some());

    let mut with_error = with_output;
    with_error.state = TaskState::Failed;
    with_error.error = Some("Task failed".to_string());
    assert_eq!(with_error.state, TaskState::Failed);
    assert!(with_error.error.is_some());

    let mut input_required = with_error;
    input_required.state = TaskState::InputRequired;
    assert_eq!(input_required.state, TaskState::InputRequired);
}

#[tokio::test]
async fn test_agent_identity_equality() {
    let identity1 = AgentIdentity::new(
        "agent-1".to_string(),
        "Agent One".to_string(),
        "1.0.0".to_string(),
    );
    let identity2 = AgentIdentity::new(
        "agent-1".to_string(),
        "Agent One".to_string(),
        "1.0.0".to_string(),
    );

    assert_eq!(identity1, identity2);

    let identity3 = AgentIdentity::new(
        "agent-2".to_string(),
        "Agent Two".to_string(),
        "1.0.0".to_string(),
    );

    assert_ne!(identity1, identity3);
}

#[tokio::test]
async fn test_task_creation() {
    let input = serde_json::json!({
        "operation": "calculate"
    });
    
    let task = Task::new("task-1".to_string(), input.clone());
    
    assert_eq!(task.task_id, "task-1");
    assert_eq!(task.state, TaskState::Submitted);
    assert_eq!(task.input, input);
    assert!(task.output.is_none());
    assert!(task.error.is_none());
    assert!(task.context_id.is_none());
}

#[tokio::test]
async fn test_task_builder_methods() {
    let mut task = Task::new("builder-test".to_string(), serde_json::json!("test"));
    
    task = task.with_context("ctx-123".to_string());
    assert_eq!(task.context_id, Some("ctx-123".to_string()));
    
    let time_before = task.updated_at;
    task = task.with_state(TaskState::Working);
    assert_eq!(task.state, TaskState::Working);
    assert!(task.updated_at >= time_before);
    
    let output_value = serde_json::json!({"status": "done"});
    task = task.with_output(output_value.clone());
    assert_eq!(task.output, Some(output_value));
}

#[tokio::test]
async fn test_a2a_message_with_response() {
    let sender = AgentIdentity::new(
        "sender".to_string(),
        "Sender".to_string(),
        "1.0.0".to_string(),
    );
    let recipient = AgentIdentity::new(
        "recipient".to_string(),
        "Recipient".to_string(),
        "1.0.0".to_string(),
    );

    let task = Task {
        task_id: "response-task".to_string(),
        context_id: None,
        input: serde_json::json!("Request"),
        output: Some(serde_json::json!({"answer": 42})),
        error: None,
        state: TaskState::Completed,
        created_at: 0,
        updated_at: 0,
    };

    let message = A2AMessage::task_response(task.clone(), sender, recipient);

    let serialized = serde_json::to_string(&message).unwrap();
    let deserialized: A2AMessage = serde_json::from_str(&serialized).unwrap();

    match deserialized.content {
        A2AContent::TaskResponse { task: response_task } => {
            assert_eq!(response_task.task_id, task.task_id);
            assert_eq!(response_task.state, TaskState::Completed);
            assert!(response_task.output.is_some());
        }
        _ => panic!("Expected TaskResponse content"),
    }
}

#[tokio::test]
async fn test_idempotency_key_consistency() {
    let task1 = Task {
        task_id: "same-task".to_string(),
        context_id: Some("ctx-1".to_string()),
        state: TaskState::Submitted,
        created_at: 100,
        updated_at: 100,
        input: serde_json::json!("Request"),
        output: None,
        error: None,
    };

    let task2 = Task {
        task_id: "same-task".to_string(),
        context_id: Some("ctx-1".to_string()),
        state: TaskState::Submitted,
        created_at: 100,
        updated_at: 100,
        input: serde_json::json!("Request"),
        output: None,
        error: None,
    };

    let key1 = format!("{}:{}", task1.task_id, task1.created_at);
    let key2 = format!("{}:{}", task2.task_id, task2.created_at);
    assert_eq!(key1, key2);
}

#[tokio::test]
async fn test_agent_discovery_serialization() {
    let identity = AgentIdentity::new(
        "discoverable-agent".to_string(),
        "Discoverable".to_string(),
        "2.0.0".to_string(),
    );

    let card = AgentCard::new(
        identity.clone(),
        "Discoverable".to_string(),
        "Discoverable agent".to_string(),
    )
    .with_capability("http".to_string(), "HTTP operations".to_string())
    .with_capability("database".to_string(), "Database access".to_string())
    .with_skill("web".to_string())
    .with_skill("db".to_string());

    let serialized = serde_json::to_string(&card).unwrap();

    let card: AgentCard = serde_json::from_str(&serialized).unwrap();
    assert_eq!(card.agent_id.id, "discoverable-agent");
    assert_eq!(card.capabilities.len(), 2);
    assert_eq!(card.skills.len(), 2);
}
