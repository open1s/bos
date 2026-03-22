# BrainOS Agent Development Guide

**Version:** 1.0
**Date:** 2026-03-22
**Audience:** Developers building business systems with the BrainOS Agent Framework

---

## Table of Contents

1. [Introduction](#1-introduction)
2. [Quick Start](#2-quick-start)
3. [Core Concepts](#3-core-concepts)
4. [Development Patterns](#4-development-patterns)
   - 4.1 [Quick Start Pattern](#41-quick-start-pattern)
   - 4.2 [Tool Development Pattern](#42-tool-development-pattern)
   - 4.3 [Multi-Agent Communication Pattern](#43-multi-agent-communication-pattern)
   - 4.4 [Workflow Orchestration Pattern](#44-workflow-orchestration-pattern)
   - 4.5 [Session Management Pattern](#45-session-management-pattern)
5. [Business System Examples](#5-business-system-examples)
6. [Best Practices](#6-best-practices)
7. [Quick Reference](#7-quick-reference)
8. [Appendix](#8-appendix)

---

## 1. Introduction

The BrainOS Agent Framework provides a Rust-based infrastructure for building distributed AI agents. Key capabilities include:

- **Agent Lifecycle Management**: Build, start, run, and stop agents
- **A2A Communication**: Agent-to-agent discovery and task delegation
- **Tool System**: Local and RPC-based tool registration and execution
- **Streaming Output**: Token streaming with backpressure control
- **Workflow Orchestration**: Sequential, parallel, and conditional execution
- **Session Persistence**: State management across restarts

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     Application Layer                        │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │   Agent     │  │   Tools     │  │   A2A Client        │  │
│  │  (Core)     │  │  (Registry) │  │  (Communication)    │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│                     Orchestration Layer                      │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │  Scheduler  │  │   Session   │  │   Streaming         │  │
│  │  (Workflow) │  │  (Persist)  │  │   (Token Flow)      │  │
│  └─────────────┘  └─────────────┘  └─────────────────────┘  │
├─────────────────────────────────────────────────────────────┤
│                     Transport Layer                          │
│  ┌─────────────────────────────────────────────────────────┐│
│  │                    Zenoh Bus                             ││
│  │         (Pub/Sub, RPC, Discovery)                       ││
│  └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

---

## 2. Quick Start

### Prerequisites

1. **Rust 1.70+** with `cargo`
2. **Zenoh router** for distributed communication
3. **OpenAI-compatible API** (optional, for real LLM)

### Installation

```fish
# Install Zenoh
cargo install zenohd

# Start Zenoh router
zenohd

# Clone and build
git clone <repo-url> bos
cd bos
cargo build --workspace
```

### Minimal Agent Example

```rust
use std::sync::Arc;
use agent::{Agent, AgentConfig, AgentOutput};
use agent::llm::OpenAiClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create LLM client
    let llm = Arc::new(OpenAiClient::new(
        "https://api.openai.com/v1".to_string(),
        std::env::var("OPENAI_API_KEY").expect("API key required"),
    ));

    // Configure agent
    let config = AgentConfig {
        name: "my-agent".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: std::env::var("OPENAI_API_KEY")?,
        system_prompt: "You are a helpful assistant.".to_string(),
        temperature: 0.7,
        max_tokens: Some(1000),
        timeout_secs: 60,
    };

    // Create and run agent
    let mut agent = Agent::new(config, llm);
    let output = agent.run("Hello, world!").await?;

    match output {
        AgentOutput::Text(text) => println!("Response: {}", text),
        AgentOutput::Error(e) => eprintln!("Error: {}", e),
    }

    Ok(())
}
```

---

## 3. Core Concepts

### 3.1 Agent

The `Agent` struct is the core abstraction for LLM-powered agents:

```rust
pub struct Agent {
    config: AgentConfig,
    llm: Arc<dyn LlmClient>,
    message_log: MessageLog,
}
```

**Key Methods:**
- `Agent::new(config, llm)` — Create agent with config and LLM client
- `agent.run(task)` — Execute task without tools
- `agent.run_with_tools(task, registry)` — Execute with tool access
- `agent.stream_run(task)` — Stream tokens as they arrive
- `agent.save_state(manager)` — Persist session state
- `agent.restore_state(manager)` — Recover from saved state

### 3.2 Tool System

Tools extend agent capabilities. The `Tool` trait defines the interface:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> ToolDescription;
    fn json_schema(&self) -> serde_json::Value;
    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError>;
}
```

**ToolRegistry** manages tool registration and execution:
- Local tools: Registered directly, executed in-process
- RPC tools: Registered via Zenoh, executed remotely
- Namespaced tools: Avoid conflicts with `{namespace}/{tool_name}` format

### 3.3 A2A Protocol

Agent-to-Agent communication enables distributed systems:

```rust
pub struct A2AClient {
    session: Arc<Session>,
    identity: AgentIdentity,
    idempotency: IdempotencyStore,
    timeout: Duration,
}
```

**Key Operations:**
- `discovery.announce(&card)` — Announce agent presence
- `discovery.discover(filter)` — Find other agents
- `client.delegate_task(&recipient, task)` — Send task to another agent
- `client.poll_status(task_id)` — Check task status

### 3.4 Scheduler

Workflow orchestration with retry and timeout:

```rust
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
    pub default_timeout: Duration,
    pub default_retries: u32,
}

pub enum StepType {
    Sequential,
    Parallel,
    Conditional { condition: ConditionType },
}
```

### 3.5 Session

State persistence for long-running agents:

```rust
pub struct AgentState {
    pub agent_id: String,
    pub message_log: Vec<Message>,
    pub context: serde_json::Value,
    pub metadata: SessionMetadata,
}
```

---

## 4. Development Patterns

### 4.1 Quick Start Pattern

#### Basic Agent Setup

```rust
use std::sync::Arc;
use agent::{Agent, AgentConfig, AgentOutput};
use agent::llm::OpenAiClient;

async fn create_agent() -> Result<Agent, AgentError> {
    let llm = Arc::new(OpenAiClient::new(
        "https://api.openai.com/v1".to_string(),
        std::env::var("OPENAI_API_KEY")
            .expect("OPENAI_API_KEY must be set"),
    ));

    let config = AgentConfig {
        name: "assistant".to_string(),
        model: "gpt-4o".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: std::env::var("OPENAI_API_KEY")?,
        system_prompt: "You are a helpful assistant.".to_string(),
        temperature: 0.7,
        max_tokens: Some(2000),
        timeout_secs: 60,
    };

    Ok(Agent::new(config, llm))
}
```

#### Agent with Tools

```rust
use agent::{Tool, ToolRegistry, ToolDescription, ToolError};
use async_trait::async_trait;

// Define a custom tool
struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str { "get_weather" }

    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: "Get current weather for a location".to_string(),
            parameters: "location: City name".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "City name"
                }
            },
            "required": ["location"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let location = args["location"].as_str()
            .ok_or_else(|| ToolError::SchemaMismatch("location required".to_string()))?;

        // Call weather API...
        let weather = fetch_weather(location).await?;

        Ok(serde_json::json!({
            "location": location,
            "temperature": weather.temp,
            "conditions": weather.conditions
        }))
    }
}

// Register and use
async fn run_with_tools() -> Result<(), AgentError> {
    let mut agent = create_agent().await?;
    let mut registry = ToolRegistry::new();

    registry.register(Arc::new(WeatherTool))?;

    let output = agent.run_with_tools(
        "What's the weather in Tokyo?",
        &registry
    ).await?;

    println!("{:?}", output);
    Ok(())
}
```

### 4.2 Tool Development Pattern

#### Local Tool Implementation

```rust
use agent::{Tool, ToolDescription, ToolError};
use async_trait::async_trait;
use std::sync::Arc;

/// Calculator tool for basic math operations
pub struct CalculatorTool {
    operation: MathOperation,
}

pub enum MathOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        match self.operation {
            MathOperation::Add => "add",
            MathOperation::Subtract => "subtract",
            MathOperation::Multiply => "multiply",
            MathOperation::Divide => "divide",
        }
    }

    fn description(&self) -> ToolDescription {
        let desc = match self.operation {
            MathOperation::Add => "Add two numbers",
            MathOperation::Subtract => "Subtract second from first",
            MathOperation::Multiply => "Multiply two numbers",
            MathOperation::Divide => "Divide first by second",
        };

        ToolDescription {
            short: desc.to_string(),
            parameters: "a: number, b: number".to_string(),
        }
    }

    fn json_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "a": {
                    "type": "number",
                    "description": "First operand"
                },
                "b": {
                    "type": "number",
                    "description": "Second operand"
                }
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let a = args["a"].as_f64()
            .ok_or_else(|| ToolError::SchemaMismatch("'a' must be a number".to_string()))?;
        let b = args["b"].as_f64()
            .ok_or_else(|| ToolError::SchemaMismatch("'b' must be a number".to_string()))?;

        let result = match self.operation {
            MathOperation::Add => a + b,
            MathOperation::Subtract => a - b,
            MathOperation::Multiply => a * b,
            MathOperation::Divide => {
                if b == 0.0 {
                    return Err(ToolError::ExecutionFailed("Division by zero".to_string()));
                }
                a / b
            }
        };

        Ok(serde_json::json!({
            "result": result,
            "operation": format!("{:?}({}, {}) = {}", self.operation, a, b, result)
        }))
    }
}

// Registration
fn register_calculator_tools(registry: &mut ToolRegistry) -> Result<(), ToolError> {
    registry.register(Arc::new(CalculatorTool { operation: MathOperation::Add }))?;
    registry.register(Arc::new(CalculatorTool { operation: MathOperation::Subtract }))?;
    registry.register(Arc::new(CalculatorTool { operation: MathOperation::Multiply }))?;
    registry.register(Arc::new(CalculatorTool { operation: MathOperation::Divide }))?;
    Ok(())
}
```

#### RPC Tool Registration (Server Side)

```rust
use bus::{RpcHandler, RpcServiceBuilder, RpcServiceError};
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: Vec<u8>,
}

struct AddHandler;

#[async_trait::async_trait]
impl RpcHandler for AddHandler {
    async fn handle(&self, _method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        // Safe deserialization with rkyv
        let json_payload: JsonPayload = rkyv::from_bytes::<JsonPayload, rkyv::rancor::Error>(payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let args: serde_json::Value = serde_json::from_slice(&json_payload.json)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let a = args["a"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("'a' required".to_string()))?;
        let b = args["b"].as_f64()
            .ok_or_else(|| RpcServiceError::Internal("'b' required".to_string()))?;

        let result = a + b;
        let response = serde_json::json!({
            "result": result,
            "operation": format!("{} + {} = {}", a, b, result)
        });

        let json = serde_json::to_vec(&response)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let result_payload = JsonPayload { json };
        let serialized = rkyv::to_bytes::<rkyv::rancor::Error>(&result_payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        Ok(serialized.into_vec())
    }
}

async fn register_rpc_tools(session: Arc<bus::Session>) -> anyhow::Result<()> {
    let tool_base = "agent/my-agent/tools/";

    RpcServiceBuilder::new()
        .service_name("add")
        .topic_prefix(tool_base)
        .build()?
        .init(&session, AddHandler).await?;

    println!("Registered: {}add", tool_base);
    Ok(())
}
```

#### RPC Tool Invocation (Client Side)

```rust
use bus::RpcClient;
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: Vec<u8>,
}

async fn call_remote_tool(
    session: Arc<bus::Session>,
    service_name: &str,
    args: &serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let mut client = RpcClient::new(service_name, "call");

    client.init(session.clone()).await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    let json = serde_json::to_vec(args)
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    let payload = JsonPayload { json };
    let request = rkyv::to_bytes::<rkyv::rancor::Error>(&payload)
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
        .into_vec();

    let response: JsonPayload = client.call(&request).await
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    serde_json::from_slice(&response.json)
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
}
```

### 4.3 Multi-Agent Communication Pattern

#### Agent Identity and Discovery

```rust
use agent::a2a::{AgentIdentity, A2ADiscovery, AgentCard};

async fn setup_agent_identity() -> (AgentIdentity, A2ADiscovery, Arc<bus::Session>) {
    // Connect to Zenoh
    let session = brainos_common::setup_bus(None).await
        .expect("Failed to connect to Zenoh");

    // Create identity
    let identity = AgentIdentity::new(
        "my-agent".to_string(),      // ID
        "My Agent".to_string(),       // Name
        "1.0.0".to_string(),          // Version
    );

    // Create discovery client
    let discovery = A2ADiscovery::new(session.clone());

    // Announce presence
    let card = AgentCard::new(
        identity.clone(),
        "My Agent".to_string(),
        "A helpful agent with calculator tools".to_string(),
    )
    .with_capability("calculation".to_string(), "Perform math operations".to_string())
    .with_capability("rpc".to_string(), "Exposes tools via RPC".to_string())
    .with_skill("calculator".to_string());

    discovery.announce(&card).await
        .expect("Failed to announce");

    println!("Announced as {}", identity.name);

    (identity, discovery, session)
}
```

#### Discovering Other Agents

```rust
async fn discover_agents(discovery: &A2ADiscovery) -> Vec<AgentCard> {
    // Discover all agents
    let agents = discovery.discover(None).await
        .expect("Discovery failed");

    println!("Found {} agents:", agents.len());
    for card in &agents {
        println!("  - {} ({})", card.name, card.agent_id.id);
        println!("    Capabilities: {:?}", card.capabilities);
        println!("    Skills: {:?}", card.skills);
    }

    agents
}

async fn discover_specific_agent(discovery: &A2ADiscovery, agent_id: &str) -> Option<AgentCard> {
    let agents = discovery.discover(None).await.ok()?;
    agents.into_iter().find(|a| a.agent_id.id == agent_id)
}
```

#### Task Delegation

```rust
use agent::a2a::{A2AClient, Task, TaskState};

async fn delegate_to_agent(
    session: Arc<bus::Session>,
    sender: AgentIdentity,
    recipient: AgentIdentity,
    task_description: &str,
) -> Result<Task, AgentError> {
    let client = A2AClient::new(session, sender);

    let task = Task::new(
        uuid::Uuid::new_v4().to_string(),
        serde_json::json!(task_description),
    );

    // Delegate and wait for response
    let result = client.delegate_task(&recipient, task).await?;

    match result.state {
        TaskState::Completed => {
            println!("Task completed: {:?}", result.output);
        }
        TaskState::Failed => {
            eprintln!("Task failed: {:?}", result.error);
        }
        _ => {}
    }

    Ok(result)
}
```

#### Handling Incoming Tasks

```rust
async fn listen_for_tasks(
    session: Arc<bus::Session>,
    identity: AgentIdentity,
) -> anyhow::Result<()> {
    let task_topic = format!("agent/{}/tasks/incoming", identity.id);
    let subscriber = session.declare_subscriber(&task_topic).await?;

    while let Ok(sample) = subscriber.recv() {
        if let Ok(message) = serde_json::from_slice::<agent::a2a::A2AMessage>(
            &sample.payload().to_bytes()
        ) {
            if let agent::a2a::A2AContent::TaskRequest { mut task } = message.content {
                // Process task
                task.state = TaskState::Working;

                // Send acknowledgment
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
                    let data = serde_json::to_vec(&response)?;
                    let _ = publisher.put(data).await;
                }

                // Execute task...
                let result = process_task(&task).await;

                // Send final response
                task.state = TaskState::Completed;
                task.output = Some(serde_json::json!(result));

                let final_response = agent::a2a::A2AMessage::task_response(
                    task,
                    identity,
                    message.sender,
                );

                if let Ok(publisher) = session.declare_publisher(&response_topic).await {
                    let data = serde_json::to_vec(&final_response)?;
                    let _ = publisher.put(data).await;
                }
            }
        }
    }

    Ok(())
}
```

### 4.4 Workflow Orchestration Pattern

#### Sequential Workflow

```rust
use agent::scheduler::{WorkflowBuilder, StepBuilder, BackoffStrategy};
use std::time::Duration;

fn create_sequential_workflow() -> Workflow {
    WorkflowBuilder::new("data-pipeline")
        .description("Process data through sequential stages")
        .default_timeout(Duration::from_secs(30))
        .default_retries(3)
        .add_step(
            StepBuilder::new("fetch-data")
                .sequential()
                .timeout(Duration::from_secs(60))
                .build()
        )
        .add_step(
            StepBuilder::new("transform-data")
                .sequential()
                .retries(5)
                .build()
        )
        .add_step(
            StepBuilder::new("store-data")
                .sequential()
                .build()
        )
        .build()
}

async fn run_workflow(workflow: &Workflow) -> WorkflowResult {
    let scheduler = Scheduler::new();
    scheduler.execute_workflow(workflow).await
}
```

#### Parallel Execution

```rust
fn create_parallel_workflow() -> Workflow {
    WorkflowBuilder::new("parallel-processing")
        .description("Process multiple data sources in parallel")
        .parallel_group("fetch-all", vec![
            ("fetch-api".to_string(), None, serde_json::json!({"source": "api"})),
            ("fetch-db".to_string(), None, serde_json::json!({"source": "database"})),
            ("fetch-cache".to_string(), None, serde_json::json!({"source": "cache"})),
        ])
        .add_step(
            StepBuilder::new("merge-results")
                .sequential()
                .build()
        )
        .build()
}
```

#### Conditional Branching

```rust
use agent::scheduler::ConditionType;

fn create_conditional_workflow() -> Workflow {
    WorkflowBuilder::new("smart-routing")
        .description("Route based on data type")
        .add_step(
            StepBuilder::new("classify-input")
                .sequential()
                .build()
        )
        .branch(
            "check-type".to_string(),
            ConditionType::JsonPath {
                path: "type".to_string(),
                expected: serde_json::json!("urgent"),
            },
            "urgent-handler".to_string(),
            "normal-handler".to_string(),
        )
        .build()
}
```

#### Remote Agent Delegation

```rust
fn create_distributed_workflow(remote_agent_id: &str) -> Workflow {
    WorkflowBuilder::new("distributed-task")
        .description("Delegate to remote agent")
        .add_step(
            StepBuilder::new("local-preprocess")
                .sequential()
                .local()
                .build()
        )
        .add_step(
            StepBuilder::new("remote-process")
                .sequential()
                .remote(remote_agent_id.to_string())
                .timeout(Duration::from_secs(120))
                .retries(2)
                .backoff(BackoffStrategy::Exponential {
                    base: Duration::from_secs(1),
                    max: Duration::from_secs(30),
                })
                .build()
        )
        .add_step(
            StepBuilder::new("local-postprocess")
                .sequential()
                .local()
                .build()
        )
        .build()
}
```

### 4.5 Session Management Pattern

#### Basic Session Persistence

```rust
use agent::session::{SessionManager, SessionConfig};
use std::path::PathBuf;

async fn setup_session_manager() -> SessionManager {
    let config = SessionConfig {
        base_dir: PathBuf::from(".bos/sessions"),
        default_ttl_secs: Some(86400), // 24 hours
        compression_enabled: true,
    };

    SessionManager::new(config)
}

async fn save_agent_state(
    agent: &Agent,
    manager: &SessionManager,
) -> Result<(), SessionError> {
    agent.save_state(manager).await
}

async fn restore_agent_state(
    agent: &mut Agent,
    manager: &SessionManager,
) -> Result<(), SessionError> {
    agent.restore_state(manager).await
}
```

#### Auto-Save Pattern

```rust
async fn run_with_auto_save(
    agent: &mut Agent,
    manager: &SessionManager,
    task: &str,
    tools: &ToolRegistry,
) -> Result<AgentOutput, AgentError> {
    // Run task
    let output = agent.run_with_tools(task, tools).await?;

    // Auto-save after each turn
    agent.auto_save(manager).await;

    Ok(output)
}
```

#### Session Recovery on Startup

```rust
async fn create_or_restore_agent(
    config: AgentConfig,
    llm: Arc<dyn LlmClient>,
    manager: &SessionManager,
) -> Result<Agent, AgentError> {
    let mut agent = Agent::new(config, llm);

    // Try to restore existing session
    match agent.restore_state(manager).await {
        Ok(()) => {
            println!("Restored previous session");
        }
        Err(SessionError::NotFound(_)) => {
            println!("Starting fresh session");
        }
        Err(e) => {
            eprintln!("Warning: Failed to restore session: {}", e);
        }
    }

    Ok(agent)
}
```

#### Session Cleanup

```rust
async fn setup_session_cleanup(manager: &mut SessionManager) {
    // Start background cleanup task (runs every hour)
    manager.start_cleanup(Duration::from_secs(3600)).await;
}

async fn list_active_sessions(manager: &SessionManager) -> Result<Vec<SessionSummary>, SessionError> {
    let sessions = manager.list().await?;

    println!("Active sessions:");
    for session in &sessions {
        println!(
            "  - {} ({} messages, updated: {})",
            session.agent_id,
            session.message_count,
            session.updated_at
        );
    }

    Ok(sessions)
}
```

---

## 5. Business System Examples

### 5.1 Calculator Service Agent (Bob Pattern)

A stateless service agent that exposes tools via RPC:

```rust
use std::sync::Arc;
use agent::a2a::{AgentIdentity, A2ADiscovery, AgentCard};
use bus::{RpcHandler, RpcServiceBuilder, RpcServiceError};
use rkyv::{Archive, Serialize, Deserialize};

#[derive(Archive, Serialize, Deserialize)]
struct JsonPayload {
    json: Vec<u8>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup
    let session = brainos_common::setup_bus(None).await?;
    let identity = AgentIdentity::new("calculator".to_string(), "Calculator".to_string(), "1.0.0".to_string());

    // Announce
    let discovery = A2ADiscovery::new(session.clone());
    let card = AgentCard::new(
        identity.clone(),
        "Calculator Service".to_string(),
        "Mathematical operations via RPC".to_string(),
    )
    .with_capability("calculation".to_string(), "Math operations".to_string())
    .with_skill("math".to_string());

    discovery.announce(&card).await?;

    // Register RPC services
    let tool_base = format!("agent/{}/tools/", identity.id);

    RpcServiceBuilder::new()
        .service_name("add")
        .topic_prefix(&tool_base)
        .build()?
        .init(&session, AddHandler::new()).await?;

    RpcServiceBuilder::new()
        .service_name("multiply")
        .topic_prefix(&tool_base)
        .build()?
        .init(&session, MultiplyHandler::new()).await?;

    println!("Calculator service running. Press Ctrl+C to exit.");

    // Keep running
    tokio::signal::ctrl_c().await?;

    Ok(())
}

struct AddHandler;
impl AddHandler { fn new() -> Self { Self } }

#[async_trait::async_trait]
impl RpcHandler for AddHandler {
    async fn handle(&self, _method: &str, payload: &[u8]) -> Result<Vec<u8>, RpcServiceError> {
        let json_payload: JsonPayload = rkyv::from_bytes(payload)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let args: serde_json::Value = serde_json::from_slice(&json_payload.json)
            .map_err(|e| RpcServiceError::Internal(e.to_string()))?;

        let a = args["a"].as_f64().unwrap_or(0.0);
        let b = args["b"].as_f64().unwrap_or(0.0);

        let response = serde_json::json!({
            "result": a + b,
            "operation": format!("{} + {} = {}", a, b, a + b)
        });

        let json = serde_json::to_vec(&response)?;
        let result_payload = JsonPayload { json };
        Ok(rkyv::to_bytes::<rkyv::rancor::Error>(&result_payload)?.into_vec())
    }
}

// Similar for MultiplyHandler...
```

### 5.2 Conversational Agent (Alice Pattern)

An LLM-powered agent that uses tools from other agents:

```rust
use std::sync::Arc;
use agent::{
    Agent, AgentConfig, AgentOutput,
    Tool, ToolRegistry, ToolDescription, ToolError,
    a2a::{AgentIdentity, A2ADiscovery, AgentCard},
};
use async_trait::async_trait;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup
    let session = brainos_common::setup_bus(None).await?;
    let identity = AgentIdentity::new("assistant".to_string(), "Assistant".to_string(), "1.0.0".to_string());

    // Create LLM client
    let llm = brainos_common::create_llm_client();

    // Configure agent
    let config = AgentConfig {
        name: "assistant".to_string(),
        model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
        base_url: std::env::var("OPENAI_API_BASE_URL")
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
        api_key: std::env::var("OPENAI_API_KEY")?,
        system_prompt: "You are a helpful assistant with access to calculator tools.".to_string(),
        temperature: 0.7,
        max_tokens: Some(2000),
        timeout_secs: 60,
    };

    let mut agent = Agent::new(config, llm);

    // Discover calculator agent
    let discovery = A2ADiscovery::new(session.clone());
    discovery.announce(&AgentCard::new(
        identity.clone(),
        "Assistant".to_string(),
        "Conversational AI".to_string(),
    )).await?;

    // Find calculator and register its tools
    let mut registry = ToolRegistry::new();

    if let Some(calc) = discovery.discover(None).await?.into_iter().find(|a| a.agent_id.id == "calculator") {
        // Register remote tools
        registry.register(Arc::new(RemoteToolInvoker::new("add", session.clone(), &calc.agent_id)))?;
        registry.register(Arc::new(RemoteToolInvoker::new("multiply", session.clone(), &calc.agent_id)))?;
    }

    // Interactive loop
    loop {
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input == "quit" { break; }

        match agent.run_with_tools(input, &registry).await? {
            AgentOutput::Text(text) => println!("Assistant: {}", text),
            AgentOutput::Error(e) => eprintln!("Error: {}", e),
        }
    }

    Ok(())
}

// Remote tool invoker (calls calculator via RPC)
struct RemoteToolInvoker {
    name: String,
    session: Arc<bus::Session>,
    target: AgentIdentity,
}

#[async_trait]
impl Tool for RemoteToolInvoker {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> ToolDescription {
        ToolDescription {
            short: format!("Call {} via RPC", self.name),
            parameters: "a, b: numbers".to_string(),
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
    async fn execute(&self, args: &serde_json::Value) -> Result<serde_json::Value, ToolError> {
        // RPC call implementation...
        call_remote_tool(self.session.clone(), &format!("agent/{}/tools/{}", self.target.id, self.name), args).await
    }
}
```

### 5.3 Workflow Orchestrator

Multi-step business process with error handling:

```rust
use agent::scheduler::{WorkflowBuilder, StepBuilder, Scheduler, BackoffStrategy};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create workflow for order processing
    let workflow = WorkflowBuilder::new("order-processing")
        .description("Process customer order end-to-end")
        .default_timeout(Duration::from_secs(300))
        .default_retries(3)
        // Step 1: Validate order
        .add_step(
            StepBuilder::new("validate-order")
                .sequential()
                .timeout(Duration::from_secs(30))
                .build()
        )
        // Step 2: Check inventory (parallel for multiple items)
        .parallel_group("check-inventory", vec![
            ("check-warehouse-a".to_string(), None, serde_json::json!({"warehouse": "A"})),
            ("check-warehouse-b".to_string(), None, serde_json::json!({"warehouse": "B"})),
        ])
        // Step 3: Process payment
        .add_step(
            StepBuilder::new("process-payment")
                .sequential()
                .timeout(Duration::from_secs(60))
                .retries(5)
                .backoff(BackoffStrategy::Exponential {
                    base: Duration::from_secs(2),
                    max: Duration::from_secs(30),
                })
                .build()
        )
        // Step 4: Conditional: digital vs physical
        .branch(
            "check-delivery-type".to_string(),
            ConditionType::JsonPath {
                path: "delivery_type".to_string(),
                expected: serde_json::json!("digital"),
            },
            "send-digital".to_string(),
            "ship-physical".to_string(),
        )
        .build();

    // Execute
    let scheduler = Scheduler::new();
    let result = scheduler.execute_workflow(&workflow).await;

    match result.status {
        WorkflowStatus::Completed => {
            println!("Order processed successfully!");
            for step in result.step_results {
                println!("  {}: {:?}", step.step_name, step.status);
            }
        }
        WorkflowStatus::Failed { failed_step } => {
            eprintln!("Order processing failed at: {}", failed_step);
        }
        WorkflowStatus::PartiallyCompleted => {
            println!("Order partially processed");
        }
    }

    Ok(())
}
```

### 5.4 Session-Aware Agent

Long-running agent with state persistence:

```rust
use agent::session::{SessionManager, SessionConfig};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Setup session manager
    let session_config = SessionConfig {
        base_dir: PathBuf::from(".bos/sessions"),
        default_ttl_secs: Some(604800), // 7 days
        compression_enabled: true,
    };

    let mut session_manager = SessionManager::new(session_config);
    session_manager.start_cleanup(Duration::from_secs(3600)).await;

    // Create or restore agent
    let llm = brainos_common::create_llm_client();
    let config = AgentConfig {
        name: "persistent-assistant".to_string(),
        model: "gpt-4o".to_string(),
        base_url: std::env::var("OPENAI_API_BASE_URL")?,
        api_key: std::env::var("OPENAI_API_KEY")?,
        system_prompt: "You are a helpful assistant with memory.".to_string(),
        temperature: 0.7,
        max_tokens: Some(2000),
        timeout_secs: 60,
    };

    let mut agent = Agent::new(config, llm);

    // Restore previous session if exists
    match agent.restore_state(&session_manager).await {
        Ok(()) => println!("Restored previous conversation"),
        Err(_) => println!("Starting fresh conversation"),
    }

    // Interactive loop with auto-save
    let mut registry = ToolRegistry::new();
    // ... register tools ...

    loop {
        print!("> ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if input.trim() == "quit" {
            // Final save before exit
            agent.save_state(&session_manager).await?;
            break;
        }

        match agent.run_with_tools(input.trim(), &registry).await {
            Ok(AgentOutput::Text(text)) => {
                println!("Assistant: {}", text);
                // Auto-save after each turn
                agent.auto_save(&session_manager).await;
            }
            Ok(AgentOutput::Error(e)) => eprintln!("Error: {}", e),
            Err(e) => eprintln!("Failed: {}", e),
        }
    }

    println!("Session saved. Goodbye!");
    Ok(())
}
```

---

## 6. Best Practices

### 6.1 Error Handling

#### Use `thiserror` for Custom Errors

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrderError {
    #[error("Order {order_id} not found")]
    NotFound { order_id: String },

    #[error("Payment failed: {reason}")]
    PaymentFailed { reason: String },

    #[error("Inventory insufficient for item {item_id}")]
    InsufficientInventory { item_id: String },

    #[error("Agent error: {0}")]
    Agent(#[from] AgentError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// Usage
async fn process_order(order_id: &str) -> Result<Order, OrderError> {
    let order = fetch_order(order_id).await?;  // Auto-converts via From

    if order.items.is_empty() {
        return Err(OrderError::NotFound { order_id: order_id.to_string() });
    }

    Ok(order)
}
```

#### Distinguish Recoverable vs Unrecoverable

```rust
async fn handle_task(task: Task) -> Result<TaskResult, AgentError> {
    // Recoverable: Return Result, let caller decide
    match execute_task(&task).await {
        Ok(result) => Ok(result),
        Err(e) if e.is_retryable() => {
            // Log and return error for retry
            tracing::warn!("Task failed, retryable: {}", e);
            Err(e)
        }
        Err(e) => {
            // Non-retryable error
            tracing::error!("Task failed permanently: {}", e);
            Err(e)
        }
    }
}

// Unrecoverable: Only for critical state corruption
fn validate_internal_state(&self) {
    if self.message_log.len() > MAX_MESSAGES {
        panic!("Internal state corrupted: message log overflow");
    }
}
```

### 6.2 Async Patterns

#### Cancellation Safety

```rust
use tokio_util::sync::CancellationToken;

async fn run_with_cancellation(
    agent: &mut Agent,
    task: &str,
    cancel_token: CancellationToken,
) -> Result<AgentOutput, AgentError> {
    tokio::select! {
        result = agent.run_with_tools(task, &ToolRegistry::new()) => {
            result
        }
        _ = cancel_token.cancelled() => {
            tracing::info!("Task cancelled");
            Err(AgentError::Session("Task cancelled".to_string()))
        }
    }
}
```

#### Proper Arc Usage

```rust
// Good: Share large state via Arc
pub struct SharedContext {
    config: Arc<AgentConfig>,
    tools: Arc<ToolRegistry>,
    session: Arc<bus::Session>,
}

impl SharedContext {
    pub fn new(config: AgentConfig, tools: ToolRegistry, session: bus::Session) -> Self {
        Self {
            config: Arc::new(config),
            tools: Arc::new(tools),
            session: Arc::new(session),
        }
    }
}

// Bad: Clone large objects
async fn bad_pattern(agent: Agent) {
    let agent_clone = agent.clone(); // Expensive!
}
```

#### Stream Handling with Backpressure

```rust
use futures::StreamExt;

async fn process_stream(agent: &mut Agent, task: &str) {
    let mut stream = agent.stream_run(task);

    while let Some(token_result) = stream.next().await {
        match token_result {
            Ok(StreamToken::Text(text)) => {
                print!("{}", text);
                std::io::stdout().flush().unwrap();
            }
            Ok(StreamToken::Done) => break,
            Err(e) => {
                eprintln!("\nError: {}", e);
                break;
            }
            _ => {}
        }
    }
    println!();
}
```

### 6.3 Performance

#### rkyv for Hot Paths

```rust
use rkyv::{Archive, Serialize, Deserialize};

// Good: Use rkyv for high-frequency messages
#[derive(Archive, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_name: String,
    pub args: Vec<u8>,  // JSON as bytes
}

// Serialize once, zero-copy access
let request = ToolRequest {
    tool_name: "add".to_string(),
    args: serde_json::to_vec(&args)?,
};
let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&request)?;
let archived: &ArchivedToolRequest = rkyv::from_bytes(&bytes)?;
```

#### Avoid Unnecessary Clones

```rust
// Bad: Clone in hot loop
for _ in 0..1000 {
    let config = agent.config().clone(); // Expensive!
    process(config).await;
}

// Good: Clone once, share
let config = Arc::new(agent.config().clone());
for _ in 0..1000 {
    process(config.clone()).await; // Arc::clone is cheap
}
```

#### Pre-allocate Collections

```rust
// Bad: Grow dynamically
let mut results = Vec::new();
for item in items {
    results.push(process(item));
}

// Good: Pre-allocate
let mut results = Vec::with_capacity(items.len());
for item in items {
    results.push(process(item));
}

// Better: Use collect when possible
let results: Vec<_> = items.into_iter().map(process).collect();
```

### 6.4 Testing

#### Unit Tests for Tools

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_add_tool() {
        let tool = CalculatorTool { operation: MathOperation::Add };

        let args = serde_json::json!({"a": 2, "b": 3});
        let result = tool.execute(&args).await.unwrap();

        assert_eq!(result["result"], 5.0);
    }

    #[tokio::test]
    async fn test_divide_by_zero() {
        let tool = CalculatorTool { operation: MathOperation::Divide };

        let args = serde_json::json!({"a": 10, "b": 0});
        let result = tool.execute(&args).await;

        assert!(result.is_err());
    }
}
```

#### Integration Tests for A2A

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_discovery() {
        let session = setup_test_bus().await;

        let identity1 = AgentIdentity::new("agent1".to_string(), "Agent 1".to_string(), "1.0.0".to_string());
        let discovery1 = A2ADiscovery::new(session.clone());

        let card1 = AgentCard::new(identity1.clone(), "Agent 1".to_string(), "Test agent".to_string());
        discovery1.announce(&card1).await.unwrap();

        let identity2 = AgentIdentity::new("agent2".to_string(), "Agent 2".to_string(), "1.0.0".to_string());
        let discovery2 = A2ADiscovery::new(session.clone());

        let agents = discovery2.discover(None).await.unwrap();
        assert!(agents.iter().any(|a| a.agent_id.id == "agent1"));
    }
}
```

#### Mock LLM for Testing

```rust
use agent::llm::{LlmClient, LlmRequest, LlmResponse, StreamToken};

pub struct MockLlmClient {
    responses: Vec<LlmResponse>,
}

impl MockLlmClient {
    pub fn new(responses: Vec<LlmResponse>) -> Self {
        Self { responses }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // Return pre-configured responses
        Ok(self.responses.first().cloned().unwrap_or(LlmResponse::Done))
    }

    fn stream_complete(&self, _request: LlmRequest) -> Pin<Box<dyn Stream<Item = Result<StreamToken, LlmError>> + Send>> {
        // Return mock stream
        futures::stream::iter(vec![
            Ok(StreamToken::Text("Hello".to_string())),
            Ok(StreamToken::Done),
        ]).boxed()
    }
}

#[tokio::test]
async fn test_agent_with_mock() {
    let mock = MockLlmClient::new(vec![
        LlmResponse::Text("Test response".to_string()),
        LlmResponse::Done,
    ]);

    let config = test_config();
    let mut agent = Agent::new(config, Arc::new(mock));

    let output = agent.run("Test input").await.unwrap();
    assert!(output.contains("Test response"));
}
```

### 6.5 Configuration

#### Environment-Based Config

```rust
use std::env;

pub struct AppConfig {
    pub agent_name: String,
    pub llm_model: String,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub zenoh_url: String,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, AgentError> {
        Ok(Self {
            agent_name: env::var("AGENT_NAME")
                .unwrap_or_else(|_| "default-agent".to_string()),
            llm_model: env::var("OPENAI_MODEL")
                .unwrap_or_else(|_| "gpt-4o".to_string()),
            llm_base_url: env::var("OPENAI_API_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            llm_api_key: env::var("OPENAI_API_KEY")
                .map_err(|_| AgentError::Config("OPENAI_API_KEY required".to_string()))?,
            zenoh_url: env::var("ZENOH_URL")
                .unwrap_or_else(|_| "tcp/127.0.0.1:7447".to_string()),
        })
    }
}
```

#### Feature Flags

```toml
# Cargo.toml
[features]
default = ["llm", "a2a"]
llm = ["dep:tokio", "dep:reqwest"]
a2a = ["dep:zenoh"]
streaming = []
session = ["dep:serde_json"]
```

```rust
// Conditional compilation
#[cfg(feature = "streaming")]
pub fn stream_run(&mut self, task: &str) -> impl Stream<Item = StreamToken> {
    // ...
}

#[cfg(not(feature = "streaming"))]
pub fn run(&mut self, task: &str) -> Result<String, AgentError> {
    // ...
}
```

#### Graceful Degradation

```rust
async fn create_llm_client() -> Arc<dyn LlmClient> {
    // Try real LLM first
    if let (Ok(key), Ok(url)) = (env::var("OPENAI_API_KEY"), env::var("OPENAI_API_BASE_URL")) {
        tracing::info!("Using real LLM client");
        return Arc::new(OpenAiClient::new(url, key));
    }

    // Fall back to mock
    tracing::warn!("No LLM API configured, using mock client");
    Arc::new(MockLlmClient::new(vec![
        LlmResponse::Text("Mock response".to_string()),
        LlmResponse::Done,
    ]))
}
```

---

## 7. Quick Reference

### 7.1 Common API Patterns

#### Agent Construction

```rust
// Minimal
let agent = Agent::new(config, llm);

// With builder
let agent = AgentBuilder::from_file(Path::new("agent.toml"))?
    .with_tool(Arc::new(MyTool))
    .build(None)
    .await?;
```

#### Tool Registration

```rust
// Local tool
registry.register(Arc::new(MyTool))?;

// Namespaced tool
registry.register_with_namespace(Arc::new(MyTool), "my-skill")?;

// From skill
registry.register_from_skill("calculator", vec![Arc::new(AddTool), Arc::new(MulTool)])?;
```

#### A2A Messaging

```rust
// Announce
discovery.announce(&card).await?;

// Discover
let agents = discovery.discover(None).await?;

// Delegate task
let result = client.delegate_task(&recipient, task).await?;
```

#### Workflow Definition

```rust
let workflow = WorkflowBuilder::new("my-workflow")
    .description("My workflow")
    .add_step(StepBuilder::new("step1").sequential().build())
    .build();
```

### 7.2 Troubleshooting Guide

#### Zenoh Connection Issues

**Symptom:** Agent fails to start with connection error

**Solutions:**
```fish
# Check Zenoh is running
zenohd --version

# Start Zenoh explicitly
zenohd -l tcp/0.0.0.0:7447

# Check port availability
lsof -i :7447
```

#### LLM API Errors

**Symptom:** "API key invalid" or timeout errors

**Solutions:**
```fish
# Verify API key
echo $OPENAI_API_KEY

# Test connectivity
curl https://api.openai.com/v1/models \
  -H "Authorization: Bearer $OPENAI_API_KEY"

# Check rate limits
# Reduce max_tokens or add retry logic
```

#### Discovery Failures

**Symptom:** Agents can't discover each other

**Solutions:**
1. Ensure both agents announced before discovery
2. Check Zenoh router is running
3. Verify agent IDs are unique
4. Add delay between announce and discover

```rust
discovery.announce(&card).await?;
tokio::time::sleep(Duration::from_secs(1)).await;
let agents = discovery.discover(None).await?;
```

#### RPC Timeouts

**Symptom:** Tool calls timeout

**Solutions:**
1. Increase timeout in RpcClient
2. Check server is running and registered
3. Verify topic names match

```rust
let mut client = RpcClient::new(service_name, method);
client.init(session).await?;
// client.timeout = Duration::from_secs(120); // If needed
```

### 7.3 Migration Guide

#### From Single Agent to Multi-Agent

1. **Add A2A discovery:**
```rust
let discovery = A2ADiscovery::new(session.clone());
discovery.announce(&card).await?;
```

2. **Register tools as RPC services:**
```rust
RpcServiceBuilder::new()
    .service_name("my-tool")
    .topic_prefix(&tool_base)
    .build()?
    .init(&session, MyHandler).await?;
```

3. **Discover and use remote tools:**
```rust
let agents = discovery.discover(None).await?;
// Register remote tools in local registry
```

#### Adding Persistence

1. **Create session manager:**
```rust
let manager = SessionManager::new(SessionConfig::default());
```

2. **Save after each turn:**
```rust
agent.auto_save(&manager).await;
```

3. **Restore on startup:**
```rust
agent.restore_state(&manager).await?;
```

#### Integrating with Existing Systems

1. **Custom LLM client:**
```rust
impl LlmClient for MyCustomClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        // Call your LLM API
    }
}
```

2. **Custom tool wrapper:**
```rust
struct ExternalApiTool {
    client: reqwest::Client,
}

#[async_trait]
impl Tool for ExternalApiTool {
    // Implement trait methods
}
```

---

## 8. Appendix

### 8.1 API Reference Summary

| Module | Key Types | Purpose |
|--------|-----------|---------|
| `agent` | `Agent`, `AgentConfig`, `MessageLog` | Core agent struct |
| `tools` | `Tool`, `ToolRegistry`, `ToolDescription` | Tool system |
| `a2a` | `A2AClient`, `AgentIdentity`, `Task` | Agent communication |
| `scheduler` | `Workflow`, `Step`, `Scheduler` | Workflow orchestration |
| `session` | `SessionManager`, `AgentState` | State persistence |
| `streaming` | `TokenStream`, `BackpressureController` | Token streaming |
| `llm` | `LlmClient`, `OpenAiClient` | LLM integration |

### 8.2 Example Projects

| Example | Location | Demonstrates |
|---------|----------|--------------|
| LLM Agent Demo | `examples/llm-agent-demo/` | Full agent lifecycle with real LLM |
| Basic Communication | `examples/basic-communication/` | A2A protocol basics |

### 8.3 Related Documentation

- Project README: `/AGENTS.md`
- Configuration: `crates/config/README.md`
- Bus Layer: `crates/bus/README.md`
- Benchmarks: `benches/README.md`

---

**End of Guide**
