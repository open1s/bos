use std::sync::Arc;
use agent::{
    a2a::{AgentIdentity, A2ADiscovery, AgentCard, TaskState, A2AMessage, A2AContent},
    Agent, AgentConfig, ToolRegistry, Tool, ToolDescription, ToolError,
};
use async_trait::async_trait;
use brainos_common::{setup_bus, setup_logging, create_llm_client};
use serde_json::Value;

/// Streaming LLM assistant
struct StreamingAssistant;

#[async_trait]
impl Tool for StreamingAssistant {
    fn name(&self) -> &str {
        "streaming_response"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Generate streaming LLM response".to_string(),
            parameters: "User prompt text".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {"type": "string"}
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| ToolError::SchemaMismatch { message: "prompt is required".to_string() })?;

        // Simulate streaming response with chunks
        let responses = vec![
            format!("Thinking about: {}...", prompt),
            "Here's my response:".to_string(),
            format!("Based on your question \"{}\"", prompt),
            "...continuing...".to_string(),
            "Done!".to_string(),
        ];

        // Return as array for simulation
        Ok(serde_json::json!(responses))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;

    println!("╔══════════════════════════════════════╗");
    println!("║   WeChat Streaming Assistant      ║");
    println!("╚══════════════════════════════════════╝\n");

    let session = setup_bus(None).await?;

    let identity = AgentIdentity::new(
        "assistant".to_string(),
        "AI Assistant".to_string(),
        "1.0.0".to_string(),
    );

    let model = std::env::var("OPENAI_MODEL")
        .unwrap_or_else(|_| "gpt-4o".to_string());

    let config = AgentConfig {
        name: "assistant".to_string(),
        model: model.clone(),
        base_url: std::env::var("OPENAI_API_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key: std::env::var("OPENAI_API_KEY")
            .unwrap_or_else(|_| "sk-test".to_string()),
        system_prompt: format!(
            "You are a helpful AI assistant in a multi-agent chat environment.
Be conversational and friendly. When asked questions, provide thoughtful responses.
You can use tools including a streaming response tool.",
        ),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };

    let llm = create_llm_client();
    let _agent = Agent::new(config, llm);

    println!("✓ Assistant initialized with model: {}\n", model);

    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Streaming AI Assistant".to_string(),
        "An AI agent with streaming LLM responses and tool support".to_string(),
    )
    .with_capability("conversation".to_string(), "Engage in multi-turn conversations".to_string())
    .with_capability("streaming".to_string(), "Streaming text generation".to_string())
    .with_capability("tools".to_string(), "Tool execution and function calling".to_string())
    .with_skill("llm".to_string())
    .with_skill("chat".to_string());

    discovery.announce(&card).await?;
    println!("✓ Announced as AI Assistant (assistant)\n");

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(StreamingAssistant))?;
    println!("✓ Registered streaming tool\n");

    let task_topic = format!("agent/{}/tasks/incoming", identity.id);
    let subscriber = session
        .declare_subscriber(&task_topic)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to subscribe: {}", e))?;

    let session_clone = session.clone();
    let identity_clone = identity.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_incoming_tasks(session_clone, identity_clone, subscriber).await {
            eprintln!("Task handler error: {}", e);
        }
    });

    println!("{}\n", "=".repeat(50));
    println!("Assistant Listening for A2A Tasks...");
    println!("{}", "=".repeat(50));
    println!();

    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
}

async fn handle_incoming_tasks(
    session: Arc<bus::Session>,
    identity: AgentIdentity,
    subscriber: zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>,
) -> anyhow::Result<()> {
    let mut task_count = 0;

    while let Ok(sample) = subscriber.recv() {
        if let Ok(message) = serde_json::from_slice::<A2AMessage>(sample.payload().to_bytes().as_ref()) {
            tracing::info!("Received message from {}", message.sender.name);

            if let A2AContent::TaskRequest { mut task } = message.content {
                task_count += 1;
                tracing::info!("Task #{}: {}", task_count, task.input);

                println!("\n┌─ Incoming Task ────────────────────");
                println!("│ From: {}", message.sender.name);
                println!("│ Task: {}", task.input);
                println!("└────────────────────────────────\n");

                task.state = TaskState::Working;

                let response = A2AMessage::task_response(
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

                // Process the task with streaming simulation
                let input = task.input.as_str().unwrap_or("");

                println!("AI Assistant: (processing...)");

                let result = simulate_streaming_response(input).await;

                println!("AI Assistant: {}", result);

                task.state = TaskState::Completed;
                task.output = Some(serde_json::Value::String(result.clone()));

                let final_response = A2AMessage::task_response(
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

async fn simulate_streaming_response(prompt: &str) -> String {
    let responses = vec![
        format!("Let me think about \"{}\"...", prompt),
        format!("Regarding \"{}\", here are my thoughts:", prompt),
        format!("Based on my understanding of {}, I'd say...", prompt),
        "To summarize:".to_string(),
        format!("That's my take on {}!", prompt),
    ];

    for (i, chunk) in responses.iter().enumerate() {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        println!("  [{}] {}", i + 1, chunk);
    }

    responses.join(" ")
}
