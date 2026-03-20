use std::sync::Arc;
use agent::{
    a2a::{AgentIdentity, A2ADiscovery, AgentCard, Task},
    Agent, AgentConfig,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::sync::RwLock;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use brainos_common::{setup_bus, setup_logging, create_llm_client};

/// Message in the chat room
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ChatMessage {
    message_id: String,
    sender_id: String,
    sender_name: String,
    content: String,
    timestamp: u64,
    message_type: MessageType,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum MessageType {
    Text,
    System,
    StreamingStart,
    StreamingChunk,
    StreamingEnd,
}

/// Conversation state
#[derive(Debug, Clone)]
struct Conversation {
    conversation_id: String,
    participants: Vec<AgentIdentity>,
    messages: Vec<ChatMessage>,
    context: HashMap<String, serde_json::Value>,
}

/// WeChat-like chat server
struct ChatServer {
    session: Arc<bus::Session>,
    identity: AgentIdentity,
    conversations: Arc<RwLock<HashMap<String, Conversation>>>,
    agent_mapping: Arc<RwLock<HashMap<String, AgentIdentity>>>, // name -> identity
}

impl ChatServer {
    async fn new() -> anyhow::Result<Self> {
        setup_logging()?;
        let session = setup_bus(None).await?;

        let identity = AgentIdentity::new(
            "wechat-server".to_string(),
            "WeChat Server".to_string(),
            "1.0.0".to_string(),
        );

        Ok(Self {
            session,
            identity,
            conversations: Arc::new(RwLock::new(HashMap::new())),
            agent_mapping: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    async fn announce_capabilities(&self) -> anyhow::Result<()> {
        let discovery = A2ADiscovery::new(self.session.clone());
        let card = AgentCard::new(
            self.identity.clone(),
            "WeChat Server".to_string(),
            "A multi-agent chat room server with streaming support".to_string(),
        )
        .with_capability("chat".to_string(), "Multi-agent messaging".to_string())
        .with_capability("streaming".to_string(), "Streaming LLM responses".to_string())
        .with_capability("persistence".to_string(), "Conversation state management".to_string())
        .with_skill("chat-server".to_string());

        discovery.announce(&card).await?;
        println!("✓ WeChat Server announced");

        Ok(())
    }

    async fn discover_agents(&self) -> anyhow::Result<Vec<AgentIdentity>> {
        let discovery = A2ADiscovery::new(self.session.clone());
        let agents = discovery.discover(None).await?;

        let mut agent_mapping = self.agent_mapping.write().await;
        for agent_card in &agents {
            agent_mapping.insert(
                agent_card.agent_id.id.clone(),
                agent_card.agent_id.clone(),
            );
        }

        println!("✓ Discovered {} agent(s)", agents.len());
        for agent_card in &agents {
            println!("  - {} ({})", agent_card.name, agent_card.agent_id.id);
        }

        Ok(agents.into_iter().map(|c| c.agent_id).collect())
    }

    async fn create_conversation(&self, participants: Vec<AgentIdentity>) -> anyhow::Result<String> {
        let conversation_id = uuid::Uuid::new_v4().to_string();

        let conversation = Conversation {
            conversation_id: conversation_id.clone(),
            participants: participants.clone(),
            messages: Vec::new(),
            context: HashMap::new(),
        };

        let mut conversations = self.conversations.write().await;
        conversations.insert(conversation_id.clone(), conversation.clone());

        println!("✓ Created conversation {} with {} participants", conversation_id, participants.len());

        Ok(conversation_id)
    }

    async fn handle_message(
        &self,
        conversation_id: &str,
        sender_id: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        let timestamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();

        let agent_mapping = self.agent_mapping.read().await;
        let sender = agent_mapping
            .get(sender_id)
            .ok_or_else(|| anyhow::anyhow!("Sender not found: {}", sender_id))?;

        // Create text message
        let text_message = ChatMessage {
            message_id: uuid::Uuid::new_v4().to_string(),
            sender_id: sender_id.to_string(),
            sender_name: sender.name.clone(),
            content: content.to_string(),
            timestamp,
            message_type: MessageType::Text,
        };

        self.add_message_to_conversation(conversation_id, text_message.clone()).await?;

        // Display message
        println!("[{}]: {}", sender.name, content);

        // Delegate task to agent for processing
        let target_agent = self.find_target_agent(conversation_id).await?;
        self.delegate_task_to_agent(conversation_id, sender, &target_agent.name, content)
            .await?;

        Ok(())
    }

    async fn find_target_agent(&self, conversation_id: &str) -> anyhow::Result<AgentIdentity> {
        let conversations = self.conversations.read().await;
        let conversation = conversations
            .get(conversation_id)
            .ok_or_else(|| anyhow::anyhow!("Conversation not found: {}", conversation_id))?;

        // Find the first agent that's not the server
        conversation
            .participants
            .iter()
            .find(|p| p.id != "wechat-server")
            .or_else(|| conversation.participants.first())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No agents in conversation"))
    }

    async fn delegate_task_to_agent(
        &self,
        _conversation_id: &str,
        sender: &AgentIdentity,
        target_name: &str,
        content: &str,
    ) -> anyhow::Result<()> {
        use agent::a2a::A2AClient;

        let agent_mapping = self.agent_mapping.read().await;

        // Find target identity
        let target = agent_mapping
            .values()
            .find(|id| id.name == target_name)
            .ok_or_else(|| anyhow::anyhow!("Target agent not found: {}", target_name))?;

        let client = A2AClient::new(self.session.clone(), sender.clone());
        let task_id = uuid::Uuid::new_v4().to_string();
        let task = Task::new(task_id, serde_json::json!(content));

        println!("  → Delegating to {}...", target.name);

        match client.delegate_task(&target, task).await {
            Ok(_) => println!("  → Task delegated successfully"),
            Err(e) => println!("  → Task delegation failed: {}", e),
        }

        Ok(())
    }

    async fn add_message_to_conversation(&self, conversation_id: &str, message: ChatMessage) -> anyhow::Result<()> {
        let mut conversations = self.conversations.write().await;
        let conversation = conversations
            .get_mut(conversation_id)
            .ok_or_else(|| anyhow::anyhow!("Conversation not found: {}", conversation_id))?;

        conversation.messages.push(message);
        Ok(())
    }

    async fn print_conversation_history(&self, conversation_id: &str) -> anyhow::Result<()> {
        let conversations = self.conversations.read().await;
        let conversation = conversations
            .get(conversation_id)
            .ok_or_else(|| anyhow::anyhow!("Conversation not found: {}", conversation_id))?;

        println!("\n Conversation History ({})", conversation_id);
        println!("{}", "─".repeat(50));

        for msg in &conversation.messages {
            match &msg.message_type {
                MessageType::Text => {
                    println!("{}: {}", msg.sender_name, msg.content);
                }
                MessageType::System => {
                    println!("* {}", msg.content);
                }
                MessageType::StreamingStart => {
                    println!("{}: (typing...)", msg.sender_name);
                }
                MessageType::StreamingChunk => {
                    println!("{}: {}", msg.sender_name, msg.content);
                }
                MessageType::StreamingEnd => {
                    // End marker, no additional output
                }
            }
        }

        println!();

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("╔══════════════════════════════════════╗");
    println!("║   WeChat Multi-Agent Chat Server    ║");
    println!("╚══════════════════════════════════════╝\n");

    let server = ChatServer::new().await?;

    // Announce server capabilities
    server.announce_capabilities().await?;

    // Wait for agents to start
    println!("\nWaiting for agents to start...");
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // Discover available agents
    let agents = server.discover_agents().await?;

    if agents.is_empty() {
        println!("⚠️  No agents found. Please start some agents first.");
        println!("Example: cargo run --bin assistant in client terminal");
        return Ok(());
    }

    let num_agents = agents.len();
    // Create a conversation with all discovered agents
    let conversation_id = server.create_conversation(agents).await?;

    // Add system message
    let welcome_msg = ChatMessage {
        message_id: uuid::Uuid::new_v4().to_string(),
        sender_id: "system".to_string(),
        sender_name: "System".to_string(),
        content: format!("Welcome to the chat! {} agents participating.", num_agents),
        timestamp: SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        message_type: MessageType::System,
    };
    server.add_message_to_conversation(&conversation_id, welcome_msg).await?;

    println!("\n{}", "=".repeat(50));
    println!("Chat Room Active - Type messages and press Enter");
    println!("Commands:");
    println!("  /history  - Show conversation history");
    println!("  /agents   - List participating agents");
    println!("  /quit     - Exit chat");
    println!("{}", "=".repeat(50));
    println!();

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut line = String::new();

    loop {
        print!("You> ");
        tokio::io::stdout().flush().await?;

        line.clear();
        let n = reader.read_line(&mut line).await?;

        if n == 0 {
            // EOF
            break;
        }

        let input = line.trim();

        if input.is_empty() {
            continue;
        }

        if input == "/quit" || input == "/exit" {
            println!("Goodbye!");
            break;
        }

        if input == "/history" {
            server.print_conversation_history(&conversation_id).await?;
            continue;
        }

        if input == "/agents" {
            let conversations = server.conversations.read().await;
            if let Some(conv) = conversations.get(&conversation_id) {
                println!("Participants:");
                for p in &conv.participants {
                    println!("  - {} ({})", p.name, p.id);
                }
            }
            continue;
        }

        // Handle as chat message
        if let Err(e) = server
            .handle_message(&conversation_id, "user", input)
            .await
        {
            eprintln!("Error: {}", e);
        }
    }

    println!("\nThank you for using WeChat Multi-Agent Chat Server!");
    Ok(())
}
