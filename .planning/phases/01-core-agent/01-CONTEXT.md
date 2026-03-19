# Phase 1: Core Agent Foundation - Context

**Gathered:** 2026-03-19
**Status:** Ready for planning

<domain>
## Phase Boundary

Build the `crates/agent` crate — the foundation of the distributed agent framework. This phase delivers: LlmClient trait with OpenAI-compatible implementation, Agent struct with reasoning loop, Tool trait + registry, SSE streaming, and config-driven loading. Agents live as services on the Zenoh bus (via `crates/bus`). Everything here is a library crate, not a binary or service.

</domain>

<decisions>
## Implementation Decisions

### LlmClient Response Shape
- `LlmResponse` enum with variants: `Text(String)`, `ToolCall { name: String, args: serde_json::Value }`, `Done` (no more content)
- Agent loop matches on response, accumulates text or executes tool call
- Reasoning loop: `complete() → LlmResponse → match → repeat or return`

### Message History Structure
- `Message` enum: `User(String)`, `Assistant(String)`, `ToolResult { name: String, content: String }`
- `MessageLog` wraps `Vec<Message>`, provides `add_user()`, `add_assistant()`, `add_tool_result()`, `to_api_format()` (converts to OpenAI `messages` array)
- History serialized as JSON for persistence (Phase 3)

### Tool Execution Model
- `async fn execute(args: serde_json::Value) -> Result<serde_json::Value, ToolError>`
- Async only — no sync variant. Tokio runtime is assumed everywhere.
- `ToolError` enum: `SchemaMismatch`, `ExecutionFailed`, `Timeout`

### Agent Response to User
- `AgentOutput` enum: `Text(String)`, `ToolCall { name: String, args: serde_json::Value, result: String }` (tool call with result), `Error(String)`
- This is what the agent returns to the caller — not to be confused with LlmResponse (which is from the LLM)

### Config Format for Agent Definitions
- Default to TOML for agent definition files
- ConfigLoader loads from file(s), returns `serde_json::Value`, agent builder deserializes
- Agent config schema in TOML:
  ```toml
  [agent]
  name = "my-agent"
  model = "gpt-4o"
  base_url = "https://api.openai.com/v1"
  api_key = "sk-..."
  system_prompt = "You are a helpful assistant."
  timeout_secs = 60
  ```

### Serialization Strategy
- Bus messages (between agents/services): rkyv (already required by bus crate)
- LLM API calls: serde_json (already in workspace)
- Agent config files: TOML → serde_json → used as-is
- AgentState (future persistence): serde_json → JSON files

### Error Handling Style
- `AgentError` enum: `Llm(LlmError)`, `Tool(ToolError)`, `Config(String)`, `Session(String)`
- No `anyhow` in the crate — typed errors all the way
- `ToolError`: `SchemaMismatch`, `ExecutionFailed`, `Timeout`, `NotFound`
- `LlmError`: `Http(reqwest::Error)`, `Parse(String)`, `Timeout`, `ApiKeyMissing`, `RateLimited`

### HTTP Client Pattern
- `OpenAiClient::new()` creates and owns `reqwest::Client`
- Single client per agent instance (connection pooling handled by reqwest)
- Timeout per request (not global client timeout)

### Claude's Discretion
- Exact naming of methods on Agent struct (beyond `run()` and `stream_run()`)
- Internal module structure (split into `client.rs`, `agent.rs`, `tools.rs` or one `lib.rs`)
- Test strategy and test data (mock LLM responses vs real API)
- Cargo feature flags (minimal features for Phase 1)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project Definition
- `.planning/PROJECT.md` — Core value, constraints, key decisions
- `.planning/REQUIREMENTS.md` — CORE-01 through STRM-01 requirements
- `.planning/ROADMAP.md` §Phase 1 — Deliverables and success criteria

### Existing Code Patterns
- `.planning/codebase/CONVENTIONS.md` — Rust naming, error handling, module structure
- `.planning/codebase/STACK.md` — Tech stack, workspace dependencies
- `crates/bus/src/lib.rs` — Bus crate exports (QueryableWrapper, RpcClient, Codec)
- `crates/config/src/loader.rs` — ConfigLoader API and patterns

### Research Insights
- `.planning/research/STACK.md` — LlmClient trait design, reqwest patterns
- `.planning/research/PITFALLS.md` §Pitfall 2 — LLM provider coupling (build trait first)
- `.planning/research/PITFALLS.md` §Pitfall 4 — Schema mismatches (validation + error recovery)
- `.planning/research/FEATURES.md` §Build order — LLM Client → Agent Core → Tool Registry

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/bus/src/rpc/client.rs` — Builder pattern for RpcClient (matches how agent would be constructed)
- `crates/bus/src/rpc/error.rs` — Typed error enum pattern (can follow for AgentError)
- `crates/config/src/loader.rs` — ConfigLoader builder pattern with chainable methods
- `crates/bus/src/codec.rs` — rkyv unit struct pattern

### Established Patterns
- Builder pattern for client construction (`RpcClient::builder()`, `SessionManagerBuilder`)
- Typed errors in dedicated `error.rs` modules
- `serde_json` for API-level serialization (bus uses rkyv separately)
- Trait + impl pattern for extensible components

### Integration Points
- `crates/bus` → agents use QueryableWrapper (receive tasks) and RpcClient (call tools over bus)
- `crates/config` → ConfigLoader loads agent definitions from TOML/YAML/JSON files
- Zenoh → already handled by bus crate; agent doesn't interact with zenoh directly
- Workspace dependencies: tokio, reqwest, serde_json, rkyv already available

</code_context>

<specifics>
## Specific Ideas

- "Build the LlmClient trait first — everything depends on it" (from research)
- "Agents as bus services" — Agent wraps QueryableWrapper, not a separate binary
- "Tools as RPC calls" — ToolRegistry uses RpcClient under the hood for bus-based tools
- No external AI SDK crates — raw reqwest only

</specifics>

<deferred>
## Deferred Ideas

- v2: Anthropic Claude client implementation — different tool format translation
- v2: Advanced token streaming over bus with backpressure
- v2: Multi-provider abstraction beyond OpenAI-compatible

</deferred>

---
*Phase: 01-core-agent*
*Context gathered: 2026-03-19*
