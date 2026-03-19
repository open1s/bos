# Requirements: BrainOS Agent Framework

**Defined:** 2026-03-19
**Core Value:** Agents can discover each other, call tools, use skills, and delegate work via MCP/A2A — all over the distributed bus with zero configuration.

## v1 Requirements

### Core Agent

- [ ] **CORE-01**: LLM Client trait — `LlmClient` trait with `complete()` and `stream_complete()`, implemented for OpenAI-compatible APIs (reqwest + tokio streams)
- [ ] **CORE-02**: Agent struct — wraps LlmClient, owns system prompt, message history (conversation turns), configurable from config
- [ ] **CORE-03**: Agent reasoning loop — receive task → call LLM → parse response → handle tools/exit → return result
- [ ] **CORE-04**: Config-driven agent loading — define agents from TOML/YAML/JSON via ConfigLoader (name, model, system prompt, tools, skills)

### Tool System

- [x] **TOOL-01**: Tool trait — `name()`, `description()`, `json_schema()`, `execute()` — sync and async variants
- [x] **TOOL-02**: Tool registry — register, lookup, list tools; handles duplicate registration
- [x] **TOOL-03**: Schema translator — convert tool JSON Schema to OpenAI format and Anthropic format
- [x] **TOOL-04**: Tool error recovery — validation failures return clear errors to LLM, not silent failures
- [x] **TOOL-05**: Bus tool execution — tools registered locally but callable over the bus via RpcClient (transparent to agent)

### Streaming

- [ ] **STRM-01**: SSE decoder — parse OpenAI SSE and Anthropic SSE streaming formats, yield tokens via `tokio_stream`
- [ ] **STRM-02**: Token streaming over bus — agent can stream tokens to subscribers via PublisherWrapper
- [ ] **STRM-03**: Backpressure handling — don't flood bus with per-token messages; batch or rate-limit

### MCP Integration

- [ ] **MCP-01**: MCP STDIO client — spawn MCP server process, send/receive JSON-RPC 2.0 messages over stdin/stdout
- [ ] **MCP-02**: MCP tool adapter — convert MCP tool definitions to brainos Tool trait
- [ ] **MCP-03**: MCP bridge — MCP tools registered transparently in ToolRegistry, execution proxied over bus

### A2A Protocol

- [ ] **A2A-01**: A2A message envelope — JSON-RPC over Zenoh: `{method, params, task_id, reply_to, idempotency_key}`
- [ ] **A2A-02**: Task state machine — `Submitted → Working → Completed | Failed | InputRequired` with tracked task IDs
- [ ] **A2A-03**: Agent discovery — agents publish capabilities to `agents/capabilities/{id}`, subscribe to wildcard for discovery
- [ ] **A2A-04**: Delegation — Agent A can delegate task to Agent B via bus, poll for result, handle timeout

### Skills System

- [ ] **SKIL-01**: Skill definition — loaded from TOML/YAML: name, description, prompt fragment, tool references, dependencies
- [ ] **SKIL-02**: Skill registry — load, validate, attach skills to agents
- [ ] **SKIL-03**: Skill composer — merge skill prompts into agent system prompt, detect tool name conflicts at load time
- [ ] **SKIL-04**: Skill namespacing — avoid conflicts by prefixing tools with skill name

### Scheduling

- [ ] **SCHD-01**: Sequential workflow — run steps A → B → C, pass output of each as input to next
- [ ] **SCHD-02**: Parallel workflow — run A, B, C simultaneously, collect all results
- [ ] **SCHD-03**: Conditional branching — branch based on output of previous step (true/false/pattern match)
- [ ] **SCHD-04**: Step timeout and retry — configurable timeout per step, retry with exponential backoff

### Session Management

- [ ] **SESS-01**: AgentState serialization — serialize message history, context, pending tasks to JSON
- [ ] **SESS-02**: Session restore — load agent state from disk, continue conversation
- [ ] **SESS-03**: Session management — save/restore/list/delete sessions by agent_id

## v2 Requirements

### Multi-Provider
- **MPROV-01**: Anthropic Claude client implementation — different tool format translation
- **MPROV-02**: Provider-agnostic tool schema — unified schema representation across providers

### Advanced Orchestration
- **ORCH-01**: Subworkflows — named workflows callable as single step
- **ORCH-02**: Workflow DAG — directed acyclic graph execution with dependency tracking

### Observability
- **OBS-01**: Agent telemetry — log token usage, tool call frequency, latency per call
- **OBS-02**: A2A tracing — trace task delegation across agents via task_id

## Out of Scope

| Feature | Reason |
|---------|--------|
| Built-in vector database | Users have their own DB preferences; too heavy for v1 |
| Python bindings | Separate effort; BrickOS Phase 4 |
| Built-in auth/authz | Defer to BrickOS Phase 3 |
| UI/CLI tooling | API/library first; tooling later |
| Multi-modal (vision, audio) | Stick to text-first; add later |
| Agent memory beyond session | In-memory + file persistence is enough for v1 |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| CORE-01 | Phase 1 | Pending |
| CORE-02 | Phase 1 | Pending |
| CORE-03 | Phase 1 | Pending |
| CORE-04 | Phase 1 | Pending |
| TOOL-01 | Phase 1 | Complete |
| TOOL-02 | Phase 1 | Complete |
| TOOL-03 | Phase 1 | Complete |
| TOOL-04 | Phase 1 | Complete |
| TOOL-05 | Phase 1 | Complete |
| STRM-01 | Phase 1 | Pending |
| STRM-02 | Phase 2 | Pending |
| STRM-03 | Phase 2 | Pending |
| MCP-01 | Phase 2 | Pending |
| MCP-02 | Phase 2 | Pending |
| MCP-03 | Phase 2 | Pending |
| A2A-01 | Phase 2 | Pending |
| A2A-02 | Phase 2 | Pending |
| A2A-03 | Phase 2 | Pending |
| A2A-04 | Phase 2 | Pending |
| SKIL-01 | Phase 2 | Pending |
| SKIL-02 | Phase 2 | Pending |
| SKIL-03 | Phase 2 | Pending |
| SKIL-04 | Phase 2 | Pending |
| SCHD-01 | Phase 3 | Pending |
| SCHD-02 | Phase 3 | Pending |
| SCHD-03 | Phase 3 | Pending |
| SCHD-04 | Phase 3 | Pending |
| SESS-01 | Phase 3 | Pending |
| SESS-02 | Phase 3 | Pending |
| SESS-03 | Phase 3 | Pending |

**Coverage:**
- v1 requirements: 27 total
- Mapped to phases: 27
- Unmapped: 0 ✓

---
*Requirements defined: 2026-03-19*
*Last updated: 2026-03-19 after initial definition*
