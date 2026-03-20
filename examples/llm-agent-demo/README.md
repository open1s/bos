# LLM Agent Demo

This example demonstrates the full BrainOS Agent Framework with real LLM integration, tool registration, and agent-to-agent communication.

## Overview

Shows:
- **Real LLM Integration**: Uses OpenAI-compatible API (set via env vars)
- **Agent Struct Usage**: Proper Agent construction and execution
- **Tool Registration**: Register local tools for LLM to call
- **A2A Communication**: Agents can discover and communicate with each other
- **Cross-Agent Tool Calls**: Agents can call tools on other agents via RPC
- **Tool Discovery**: Discover tools available on other agents

## Architecture

```
┌─────────────────┐                    ┌─────────────────┐
│   Agent Alice   │                    │   Agent Bob     │
│                 │     A2A Task       │                 │
│  - Real LLM     │◀──────────────────▶│  - Real LLM     │
│  - Local Tools  │   + Tool Call      │  - Local Tools  │
│  - RPC Server   │◀──────────────────▶│  - RPC Server   │
└─────────────────┘                    └─────────────────┘
```

## Prerequisites

Set environment variables for real LLM:

```bash
# Required for real LLM (otherwise uses mock)
export OPENAI_API_KEY="your-api-key"
export OPENAI_API_BASE_URL="https://api.openai.com/v1"  # or your OpenAI-compatible endpoint
export OPENAI_MODEL="gpt-4o"  # or your preferred model
```

For Zenoh (distributed bus):
```bash
# Start Zenoh router if not running
zenohd
```

## Running

### Start Agent Bob (Calculator Agent)

```bash
cd examples/llm-agent-demo
cargo run --bin bob
```

Bob will:
- Announce capabilities and tools
- Register `add`, `multiply`, `subtract` tools via RPC
- Be discoverable by other agents
- Process incoming A2A tasks

### Start Agent Alice (Conversational Agent)

```bash
cd examples/llm-agent-demo
cargo run --bin alice
```

Alice will:
- Use real LLM for conversation
- Discover Bob's tools
- Call Bob's tools via RPC when needed
- Respond to user messages

## Example Interactions

**Alice (Conversational Agent):**

```
Enter your message (or 'quit'): What's 15 + 27?

[Alice discovers Bob's add tool]
[Alice calls Bob's add tool via RPC]
→ Bob's add returned: 42

Answer: 15 + 27 = 42
```

```
Enter your message (or 'quit'): What if we multiply 12 by 8?

[Alice calls Bob's multiply tool]
→ Bob's multiply returned: 96

Answer: 12 × 8 = 96
```

```
Enter your message (or 'quit'): Calculate 100 - 37

[Alice calls Bob's subtract tool]
→ Bob's subtract returned: 63

Answer: 100 - 37 = 63
```

```
Enter your message (or 'quit'): Hello Bob!

[Alice sends greeting via A2A]
→ Bob responded: "Nice to meet you Alice!"

Answer: Bob says "Nice to meet you Alice!"
```

## Key Concepts Demonstrated

### 1. Agent Construction with Real LLM

```rust
let llm = create_llm_client();  // Real or mock based on env vars
let config = AgentConfig {
    name: "alice".to_string(),
    model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
    base_url: std::env::var("OPENAI_API_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
    api_key: std::env::var("OPENAI_API_KEY")
        .unwrap_or_else(|_| "sk-test".to_string()),
    system_prompt: "You are Alice, a helpful AI assistant...".to_string(),
    temperature: 0.7,
    max_tokens: Some(1000),
    timeout_secs: 60,
};

let mut agent = Agent::new(config, llm);
```

### 2. Tool Registration (Alice)

Alice registers tools locally for the LLM to call:

```rust
let mut tool_registry = ToolRegistry::new();

// Register tool that calls Bob via RPC
let add_tool = Arc::new(ToolInvoker::new(
    "add",
    "Add two numbers using Bob's calculator",
    &bob_rpc_client,
));

tool_registry.register(add_tool)?;
```

### 3. Tool Registration as RPC Service (Bob)

Bob exposes tools as RPC endpoints:

```rust
let add_service = RpcService::builder("agent/bob/tools/add")
    .with_handler(Arc::new(AddHandler::new()))
    .build();

add_service.register(session.clone()).await?;
```

### 4. A2A Communication

Agents communicate via A2A protocol:

```rust
let client = A2AClient::new(session.clone(), identity);
let task = Task::new(task_id, message);

let result = client.delegate_task(&recipient, task).await?;
```

### 5. Tool Discovery

Alice can discover Bob's tools:

```rust
let discovery = RpcDiscovery::new();
let services = discovery.discover_all(Some(session.clone())).await?;

// Filter for Bob's tools
let bob_tools: Vec<_> = services
    .into_iter()
    .filter(|s| s.name.starts_with("agent/bob/tools/"))
    .collect();
```

## Error Handling

The demo demonstrates proper error handling for:
- **LLM API failures**: Falls back to mock if API key missing
- **RPC call failures**: Gracefully handle timeouts and network issues
- **Agent discovery**: Handle cases where agents are offline
- **Tool execution**: Catch and report errors from tool calls

## Next Steps

- Try `tool-interactions` for more advanced tool patterns
- Explore `wechat-demo` for streaming responses and conversation state
- Read the main README for framework overview

## Troubleshooting

### LLM API Errors

If real LLM fails:
```bash
# Check API key
echo $OPENAI_API_KEY

# Test connectivity
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY"
```

### Discovery Not Working

If agents can't discover each other:
```bash
# Check Zenoh is running
zenohd --version

# List Zenoh peers
zenohd -s tcp/0.0.0.0:7447
```

### Tool Call Failing

If RPC tool calls fail:
```bash
# Check if services are registered
# In agent logs, look for:
# "Registered RPC service: agent/bob/tools/add"

# Test RPC directly
# Use the `test-rpc` command in the agent interactive mode
```
