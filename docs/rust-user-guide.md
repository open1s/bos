# BrainOS Rust API User Guide

This guide provides a unified, consistent API for using BrainOS in Rust. The API is designed to match the Python and JavaScript APIs for a seamless cross-language experience.

## Table of Contents

1. [Installation](#installation)
2. [Quick Start](#quick-start)
3. [Core Concepts](#core-concepts)
4. [Agent API](#agent-api)
5. [Tool Registration](#tool-registration)
6. [Bus Communication](#bus-communication)
7. [Query/Queryable](#queryqueryable)
8. [Caller/Callable](#callercallable)
9. [Configuration](#configuration)
10. [API Reference](#api-reference)

---

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
agent = { path = "../crates/agent" }
bus = { path = "../crates/bus" }
config = { path = "../crates/config" }
tokio = { workspace = true, features = ["full"] }
serde = { workspace = true }
serde_json = { workspace = true }
```

---

## Quick Start

### Using AgentBuilder (TOML-based, Recommended)

```rust
use agent::AgentBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let toml = r#"
        name = "assistant"
        model = "nvidia/meta/llama-3.1-8b-instruct"
        base_url = "https://integrate.api.nvidia.com/v1"
        api_key = "your-api-key"
    "#;

    let builder = AgentBuilder::from_toml(toml)?;
    let agent = builder.build(None).await?;

    let result = agent.run_simple("What is 42 + 58?").await?;
    println!("{}", result);

    Ok(())
}
```

### Using AgentConfig Directly

```rust
use agent::{Agent, AgentConfig};
use agent::llm::LlmProvider;
use agent::llm::openai::OpenAiClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("nvidia/meta/llama-3.1-8b-instruct");

    let provider = LlmProvider::new();
    provider.register_vendor("openai", Box::new(OpenAiClient::new(
        "https://api.openai.com/v1",
        "sk-...".to_string(),
    )));

    let agent = Agent::new(config, Arc::new(provider));

    let result = agent.run_simple("What is 42 + 58?").await?;
    println!("{}", result);

    Ok(())
}
```

---

## Core Concepts

### AgentBuilder (TOML-based)

The recommended way to create an Agent is using `AgentBuilder` which can load configuration from TOML:

```rust
use agent::AgentBuilder;

let builder = AgentBuilder::from_toml(toml_str)?;
// Or from a file:
let builder = AgentBuilder::from_file("/path/to/config.toml")?;

// Add tools
builder = builder.with_tool(Arc::new(MyTool));

// Build the agent
let agent = builder.build(None).await?;
```

### Agent

The `Agent` is the main component that powers AI interactions with tool support:

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default()
    .name("assistant")
    .system_prompt("You are a helpful assistant.");

// Agent::new requires a config and an LLM provider
let agent = Agent::new(config, llm_provider);
```

### Tool

Tools are functions that the LLM can call. Implement the `Tool` trait:

```rust
use agent::{Tool, ToolDescription};
use async_trait::async_trait;
use serde_json::Value;

struct Calculator;

#[async_trait]
impl Tool for Calculator {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Evaluate a mathematical expression".to_string(),
            parameters: "expression: string".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": {
                    "type": "string",
                    "description": "Math expression to evaluate"
                }
            },
            "required": ["expression"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, agent::ToolError> {
        let expr = args.get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        
        let result = eval::eval(expr).map_err(|e| agent::ToolError::ExecutionFailed(e.to_string()))?;
        Ok(serde_json::json!({ "result": result }))
    }
}
```

---

## Agent API

### Creating an Agent

```rust
use agent::{Agent, AgentConfig};

// Basic agent
let config = AgentConfig::default();
let agent = Agent::builder()
    .config(config)
    .build()?;

// With custom settings
let config = AgentConfig::default()
    .name("coder")
    .model("gpt-4")
    .system_prompt("You are a helpful coding assistant.")
    .temperature(0.5)
    .timeout(std::time::Duration::from_secs(180));

let agent = Agent::builder()
    .config(config)
    .build()?;
```

### Adding Tools

```rust
use agent::tools::ToolRegistry;

let mut registry = ToolRegistry::new();
registry.add(Arc::new(Calculator)).await?;
```

### Running the Agent

```rust
// Simple Q&A (no tool use)
let result = agent.run_simple("What is Python?").await?;

// Run with tool use (ReAct)
let result = agent.react("Calculate 2 + 2").await?;

// Streaming response
let mut stream = agent.stream("Tell me a story").await?;
while let Some(token) = stream.next().await {
    print!("{}", token);
}
```

---

## Tool Registration

### Implementing the Tool Trait

```rust
use async_trait::async_trait;
use serde_json::Value;
use agent::{Tool, ToolDescription, ToolError};

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "my_tool"
    }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Description of my tool".to_string(),
            parameters: "param1: string, param2: integer".to_string(),
        }
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "param1": { "type": "string" },
                "param2": { "type": "integer" }
            },
            "required": ["param1"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let param1 = args.get("param1")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        
        Ok(serde_json::json!({ "result": param1 }))
    }
}
```

### Registering Tools

```rust
use agent::tools::ToolRegistry;
use std::sync::Arc;

let mut registry = ToolRegistry::new();
registry.add(Arc::new(MyTool)).await?;
```

### MCP Tools (Model Context Protocol)

Register MCP tools from an MCP client:

```rust
use agent::McpClient;
use std::sync::Arc;

// Create and initialize MCP client
let mcp_client = Arc::new(McpClient::new());
agent.register_mcp_tools(mcp_client.clone()).await?;

// With namespace
agent.register_mcp_tools_with_namespace(mcp_client, "mcp").await?;
```

### Remote Agent Tools

Create tools that call other agents via RPC:

```rust
use bus::Session;

// Add a remote agent as a tool
agent.add_remote_agent_tool(
    "remote_assistant",  // tool name
    "tcp://localhost:5555".to_string(),  // endpoint
    Arc::new(session),   // bus session
).await?;
```

### Skills Loading

Load skills from a directory:

```rust
use std::path::PathBuf;

// Register skills from a directory
agent.register_skills_from_dir(PathBuf::from("/path/to/skills"))?;

// Get skill schemas for LLM
let schemas = agent.get_skills_schemas();

// Get skill content (name, description pairs)
let content = agent.get_skills_content();
```

---

## Bus Communication

The Bus provides pub/sub messaging between components.

### Creating a Bus

```rust
use bus::{Bus, BusConfig};

let config = BusConfig::default()
    .mode("peer");
    
let bus = Bus::create(config).await?;
```

### Publisher

```rust
use bus::Publisher;

let publisher = bus.create_publisher("my/topic").await?;
publisher.publish_text("hello").await?;
publisher.publish_json(serde_json::json!({"key": "value"})).await?;
```

### Subscriber

```rust
use bus::Subscriber;

let subscriber = bus.create_subscriber("my/topic").await?;

// One-shot receive
let msg = subscriber.recv().await?;

// With timeout
let msg = subscriber.recv_with_timeout(Duration::from_millis(5000)).await?;

// Callback loop
subscriber.run(|msg| async move {
    println!("Received: {}", msg);
}).await?;
```

---

## Query/Queryable

Request-response pattern with timeout support.

### Server Side (Queryable)

```rust
use bus::{Bus, BusConfig, Queryable};

fn upper_handler(text: &str) -> String {
    text.to_uppercase()
}

let bus = Bus::create(BusConfig::default()).await?;
let queryable = bus.create_queryable("svc/upper").await?;
queryable.set_handler(upper_handler);
queryable.start().await?;
```

### Client Side (Query)

```rust
use bus::{Bus, BusConfig, Query};

let bus = Bus::create(BusConfig::default()).await?;
let query = bus.create_query("svc/upper").await?;

let result = query.query_text("hello").await?;  // "HELLO"
let result = query.query_text_timeout_ms("hello", 5000).await?;  // with timeout
```

---

## Caller/Callable

RPC-style request-response pattern.

### Server Side (Callable)

```rust
use bus::{Bus, BusConfig, Callable};

fn echo_handler(text: &str) -> String {
    format!("echo: {}", text)
}

let bus = Bus::create(BusConfig::default()).await?;
let callable = bus.create_callable("svc/echo").await?;
callable.set_handler(echo_handler);
callable.start().await?;
```

### Client Side (Caller)

```rust
use bus::{Bus, BusConfig, Caller};

let bus = Bus::create(BusConfig::default()).await?;
let caller = bus.create_caller("svc/echo").await?;

let result = caller.call_text("ping").await?;  // "echo: ping"
```

---

## Configuration

### Using Config Files

BrainOS looks for config in:
- `~/.bos/conf/config.toml`
- `./conf/config.toml`

Example config:
```toml
[global_model]
api_key = "your-api-key"
base_url = "https://integrate.api.nvidia.com/v1"
model = "nvidia/meta/llama-3.1-8b-instruct"
```

### Using ConfigLoader

```rust
use config::ConfigLoader;

let loader = ConfigLoader::new();
loader.discover();
let config = loader.load_sync()?;
```

### Environment Variables

- `BOS_API_KEY` - API key for LLM
- `BOS_BASE_URL` - Base URL for LLM API
- `BOS_MODEL` - Model name

---

## API Reference

For the complete API reference, see [Rust API Reference](./api-reference/rust-api.md).

### `Agent`

LLM-powered agent with tool support.

**Constructor:**
```rust
Agent::new(config: AgentConfig, llm: Arc<LlmProvider>) -> Self
```

**Methods:**
| Method | Description |
|--------|-------------|
| `run_simple(task)` | Simple conversation |
| `react(task)` | Run with ReAct reasoning |
| `stream(task)` | Stream response |
| `add_remote_agent_tool(...)` | Add a remote agent as a tool |
| `register_mcp_tools(...)` | Register MCP tools |
| `register_skills_from_dir(path)` | Load skills from directory |
| `get_skills_schemas()` | Get skill schemas for LLM |
| `save_session(path)` | Save agent session to file |
| `restore_session(path)` | Restore session from file |

### `AgentBuilder`

TOML-based builder for creating agents (alias: `TomlAgentBuilder`).

**Factory Methods:**
| Method | Description |
|--------|-------------|
| `from_toml(toml_str)` | Create from TOML string |
| `from_file(path)` | Create from TOML file |
| `with_tool(tool)` | Add a tool |

**Example:**
```rust
let builder = AgentBuilder::from_toml(toml_str)?;
let agent = builder.build(None).await?;
```

### `AgentConfig`

Agent configuration.

**Fields:**
| Field | Default | Description |
|-------|---------|-------------|
| `name` | `"agent"` | Agent name |
| `model` | `"gpt-4"` | Model name |
| `base_url` | `"https://api.openai.com/v1"` | API base URL |
| `api_key` | `""` | API key |
| `system_prompt` | `"You are a helpful assistant."` | System prompt |
| `temperature` | `0.7` | Sampling temperature |
| `timeout_secs` | `60` | Request timeout |
| `max_steps` | `10` | Maximum ReAct steps |
| `circuit_breaker` | `None` | Circuit breaker config |
| `rate_limit` | `None` | Rate limiter config |
| `context_compaction_threshold_tokens` | `24000` | Tokens before compaction |
| `context_compaction_trigger_ratio` | `0.85` | Trigger ratio for compaction |
| `context_compaction_keep_recent_messages` | `12` | Messages to keep during compaction |
| `context_compaction_max_summary_chars` | `4000` | Max summary length |
| `context_compaction_summary_max_tokens` | `600` | Max summary tokens |
| `name(name)` | Set agent name |
| `model(model)` | Set model |
| `system_prompt(prompt)` | Set system prompt |
| `temperature(temp)` | Set temperature |
| `timeout(duration)` | Set timeout |
| `base_url(url)` | Set base URL |
| `api_key(key)` | Set API key |
| `rate_limit(config)` | Set rate limiter config |
| `circuit_breaker(config)` | Set circuit breaker config |

**Resilience Configuration:**

```rust
use agent::{Agent, AgentConfig};
use react::{CircuitBreakerConfig, RateLimiterConfig};
use std::time::Duration;

let config = AgentConfig::default()
    .name("assistant")
    .model("gpt-4")
    .circuit_breaker(CircuitBreakerConfig {
        max_failures: 5,
        cooldown: Duration::from_secs(30),
    })
    .rate_limit(RateLimiterConfig {
        capacity: 40,
        window: Duration::from_secs(60),
        max_retries: 3,
        retry_backoff: Duration::from_secs(1),
        auto_wait: true,
    });
```

**Circuit Breaker Options:**
| Field | Default | Description |
|-------|---------|-------------|
| `max_failures` | 5 | Failures before opening circuit |
| `cooldown` | 30s | Seconds before half-open state |

**Rate Limiter Options:**
| Field | Default | Description |
|-------|---------|-------------|
| `capacity` | 40 | Max requests per window |
| `window` | 60s | Window duration |
| `max_retries` | 3 | Retry attempts on 429 errors |
| `retry_backoff` | 1s | Initial backoff duration |
| `auto_wait` | true | Auto-wait when rate limited |

### `ToolRegistry`

Tool registry for managing tools.

**Methods:**
| Method | Description |
|--------|-------------|
| `add(tool)` | Add a tool |
| `get(name)` | Get a tool |
| `iter()` | Iterate tools |

### `Bus`

Message bus for inter-component communication.

**Methods:**
| Method | Description |
|--------|-------------|
| `create(config)` | Create a new bus |
| `publish_text(topic, payload)` | Publish text |
| `publish_json(topic, data)` | Publish JSON |
| `create_publisher(topic)` | Create publisher |
| `create_subscriber(topic)` | Create subscriber |
| `create_query(topic)` | Create query client |
| `create_queryable(topic)` | Create queryable server |
| `create_caller(name)` | Create caller client |
| `create_callable(uri)` | Create callable server |

### `Publisher`

Message publisher.

**Methods:**
| Method | Description |
|--------|-------------|
| `publish_text(payload)` | Publish text |
| `publish_json(data)` | Publish JSON |

### `Subscriber`

Message subscriber.

**Methods:**
| Method | Description |
|--------|-------------|
| `recv()` | Receive message |
| `recv_with_timeout(duration)` | Receive with timeout |
| `run(handler)` | Run callback loop |

### `Query`

Query client for request-response.

**Methods:**
| Method | Description |
|--------|-------------|
| `query_text(payload)` | Send query |
| `query_text_timeout_ms(payload, ms)` | Send with timeout |

### `Queryable`

Queryable server.

**Methods:**
| Method | Description |
|--------|-------------|
| `set_handler(handler)` | Set handler |
| `start()` | Start server |
| `run(handler)` | Run with handler |

### `Caller`

Caller client for RPC.

**Methods:**
| Method | Description |
|--------|-------------|
| `call_text(payload)` | Call remote service |

### `Callable`

Callable server.

**Methods:**
| Method | Description |
|--------|-------------|
| `set_handler(handler)` | Set handler |
| `start()` | Start server |
| `run(handler)` | Run with handler |
| `is_started()` | Check if running |

### `Tool`

Trait for custom tools.

**Required Methods:**
| Method | Description |
|--------|-------------|
| `name()` | Tool name |
| `description()` | Tool description |
| `json_schema()` | JSON schema |
| `execute(args)` | Execute tool |

---

## Examples

### Complete Example with Tools

```rust
use agent::{Agent, AgentConfig, Tool, ToolDescription, ToolError, tools::ToolRegistry};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio_stream::StreamExt;

struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str { "add" }
    
    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Add two numbers".to_string(),
            parameters: "a: integer, b: integer".to_string(),
        }
    }
    
    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        })
    }
    
    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let a = args.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
        let b = args.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(serde_json::json!({ "result": a + b }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("nvidia/meta/llama-3.1-8b-instruct");
    
    let mut registry = ToolRegistry::new();
    registry.add(Arc::new(AddTool)).await?;
    
    let agent = Agent::builder()
        .config(config)
        .registry(registry)
        .build()?;
    
    let result = agent.react("What is 5 + 3?").await?;
    println!("{}", result);
    
    Ok(())
}
```

### Pub/Sub Example

```rust
use bus::{Bus, BusConfig, Publisher, Subscriber};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = Bus::create(BusConfig::default()).await?;
    
    // Publisher
    let publisher = bus.create_publisher("events/start").await?;
    publisher.publish_text("Hello subscribers!").await?;
    
    // Subscriber
    let subscriber = bus.create_subscriber("events/start").await?;
    let msg = subscriber.recv_with_timeout(Duration::from_secs(5)).await?;
    println!("Received: {}", msg);
    
    Ok(())
}
```

### Query/Response Example

```rust
use bus::{Bus, BusConfig, Query, Queryable};

fn uppercase(text: &str) -> String {
    text.to_uppercase()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = Bus::create(BusConfig::default()).await?;
    
    // Server
    let queryable = bus.create_queryable("svc/uppercase").await?;
    queryable.set_handler(uppercase);
    queryable.start().await?;
    
    // Client
    let query = bus.create_query("svc/uppercase").await?;
    let result = query.query_text("hello world").await?;
    println!("{}", result);  // "HELLO WORLD"
    
    Ok(())
}
```

### RPC Example

```rust
use bus::{Bus, BusConfig, Caller, Callable};

fn echo(text: &str) -> String {
    format!("echo: {}", text)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus = Bus::create(BusConfig::default()).await?;
    
    // Server
    let callable = bus.create_callable("svc/echo").await?;
    callable.set_handler(echo);
    callable.start().await?;
    
    // Client
    let caller = bus.create_caller("svc/echo").await?;
    let result = caller.call_text("ping").await?;
    println!("{}", result);  // "echo: ping"
    
    Ok(())
}
```

---

## Hooks, Plugins, and Sessions

### Hooks

Hooks allow you to intercept and react to events during agent execution.

#### Using Hooks

```rust
use agent::{Agent, AgentConfig, HookEvent};
use agent::hooks::{HookContext, HookDecision, HookRegistry};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("gpt-4");
    
    let mut registry = agent::tools::ToolRegistry::new();
    // Add your tools here
    
    let mut agent = Agent::builder()
        .config(config)
        .registry(registry)
        .build()?;
    
    // Register hooks
    agent.hooks().register(HookEvent::BeforeToolCall, |ctx| {
        let tool_name = ctx.get::<String>("tool_name").unwrap_or_default();
        println!("Before tool call: {}", tool_name);
        HookDecision::Continue
    });
    
    agent.hooks().register(HookEvent::AfterToolCall, |ctx| {
        let tool_name = ctx.get::<String>("tool_name").unwrap_or_default();
        let tool_result = ctx.get::<String>("tool_result").unwrap_or_default();
        println!("After tool call: {} = {}", tool_name, tool_result);
        HookDecision::Continue
    });
    
    agent.hooks().register(HookEvent::BeforeLlmCall, |ctx| {
        let prompt = ctx.get::<String>("prompt").unwrap_or_default();
        println!("Before LLM call: {}", prompt);
        HookDecision::Continue
    });
    
    agent.hooks().register(HookEvent::AfterLlmCall, |ctx| {
        let response = ctx.get::<String>("response").unwrap_or_default();
        println!("After LLM call: {}", response);
        HookDecision::Continue
    });
    
    agent.hooks().register(HookEvent::OnError, |ctx| {
        let error = ctx.get::<String>("error").unwrap_or_default();
        println!("Error: {}", error);
        HookDecision::Continue
    });
    
    let result = agent.run_simple("Hello").await?;
    println!("{}", result);
    
    Ok(())
}
```

#### Hook Events

| Event | Description |
|-------|-------------|
| `BeforeToolCall` | Fired before a tool is called |
| `AfterToolCall` | Fired after a tool completes |
| `BeforeLlmCall` | Fired before LLM API call |
| `AfterLlmCall` | Fired after LLM API call |
| `OnMessage` | Fired for each message |
| `OnComplete` | Fired when agent completes |
| `OnError` | Fired when an error occurs |

#### Hook Context

Store and retrieve data for hook callbacks:

| Method | Description |
|--------|-------------|
| `HookContext::new(agent_id: &str) -> HookContext` | Create new context |
| `set<T: ToString>(&mut self, key: &str, value: &T) -> &mut Self` | Set a value |
| `get<T: FromStr>(&self, key: &str) -> Option<T>` | Get a value |
| `remove(&mut self, key: &str) -> Option<String>` | Remove a value |

#### Hook Decisions

Control execution flow:

| Decision | Description |
|----------|-------------|
| `HookDecision::Continue` | Proceed normally |
| `HookDecision::Abort` | Abort current operation |
| `HookDecision::Error(msg: String)` | Return an error |

### Plugins

Plugins allow you to preprocess and postprocess LLM requests and responses.

#### Using Plugins

```rust
use agent::{Agent, AgentConfig};
use agent::plugin::{AgentPlugin, LlmRequestWrapper, LlmResponseWrapper};
use async_trait::async_trait;
use std::sync::Arc;

struct MyPlugin;

#[async_trait]
impl AgentPlugin for MyPlugin {
    async fn process_llm_request(&self, wrapper: LlmRequestWrapper) -> LlmRequestWrapper {
        // Modify request before sending to LLM
        // Example: add system prompt prefix
        wrapper
    }
    
    async fn process_llm_response(&self, wrapper: LlmResponseWrapper) -> LlmResponseWrapper {
        // Modify response after receiving from LLM
        wrapper
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("gpt-4");
    
    let mut registry = agent::tools::ToolRegistry::new();
    // Add your tools here
    
    let mut agent = Agent::builder()
        .config(config)
        .registry(registry)
        .build()?;
    
    // Register plugin
    agent.plugins().register_blocking(Arc::new(MyPlugin()));
    
    let result = agent.run_simple("Hello").await?;
    println!("{}", result);
    
    Ok(())
}
```

### Session Management

BrainOS provides session management for persisting agent state across restarts.

#### Session Operations

```rust
use agent::{Agent, AgentConfig};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("gpt-4");
    
    let mut registry = agent::tools::ToolRegistry::new();
    // Add your tools here
    
    let mut agent = Agent::builder()
        .config(config)
        .registry(registry)
        .build()?;
    
    // Save session
    agent.save_message_log("/tmp/session.json")?;
    
    // Later, restore session
    // agent.restore_message_log("/tmp/session.json")?;
    
    let result = agent.run_simple("Hello").await?;
    println!("{}", result);
    
    Ok(())
}
```

#### Session Info Methods

| Method | Description |
|--------|-------------|
| `add_message(&mut self, message: LlmMessage)` | Add message to conversation log |
| `get_messages(&self) -> Vec<LlmMessage>` | Get conversation messages |
| `save_message_log(&self, path: &str) -> Result<(), AgentError>` | Save message log to file |
| `restore_message_log(&mut self, path: &str) -> Result<(), AgentError>` | Restore message log from file |

---

## Error Handling

---

## Differences from Python/JavaScript

| Feature | Python | JavaScript | Rust |
|---------|--------|------------|------|
| Tool decorator | `@tool()` | `ToolDef` class | `Tool` trait |
| Agent builder | `.with_model()` | `.withModel()` | `Agent::builder()` |
| Context manager | `async with` | `await start()/stop()` | `await` |
| Tool callback | `callback(args)` | `callback(args)` | `async fn execute(args)` |
| Error handling | `Exception` | `Error` | `Result<T, E>` |

---

## Async Runtime

BrainOS uses Tokio as its async runtime. Ensure your `main` function is async:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Your code here
    Ok(())
}
```
