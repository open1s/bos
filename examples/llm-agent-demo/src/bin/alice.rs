use std::sync::Arc;
use std::io::{self, Write};
use agent::{
    a2a::{AgentIdentity, A2ADiscovery, AgentCard, TaskState},
    Agent, AgentConfig,
    Tool, ToolRegistry, ToolDescription, ToolError,
};
use async_trait::async_trait;
use brainos_common::{setup_bus, setup_logging, create_llm_client};
use bus::RpcClient;
use rkyv::{Archive, Serialize, Deserialize};
use serde_json::Value;

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: Vec<u8>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_logging()?;
    
    println!("╔══════════════════════════════════════╗");
    println!("║   Alice's Conversational Agent     ║");
    println!("╚══════════════════════════════════════╝\n");
    
    let session = setup_bus(None).await?;
    
    let identity = AgentIdentity::new(
        "alice".to_string(),
        "Alice".to_string(),
        "1.0.0".to_string(),
    );
    
    tracing::info!("Created identity for Alice");
    
    let model = std::env::var("OPENAI_MODEL")
        .unwrap_or_else(|_| "gpt-4o".to_string());
    
    let config = AgentConfig {
        name: "alice".to_string(),
        model: model.clone(),
        base_url: std::env::var("OPENAI_API_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key: std::env::var("OPENAI_API_KEY")
            .unwrap_or_else(|_| "sk-test".to_string()),
        system_prompt: format!(
            "You are Alice, a helpful AI assistant with access to calculator tools.
Bob is a calculator agent who has these RPC tools:
- add(a, b): Add two numbers
- multiply(a, b): Multiply two numbers  
- subtract(a, b): Subtract second from first

When the user asks for calculations:
1. First check if Bob's tools are available
2. Call the appropriate tool via RPC at 'agent/bob/tools/<operation>'
3. Return the result with explanation

Be helpful and conversational. Say what tools you're using when calling them."
        ),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };
    
    let llm = create_llm_client();
    let mut agent = Agent::new(config, llm);
    
    println!("✓ Agent initialized with model: {}\n", model);
    
    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Alice".to_string(),
        "A conversational AI agent with calculator tool access".to_string(),
    )
    .with_capability("conversation".to_string(), "Engage in conversations".to_string())
    .with_capability("calculation".to_string(), "Perform calculations via Bob's tools".to_string())
    .with_skill("llm".to_string());
    
    discovery.announce(&card).await?;
    println!("✓ Announced as Alice (alice)\n");
    
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    
    let agents = discovery.discover(None).await?;
    println!("✓ Discovered {} agent(s):\n", agents.len());

    let mut bob_identity: Option<AgentIdentity> = None;
    for agent_card in &agents {
        println!("  - {} ({}) - Skills: {:?}", 
            agent_card.name, agent_card.agent_id.id, agent_card.skills);
        if agent_card.agent_id.id == "bob" {
            bob_identity = Some(agent_card.agent_id.clone());
        }
    }
    
    let bob_rpc_services = if let Some(bob) = bob_identity.clone() {
        discover_bob_tools(session.clone(), &bob).await?
    } else {
        vec![]
    };
    
    let bob_identity = bob_identity.ok_or_else(|| {
        anyhow::anyhow!("Bob agent not found. Start bob agent first.")
    })?;
    
    println!("\nBob's RPC Tools:");
    for tool_name in &bob_rpc_services {
        println!("  - {}{}", "agent/bob/tools/", tool_name);
    }
    
    let mut tool_registry = ToolRegistry::new();
    
    for tool_name in &bob_rpc_services {
        let tool = Arc::new(BobToolInvoker::new(
            tool_name.clone(),
            session.clone(),
            &bob_identity,
        ));
        tool_registry.register(tool)?;
    }

    let task_topic = format!("agent/{}/tasks/incoming", identity.id);
    let subscriber = session.declare_subscriber(&task_topic).await
        .map_err(|e| anyhow::anyhow!("Failed to subscribe to tasks: {}", e))?;

    tokio::spawn(incoming_task_handler(
        session.clone(),
        identity.clone(),
        subscriber,
    ));
    
    println!("\n{}", "=".repeat(50));
    println!("Interactive Mode");
    println!("{}", "=".repeat(50));
    println!("\nEnter messages for Alice to process (or 'quit'):\n");
    
    loop {
        print!("> ");
        io::stdout().flush()?;
        
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        
        let input = input.trim();
        
        if input.is_empty() {
            continue;
        }
        
        if input == "quit" || input == "exit" {
            break;
        }
        
        println!();
        match agent.run_with_tools(input, &tool_registry).await {
            Ok(output) => {
                match output {
                    agent::AgentOutput::Text(text) => {
                        println!("Alice: {}", text);
                    }
                    agent::AgentOutput::Error(e) => {
                        eprintln!("Error: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed: {}", e);
            }
        }
        println!();
    }
    
    println!("\nGoodbye!");
    Ok(())
}

async fn discover_bob_tools(
    session: Arc<bus::Session>,
    bob: &AgentIdentity,
) -> anyhow::Result<Vec<String>> {
    let discovery = bus::DiscoveryRegistry::new()
        .session(session)
        .timeout(std::time::Duration::from_secs(2));

    let services = discovery.list_services().await?;

    let bob_tools: Vec<String> = services
        .into_iter()
        // Filter for Bob's tools by checking if topic_prefix contains the agent ID
        .filter(|s| s.topic_prefix.contains(&format!("agent/{}/", bob.id)))
        .map(|s| {
            // Extract the tool name from topic_prefix
            // Expected format: "agent/bob/tools/add"
            s.topic_prefix
                .strip_prefix(&format!("agent/{}/tools/", bob.id))
                .unwrap_or(&s.service_name)
                .to_string()
        })
        .collect();

    Ok(bob_tools)
}

async fn incoming_task_handler(
    session: Arc<bus::Session>,
    identity: AgentIdentity,
    subscriber: zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>,
) -> anyhow::Result<()> {
    while let Ok(sample) = subscriber.recv() {
        if let Ok(message) = serde_json::from_slice::<agent::a2a::A2AMessage>(&sample.payload().to_bytes()) {
            if let agent::a2a::A2AContent::TaskRequest { mut task } = message.content {
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
                    let data = serde_json::to_vec(&response)
                        .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?;
                    let _ = publisher.put(data).await;
                }

                let task_input = task.input.as_str().unwrap_or("");
                let result = format!("Hello from Alice! You said: \"{}\"", task_input);

                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                task.state = TaskState::Completed;
                task.output = Some(serde_json::Value::String(result.clone()));

                let final_response = agent::a2a::A2AMessage::task_response(
                    task.clone(),
                    identity.clone(),
                    message.sender.clone(),
                );

            let final_response_topic = response_topic.clone();
            if let Ok(publisher) = session.declare_publisher(final_response_topic).await {
                    let data = serde_json::to_vec(&final_response)
                        .map_err(|e| anyhow::anyhow!("Failed to serialize: {}", e))?;
                    let _ = publisher.put(data).await;
                }
            }
        }
    }
    
    Ok(())
}

struct BobToolInvoker {
    name: String,
    session: Arc<bus::Session>,
    bob: AgentIdentity,
}

impl BobToolInvoker {
    fn new(name: String, session: Arc<bus::Session>, bob: &AgentIdentity) -> Self {
        Self { name, session, bob: bob.clone() }
    }
}

#[async_trait]
impl Tool for BobToolInvoker {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!("Call Bob's {} tool via RPC", self.name),
            parameters: "Numbers: a, b".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        Value::Object({
            let mut map = serde_json::Map::new();
            map.insert("type".to_string(), Value::String("object".to_string()));
            
            let mut properties = serde_json::Map::new();
            properties.insert("a".to_string(), Value::Object({
                let mut m = serde_json::Map::new();
                m.insert("type".to_string(), Value::String("number".to_string()));
                m
            }));
            properties.insert("b".to_string(), Value::Object({
                let mut m = serde_json::Map::new();
                m.insert("type".to_string(), Value::String("number".to_string()));
                m
            }));
            
            map.insert("properties".to_string(), Value::Object(properties));
            map.insert("required".to_string(), Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]));
            
            map
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let service_name = format!("agent/{}/tools/{}", self.bob.id, self.name);

        let mut client = RpcClient::new(&service_name, &self.name);

        client.init(self.session.clone()).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let json = serde_json::to_vec(args)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let payload = JsonPayload { json };
        let request = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .into_vec();

        // The RpcClient.call() method deserializes the response directly to
        // JsonPayload using rkyv, so we annotate the type explicitly
        let response: JsonPayload = client.call(&request)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let mut result: Value = serde_json::from_slice(&response.json)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        result["operation"] = Value::String(format!("via Bob's {} tool", self.name));

        Ok(result)
    }
}
