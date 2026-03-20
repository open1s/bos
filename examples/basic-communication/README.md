# Basic A2A Communication Example

This example demonstrates two agents exchanging messages via the Agent-to-Agent (A2A) protocol.

## Overview

Shows:
- Agent identity creation and announcement
- Task delegation between agents
- Incoming task handling
- Agent discovery
- Proper error handling and logging

## Architecture

```
┌───────────┐                    ┌───────────┐
│  Agent 1  │                    │  Agent 2  │
│ (sender)  │ ──── A2A Task ────▶│ (receiver)│
│           │                    │           │
│ Id: agent-1│                    │ Id: agent-2│
│ Name: Bob │                    │ Name: Alice│
└───────────┘                    └───────────┘
```

## Running

```bash
cd examples/basic-communication

# Terminal 1: Start Agent 1 (Bob)
cargo run --bin agent1

# Terminal 2: Start Agent 2 (Alice)
cargo run --bin agent2

# Follow prompts in Agent 1 to send tasks to Agent 2
```

## Key Components

### Agent Identity

```rust
let identity = AgentIdentity::new(
    "agent-1".to_string(),
    "Bob".to_string(),
    "0.1.0".to_string(),
);
```

### Agent Discovery

```rust
let discovery = A2ADiscovery::new(session.clone());
let card = AgentCard::new(
    identity.clone(),
    "Bob".to_string(),
    "A helpful assistant agent".to_string(),
);

discovery.announce(&card).await?;
```

### Task Delegation

```rust
let client = A2AClient::new(session.clone(), sender_identity);

let task = Task::new(
    uuid::Uuid::new_v4().to_string(),
    "Hello from Bob!".to_string(),
);

let result = client.delegate_task(&recipient_identity, task).await?;
```

### Incoming Task Handler

Each agent subscribes to their task topic and processes incoming tasks:

```rust
let task_topic = format!("agent/{}/tasks/incoming", identity.id);
let subscriber = session.declare_subscriber(&task_topic).await?;

while let Ok(sample) = subscriber.recv() {
    if let Ok(message) = serde_json::from_slice::<A2AMessage>(&sample.payload().to_bytes()) {
        if let A2AContent::TaskRequest { task } = message.content {
            process_task(session.clone(), identity.clone(), task).await?;
        }
    }
}
```

## Configuration

No configuration files required. The example uses:
- Default Zenoh configuration (peer mode)
- Mock LLM client (no API key needed)
- Command-line mode for agent selection

## Expected Output

**Agent 1 (Bob):**
```
Connected to Zenoh bus
Announced as Bob (agent-1)
Discovered 1 agent: Alice (agent-2)

Enter task to send to Alice (or 'quit'): Hello Alice!
Sending task to Alice...
Response: Hello from Alice! It's nice to meet you.
```

**Agent 2 (Alice):**
```
Connected to Zenoh bus
Announced as Alice (agent-2)
Received task from Bob: "Hello Alice!"
Processing task...
Task completed: "Greetings! It's nice to meet you."
```

## Error Handling

The example demonstrates various error scenarios:
- **Timeout**: Task delegation fails if no response within timeout
- **Agent not found**: Discovery fails if recipient not online
- **Invalid task**: Malformed messages are logged and ignored

## Next Steps

After understanding basic communication:
1. Try `tool-interactions` to learn cross-agent tool calls
2. Explore `wechat-demo` for streaming responses and conversations
