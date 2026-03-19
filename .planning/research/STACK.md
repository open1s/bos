# Stack Research

**Domain:** Distributed LLM Agent Orchestration Framework (Rust/Zenoh)
**Researched:** 2026-03-19
**Confidence:** MEDIUM-HIGH

## Core Stack

### LLM Client Layer

**Constraint: "No external AI SDK crates" — use raw reqwest + OpenAI-compatible API.**

Build a thin `LlmClient` trait from scratch:
```rust
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    async fn stream_complete(&self, req: LlmRequest) -> Result<StreamToken, LlmError>;
    fn provider_name(&self) -> &'static str;
    fn supports_tools(&self) -> bool;
}
```

**reqwest** — HTTP client for OpenAI-compatible API calls. Use `reqwest::Client` with timeout, connection pooling.
- Keep a single `Client` instance (clones share connection pool)
- For streaming: use `reqwest::get_sse()` or `reqwest::Client::request().send()` with `bytes_stream()`
- Tokio runtime for async

**serde_json** — JSON serialization for API requests/responses. Already in workspace.

**Key insight from research:** The existing `llm` crate (graniet) and `cloudllm` crate exist, but the constraint says no external AI SDK. So build a minimal custom client trait that wraps reqwest. Study `cloudllm`'s approach to `ClientWrapper` for provider abstraction patterns — but don't depend on the crate.

### Streaming

**Tokio async streams** (`tokio_stream::StreamExt`) — for handling SSE from LLM providers.

**Critical:** SSE parsing from scratch. Each provider formats streaming differently:
- OpenAI: `data: {"choices":[{"delta":{"content":"..."}}]}\n\n`
- Anthropic: `data: {"type":"content_block_delta","index":0,"delta":{"type":"text","text":"..."}}\n\n`
- SSE lines prefixed with `data: `

Implement a thin `SseDecoder` that yields tokens.

**Why not `eventsource-stream` crate:** Simple enough to parse manually, avoids another dep.

### Serialization

**rkyv** (already in workspace) — for agent messages on the bus. Zero-copy deserialization is critical for high-throughput agent coordination.

**serde_json** — for LLM API calls (OpenAI-compatible JSON API).

**Do NOT use `serde` for bus messages** — rkyv only. Keep them separate.

### Bus Communication

**crates/bus** (existing) — Zenoh pub/sub wrappers. Agents live as QueryableWrappers. Tools are RpcClients. MCP bridges become bus-based proxies.

**zenoh** (already implied by bus crate) — distributed communication substrate.

### Tool Calling

Build from scratch. Study:
- `struct-llm` crate pattern: trait `StructuredOutput` + derive macro generating JSON Schema from Rust types
- `cloudllm`'s tool registry approach: `ToolRegistry` handles multiple sources
- **Provider-specific schema translation** — OpenAI uses `{"type":"function","function":{...}}`, Anthropic uses `{"name":"...","description":"...","input_schema":{...}}`

**No external tool-calling crate.** Build minimal:
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn json_schema(&self) -> serde_json::Value;
    fn execute(&self, args: serde_json::Value) -> impl Future<Output = Result<serde_json::Value, ToolError>> + Send;
}
```

### MCP Integration

**STDIO transport only for MCP** — MCP servers communicate over stdin/stdout. Bridge to bus via local proxy.

**No MCP crate dependency.** Implement MCP JSON-RPC protocol from spec:
- JSON-RPC 2.0 over STDIO
- `initialize`, `tools/list`, `tools/call` messages
- Async process spawning with `tokio::process`

**Reference:** `cloudllm` MCP registry pattern, but implement manually for control.

### A2A Protocol

**JSON-RPC 2.0 over Zenoh** — agents communicate via the bus using JSON-RPC.

Design task state machine on the bus:
- `AgentMessage` envelope: `{method, params, task_id, reply_to}`
- `TaskStatus` enum: `Submitted | Working | Completed | Failed | InputRequired`
- Idempotency keys for deduplication

**Reference:** Anthropic's A2A spec patterns.

### Skills System

**Config-driven** — load from TOML/YAML via `crates/config`.

**No skill crate dependency.** Design:
```rust
pub struct Skill {
    pub name: String,
    pub description: String,
    pub prompt_template: String,  // Jinja2-like template
    pub tools: Vec<ToolRef>,       // References to registered tools
    pub dependencies: Vec<String>, // Other skills this depends on
}
```

Use `config` crate to load skill definitions. Use `serde_json` + basic template engine (or ` MINIJINDE`/`tinytemplate` for templating). Keep it simple — avoid full Jinja2.

### Scheduling

**Tokio for concurrency** — `tokio::task::JoinSet` for parallel execution.

**No scheduling crate dependency.** Build simple:
```rust
pub enum Step {
    Sequential(Vec<WorkflowStep>),
    Parallel(Vec<WorkflowStep>),
    Conditional { condition: Box<dyn Fn(&AgentState) -> bool + Send>, then: Box<WorkflowStep>, else_: Box<WorkflowStep> },
}
```

### Session State

**In-memory with optional persistence** — `serde_json` for serialization.

**No database for v1.** Agents can serialize state to files. Define `AgentState` struct that holds message history and context.

## What NOT to Use and Why

| Library | Reason to Avoid |
|---------|------------------|
| `llm` (graniet) | Full-featured but adds dependency weight; build minimal client instead |
| `cloudllm` | Too heavy, bundles everything; we're building lean and composable |
| `langchain-rust` | Chain-centric design, doesn't fit bus-based architecture |
| `eventsource-stream` | SSE parsing is simple enough to hand-roll |
| `async-openai` | OpenAI-only; we need multi-provider from day 1 |
| Full Jinja2 | Overkill for simple prompt templating |
| Database crates (sled, rocksdb) | Out of scope for v1; file-based persistence is fine |

## Confidence Notes

- **reqwest + tokio streams**: Very confident — industry standard for async HTTP in Rust
- **rkyv**: Very confident — already in workspace, proven for Zenoh
- **zenoh**: Very confident — already dependency of bus crate
- **Custom LLM client**: Confident — simple enough to build, gives full control
- **MCP manual impl**: Medium confidence — protocol is stable but verbose; worth considering `mcp-client` crate if one exists with simple API

---
*Stack research for: Distributed LLM Agent Orchestration Framework*
*Researched: 2026-03-19*
