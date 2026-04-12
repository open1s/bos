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

```rust
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("nvidia/meta/llama-3.1-8b-instruct");

    let agent = Agent::builder()
        .config(config)
        .build()?;

    let result = agent.run_simple("What is 42 + 58?").await?;
    println!("{}", result);

    Ok(())
}
```

---

## Core Concepts

### Agent

The `Agent` is the main component that powers AI interactions with tool support:

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default()
    .name("assistant")
    .system_prompt("You are a helpful assistant.");

let agent = Agent::builder()
    .config(config)
    .build()?;
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

### `Agent`

LLM-powered agent with tool support.

**Builder:**
```rust
Agent::builder()
    .config(config)
    .registry(registry)
    .build()
```

**Methods:**
| Method | Description |
|--------|-------------|
| `run_simple(task)` | Simple conversation |
| `react(task)` | Run with ReAct reasoning |
| `stream(task)` | Stream response |

### `AgentConfig`

Agent configuration.

**Methods:**
| Method | Description |
|--------|-------------|
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

## Error Handling

```rust
use agent::AgentError;

match agent.run_simple("Hello").await {
    Ok(result) => println!("{}", result),
    Err(AgentError::Llm(e)) => println!("LLM error: {}", e),
    Err(AgentError::Tool(e)) => println!("Tool error: {}", e),
    Err(e) => println!("Error: {}", e),
}
```

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
