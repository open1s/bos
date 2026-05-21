# BrainOS Rust API Reference

This document provides the complete API reference for the BrainOS Rust crate (`agent`).

## Main Entry Point

### Agent

The core abstraction for AI agents with LLM integration, tool registries, skill management, hooks, and plugins.

#### Constructor

```rust
use agent::{Agent, AgentConfig, LlmProvider};
use std::sync::Arc;

let config = AgentConfig::default()
    .name("assistant")
    .model("gpt-4")
    .base_url("https://api.openai.com/v1")
    .api_key("sk-...")
    .system_prompt("You are helpful.")
    .temperature(0.7)
    .max_tokens(Some(4096))
    .timeout_secs(120);

let mut llm = LlmProvider::new();
llm.with_nvidia("nvidia/meta/llama-3.1-8b-instruct", base_url, api_key);
// or
llm.with_openrouter("openrouter/anthropic/claude-3", base_url, api_key);

let agent = Agent::new(config, Arc::new(llm));
```

### AgentBuilder (TOML-based)

Create agents from TOML configuration using `AgentBuilder` (aliased as `TomlAgentBuilder`):

```rust
use agent::AgentBuilder;

let toml = r#"
    name = "assistant"
    model = "gpt-4"
    base_url = "https://api.openai.com/v1"
    api_key = "sk-..."
"#;

let builder = AgentBuilder::from_toml(toml)?;
// Or from file:
let builder = AgentBuilder::from_file(Path::new("agent.toml"))?;

// Add tools
let builder = builder.with_tool(Arc::new(MyTool));

// Build the agent (None for no Zenoh session)
let agent = builder.build(None).await?;
```

---

## AgentConfig

Agent configuration structure with builder pattern.

#### Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | `String` | `"agent"` | Agent name |
| `model` | `String` | `"gpt-4"` | Model name |
| `base_url` | `String` | `"https://api.openai.com/v1"` | Base API URL |
| `api_key` | `String` | `""` | API key for LLM |
| `system_prompt` | `String` | `"You are a helpful assistant."` | System prompt |
| `temperature` | `f32` | `0.7` | Temperature (0.0 to 2.0) |
| `max_tokens` | `Option<u32>` | `None` | Max tokens for completion |
| `timeout_secs` | `u64` | `60` | Request timeout in seconds |
| `max_steps` | `usize` | `10` | Maximum ReAct steps |
| `circuit_breaker` | `Option<CircuitBreakerConfig>` | `None` | Circuit breaker config |
| `rate_limit` | `Option<RateLimiterConfig>` | `None` | Rate limiter config |

#### Builder Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `name(name)` | Set agent name | `Self` |
| `model(model)` | Set model name | `Self` |
| `base_url(url)` | Set base URL | `Self` |
| `api_key(key)` | Set API key | `Self` |
| `system_prompt(prompt)` | Set system prompt | `Self` |
| `temperature(temp)` | Set temperature | `Self` |
| `max_tokens(tokens)` | Set max tokens | `Self` |
| `timeout_secs(secs)` | Set timeout | `Self` |
| `max_steps(steps)` | Set max steps | `Self` |
| `circuit_breaker(config)` | Set circuit breaker | `Self` |
| `rate_limit(config)` | Set rate limiter | `Self` |

---

## LlmProvider

LLM provider with vendor routing support.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `new()` | Create new provider | `LlmProvider` |
| `register_vendor(name, vendor)` | Register an LLM vendor | `&mut Self` |
| `with_nvidia(model, base_url, api_key)` | Register Nvidia vendor | `&mut Self` |
| `with_openrouter(model, base_url, api_key)` | Register OpenRouter vendor | `&mut Self` |
| `as_dyn(self: Arc<Self>)` | Convert to dyn LlmClient | `Box<dyn LlmClient>` |

---

## Agent Methods

#### Accessors

| Method | Description | Returns |
|--------|-------------|---------|
| `config()` | Get agent configuration | `&AgentConfig` |
| `registry()` | Get tool registry | `Option<&Arc<ToolRegistry>>` |
| `hooks()` | Get hooks registry | `&HookRegistry` |
| `plugins()` | Get plugins registry | `&PluginRegistry` |
| `metrics()` | Get call metrics | `CallMetrics` |
| `last_token_usage()` | Get last stream token usage | `Option<(u64, u64)>` |
| `last_stream_tool_calls()` | Get last stream tool call count | `u64` |
| `tool_invocation_count()` | Get tool invocation count | `u64` |

#### Session Management

| Method | Description | Returns |
|--------|-------------|---------|
| `add_message(message)` | Add message to conversation | `()` |
| `session()` | Get session guard | `MutexGuard<AgentSession>` |
| `session_mut()` | Get mutable session guard | `&mut MutexGuard<AgentSession>` |
| `session_state()` | Get full session state | `AgentState` |
| `save_session(path)` | Save session to file | `Result<(), AgentError>` |
| `restore_session(path)` | Restore session from file | `Result<(), AgentError>` |

#### Execution

| Method | Description | Returns |
|--------|-------------|---------|
| `react(task)` | Run with ReAct reasoning (tools + skills) | `Result<String, AgentError>` |
| `run_simple(task)` | Run simple task (delegates to react) | `Result<String, AgentError>` |
| `stream(task)` | Stream response tokens | `Pin<Box<dyn Stream<Item = Result<StreamToken, AgentError>>>>` |
| `stop()` | Stop current execution | `()` |

#### Tool Registration

| Method | Description | Returns |
|--------|-------------|---------|
| `add_tool(tool)` | Register a tool (logs errors) | `()` |
| `try_add_tool(tool)` | Register a tool (returns error) | `Result<(), ToolError>` |
| `add_remote_agent_tool(name, endpoint, session)` | Add tool calling remote agent | `Result<(), ToolError>` |
| `clear_runtime_extensions()` | Clear tools, hooks, plugins | `()` |

#### Skills

| Method | Description | Returns |
|--------|-------------|---------|
| `register_skills_from_dir(dir)` | Load skills from directory | `Result<(), SkillError>` |
| `get_skills_schemas()` | Get skill schemas for LLM | `Vec<Value>` |
| `get_skills_content()` | Get skill content pairs | `Vec<(&str, &str)>` |

#### MCP

| Method | Description | Returns |
|--------|-------------|---------|
| `register_mcp_tools(client)` | Register MCP tools (namespace: "mcp") | `Result<(), McpError>` |
| `register_mcp_tools_with_namespace(client, ns)` | Register MCP tools with namespace | `Result<(), McpError>` |

#### Bus Integration

| Method | Description | Returns |
|--------|-------------|---------|
| `rpc_client(endpoint, session)` | Create RPC client for another agent | `AgentRpcClient` |
| `as_callable_server(endpoint, session)` | Expose agent as callable server | `AgentCallableServer` |

#### Plugins & Hooks

| Method | Description | Returns |
|--------|-------------|---------|
| `add_plugin(plugin)` | Register a plugin | `()` |
| `add_hook(event, hook)` | Register a hook | `()` |

#### Metrics

| Method | Description | Returns |
|--------|-------------|---------|
| `record_stream_call(...)` | Record stream call metrics | `()` |
| `record_llm_error()` | Record LLM error | `()` |
| `record_tool_calls(count, time)` | Record tool calls | `()` |
| `reset_metrics()` | Reset all metrics | `()` |

---

## Tool Trait

```rust
use agent::Tool;
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str {
        "my_tool"
    }

    fn description(&self) -> String {
        "Does something useful".to_string()
    }

    fn json_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "input": { "type": "string" }
            },
            "required": ["input"]
        })
    }

    async fn execute(&self, args: &Value) -> Result<Value, ToolError> {
        let input = args["input"].as_str().unwrap();
        Ok(serde_json::json!({ "result": format!("Processed: {}", input) }))
    }
}
```

---

## FunctionTool

Convenient tool creation from closures.

```rust
use agent::tools::FunctionTool;

let tool = FunctionTool::new(
    "calculator",
    "Evaluate a math expression",
    serde_json::json!({
        "type": "object",
        "properties": {
            "expression": { "type": "string" }
        },
        "required": ["expression"]
    }),
    |args| {
        let expr = args["expression"].as_str().unwrap();
        Ok(serde_json::json!({ "result": "42" }))
    },
);

// Skill tool
let skill_tool = FunctionTool::skill(
    "my-skill",
    "Get instructions for my-skill",
    serde_json::json!({"type": "object", "properties": {}}),
    |_args| Ok(serde_json::json!({ "instructions": "..." })),
);
```

---

## ToolRegistry

Registry for managing multiple tools.

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `new()` | Create empty registry | `ToolRegistry` |
| `register(tool)` | Register a tool | `Result<(), ToolError>` |
| `register_async(tool)` | Register an async tool | `Result<(), ToolError>` |
| `get(name)` | Get tool by name | `Option<Arc<dyn Tool>>` |
| `get_async(name)` | Get async tool by name | `Option<Arc<dyn AsyncTool>>` |
| `iter()` | Iterate over sync tools | `impl Iterator` |
| `async_tool_names()` | Get async tool names | `Vec<String>` |

---

## Hooks

#### HookEvent

| Event | Description |
|-------|-------------|
| `BeforeToolCall` | Before tool execution |
| `AfterToolCall` | After tool execution |
| `BeforeLlmCall` | Before LLM API call |
| `AfterLlmCall` | After LLM API call |
| `OnMessage` | For each message |
| `OnComplete` | When agent completes |
| `OnError` | When error occurs |

#### HookDecision

| Decision | Description |
|----------|-------------|
| `Continue` | Proceed normally |
| `Abort` | Abort operation |
| `Error(msg)` | Return error with message |

#### HookContext

| Method | Description |
|--------|-------------|
| `new(agent_id)` | Create new context |
| `set(key, value)` | Set context data |
| `get(key)` | Get context data |

#### AgentHook Trait

```rust
#[async_trait]
impl AgentHook for MyHook {
    async fn on_event(&self, event: HookEvent, ctx: &HookContext) -> HookDecision {
        println!("[{}] agent={}", event, ctx.agent_id);
        HookDecision::Continue
    }
}
```

#### HookRegistry

| Method | Description |
|--------|-------------|
| `new()` | Create empty registry |
| `register(event, hook)` | Register a hook |
| `register_blocking(event, hook)` | Sync register |
| `trigger(event, ctx)` | Trigger hooks (async) |
| `trigger_all(event, ctx)` | Trigger all hooks (async) |
| `trigger_all_blocking(event, ctx)` | Trigger all hooks (sync) |
| `get_hooks(event)` | Get hooks for event |
| `clear_all()` | Clear all hooks |
| `clear_all_blocking()` | Sync clear |

---

## Plugins

#### AgentPlugin Trait

```rust
#[async_trait]
impl AgentPlugin for MyPlugin {
    async fn on_llm_request(&self, req: LlmRequestWrapper) -> Option<LlmRequestWrapper> {
        // Modify request before LLM call
        Some(req)
    }

    async fn on_llm_response(&self, resp: LlmResponseWrapper) -> Option<LlmResponseWrapper> {
        // Modify response after LLM call
        Some(resp)
    }

    async fn on_tool_call(&self, call: ToolCallWrapper) -> Option<ToolCallWrapper> {
        // Modify tool call before execution
        Some(call)
    }

    async fn on_tool_result(&self, result: ToolResultWrapper) -> Option<ToolResultWrapper> {
        // Modify tool result after execution
        Some(result)
    }

    async fn on_stream_token(&self, token: StreamTokenWrapper) -> Option<StreamTokenWrapper> {
        // Modify stream token
        Some(token)
    }
}
```

#### PluginRegistry

| Method | Description |
|--------|-------------|
| `new()` | Create empty registry |
| `register(plugin)` | Register a plugin |
| `register_blocking(plugin)` | Sync register |
| `has_plugins()` | Check if any plugins |
| `on_llm_request(req)` | Trigger on_llm_request |
| `on_llm_response(resp)` | Trigger on_llm_response |
| `on_tool_call(call)` | Trigger on_tool_call |
| `on_tool_result(result)` | Trigger on_tool_result |
| `on_stream_token(token)` | Trigger on_stream_token |
| `clear()` | Clear all plugins |
| `clear_blocking()` | Sync clear |

---

## Resilience

#### CircuitBreakerConfig

| Field | Default | Description |
|-------|---------|-------------|
| `max_failures` | 5 | Failures before opening circuit |
| `cooldown` | 30s | Seconds before half-open state |

#### RateLimiterConfig

| Field | Default | Description |
|-------|---------|-------------|
| `capacity` | 40 | Max requests per window |
| `window` | 60s | Window duration |
| `max_retries` | 3 | Retry attempts on 429 |
| `retry_backoff` | 1s | Initial backoff |
| `auto_wait` | true | Auto-wait when rate limited |

#### Example

```rust
use agent::{AgentConfig, CircuitBreakerConfig, RateLimiterConfig};
use std::time::Duration;

let config = AgentConfig::default()
    .name("resilient-agent")
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

---

## MCP Client

```rust
use agent::McpClient;

// Spawn process-based MCP server
let client = McpClient::spawn("npx", &["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]).await?;
client.initialize().await?;

// List and call tools
let tools = client.list_tools().await?;
let result = client.call_tool("read_file", r#"{"path": "/tmp/test.txt"}"#).await?;
```

#### Methods

| Method | Description | Returns |
|--------|-------------|---------|
| `spawn(cmd, args)` | Spawn MCP server | `Result<McpClient, McpError>` |
| `connect_http(url)` | Connect via HTTP | `Result<McpClient, McpError>` |
| `initialize()` | Initialize connection | `Result<(), McpError>` |
| `list_tools()` | List available tools | `Result<Vec<Tool>, McpError>` |
| `call_tool(name, args)` | Call a tool | `Result<String, McpError>` |
| `list_prompts()` | List prompts | `Result<Vec<Prompt>, McpError>` |
| `list_resources()` | List resources | `Result<Vec<Resource>, McpError>` |
| `read_resource(uri)` | Read resource | `Result<String, McpError>` |
| `get_capabilities()` | Get server capabilities | `Option<ServerCapabilities>` |

---

## Skills

#### SkillLoader

```rust
use agent::skills::SkillLoader;

let mut loader = SkillLoader::new(PathBuf::from("./skills"));
loader.discover()?;
for meta in loader.list() {
    let skill = loader.load(&meta.name).unwrap();
    println!("{}: {}", meta.name, meta.description);
}
```

#### SkillMetadata

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Skill name |
| `description` | `String` | Skill description |
| `category` | `String` | Skill category |
| `tags` | `Vec<String>` | Skill tags |

#### SkillContent

| Field | Type | Description |
|-------|------|-------------|
| `metadata` | `SkillMetadata` | Skill metadata |
| `instructions` | `String` | Skill instructions |

---

## Session

#### AgentSession

| Method | Description |
|--------|-------------|
| `new()` | Create new session |
| `push(message)` | Add message |
| `messages()` | Get all messages |
| `take_messages()` | Take and clear messages |
| `restore_messages(msgs)` | Restore messages |
| `len()` | Message count |
| `session_context()` | Get session context |
| `save(path)` | Save to file |
| `restore(path)` | Restore from file |

#### LlmMessage

```rust
use agent::LlmMessage;

let msg = LlmMessage::system("System prompt");
let msg = LlmMessage::user("User message");
let msg = LlmMessage::assistant("Assistant response");
let msg = LlmMessage::assistant_tool_call("tool_name", "id", args);
let msg = LlmMessage::tool_result("id", "result");
```

---

## StreamToken

Tokens emitted by the streaming API.

| Variant | Description |
|---------|-------------|
| `Text(String)` | Text content |
| `ToolCall { name, id, args }` | Tool call start |
| `ToolResult { id, result }` | Tool result |
| `Done` | Stream complete |
| `Error(String)` | Error token |

---

## AgentError

| Variant | Description |
|---------|-------------|
| `Config(String)` | Configuration error |
| `Session(String)` | Session error |
| `Tool(ToolError)` | Tool error |
| `Llm(LlmError)` | LLM error |
| `Mcp(McpError)` | MCP error |
| `Skill(SkillError)` | Skill error |
| `Security(SecurityError)` | Security error |

---

## Bus Integration

#### AgentRpcClient

Typed RPC client for calling another agent.

```rust
let client = agent.rpc_client("other-agent", session);
let result = client.call("task description").await?;
```

#### AgentCallableServer

Expose agent as a callable server on the bus.

```rust
let server = agent.as_callable_server("my-agent", session);
server.start().await?;
```

#### AgentCallerTool

Tool that calls another agent via bus Caller.

```rust
agent.add_remote_agent_tool("remote-assistant", "assistant-endpoint", session)?;
```

---

## Example Usage

```rust
use agent::{Agent, AgentConfig, LlmProvider, Tool};
use agent::tools::FunctionTool;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure agent
    let config = AgentConfig::default()
        .name("assistant")
        .model("nvidia/meta/llama-3.1-8b-instruct")
        .api_key("your-api-key");

    // Create LLM provider
    let mut llm = LlmProvider::new();
    llm.with_nvidia(
        "nvidia/meta/llama-3.1-8b-instruct",
        "https://integrate.api.nvidia.com/v1",
        "your-api-key",
    );

    // Create agent
    let mut agent = Agent::new(config, Arc::new(llm));

    // Add a tool
    let calc = FunctionTool::new(
        "calculator",
        "Evaluate a math expression",
        serde_json::json!({
            "type": "object",
            "properties": {
                "expression": { "type": "string" }
            },
            "required": ["expression"]
        }),
        |args| {
            let expr = args["expression"].as_str().unwrap_or("0");
            Ok(serde_json::json!({ "result": expr }))
        },
    );
    agent.add_tool(Arc::new(calc));

    // Run simple task
    let result = agent.run_simple("What is Python?").await?;
    println!("Simple: {}", result);

    // Run with ReAct (tools + skills)
    let result = agent.react("What is 2 + 2?").await?;
    println!("ReAct: {}", result);

    // Stream response
    let mut stream = agent.stream("Count to 3").await?;
    use futures::StreamExt;
    while let Some(token) = stream.next().await {
        match token {
            Ok(agent::StreamToken::Text(text)) => print!("{}", text),
            Ok(agent::StreamToken::Done) => break,
            Err(e) => eprintln!("Error: {}", e),
            _ => {}
        }
    }
    println!();

    Ok(())
}
```

---

## Best Practices

- Use `AgentConfig` builder pattern for configuration
- Use `LlmProvider::with_nvidia()` or `with_openrouter()` for vendor setup
- Prefer `try_add_tool()` for explicit error handling
- Use `AgentBuilder::from_toml()` for TOML-based configuration
- Cache engine/context automatically between calls for performance
- Call `stop()` to cancel long-running operations
- Use `clear_runtime_extensions()` to release callback resources in bindings
- Configure circuit breaker and rate limiter for production resilience
