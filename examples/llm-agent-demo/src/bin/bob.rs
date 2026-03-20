use std::sync::Arc;
use agent::{
    a2a::{AgentIdentity, A2ADiscovery, AgentCard, Task, TaskState},
    Tool, ToolRegistry, ToolDescription, ToolError,
};
use async_trait::async_trait;
use brainos_common::{setup_bus, setup_logging};
use bus::{RpcService, RpcServiceBuilder, RpcHandler, RpcServiceError};
use rkyv::{Archive, Serialize, Deserialize};
use serde_json::Value;

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;
    
    println!("╔══════════════════════════════════════╗");
    println!("║   Bob's Calculator Agent           ║");
    println!("╚══════════════════════════════════════╝\n");
    
    let session = setup_bus(None).await?;
    
    let identity = AgentIdentity::new(
        "bob".to_string(),
        "Bob".to_string(),
        "1.0.0".to_string(),
    );
    
    tracing::info!("Created identity for Bob");
    
    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Bob's Calculator".to_string(),
        "A calculator agent that performs mathematical operations via RPC".to_string(),
    )
    .with_capability("calculation".to_string(), "Perform math operations".to_string())
    .with_capability("rpc".to_string(), "Exposes tools via RPC".to_string())
    .with_skill("calculator".to_string());
    
    discovery.announce(&card).await?;
    println!("✓ Announced as Bob (bob)\n");
    
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    let agents = discovery.discover(None).await?;
    println!("✓ Discovered {} agent(s):\n", agents.len());
    
    for agent_card in &agents {
        println!("  - {} ({})", agent_card.name, agent_card.agent_id.id);
    }
    
    println!("\n{}", "=".repeat(40));
    println!("Registering RPC Calculator Tools");
    println!("{}", "=".repeat(40));
    println!();
    
    let tool_base = format!("agent/{}/tools/", identity.id);

    let add_service = bus::RpcServiceBuilder::new()
        .service_name("add")
        .topic_prefix(&tool_base)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build add service: {}", e))?
        .init(&session, AddHandler::new())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to init add service: {}", e))?;
    println!("✓ Registered RPC service: {}add", tool_base);

    let multiply_service = bus::RpcServiceBuilder::new()
        .service_name("multiply")
        .topic_prefix(&tool_base)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build multiply service: {}", e))?
        .init(&session, MultiplyHandler::new())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to init multiply service: {}", e))?;
    println!("✓ Registered RPC service: {}multiply", tool_base);

    let subtract_service = bus::RpcServiceBuilder::new()
        .service_name("subtract")
        .topic_prefix(&tool_base)
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build subtract service: {}", e))?
        .init(&session, SubtractHandler::new())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to init subtract service: {}", e))?;
    println!("✓ Registered RPC service: {}subtract", tool_base);
    
    println!("\n{}", "=".repeat(40));
    println!("Listening for A2A Tasks...");
    println!("{}", "=".repeat(40));
    println!("\nAvailable Tools (via RPC):");
    println!("  add(a, b)      - Add two numbers");
    println!("  multiply(a, b) - Multiply two numbers");
    println!("  subtract(a, b) - Subtract second from first");
    println!("\n(Press Ctrl+C to exit)\n");
    
    let task_topic = format!("agent/{}/tasks/incoming", identity.id);
    let subscriber = session.declare_subscriber(&task_topic).await
        .map_err(|e| anyhow::anyhow!("Failed to subscribe to tasks: {}", e))?;
    
    incoming_task_handler(session.clone(), identity, subscriber).await?;
    
    Ok(())
}

async fn incoming_task_handler(
    session: Arc<bus::Session>,
    identity: AgentIdentity,
    subscriber: zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>,
) -> anyhow::Result<()> {
    let mut task_count = 0;
    
    while let Ok(sample) = subscriber.recv() {
        match serde_json::from_slice::<agent::a2a::A2AMessage>(&sample.payload().to_bytes()) {
            Ok(message) => {
                tracing::info!("Received message from {}", message.sender.name);

                if let agent::a2a::A2AContent::TaskRequest { mut task } = message.content {
                    task_count += 1;
                    let task_input = task.input.as_str().unwrap_or("");
                    tracing::info!("Task #{}: {}", task_count, task_input);

                    println!("\n{}", "─".repeat(40));
                    println!("Task #{}", task_count);
                    println!("{}", "─".repeat(40));
                     println!("From: {}", message.sender.name);
                     println!("Task ID: {}", task.task_id);
                     println!("Message: {}", task_input);
                     println!("{}", "─".repeat(40));

                    task.state = TaskState::Working;
                    
                    let response_message = agent::a2a::A2AMessage::task_response(
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
                        let data = serde_json::to_vec(&response_message)
                            .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?;
                        
                        let _ = publisher.put(data).await;
                    }

                    let task_input = task.input.as_str().unwrap_or("");
                    let result = process_task(task_input, &identity.name);

                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    
                    task.state = TaskState::Completed;
                    task.output = Some(serde_json::Value::String(result.clone()));

                    let final_response = agent::a2a::A2AMessage::task_response(
                        task.clone(),
                        identity.clone(),
                        message.sender.clone(),
                    );
                    
                    if let Ok(publisher) = session.declare_publisher(&response_topic).await {
                        let data = serde_json::to_vec(&final_response)
                            .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?;
                        
                        if let Err(e) = publisher.put(data).await {
                            tracing::error!("Failed to send response: {}", e);
                        } else {
                            println!("Status: Completed");
                            println!("Response: {}\n", result);
                        }
                    }
                    
                    tracing::info!("Task #{} completed", task_count);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to parse message: {}", e);
            }
        }
    }
    
    Ok(())
}

fn process_task(task: &str, _agent_name: &str) -> String {
    let task_lower = task.to_lowercase();
    
    if task_lower.contains("hello") || task_lower.contains("hi") {
        "Nice to meet you! I'm Bob, and I'm ready to help with calculations.".to_string()
    } else if task_lower.contains("can you") || task_lower.contains("help") {
        "I'm Bob! I can help with math operations. I have add, multiply, and subtract tools available via RPC.".to_string()
    } else if task_lower.contains("status") || task_lower.contains("ready") {
        "I'm running and ready! My tools are registered and waiting for RPC calls.".to_string()
    } else {
        format!("Thanks for the message! I'm Bob - your calculator agent. You can ask me to do calculations, or you can call my tools directly via RPC.")
    }
}

struct AddHandler;
impl AddHandler {
    fn new() -> Self { Self }
}

#[async_trait::async_trait]
impl RpcHandler for AddHandler {
    async fn handle(&self, _method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        let json_payload: JsonPayload = unsafe {
            rkyv::from_bytes_unchecked::<JsonPayload, rkyv::rancor::Error>(payload)
                .map_err(|e| RpcServiceError::Internal(e.to_string()))?
        };

        let args: Value = serde_json::from_str(&json_payload.json)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let a = args["a"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'a' required".to_string()))?;
        let b = args["b"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'b' required".to_string()))?;

        let result = a + b;

        let response = serde_json::json!({
            "result": result,
            "operation": format!("{} + {} = {}", a, b, result)
        });

        let json = serde_json::to_string(&response)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let result_payload = JsonPayload { json };
        let serialized = rkyv::to_bytes::<rkyv::rancor::Error>(&result_payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        Ok(serialized.into_vec())
    }
}

struct MultiplyHandler;
impl MultiplyHandler {
    fn new() -> Self { Self }
}

#[async_trait::async_trait]
impl RpcHandler for MultiplyHandler {
    async fn handle(&self, _method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        let json_payload: JsonPayload = unsafe {
            rkyv::from_bytes_unchecked::<JsonPayload, rkyv::rancor::Error>(payload)
                .map_err(|e| RpcServiceError::Internal(e.to_string()))?
        };

        let args: Value = serde_json::from_str(&json_payload.json)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let a = args["a"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'a' required".to_string()))?;
        let b = args["b"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'b' required".to_string()))?;

        let result = a * b;

        let response = serde_json::json!({
            "result": result,
            "operation": format!("{} × {} = {}", a, b, result)
        });

        let json = serde_json::to_string(&response)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let result_payload = JsonPayload { json };
        let serialized = rkyv::to_bytes::<rkyv::rancor::Error>(&result_payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        Ok(serialized.into_vec())
    }
}

struct SubtractHandler;
impl SubtractHandler {
    fn new() -> Self { Self }
}

#[async_trait::async_trait]
impl RpcHandler for SubtractHandler {
    async fn handle(&self, _method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        let json_payload: JsonPayload = unsafe {
            rkyv::from_bytes_unchecked::<JsonPayload, rkyv::rancor::Error>(payload)
                .map_err(|e| RpcServiceError::Internal(e.to_string()))?
        };

        let args: Value = serde_json::from_str(&json_payload.json)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let a = args["a"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'a' required".to_string()))?;
        let b = args["b"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("field 'b' required".to_string()))?;

        let result = a - b;

        let response = serde_json::json!({
            "result": result,
            "operation": format!("{} - {} = {}", a, b, result)
        });

        let json = serde_json::to_string(&response)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let result_payload = JsonPayload { json };
        let serialized = rkyv::to_bytes::<rkyv::rancor::Error>(&result_payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        Ok(serialized.into_vec())
    }
}
