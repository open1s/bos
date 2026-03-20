use std::sync::Arc;
use std::time::SystemTime;
use agent::a2a::{AgentIdentity, A2ADiscovery, AgentCard, A2AContent, Task, TaskState};
use brainos_common::{setup_bus, setup_logging};

#[derive(Debug, Clone)]
struct ChatMessage {
    sender: String,
    content: String,
    timestamp: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║ Agent 2 (Alice) - A2A Communication ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;

    let identity = AgentIdentity::new(
        "agent-2".to_string(),
        "Alice".to_string(),
        "0.1.0".to_string(),
    );

    tracing::info!("Created identity for Alice (agent-2)");

    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Alice".to_string(),
        "A friendly agent who responds to messages".to_string(),
    )
    .with_capability("conversation".to_string(), "Engage in conversations".to_string())
    .with_capability("messaging".to_string(), "Receive and respond to messages".to_string());

    discovery.announce(&card).await?;
    println!("✓ Announced as Alice (agent-2)\n");

    println!("Listening for incoming tasks...\n");

    let task_topic = format!("agent/{}/tasks/incoming", identity.id);
    let subscriber = session
        .declare_subscriber(&task_topic)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to subscribe: {}", e))?;

    let mut task_count = 0;
    let mut message_history: Vec<ChatMessage> = Vec::new();

    while let Ok(sample) = subscriber.recv() {
        if let Ok(message) = serde_json::from_slice::<agent::a2a::A2AMessage>(&sample.payload().to_bytes()) {
            tracing::info!("Received message from {}", message.sender.name);

            if let A2AContent::TaskRequest { mut task } = message.content {
                task_count += 1;
                let input = task.input.as_str().unwrap_or("unknown");

                println!("\n┌─ Incoming Task #{} ──────────────────────", task_count);
                println!("│ From: {} ({})", message.sender.name, message.sender.id);
                println!("│ Message: {}", input);
                println!("└──────────────────────────────────────\n");

                task.state = TaskState::Working;

                let response = agent::a2a::A2AMessage::task_response(
                    task.clone(),
                    identity.clone(),
                    message.sender.clone(),
                );

                let response_topic = format!(
                    "agent/{}/responses/{}",
                    message.sender.id,
                    message.message_id
                );

                if let Ok(publisher) = session.declare_publisher(&response_topic).await {
                    let data = serde_json::to_vec(&response)?;
                    let _ = publisher.put(data).await;
                }

                let timestamp = SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();

                message_history.push(ChatMessage {
                    sender: message.sender.name.clone(),
                    content: input.to_string(),
                    timestamp,
                });

                let response_text = generate_response(input, &message_history);

                println!("{}: {}\n", identity.name, response_text);

                task.state = TaskState::Completed;
                task.output = Some(serde_json::json!(response_text));

                let final_response = agent::a2a::A2AMessage::task_response(
                    task.clone(),
                    identity.clone(),
                    message.sender.clone(),
                );

                if let Ok(publisher) = session.declare_publisher(&response_topic).await {
                    let data = serde_json::to_vec(&final_response)?;
                    let _ = publisher.put(data).await;
                }

                tracing::info!("Task #{} completed", task_count);
            }
        }
    }

    Ok(())
}

fn generate_response(input: &str, history: &[ChatMessage]) -> String {
    let input_lower = input.to_lowercase();

    let greetings = vec![
        "Hello! Nice to hear from you.",
        "Hi there! How can I help you today?",
        "Greetings! It's a beautiful day for conversation.",
        "Hey! Great to hear from you!",
    ];

    if input_lower.contains("hello") || input_lower.contains("hi") || input_lower.contains("hey") {
        fastrand::choice(&greetings).unwrap().to_string()
    } else if input_lower.contains("how are you") {
        "I'm doing great, thank you for asking! How about you?".to_string()
    } else if input_lower.contains("weather") {
        "I wish I could check the weather for you, but I'm just a simple demo agent!".to_string()
    } else if input_lower.contains("time") {
        format!("The current time on my system is {:?}", SystemTime::now())
    } else if input_lower.contains("help") {
        "I'm here to help! You can send me messages and I'll respond. Try saying hello!".to_string()
    } else if input_lower.contains("name") {
        "My name is Alice! What about you?".to_string()
    } else {
        format!(
            "That's interesting! You said: \"{}\". Tell me more!",
            input
        )
    }
}