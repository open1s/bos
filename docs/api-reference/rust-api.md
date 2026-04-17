# BrainOS Rust API Reference

This document provides the complete API reference for the BrainOS Rust crate (`agent`).

## Main Entry Point

### Agent

The main abstraction for AI agents with LLM integration, tool registries, and skill management.

#### Builder Pattern

Create an agent using the fluent builder pattern:

```rust
use agent::{Agent, AgentConfig};

let agent = Agent::builder()
    .name("assistant")
    .model("gpt-4")
    .system_prompt("You are a helpful assistant.")
    .temperature(0.7)
    .timeout_secs(120)
    .build()?;
```

#### Direct Construction

```rust
use agent::{Agent, AgentConfig};
use react::llm::vendor::OpenAiClient;

let config = AgentConfig::default()
    .name("assistant")
    .model("gpt-4")
    .system_prompt("You are a helpful assistant.")
    .temperature(0.7)
    .timeout_secs(120);

let llm = Arc::new(OpenAiClient::new(
    config.base_url.clone(),
    config.model.clone(),
    config.api_key.clone(),
));

let agent = Agent::new(config, llm);
```

#### Configuration Methods

| Method | Description |
|--------|-------------|
| `name(name: impl Into<String>)` | Set agent name |
| `model(model: impl Into<String>)` | Set model name (e.g., "gpt-4", "claude-3") |
| `base_url(url: impl Into<String>)` | Set base API URL |
| `api_key(key: impl Into<String>)` | Set API key for LLM |
| `system_prompt(prompt: impl Into<String>)` | Set system prompt |
| `temperature(temp: f32)` | Set temperature (0.0 to 2.0) |
| `max_tokens(tokens: u32)` | Set max tokens for completion |
| `timeout_secs(secs: u64)` | Set timeout in seconds |
| `rate_limit(config: RateLimiterConfig)` | Set rate limiter configuration |
| `circuit_breaker(config: CircuitBreakerConfig)` | Set circuit breaker configuration |
| `context_compaction_threshold_tokens(tokens: usize)` | Set context compaction threshold |
| `context_compaction_trigger_ratio(ratio: f32)` | Set compaction trigger ratio |
| `context_compaction_keep_recent_messages(count: usize)` | Set recent messages to keep |
| `context_compaction_max_summary_chars(chars: usize)` | Set max chars for summary |
| `context_compaction_summary_max_tokens(tokens: u32)` | Set max tokens for LLM-generated summary |
| `tool(tool: Arc<dyn Tool>)` | Add a single tool |
| `tools<T>(tools: T)` | Add multiple tools from iterable |
| `skills_dir(dir: PathBuf)` | Set skills directory |
| `with_hooks(hooks: HookRegistry)` | Set hooks registry |
| `with_plugins(plugins: PluginRegistry)` | Set plugins registry |
| `plugin(plugin: Arc<dyn AgentPlugin>)` | Add a single plugin |
| `plugins(plugins: PluginRegistry)` | Set plugins registry |

#### Resilience Configuration

The Agent supports configuring circuit breaker and rate limiter for resilience:

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

#### Agent Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `config(&self) -> &AgentConfig` | Get agent configuration | `&AgentConfig` |
| `registry(&self) -> Option<&Arc<ToolRegistry>>` | Get tool registry | `Option<&Arc<ToolRegistry>>` |
| `hooks(&self) -> &HookRegistry` | Get hooks registry | `&HookRegistry` |
| `plugins(&self) -> &PluginRegistry` | Get plugins registry | `&PluginRegistry` |
| `add_message(&mut self, message: LlmMessage)` | Add message to conversation log | `()` |
| `get_messages(&self) -> Vec<LlmMessage>` | Get conversation messages | `Vec<LlmMessage>` |
| `save_message_log(&self, path: &str) -> Result<(), AgentError>` | Save message log to file | `Result<(), AgentError>` |
| `restore_message_log(&mut self, path: &str) -> Result<(), AgentError>` | Restore message log from file | `Result<(), AgentError>` |
| `add_remote_agent_tool(&mut self, tool_name: impl Into<String>, endpoint: impl Into<String>, session: Arc<bus::Session>) -> Result<(), ToolError>` | Add tool that calls remote agent | `Result<(), ToolError>` |
| `rpc_client(&self, endpoint: impl Into<String>, session: Arc<bus::Session>) -> AgentRpcClient` | Create RPC client for another agent | `AgentRpcClient` |
| `as_callable_server(&self, endpoint: impl Into<String>, session: Arc<bus::Session>) -> AgentCallableServer` | Expose agent as callable server | `AgentCallableServer` |

#### Running the Agent

| Method | Description | Returns |
|--------|-------------|---------|
| `react(&self, task: &str) -> Result<String, AgentError>` | Run agent with ReAct reasoning (supports tools/skills) | `Result<String, AgentError>` |
| `run_simple(&self, task: &str) -> Result<String, AgentError>` | Run agent with simple loop (single LLM call, supports tools/skills) | `Result<String, AgentError>` |
| `stream(&self, task: &str) -> Result<Pin<Box<dyn Stream<Item = Result<String, AgentError>>>>, AgentError>` | Stream response tokens | `Result<Pin<Box<dyn Stream<...>>>, AgentError>` |

#### Example Usage

```rust
use agent::{Agent, AgentConfig};
use agent::tools::Tool;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

struct Calculator;

#[async_trait]
impl Tool for Calculator {
    fn name(&self) -> &str {
        "calculator"
    }
    
    fn description(&self) -> agent::ToolDescription {
        agent::ToolDescription {
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
        
        // Simple eval for demo - in practice use a proper expression evaluator
        let result = match expr {
            "2+2" => 4,
            "10*5" => 50,
            _ => 0, // Simplified
        };
        
        Ok(serde_json::json!({ "result": result }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("gpt-4");
    
    let mut registry = agent::tools::ToolRegistry::new();
    registry.add(Arc::new(Calculator)).await?;
    
    let agent = Agent::builder()
        .config(config)
        .registry(registry)
        .build()?;
    
    // Simple Q&A (no tool use)
    let result = agent.run_simple("What is Python?").await?;
    println!("Simple result: {}", result);
    
    // Run with tool use (ReAct)
    let result = agent.react("What is 2 + 2? What is 10 * 5?").await?;
    println!("ReAct result: {}", result);
    
    // Streaming response
    let mut stream = agent.stream("Count from 1 to 3").await?;
    while let Some(token) = stream.next().await {
        print!("{}", token?);
    }
    println!();
    
    Ok(())
}
```

---

## AgentConfig

Agent configuration structure.

### Fields

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `name` | `String` | Agent name | `"agent"` |
| `model` | `String` | Model name | `"gpt-4"` |
| `base_url` | `String` | Base API URL | `"https://api.openai.com/v1"` |
| `api_key` | `String` | API key for LLM | `""` |
| `system_prompt` | `String` | System prompt | `"You are a helpful assistant."` |
| `temperature` | `f32` | Temperature (0.0 to 2.0 to 2.0 | `0.7` |
| `0.0` |
| `max_tokens` | `Option<u32>` | Max tokens for completion | `None` |
| `timeout_secs` | `u64` | Timeout in seconds | `60` |
| `max_steps` | `usize` | Maximum ReActp2: 10` | `'s` | Optionally