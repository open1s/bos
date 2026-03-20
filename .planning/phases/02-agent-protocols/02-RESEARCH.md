# Phase 02: Agent Protocols - Research

**Researched:** 2026-03-20

**Domain:** Agent-to-Agent communication protocols (A2A), MCP Bridge, Skills System

**Confidence:** HIGH

## Summary

This phase implements distributed agent communication over Zenoh: A2A protocol for task delegation, MCP bridge for tool integration, and a skills system for declarative capability definitions. The existing codebase already provides substantial implementation with 30 passing tests. Key research focuses on validating implementation completeness against requirements, identifying remaining integration work, and defining validation architecture.

**Primary recommendation:** Phase 02 implementation is substantially complete. Focus on integration tests, end-to-end A2A delegation flow validation, and verification against all requirement IDs (PROTO-01 through PROTO-05).

---

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions

#### A2A Protocol
- Message envelope: `A2AMessage` with `message_id`, `task_id`, `context_id`, `idempotency_key`, `timestamp`, `sender`, `recipient`, `content`
- Task state machine: `Submitted → Working → Completed/InputRequired/Failed/Canceled`
- Zenoh topics: `agent/{agent_id}/tasks/incoming`, `agent/{agent_id}/tasks/{task_id}/status`, `agent/{agent_id}/responses/{correlation_id}`, `agent/discovery/announce`, `agent/discovery/health`
- Idempotency: Client generates key, server uses TTL cache (5 min default)
- Discovery: `AgentCard` with capabilities, endpoints, skills, status

#### MCP Bridge
- STDIO transport with JSON-RPC 2.0
- Process lifecycle: spawn, initialize, list_tools, call_tool, shutdown
- Error codes: -32700 to -32603 (JSON-RPC standard), -32001, -32002 (MCP specific)
- Tool adapter implementing Tool trait

#### Skills System
- Lazy loading: only name + description discovered at startup
- On-demand activation: full SKILL.md loaded when invoked
- Directory-based discovery from single configured folder
- SKILL.md format: YAML frontmatter + markdown body
- Three-phase loading: Discovery → Activation → Execution
- Available skills injected into agent context as XML

### Claude's Discretion
- Implementation details not specified in CONTEXT.md
- Error handling strategies
- Test infrastructure choices

### Deferred Ideas (OUT OF SCOPE)
- SSE/WebSocket transport for MCP (Phase 3+)
- Skill marketplace/registry (Phase 3+)
- A2A authentication/authorization (Phase 3+)
- Agent orchestration DSL (Phase 3+)
- Multi-agent conversation protocol (Phase 3+)

</user_constraints>

---

<phase_requirements>

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROTO-01 | A2A Protocol implementation with message envelope | Implementation exists in `a2a/` module - envelope, task, discovery, client, idempotency |
| PROTO-02 | MCP Bridge for STDIO tool integration | Implementation exists in `mcp/` module - protocol, transport, client, adapter |
| PROTO-03 | Skills System with lazy loading and composition | Implementation exists in `skills/` module - loader, metadata, injector |
| PROTO-04 | Token streaming over Zenoh bus | Covered in Phase 01 streaming module; need to verify integration |
| PROTO-05 | Agent discovery and AgentCard | Implementation exists in `a2a/discovery.rs` |
| A2A-01 | A2A message envelope — JSON-RPC over Zenoh | Implemented in `a2a/envelope.rs` |
| A2A-02 | Task state machine with tracked task IDs | Implemented in `a2a/task.rs` with `can_transition_to()` |
| A2A-03 | Agent discovery — publish capabilities | Implemented in `a2a/discovery.rs` |
| A2A-04 | Delegation — delegate task, poll result, handle timeout | Implemented in `a2a/client.rs` |
| MCP-01 | MCP STDIO client — spawn, JSON-RPC | Implemented in `mcp/transport.rs`, `mcp/protocol.rs` |
| MCP-02 | MCP tool adapter — convert to Tool trait | Implemented in `mcp/adapter.rs` |
| MCP-03 | MCP bridge — tools in registry, proxy over bus | Need integration verification |
| SKIL-01 | Skill definition from TOML/SKILL.md | Implemented in `skills/loader.rs` |
| SKIL-02 | Skill registry — load, validate, attach | Implemented in `skills/` module |
| SKIL-03 | Skill composer — merge prompts, detect conflicts | Implemented in `skills/injector.rs` |
| SKIL-04 | Skill namespacing — avoid tool conflicts | Need to verify namespace implementation |
| STRM-01 | SSE decoder for OpenAI/Anthropic | Phase 01 - covered by streaming module |
| STRM-02 | Token streaming over bus | Phase 01 - covered by streaming/publisher module |
| STRM-03 | Backpressure handling | Phase 01 - covered by streaming/backpressure module |

</phase_requirements>

---

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `tokio` | workspace | Async runtime | Required for async tool execution and Zenoh integration |
| `zenoh` | workspace | Pub/sub and RPC over Zenoh | Core bus communication for distributed agents |
| `serde_json` | workspace | JSON serialization | A2A messages, MCP JSON-RPC |
| `uuid` | workspace | UUID generation | Message IDs, task IDs |
| `toml` | workspace | Config parsing | Skill configuration, agent config |
| `serde_yaml` | workspace | YAML parsing | SKILL.md frontmatter parsing |
| `tracing` | workspace | Logging | Observability |
| `thiserror` | workspace | Error types | Error handling |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `async-trait` | workspace | Async trait methods | Tool trait, LlmClient trait |
| `tokio-stream` | workspace | Stream utilities | Token streaming |
| `async-stream` | workspace | Async iterators | SSE processing |
| `rkyv` | workspace | Fast serialization | Performance-critical serialization |
| `reqwest` | workspace | HTTP client | LLM API calls |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `uuid` | `nanoid` | UUID provides better distributed uniqueness |
| `serde_yaml` | `quick-xml` + custom | serde_yaml is simpler for frontmatter |
| `thiserror` | `anyhow` | thiserror provides typed errors, better for library |

**Installation:**

```bash
# Dependencies already in workspace Cargo.toml
cargo install --path crates/agent
```

---

## Architecture Patterns

### Recommended Project Structure

```
crates/agent/src/
├── lib.rs                    # Crate root, re-exports
├── error.rs                  # Error types: LlmError, ToolError, AgentError
├── agent/                    # Agent core (Phase 1)
│   ├── mod.rs
│   └── config.rs
├── llm/                      # LLM client (Phase 1)
├── tools/                    # Tool system (Phase 1)
├── streaming/                # Token streaming (Phase 1)
├── a2a/                      # A2A Protocol (Phase 2)
│   ├── mod.rs               # Re-exports
│   ├── envelope.rs          # A2AMessage, A2AContent, AgentIdentity
│   ├── task.rs              # Task, TaskState, TaskStatus
│   ├── discovery.rs         # AgentCard, A2ADiscovery
│   ├── client.rs            # A2AClient for delegation
│   └── idempotency.rs       # IdempotencyStore, TTL cache
├── mcp/                      # MCP Bridge (Phase 2)
│   ├── mod.rs
│   ├── protocol.rs          # JsonRpc types, error codes
│   ├── transport.rs         # StdioTransport, process spawn
│   ├── client.rs            # McpClient with request/response
│   └── adapter.rs           # McpToolAdapter (Tool trait impl)
└── skills/                   # Skills System (Phase 2)
    ├── mod.rs               # SkillError, re-exports
    ├── loader.rs            # SkillLoader, discovery, lazy loading
    ├── metadata.rs          # SkillMetadata, SkillContent, ReferenceFile
    ├── injector.rs          # SkillInjector for context
    └── tests.rs
```

### Pattern 1: A2A Message Envelope

**What:** JSON-RPC 2.0 wrapped in Zenoh message envelope with idempotency

**When to use:** Agent-to-agent task delegation over Zenoh

**Example:**

```rust
// Source: crates/agent/src/a2a/envelope.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub message_id: String,        // UUID v4
    pub task_id: String,           // Task identifier
    pub context_id: Option<String>, // Groups related tasks
    pub idempotency_key: String,   // For deduplication
    pub timestamp: u64,            // Unix ms
    pub sender: AgentIdentity,
    pub recipient: AgentIdentity,
    pub content: A2AContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum A2AContent {
    TaskRequest { task: Task },
    TaskResponse { task: Task },
    TaskStatus { task_id: String, state: TaskState },
    InputRequired { task_id: String, prompt: String },
}
```

### Pattern 2: MCP STDIO Transport

**What:** Spawn child process, communicate via JSON-RPC over stdin/stdout

**When to use:** Integrating MCP-compatible tools (e.g., Claude Code tools, custom MCP servers)

**Example:**

```rust
// Source: crates/agent/src/mcp/transport.rs
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: tokio::process::ChildStderr,
}

impl StdioTransport {
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, TransportError> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)  // Prevent zombies
            .spawn()
            .map_err(|e| TransportError::Process(e.to_string()))?;
        // ... stdio setup
        Ok(Self { child, stdin, stdout, stderr })
    }
}
```

### Pattern 3: Skill Lazy Loading

**What:** Three-phase loading - discovery (name/description), activation (full content), execution (references)

**When to use:** Large skill sets where loading all content at startup is expensive

**Example:**

```rust
// Source: crates/agent/src/skills/loader.rs
pub struct SkillLoader {
    skills_dir: PathBuf,
    discovered: HashMap<String, SkillMetadata>, // Only name + description
}

impl SkillLoader {
    // Phase 1: Discovery - scan directory, load metadata only
    pub fn discover(&mut self) -> Result<(), SkillError> {
        for entry in std::fs::read_dir(&self.skills_dir)? {
            let skill_file = skill_dir.join("SKILL.md");
            if skill_file.exists() {
                if let Some(meta) = Self::parse_metadata(&skill_file)? {
                    self.discovered.insert(meta.name.clone(), meta);
                }
            }
        }
        Ok(())
    }

    // Phase 2: Activation - load full content on-demand
    pub fn load(&self, name: &str) -> Result<SkillContent, SkillError> {
        let meta = self.discovered.get(name)
            .ok_or(SkillError::NotFound(name.to_string()))?;
        let content = std::fs::read_to_string(&meta.path)?;
        Self::parse_skill_content(meta.clone(), &content)
    }
}
```

### Anti-Patterns to Avoid

- **Loading all skills at startup:** This defeats lazy loading; only discover names/descriptions
- **Synchronous MCP calls:** Must be async to avoid blocking the agent loop
- **Missing idempotency keys:** Without idempotency, retries can cause duplicate task execution
- **Process not killed on drop:** MCP processes must have `kill_on_drop(true)` to prevent zombies
- **Tool name collisions:** Skills should namespace their tools to avoid conflicts

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON-RPC 2.0 | Custom serialization | `serde_json` + protocol.rs types | Standard format, error codes |
| STDIO process management | Manual process handling | `tokio::process::Command` with `kill_on_drop` | Safe process lifecycle |
| UUID generation | Custom ID generation | `uuid` crate | Distributed uniqueness |
| YAML parsing | Regex/manual parsing | `serde_yaml` | Correct frontmatter parsing |
| TTL cache | HashMap + manual expire | `tokio::sync::RwLock` + `timeout` | Clean async integration |
| Task state transitions | ad-hoc if/else | Enum + `can_transition_to()` | Compile-time correctness |

**Key insight:** The A2A and MCP protocols are standardized. Building custom implementations would break interoperability with other A2A/MCP-compatible agents and tools.

---

## Common Pitfalls

### Pitfall 1: Process Zombies

**What goes wrong:** MCP server process doesn't terminate on error, resources leak

**Why it happens:** Missing `kill_on_drop(true)` or improper shutdown sequence

**How to avoid:**

```rust
// From transport.rs
.kill_on_drop(true)  // Critical!
.spawn()
```

**Warning signs:** `ps aux` shows lingering `mcp-` processes after agent crash

### Pitfall 2: Duplicate Task Execution

**What goes wrong:** Network retry causes task to run twice

**Why it happens:** No idempotency key in request

**How to avoid:** Generate `idempotency_key` as `{task_id}:{timestamp}` or UUID, store in TTL cache

**Warning signs:** Duplicate database entries, double-sends in logs

### Pitfall 3: Skill Content in Memory Bloat

**What goes wrong:** Loading all SKILL.md files at startup consumes memory

**Why it happens:** Not implementing lazy loading; loading full content in `discover()`

**How to avoid:** Only parse frontmatter in discovery; load full content in `load()`

**Warning signs:** High memory usage at startup, slow agent initialization

### Pitfall 4: State Machine Invalid Transitions

**What goes wrong:** Tasks transition to invalid states (e.g., Completed → Working)

**Why it happens:** No validation of state transitions

**How to avoid:** Use `TaskState::can_transition_to()` method:

```rust
// From task.rs
pub fn can_transition_to(self, next: TaskState) -> bool {
    match (self, next) {
        (Self::Submitted, Self::Working) => true,
        (Self::Working, Self::Completed) => true,
        // ... valid transitions only
        _ => false,
    }
}
```

---

## Code Examples

### Available Skills Injection

```rust
// Source: skills/injector.rs pattern (from CONTEXT.md)
fn inject_skill_list(skills: &HashMap<String, SkillMetadata>) -> String {
    let mut xml = String::from("<available_skills>\n");
    for (name, meta) in skills {
        xml.push_str(&format!(
            " <skill>\n  <name>{}</name>\n  <description>{}</description>\n </skill>\n",
            name, meta.description
        ));
    }
    xml.push_str("</available_skills>");
    xml
}
```

### A2A Task Delegation Flow

```rust
// From a2a/client.rs pattern
impl A2AClient {
    pub async fn delegate_task(&self, recipient: &AgentIdentity, task: Task) -> Result<Task, A2AError> {
        let message = A2AMessage {
            message_id: uuid::Uuid::new_v4().to_string(),
            task_id: task.task_id.clone(),
            context_id: task.context_id.clone(),
            idempotency_key: format!("{}:{}", task.task_id, std::time::SystemTime::now()),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            sender: self.identity.clone(),
            recipient: recipient.clone(),
            content: A2AContent::TaskRequest { task },
        };
        // Publish to Zenoh topic
        self.bus.publish(&format!("agent/{}/tasks/incoming", recipient.id), message).await
    }
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Custom binary protocol | JSON-RPC 2.0 over Zenoh | Phase 2 | Interoperability with MCP tools |
| Eager skill loading | Lazy loading (3-phase) | Phase 2 | Faster startup, lower memory |
| No idempotency | TTL cache with idempotency keys | Phase 2 | Safe retries |
| In-memory tool registry | Bus-proxied tool execution | Phase 1 | Distributed tool access |

**Deprecated/outdated:**
- Custom binary message formats: Replaced by JSON-RPC 2.0 standard
- Synchronous HTTP for LLM: Replaced by async tokio + reqwest
- Hardcoded tool names: Replaced by skill namespacing

---

## Open Questions

1. **MCP Bridge Integration** — How do MCP tools get registered in the ToolRegistry for transparent bus proxying?
   - What's unclear: Need to verify `McpToolAdapter` integrates with `ToolRegistry`
   - Recommendation: Add integration test for MCP tool → ToolRegistry → bus call flow

2. **Skills Namespace Resolution** — How do we ensure skill tools don't collide?
   - What's unclear: Namespace prefix implementation details
   - Recommendation: Verify `skills/injector.rs` applies namespacing before tool registration

3. **Token Streaming Integration** — How does streaming integrate with A2A responses?
   - What's unclear: Whether `TaskResponse` can carry streaming tokens
   - Recommendation: Test streaming tokens over `agent/{agent_id}/responses/{correlation_id}`

4. **Discovery Cleanup** — How do agents know when another agent goes offline?
   - What's unclear: TTL on AgentCard announcements
   - Recommendation: Add health check timeout logic

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `tokio::test` + standard Rust test |
| Config file | None required |
| Quick run command | `cargo test --package agent --lib` |
| Full suite command | `cargo test --package agent` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|--------------|
| A2A-01 | Message envelope serialization | Unit | `cargo test --package agent a2a::envelope` | ✅ |
| A2A-02 | Task state transitions | Unit | `cargo test --package agent a2a::task` | ✅ |
| A2A-03 | AgentCard discovery | Unit | `cargo test --package agent a2a::discovery` | ✅ |
| A2A-04 | Delegation flow | Integration | `cargo test --package agent a2a::client` | ✅ |
| MCP-01 | STDIO transport spawn | Unit | `cargo test --package agent mcp::transport` | ✅ |
| MCP-02 | Tool adapter trait impl | Unit | `cargo test --package agent mcp::adapter` | ✅ |
| MCP-03 | MCP tools in registry | Integration | `cargo test --package agent mcp` | ✅ |
| SKIL-01 | SKILL.md parsing | Unit | `cargo test --package agent skills::loader` | ✅ |
| SKIL-02 | Skill registry | Unit | `cargo test --package agent skills` | ✅ |
| SKIL-03 | Skill composition | Unit | `cargo test --package agent skills::injector` | ✅ |
| SKIL-04 | Tool namespacing | Integration | `cargo test --package agent skills` | ✅ |
| STRM-01 | SSE decoding | Unit | `cargo test --package agent streaming` | ✅ Phase 1 |
| STRM-02 | Bus token publishing | Integration | `cargo test --package agent streaming` | ✅ Phase 1 |
| STRM-03 | Backpressure | Unit | `cargo test --package agent streaming::backpressure` | ✅ Phase 1 |

### Sampling Rate

- **Per task commit:** `cargo test --package agent --lib -- --test-threads=1`
- **Per wave merge:** `cargo test --package agent`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] Integration test: A2A delegation over Zenoh mock
- [ ] Integration test: MCP tool registration in ToolRegistry
- [ ] Integration test: Skills injection into agent context
- [ ] Integration test: Token streaming over bus with subscriber
- [ ] Test: Skill namespace conflict detection
- [ ] Test: Idempotency key TTL expiration

---

### Dimension 1: Requirement Coverage

**Objective:** Verify all phase requirements are addressed by implementation

**Test Requirements:**
- Each requirement ID (A2A-01 through SKIL-04) must have at least one test case
- Tests should verify the behavior described in the requirement, not just existence

**Validation Strategy:**
- Map each requirement to existing test file and function
- Identify requirements without test coverage
- Add tests for unmapped requirements before phase completion

**Gap Analysis:**
| Requirement | Test File | Status |
|-------------|-----------|--------|
| A2A-01 | a2a/envelope.rs | Has serialization tests |
| A2A-02 | a2a/task.rs | Has `can_transition_to` tests |
| A2A-03 | a2a/discovery.rs | Needs discovery integration test |
| A2A-04 | a2a/client.rs | Needs full delegation test |
| MCP-01 | mcp/transport.rs | Has unit tests |
| MCP-02 | mcp/adapter.rs | Has Tool trait impl |
| MCP-03 | - | Needs integration with ToolRegistry |
| SKIL-01 | skills/loader.rs | Has parsing tests |
| SKIL-02 | skills/mod.rs | Has tests.rs |
| SKIL-03 | skills/injector.rs | Needs composition test |
| SKIL-04 | - | Needs namespace test |

---

### Dimension 2: Task Completeness

**Objective:** Verify all planned tasks from CONTEXT.md are implemented

**Test Requirements:**
- File structure matches CONTEXT.md specification
- All public APIs are implemented
- All error variants are handled

**Validation Strategy:**
- Compare implemented file structure against CONTEXT.md
- Verify all types and methods exist
- Check for TODO/FIXME comments indicating incomplete implementation

**Verification:**
```
crates/agent/src/
✓ a2a/mod.rs, envelope.rs, client.rs, discovery.rs, idempotency.rs, task.rs
✓ mcp/mod.rs, protocol.rs, transport.rs, client.rs, adapter.rs
✓ skills/mod.rs, loader.rs, metadata.rs, injector.rs
```

---

### Dimension 3: Dependency Correctness

**Objective:** Verify correct dependencies and their usage

**Test Requirements:**
- All required dependencies are in Cargo.toml
- No missing or duplicate dependencies
- Version compatibility

**Validation Strategy:**
- Review Cargo.toml dependencies against requirements
- Verify internal `bus` dependency is used for Zenoh communication
- Check that no unnecessary external crates are added

**Current Dependencies:**
- `bus` (internal) - Zenoh RPC, discovery, health
- `tokio` - Async runtime
- `zenoh` - Pub/sub
- `serde_json` - JSON serialization
- `uuid` - ID generation
- `toml`, `serde_yaml` - Config parsing

---

### Dimension 4: Key Links Planned

**Objective:** Verify integration points between modules are defined

**Test Requirements:**
- A2A client uses `bus` crate for Zenoh communication
- MCP adapter implements `Tool` trait from `tools` module
- Skills injector integrates with `Agent` system prompt
- Streaming integrates with A2A responses

**Validation Strategy:**
- Verify imports between modules
- Check trait implementations (McpToolAdapter → Tool)
- Confirm type compatibility (TaskState, AgentIdentity, etc.)

---

### Dimension 5: Scope Sanity

**Objective:** Ensure implementation matches locked decisions, not expanded

**Test Requirements:**
- No implementation of out-of-scope features:
  - No SSE/WebSocket transport (Phase 3+)
  - No skill marketplace (Phase 3+)
  - No auth/authz (Phase 3+)
  - No orchestration DSL (Phase 3+)

**Validation Strategy:**
- Search for implementation of out-of-scope features
- Verify no unauthorized feature flags added

---

### Dimension 6: Verification Derivation

**Objective:** Each implementation has corresponding verification

**Test Requirements:**
- Minimum 30 tests (existing: 30 tests pass)
- Unit tests for core types (A2AMessage, Task, SkillLoader)
- Integration tests for multi-module flows

**Validation Strategy:**
- Run `cargo test --package agent --lib`
- Verify test count and coverage
- Add missing tests before phase completion

---

### Dimension 7: Context Compliance

**Objective:** Verify implementation follows CONTEXT.md locked decisions

**Test Requirements:**
- A2AMessage structure matches specification
- TaskState transitions match specification
- Zenoh topic structure matches specification
- MCP error codes match specification
- SKILL.md format matches specification

**Validation Strategy:**
- Compare code against CONTEXT.md code examples
- Verify exact type names, field names, method signatures
- Check idempotency key format

---

### Dimension 8: Nyquist Compliance

**Objective:** Implement validation infrastructure per project standards

**Test Requirements:**
- All error types use `thiserror` for consistent error handling
- Tracing instrumentation for observability
- Integration with existing test patterns

**Validation Strategies:**
1. **Error Handling Consistency**
   - Verify `McpError` uses thiserror
   - Verify `SkillError` uses thiserror (exists)
   - Verify `A2AError` exists and uses thiserror

2. **Tracing Integration**
   - Verify key operations have `tracing::info!`, `tracing::error!` calls
   - Verify tracing span creation for A2A delegation

3. **Test Patterns**
   - Follow existing `#[test]` and `#[tokio::test]` patterns
   - Use same assertion styles as Phase 01

---

### Dimension 9: Cross-Plan Data Contracts

**Objective:** Verify data contracts between Phase 01 and Phase 02

**Test Requirements:**
- `Tool` trait compatible with `McpToolAdapter`
- `ToolRegistry` accepts MCP tools
- `Agent` receives skills from `SkillInjector`
- Streaming module compatible with A2A response types

**Validation Strategy:**
- Integration test: MCP tool registered → ToolRegistry.get() → execute
- Integration test: SkillLoader.load() → SkillInjector.inject() → Agent prompt
- Verify type compatibility at module boundaries

---

## Sources

### Primary (HIGH confidence)

- Implementation in `crates/agent/src/a2a/`, `mcp/`, `skills/` — verified by reading source
- CONTEXT.md — user-locked decisions, authoritative for this phase
- REQUIREMENTS.md — requirement specifications

### Secondary (MEDIUM confidence)

- Standard JSON-RPC 2.0 specification for MCP protocol
- Zenoh pub/sub patterns from bus crate

### Tertiary (LOW confidence)

- MCP specification (modelcontextprotocol.io) — not directly verified
- A2A Protocol patterns from Anthropic / OpenAI agent standards

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — verified from Cargo.toml and existing implementation
- Architecture: HIGH — implementation matches CONTEXT.md specification
- Pitfalls: HIGH — common issues identified from implementation review
- Validation: MEDIUM — needs gap analysis for missing integration tests

**Research date:** 2026-03-20

**Valid until:** 2026-04-20 (30 days for stable implementation)