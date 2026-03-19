# Phase 1 Research: Core Agent Foundation

**Researched:** 2026-03-19
**Phase:** 01 - core-agent
**Confidence:** HIGH

## Validation Architecture

How to verify each deliverable:

1. **LlmClient (CORE-01):** Mock test with fake SSE stream → verify tokens yielded, correct parsing
2. **Agent struct (CORE-02):** Unit test: construct from config, give task, verify message history grows
3. **Agent loop (CORE-03):** Integration test: mock LLM returns Text → Done, verify loop exits cleanly
4. **Config loading (CORE-04):** TOML file → ConfigLoader → AgentBuilder → verify agent fields match
5. **Tool trait (TOOL-01):** Implement dummy tool, register, verify name/desc/schema/execute work
6. **Tool registry (TOOL-02):** Register duplicate → error, lookup by name → found, list → all tools
7. **Schema translator (TOOL-03):** Tool with known schema → translate to OpenAI format → verify structure
8. **Schema validation (TOOL-04):** Pass bad JSON to execute → SchemaMismatch error with field name
9. **Bus tool execution (TOOL-05):** Tool on remote service → agent calls via RpcClient → result returned
10. **SSE decoder (STRM-01):** Raw SSE bytes → decoder → tokens in order, [DONE] ends stream

---

## 1. LlmClient Trait & OpenAI Implementation

### Trait Design (CORE-01)

The trait is the foundation — everything depends on it. Must be built first per CONTEXT.md locked decisions.

```rust
// From CONTEXT.md locked decision:
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream_complete(&self, req: LlmRequest) -> Result<StreamToken, LlmError>;
}
```

**Key design decisions:**
- `Send + Sync` — enables use in multi-threaded contexts
- `complete()` returns `LlmResponse` — the agent-facing response
- `stream_complete()` returns `StreamToken` — per-token yields for streaming
- No provider-specific methods on trait — implement per provider in separate impl blocks

**LlmResponse variants** (from CONTEXT.md locked decision):
```rust
pub enum LlmResponse {
    Text(String),                        // Accumulated text chunk
    ToolCall { name: String, args: serde_json::Value },  // LLM wants to call a tool
    Done,                               // No more content, stop loop
}
```

**LlmRequest shape** (derived from requirements + OpenAI API):
```rust
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,  // From MessageLog::to_api_format()
    pub tools: Option<Vec<OpenAiTool>>, // Translated from ToolRegistry
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}
```

### OpenAI Implementation with reqwest

**HTTP client pattern** (from CONTEXT.md locked decision):
- `OpenAiClient::new()` owns `reqwest::Client` (connection pooling)
- Timeout per-request via `timeout()` on request builder
- Single client instance per agent

```rust
pub struct OpenAiClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
}

impl OpenAiClient {
    pub fn new(base_url: String, api_key: String, default_model: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("valid reqwest client"),
            base_url,
            api_key,
            default_model,
        }
    }

    async fn build_request(&self, req: &LlmRequest) -> Result<reqwest::Request, LlmError> {
        let body = serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "stream": false,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "tools": req.tools,
        });

        self.client
            .post(format!("{}/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .build()
            .map_err(LlmError::Http)
    }
}
```

### Streaming: SSE Decoder (STRM-01)

OpenAI SSE format:
```
data: {"choices":[{"delta":{"content":"Hello"}}]}\n\n
data: {"choices":[{"delta":{"content":" world"}}]}\n\n
data: [DONE]\n\n
```

**SSE parsing strategy** (from STACK.md — don't use eventsource-stream crate):
- Read bytes stream line by line
- Lines starting with `data: ` are event data
- Empty line (`\n` or `\r\n`) ends the event
- `data: [DONE]` signals end of stream

```rust
// tokio_stream wrapper for SSE
pub struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub fn decode_chunk(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let text = String::from_utf8_lossy(chunk);
        let mut events = Vec::new();
        
        for line in text.lines() {
            if line.is_empty() {
                // Empty line: flush buffer as one event
                if !self.buffer.is_empty() {
                    events.push(SseEvent::Data(self.buffer.clone()));
                    self.buffer.clear();
                }
            } else if line.starts_with("data: ") {
                self.buffer.push_str(&line[6..]);
            }
            // Ignore other SSE lines (event: type, id:, etc.)
        }
        events
    }
}

pub enum SseEvent {
    Data(String),  // JSON payload
    Done,
}
```

**Token yielding via tokio_stream:**
```rust
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

pub async fn stream_complete(
    &self,
    req: LlmRequest,
) -> Result<ReceiverStream<Result<String, LlmError>>, LlmError> {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let request = self.build_request_stream(&req).await?;
    
    let client = self.client.clone();
    let base_url = self.base_url.clone();
    let api_key = self.api_key.clone();
    
    tokio::spawn(async move {
        let response = client.execute(request).await;
        match response {
            Ok(resp) => {
                let mut stream = resp.bytes_stream();
                let mut decoder = SseDecoder::new();
                
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            let events = decoder.decode_chunk(&bytes);
                            for event in events {
                                match event {
                                    SseEvent::Data(data) => {
                                        if data == "[DONE]" {
                                            let _ = tx.send(Ok(String::new())).await;
                                            return;
                                        }
                                        // Parse delta
                                        if let Ok(parsed) = serde_json::from_str::<SseResponse>(&data) {
                                            if let Some(content) = parsed.choices[0].delta.content {
                                                let _ = tx.send(Ok(content)).await;
                                            }
                                        }
                                    }
                                    SseEvent::Done => return,
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(LlmError::Http(e))).await;
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Err(LmError::Http(e))).await;
            }
        }
    });
    
    Ok(ReceiverStream::new(rx))
}
```

**OpenAI SSE response parsing:**
```rust
#[derive(serde::Deserialize)]
struct SseResponse {
    choices: Vec<SseChoice>,
}

#[derive(serde::Deserialize)]
struct SseChoice {
    delta: SseDelta,
}

#[derive(serde::Deserialize)]
struct SseDelta {
    content: Option<String>,
    #[serde(rename = "tool_calls")]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(serde::Deserialize)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    #[serde(rename = "function")]
    function: Option<ToolCallFunction>,
}

#[derive(serde::Deserialize)]
struct ToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}
```

### Error Handling

**LlmError enum** (from CONTEXT.md locked decision):
```rust
#[derive(Error, Debug)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    
    #[error("Failed to parse LLM response: {0}")]
    Parse(String),
    
    #[error("Request timed out")]
    Timeout,
    
    #[error("API key missing")]
    ApiKeyMissing,
    
    #[error("Rate limited: {0}")]
    RateLimited(String),
}
```

### Key Insights for LlmClient
1. **Build trait first** — everything depends on it (Pitfall 2 from PITFALLS.md)
2. **SSE parsing from scratch** — simple enough, avoid another dep
3. **Timeout per-request** — use `timeout()` combinator, not client-level timeout
4. **Tool call streaming** — OpenAI streams tool_call deltas similarly to text; accumulate `arguments` string until complete
5. **Connection pooling** — single `reqwest::Client` shared across requests

---

## 2. Agent Loop & State Machine

### Agent Struct (CORE-02, CORE-03)

The agent wraps LlmClient, owns message history, and implements the reasoning loop.

```rust
pub struct Agent {
    llm: Arc<dyn LlmClient>,
    config: AgentConfig,
    message_log: MessageLog,
    tool_registry: ToolRegistry,
}

impl Agent {
    pub async fn run(&self, task: &str) -> Result<AgentOutput, AgentError> {
        self.message_log.add_user(task.to_string());
        
        loop {
            let response = self.llm.complete(self.make_request()).await
                .map_err(AgentError::Llm)?;
            
            match response {
                LlmResponse::Text(text) => {
                    self.message_log.add_assistant(text.clone());
                    return Ok(AgentOutput::Text(text));
                }
                LlmResponse::ToolCall { name, args } => {
                    // Execute tool
                    let result = self.execute_tool(&name, args).await?;
                    self.message_log.add_tool_result(name.clone(), result.clone());
                    // Continue loop with tool result in history
                }
                LlmResponse::Done => {
                    return Err(AgentError::Session("LLM returned Done without content".into()));
                }
            }
        }
    }
    
    pub async fn stream_run(&self, task: &str) -> Result<impl tokio_stream::Stream<Item = AgentOutput>, AgentError> {
        self.message_log.add_user(task.to_string());
        
        let output_rx = tokio::sync::mpsc::channel(100);
        let output_tx = Arc::new(output_rx.0);
        
        let llm = self.llm.clone();
        let message_log = Arc::new(tokio::sync::Mutex::new(MessageLog::new()));
        message_log.lock().await.add_user(task.to_string());
        
        // Stream tokens as they arrive
        let mut stream = self.llm.stream_complete(self.make_request()).await
            .map_err(AgentError::Llm)?;
        
        // (Simplified — full implementation needs tool call handling in stream)
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
```

**Key loop behaviors:**
1. Add user message to history before first call
2. Call LLM with current message history + tools
3. Match on LlmResponse variant
4. Text → return as output
5. ToolCall → execute tool, add result to history, loop again
6. Done → return error (shouldn't happen in normal flow)

### Message History (CORE-02)

```rust
// From CONTEXT.md locked decision:
pub enum Message {
    User(String),
    Assistant(String),
    ToolResult { name: String, content: String },
}

#[derive(Default)]
pub struct MessageLog {
    messages: Vec<Message>,
}

impl MessageLog {
    pub fn add_user(&mut self, content: String) {
        self.messages.push(Message::User(content));
    }
    
    pub fn add_assistant(&mut self, content: String) {
        self.messages.push(Message::Assistant(content));
    }
    
    pub fn add_tool_result(&mut self, name: String, content: String) {
        self.messages.push(Message::ToolResult { name, content });
    }
    
    pub fn to_api_format(&self) -> Vec<OpenAiMessage> {
        self.messages.iter().map(|m| match m {
            Message::User(s) => OpenAiMessage {
                role: "user".into(),
                content: s.clone(),
            },
            Message::Assistant(s) => OpenAiMessage {
                role: "assistant".into(),
                content: s.clone(),
            },
            Message::ToolResult { name, content } => OpenAiMessage {
                role: "tool".into(),
                content: content.clone(),
                #[rename = "tool_call_id"] // Wait, this needs to be handled differently
                // Actually OpenAI uses: role="tool", content=result, tool_call_id=id
                tool_call_id: Some(/* passed separately */),
            },
        }).collect()
    }
}
```

**OpenAI API message format:**
```rust
#[derive(serde::Serialize, Clone)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "tool_calls")]
    pub tool_calls: Option<Vec<TellCall>>,
}

#[derive(serde::Serialize, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "function")]
    pub function: ToolCallFunction,
}

#[derive(serde::Serialize, Clone)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,  // JSON string of arguments
}
```

### AgentOutput Enum

```rust
// From CONTEXT.md locked decision:
pub enum AgentOutput {
    Text(String),  // Agent's text response to caller
    ToolCall { name: String, args: serde_json::Value, result: String },
    Error(String),
}
```

### Key Insights for Agent Loop
1. **Loop until Text or Done** — tool calls continue the loop, text returns to caller
2. **Message history grows** — each turn adds user msg + LLM response + tool results
3. **to_api_format()** — converts internal Message enum to OpenAI API format
4. **Tool calls accumulate arguments** — streaming tool calls build up the args string

---

## 3. Tool System

### Tool Trait (TOOL-01)

```rust
// From CONTEXT.md locked decision:
// async fn execute(args: serde_json::Value) -> Result<serde_json::Value, ToolError>
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn json_schema(&self) -> serde_json::Value;
    
    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError>;
}
```

**Note:** CONTEXT.md says "async only — no sync variant. Tokio runtime is assumed everywhere." Use `async-trait` crate for async trait methods.

### ToolRegistry (TOOL-02)

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self;
    
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), ToolError> {
        if self.tools.contains_key(tool.name()) {
            return Err(ToolError::DuplicateRegistration(tool.name().into()));
        }
        self.tools.insert(tool.name().into(), tool);
        Ok(())
    }
    
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    
    pub fn list(&self) -> Vec<(String, String)> {
        self.tools.iter()
            .map(|(k, v)| (k.clone(), v.description().into()))
            .collect()
    }
    
    pub fn to_openai_format(&self) -> Vec<OpenAiTool> {
        self.tools.values()
            .map(|tool| tool.to_openai_tool())
            .collect()
    }
}
```

### Schema Translator (TOOL-03)

Convert internal tool schema to OpenAI format:

```rust
// Internal tool: Tool trait provides json_schema() → arbitrary JSON schema
// OpenAI format: {"type":"function","function":{"name":"...","description":"...","parameters":{...}}}

pub trait ToolSchemaTranslator {
    fn to_openai_format(tool: &dyn Tool) -> OpenAiTool {
        OpenAiTool {
            #[serde(rename = "type")]
            tool_type: "function".into(),
            function: OpenAiFunction {
                name: tool.name().into(),
                description: tool.description().into(),
                parameters: tool.json_schema(),
            },
        }
    }
}

#[derive(serde::Serialize, Clone)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAiFunction,
}

#[derive(serde::Serialize, Clone)]
pub struct OpenAiFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema object
}
```

### Tool Error Recovery (TOOL-04)

**ToolError enum** (from CONTEXT.md locked decision):
```rust
#[derive(Error, Debug)]
pub enum ToolError {
    #[error("Schema mismatch: {0}")]
    SchemaMismatch(String),
    
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    
    #[error("Timeout")]
    Timeout,
    
    #[error("Tool not found: {0}")]
    NotFound(String),
}
```

**Schema validation pattern:**
```rust
async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
    // Validate against json_schema before execution
    if let Err(e) = validate_args(&args, &self.json_schema()) {
        return Err(ToolError::SchemaMismatch(e.to_string()));
    }
    // Proceed with execution
}

fn validate_args(args: &Value, schema: &Value) -> Result<(), jsonschema::ValidationError> {
    // Use jsonschema crate to validate
    // Compile schema once, reuse
}
```

### Bus Tool Execution (TOOL-05)

Tools registered locally but callable over the bus via RpcClient. Transparent to agent.

```rust
// Agent wraps QueryableWrapper from bus crate (from CONTEXT.md)
// Tools that live on other services: RpcClient calls over bus

pub struct BusToolClient {
    rpc_client: RpcClient,
    service_name: String,
}

impl BusToolClient {
    pub async fn execute(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.rpc_client
            .call(&format!("tool/{}", name), args)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}
```

### Key Insights for Tool System
1. **Async trait** — use `async-trait` crate (already workspace dep)
2. **Schema validation first** — catch mismatches before execution, return clear errors
3. **JSON Schema library** — use `jsonschema` crate for validation
4. **ToolRegistry as source of truth** — single registry, used for both local tools and bus tools
5. **Schema translation centralized** — ToolRegistry handles OpenAI format conversion

---

## 4. Config-Driven Loading (CORE-04)

### TOML Agent Config

```toml
# From CONTEXT.md locked decision:
[agent]
name = "my-agent"
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
system_prompt = "You are a helpful assistant."
timeout_secs = 60

# Optional: pre-register tools
[[agent.tools]]
name = "calculator"
type = "local"  # or "bus"

[[agent.tools]]
name = "search"
type = "bus"
service = "search-service"
```

### Config Deserialization

```rust
#[derive(serde::Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub system_prompt: String,
    pub timeout_secs: u64,
    #[serde(default)]
    pub tools: Vec<ToolRef>,
}

#[derive(serde::Deserialize)]
pub struct ToolRef {
    pub name: String,
    #[serde(rename = "type")]
    pub tool_type: String,  // "local" or "bus"
    pub service: Option<String>,
}

impl Agent {
    pub async fn from_config(config_path: &str) -> Result<Self, AgentError> {
        let mut loader = ConfigLoader::new()
            .add_file(config_path);
        
        let config: AgentConfig = loader.load_typed()
            .await
            .map_err(|e| AgentError::Config(e.to_string()))?;
        
        Self::from_config_value(config)
    }
    
    pub fn from_config_value(config: AgentConfig) -> Result<Self, AgentError> {
        let llm = OpenAiClient::new(
            config.base_url.clone(),
            config.api_key.clone(),
            config.model.clone(),
        );
        
        let mut registry = ToolRegistry::new();
        
        // Load tools based on config
        for tool_ref in &config.tools {
            match tool_ref.tool_type.as_str() {
                "local" => {
                    // Load local tool implementation
                    let tool = load_local_tool(&tool_ref.name)?;
                    registry.register(tool)?;
                }
                "bus" => {
                    let service = tool_ref.service.as_ref()
                        .ok_or_else(|| AgentError::Config("bus tool needs service name".into()))?;
                    let bus_tool = BusToolClient::new(service.clone(), tool_ref.name.clone());
                    registry.register_bus_tool(bus_tool)?;
                }
                _ => return Err(AgentError::Config(format!("unknown tool type: {}", tool_ref.tool_type))),
            }
        }
        
        let message_log = MessageLog::new();
        
        Ok(Self {
            llm: Arc::new(llm),
            config,
            message_log,
            tool_registry: registry,
        })
    }
}
```

### Key Insights for Config
1. **TOML → ConfigLoader → AgentConfig** — standard pipeline from config crate
2. **ConfigLoader handles all formats** — TOML, YAML, JSON all work transparently
3. **AgentBuilder pattern** — fluent API for programmatic construction
4. **Tool refs in config** — tools listed in config, loaded at startup

---

## 5. Module Structure

Based on the deliverables and codebase patterns:

```
crates/agent/src/
├── lib.rs              # crate root, re-exports
├── error.rs            # AgentError, LlmError, ToolError (thiserror)
├── llm/
│   ├── mod.rs
│   ├── client.rs       # LlmClient trait
│   ├── openai.rs       # OpenAiClient implementation
│   ├── request.rs     # LlmRequest, OpenAiMessage types
│   └── response.rs     # LlmResponse, OpenAiTool types
├── agent/
│   ├── mod.rs
│   ├── agent.rs        # Agent struct + run() + stream_run()
│   ├── loop.rs         # Reasoning loop logic (optional split)
│   └── config.rs       # AgentConfig, from_config()
├── tools/
│   ├── mod.rs
│   ├── tool.rs         # Tool trait
│   ├── registry.rs     # ToolRegistry
│   ├── translator.rs    # Schema translation
│   ├── validator.rs     # JSON Schema validation
│   └── bus_client.rs   # BusToolClient for remote tools
├── streaming/
│   ├── mod.rs
│   ├── sse.rs          # SseDecoder
│   └── stream_token.rs # StreamToken types
└── history/
    ├── mod.rs
    ├── message.rs      # Message enum
    └── message_log.rs  # MessageLog struct
```

**Module count:** ~10 files, 2-3 layers (error at bottom, then building blocks, then high-level Agent)

---

## 6. Dependencies for Phase 1

Add to `crates/agent/Cargo.toml`:

```toml
[dependencies]
# Already in workspace
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }
tokio-stream = { workspace = true }

# New for agent crate
reqwest = { workspace = true }
jsonschema = "0.29"  # JSON Schema validation

# Internal
bus = { path = "../bus" }
config = { path = "../config" }
```

---

## 7. Build Order (from research)

Per FEATURES.md §Build order: **LLM Client → Agent Core → Tool Registry → then parallelize SSE, Config, Bus**

This maps to the roadmap's 3 plans:
1. **Plan 01:** Crate scaffold + LlmClient trait + Agent struct + reasoning loop (CORE-01, CORE-02, CORE-03)
2. **Plan 02:** Tool trait + registry + schema translator + error recovery (TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05)
3. **Plan 03:** SSE streaming + config-driven loading + integration tests (STRM-01, CORE-04)

---

## 8. Key Insights Summary

### Most Important for Planning

1. **LlmClient trait first** — everything else depends on it. Define trait + OpenAI impl together in Plan 01.

2. **SSE parsing from scratch** — lines starting with `data: `, accumulate until empty line, handle `[DONE]`. Don't use a crate.

3. **Tool trait + async-trait** — CONTEXT.md says async only. Use `#[async_trait]` macro.

4. **MessageLog is the conversation state** — it accumulates across turns, converts to OpenAI API format.

5. **Schema validation in Tool::execute()** — validate args against json_schema before execution, return ToolError::SchemaMismatch with field details.

6. **Builder pattern** — AgentBuilder, ConfigLoader → AgentConfig → Agent, matching bus crate's RpcClientBuilder pattern.

7. **Error hierarchy** — three error types (AgentError, LlmError, ToolError) in a dedicated error.rs, all using thiserror.

8. **Test strategy** — mock LlmClient in tests (no real API calls), fake SSE streams for streaming tests.

---
*Research for: Phase 1 - Core Agent Foundation*
*Based on: CONTEXT.md, REQUIREMENTS.md, STACK.md, PITFALLS.md, FEATURES.md, CONVENTIONS.md*
