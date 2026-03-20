# BrainOS Agent Examples

This directory contains realistic examples demonstrating how to use the BrainOS Agent Framework.

## Overview

| Example | Complexity | Demonstrates | Real LLM? |
|---------|-----------|--------------|-----------|
| [llm-agent-demo](./llm-agent-demo/) | Advanced | Full Agent lifecycle with real LLM, tools, A2A communication | ✓ Yes |
| [basic-communication](./basic-communication/) | Simple | Two agents exchanging messages via A2A protocol | - Optional |

## Quick Start

**RECOMMENDED**: Start with `llm-agent-demo` for the complete framework experience with real LLM.

```bash
# Terminal 1: Start calculator agent
cd examples/llm-agent-demo
cargo run --bin bob

# Terminal 2: Start conversational agent (requires LLM API key)
export OPENAI_API_KEY="your-key"
export OPENAI_API_BASE_URL="https://api.openai.com/v1"
export OPENAI_MODEL="gpt-4o"

cargo run --bin alice
```

1. **Rust 1.70+** - The examples use modern Rust features
2. **Zenoh** - Install and start Zenoh router:
   ```bash
   # Install Zenoh
   cargo install zenohd

   # Start Zenoh router (default port 7447)
   zenohd
   ```
 3. **OpenAI-compatible API** - Real LLM examples require API configuration:
    ```bash
    # For llm-agent-demo and wechat-demo
    export OPENAI_API_KEY="your-actual-key"
    export OPENAI_API_BASE_URL="https://api.openai.com/v1"
    export OPENAI_MODEL="gpt-4o"  # or your preferred model
    ```
    **Note**: If not set, examples will use a mock LLM client for testing.

## Common Patterns

All examples share these patterns from `brainos-common/`:

### Bus Setup

```rust
use brainos_common::setup_bus;

// Connect to Zenoh with default config
let session = setup_bus(None).await?;

// Or connect with custom config
let config = ZenohConfig {
    mode: "client".to_string(),
    connect: vec!["tcp/127.0.0.1:7447".to_string()],
    ..Default::default()
};
let session = setup_bus(Some(config)).await?;
```

### Agent Construction

```rust
use brainos_common::create_llm_client;
use agent::{Agent, AgentConfig};

// Create mock LLM for examples
let llm = Arc::new(MockLlmClient::new(vec![
    LlmResponse::Text("Hello!".to_string()),
    LlmResponse::Done,
]));

// Build agent
let config = AgentConfig {
    name: "example-agent".to_string(),
    model: "gpt-4o".to_string(),
    base_url: std::env::var("OPENAI_API_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
    api_key: std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| "sk-test".to_string()),
    system_prompt: "You are a helpful assistant.".to_string(),
    temperature: 0.7,
    max_tokens: Some(1000),
    timeout_secs: 60,
};

let agent = Agent::new(config, llm);
```

### A2A Communication

```rust
use agent::a2a::{AgentIdentity, A2AClient};

// Create agent identities
let sender = AgentIdentity::new(
    "agent-1".to_string(),
    "Agent One".to_string(),
    "0.1.0".to_string(),
);

let recipient = AgentIdentity::new(
    "agent-2".to_string(),
    "Agent Two".to_string(),
    "0.1.0".to_string(),
);

// Create A2A client
let client = A2AClient::new(session.clone(), sender);

// Delegate task
let task = Task::new(
    "task-123".to_string(),
    "Say hello".to_string(),
);

let result = client.delegate_task(&recipient, task).await?;
```

### Tool Registration

```rust
use agent::{Tool, ToolRegistry, ToolDescription, ToolError};
use async_trait::async_trait;
use std::sync::Arc;

struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str {
        "add"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Add two numbers".to_string(),
            parameters: "Two numbers: a, b".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "number"},
                "b": {"type": "number"}
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let a = args["a"].as_f64().ok_or_else(|| {
            ToolError::SchemaMismatch("field 'a' is required".to_string())
        })?;
        let b = args["b"].as_f64().ok_or_else(|| {
            ToolError::SchemaMismatch("field 'b' is required".to_string())
        })?;
        Ok(serde_json::json!(a + b))
    }
}

// Register tool
let mut registry = ToolRegistry::new();
registry.register(Arc::new(AddTool))?;

// Execute tool
let result = registry.execute("add", serde_json::json!({"a": 1, "b": 2})).await?;
```

## Running Examples

### LLM Agent Demo (RECOMMENDED)

Complete demonstration with real LLM, tools, and A2A communication.

```bash
# Terminal 1: Start Bob (calculator agent)
cd examples/llm-agent-demo
cargo run --bin bob

# Terminal 2: Start Alice (conversational agent with real LLM)
export OPENAI_API_KEY="your-actual-key"
export OPENAI_API_BASE_URL="https://api.openai.com/v1"
export OPENAI_MODEL="gpt-4o"

cargo run --bin alice
```

Alice will use real LLM to:
- Understand natural language requests
- Discover Bob's calculator tools
- Call Bob's tools via RPC when calculations are needed
- Respond with helpful, conversational answers

**Try these interactions:**
- "What's 15 + 27?"
- "Multiply 12 by 8"
- "Calculate 100 - 37"
- "Hello Bob!"

### Basic Communication

```bash
cd examples/basic-communication
cargo run --bin agent1
# In another terminal:
cargo run --bin agent2
```

## Common Utilities

The `examples/brainos-common/` directory provides reusable utilities across all examples:

- **bus.rs**: Zenoh session setup and management (via `setup_bus()`)
- **llm.rs**: LLM client factory for real/mock LLM creation (via `create_llm_client()`)
- **logging.rs**: Structured logging setup (via `setup_logging()`)

## Learning Path

1. **Start with `llm-agent-demo`** (recommended): Full framework experience with:
   - Real LLM integration (or mock for testing)
   - Agent struct usage
   - Tool registration (local and RPC)
   - A2A protocol communication
   - Cross-agent tool calls

2. **Explore `basic-communication`**: Lower-level A2A protocol without Agent wrapper
   - Understand A2A message flow
   - Agent discovery and announcement
   - Direct task delegation
   - Manual response handling

3. **Coming soon**: `wechat-demo` - Terminal chat with streaming responses and conversation state

## Example Details

### LLM Agent Demo Components

**Bob (Calculator Agent):**
- Registers RPC services: `add`, `multiply`, `subtract`
- Handles A2A tasks from other agents
- Performs calculations via RPC handlers

**Alice (Conversational Agent):**
- Uses real LLM for natural language processing
- Discovers Bob's tools via RPC discovery
- Calls Bob's tools when calculations needed
- Maintains conversation context via Agent struct

**Key Concepts Demonstrated:**
- Full Agent lifecycle with `Agent::new()` and `agent.run_with_tools()`
- Real vs Mock LLM via `create_llm_client()`
- Tool registration with `ToolRegistry`
- Cross-agent tool calls via `RpcClient`
- A2A protocol via `A2AClient::delegate_task()`

### Basic Communication Components

**Lower-level A2A demonstration:**
- Manual message creation and parsing
- Direct Zenoh pub/sub on agent topics
- Tool discovery and capability announcements
- Simple request/response pattern without Agent wrapper

## Troubleshooting

### Zenosh Connection Issues

If examples fail to connect to Zenoh:
```bash
# Check Zenoh is running
zenohd --version

# Start Zenoh explicitly
zenohd -l tcp/0.0.0.0:7447
```

### API Key Errors

For examples that make real LLM calls (wechat-demo):
```bash
export OPENAI_API_KEY="your-actual-key"
export OPENAI_API_BASE_URL="https://api.openai.com/v1"
```

### Port Conflicts

Zenoh uses port 7447 by default. Use a different port:
```bash
export ZENOH_PORT=7448
zenohd -l tcp/0.0.0.0:7448
```

Then update ZenohConfig in examples.

## Contributing

When adding new examples:

1. Follow the directory structure (create workspace member in root Cargo.toml)
2. Use common utilities from `brainos-common/` (add as dependency in Cargo.toml)
3. Include proper error handling and logging
4. Add comprehensive documentation (README.md in example directory)
5. Ensure code compiles with `cargo check`
6. Test with Zenoh running: `zenohd -l tcp/0.0.0.0:7447`

## Framework Validation Roadmap

A comprehensive roadmap has been created to validate all framework requirements through demos.

**Status**: 5 phases planned, 15 demos to be created

**See**: `.planning/ROADMAP.md` for detailed phase breakdown, requirements coverage, and execution plan.

### Current Validation Status

| Component | Demo Status | Issues Found | Health Score |
|-----------|-------------|--------------|--------------|
| **RPC Handler API** | ✅ llm-agent-demo | 2-phase init, unsafe rkyv | 6/10 |
| **Discovery API** | ✅ llm-agent-demo | Naming confusion, overlap | 4/10 |
| **A2A Protocol** | ✅ llm-agent-demo | Good state machine | 7/10 |
| **Agent Framework** | ✅ llm-agent-demo | Mutating API, missing async init | 6/10 |
| **Tool System** | ✅ llm-agent-demo | Clean trait, separate from RPC | 7/10 |
| **LLM Integration** | ✅ llm-agent-demo | Real/mock client factory | 8/10 |

**Overall Framework Health**: **5.8/10** - Functional but requires ergonomic improvements

**Next Steps**:
1. Execute Phase 1 (Bus validation) → 3 new demo projects
2. Execute Phase 5 (Ergonomics) → Fix top 10 pain points
3. Update llm-agent-demo-v2 with all improvements
