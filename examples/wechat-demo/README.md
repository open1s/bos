# WeChat Multi-Agent Chat Demo

This example demonstrates a WeChat-like multi-agent chat system with:
- Multi-agent conversations
- Streaming LLM responses
- Conversation state management
- A2A protocol message routing

## Overview

The demo consists of two components:

**Server (wechat-server)**: 
- Multi-agent chat room manager
- Conversation state persistence
- Message routing between agents
- Display of chat history

**Client (wechat-assistant)**:
- AI assistant agent with streaming responses
- Tool execution capability
- A2A task handling

## Features Demonstrated

1. **Multi-Agent Communication**
   - Multiple agents in a chat room
   - Message routing via A2A protocol
   - Agent discovery and capability announcement

2. **Streaming Responses**
   - Simulated streaming text generation
   - Real-time output of response chunks
   - Typing indicators

3. **Conversation State**
   - Message history storage
   - Participant management
   - Context persistence

## Prerequisites

1. **Zenoh** - Install and start Zenoh router:
   ```bash
   cargo install zenohd
   zenohd -l tcp/0.0.0.0:7447
   ```

2. **OpenAI-compatible API** (optional, for real LLM):
   ```bash
   export OPENAI_API_KEY="your-api-key"
   export OPENAI_API_BASE_URL="https://api.openai.com/v1"
   export OPENAI_MODEL="gpt-4o"
   ```

## Running

### Terminal 1: Start the Chat Server

```bash
cd examples/wechat-demo
cargo run --bin server
```

The server will:
1. Announce its capabilities
2. Discover available agents
3. Wait for user input
4. Route messages between agents

### Terminal 2: Start the AI Assistant

```bash
cd examples/wechat-demo
cargo run --bin client
```

The assistant will:
1. Announce as an AI agent with streaming capabilities
2. Listen for A2A tasks from the server
3. Process incoming messages with simulated streaming
4. Return responses

## Usage Examples

**From the server terminal:**

```
> Hello

AI Assistant: (processing...)
  [1] Let me think about "Hello"...
  [2] Regarding "Hello", here are my thoughts:
  [3] Based on my understanding of Hello, I'd say...
  [4] To summarize:
  [5] That's my take on Hello!
```

```
> What's 5 + 7?

AI Assistant: (processing...)
  [1] Let me think about "What's 5 + 7?"...
  [2] Regarding "What's 5 + 7?", here are my thoughts:
  [3] Based on my understanding of What's 5 + 7?, I'd say...
  [4] To summarize:
  [5] That's my take on What's 5 + 7?!
```

### Server Commands

- `/history` - Show conversation history
- `/agents` - List participating agents
- `/quit` - Exit chat

## Architecture

```
┌─────────────────────────────────────────────────┐
│         WeChat Server (chat room)              │
│─────────────────────────────────────────────────│
│  • Manages conversations                        │
│  • Routes messages between agents              │
│  • Persists message history                    │
│  • Displays chat UI                             │
└─────────────────┬───────────────────────────────┘
                  │ A2A Protocol
        ┌─────────┼─────────┐
        │         │         │
        ▼         ▼         ▼
   ┌────────┐ ┌──────┐ ┌──────┐
   │Assistant│ │Agent2│ │Agent3│
   │(LLM)   │ │      │ │      │
   └────────┘ └──────┘ └──────┘
```

## Key Components

### Server (server.rs)

**Main Structures:**

```rust
struct ChatMessage {
    message_id: String,
    sender_id: String,
    sender_name: String,
    content: String,
    timestamp: u64,
    message_type: MessageType,  // Text, System, StreamingStart/Chunk/End
}

struct Conversation {
    conversation_id: String,
    participants: Vec<AgentIdentity>,
    messages: Vec<ChatMessage>,
    context: HashMap<String, Value>,
}
```

**Key Methods:**
- `announce_capabilities()` - Let other agents discover the server
- `discover_agents()` - Find available agents on the bus
- `create_conversation()` - Create a new chat room
- `handle_message()` - Process incoming messages and route to agents
- `delegate_task_to_agent()` - Send A2A task to target agent

### Assistant (client.rs)

**Streaming Simulation:**
```rust
async fn simulate_streaming_response(prompt: &str) -> String {
    let responses = vec![
        format!("Let me think about \"{}\"...", prompt),
        // ... more chunks
    ];
    
    for (i, chunk) in responses.iter().enumerate() {
        println!("  [{}] {}", i + 1, chunk);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    
    responses.join(" ")
}
```

## A2A Protocol Usage

The server uses A2A protocol to communicate with agents:

```rust
use agent::a2a::{A2AClient, Task};

let client = A2AClient::new(session.clone(), sender_identity);
let task = Task::new(task_id, Value::String(message));
client.delegate_task(&recipient, task).await?;
```

The assistant handles tasks by subscribing to the A2A task topic:

```rust
let task_topic = format!("agent/{}/tasks/incoming", identity.id);
let subscriber = session.declare_subscriber(&task_topic).await?;
// Then handle incoming TaskRequest messages
```

## Streaming Implementation

The demo simulates streaming by:
1. Splitting responses into multiple chunks
2. Outputting each chunk with a delay
3. Displaying chunk numbers for visual feedback

In a real implementation, you would:
- Use LLM client's streaming API
- Accumulate chunks and display in real-time
- Provide typing indicators
- Support cancellation

## State Management

**Server-side:**
- `conversations`: HashMap of conversation_id → Conversation
- `agent_mapping`: HashMap of name → AgentIdentity

**Conversation state includes:**
- Participant list
- Full message history
- Context key-value store

## Extension Ideas

1. **Multiple Conversations**
   - Support concurrent chat rooms
   - Switch between conversations
   - Conversation listings

2. **Rich Media**
   - Image/file attachments
   - Message reactions
   - Typing indicators

3. **Advanced Streaming**
   - Real LLM streaming integration
   - Cancelable requests
   - Progress indicators

4. **Multi-Agent Workflows**
   - Agent task delegation chains
   - Parallel processing
   - Result aggregation

## Troubleshooting

### "No agents found" error

Start the assistant agent first, then restart the server:
```bash
# Terminal 1
cargo run --bin client

# Terminal 2
cargo run --bin server
```

### Zenoh connection issues

Ensure Zenoh is running:
```bash
zenohd -l tcp/0.0.0.0:7447
```

Check port availability (default 7447).

### Messages not being routed

Verify:
- Server announced capabilities
- Assistant announced capabilities
- Both can see each other in `/agents` list
- Same Zenoh instance

## Next Steps

Try these enhancements:
1. Add a second assistant agent with different capabilities
2. Implement conversation persistence to disk
3. Add tool support for real calculations
4. Integrate actual LLM streaming API
5. Add file sharing capabilities

## Related Examples

- [llm-agent-demo](../llm-agent-demo/) - Basic agent interaction with tools
- [basic-communication](../basic-communication/) - Direct A2A protocol usage
