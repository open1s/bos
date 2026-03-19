# Phase 02: Agent Protocols (A2A, MCP, Skills)

**Phase:** 02-agent-protocols
**Status:** Planning
**Created:** 2026-03-19

---

## Overview

Build the communication and capability layer for distributed agent orchestration:
1. **A2A Protocol** — Agent-to-Agent task delegation over Zenoh
2. **MCP Bridge** — Model Context Protocol (STDIO) tool integration
3. **Skills System** — Declarative skill definitions with composition

---

## Locked Decisions

### A2A Protocol

#### Message Envelope
```rust
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

pub enum A2AContent {
    TaskRequest { task: Task },
    TaskResponse { task: Task },
    TaskStatus { task_id: String, state: TaskState },
    InputRequired { task_id: String, prompt: String },
}

pub struct AgentIdentity {
    pub id: String,
    pub name: String,
    pub version: String,
}
```

#### Task State Machine
```
Submitted → Working → Completed
          → Working → InputRequired → Working
          → Working → Failed
          → Submitted → Canceled
          → Working → Canceled
```

```rust
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Canceled,
}
```

#### Zenoh Topic Structure
- `agent/{agent_id}/tasks/incoming` — receive task delegations
- `agent/{agent_id}/tasks/{task_id}/status` — publish status updates
- `agent/{agent_id}/responses/{correlation_id}` — reply to specific request
- `agent/discovery/announce` — AgentCard announcements
- `agent/discovery/health` — Health status updates

#### Idempotency
- Client generates `idempotency_key` (format: `{task_id}:{timestamp}`)
- Server stores processed keys in TTL cache (default 5 min)
- Duplicate requests return cached result

#### Discovery (AgentCard)
```rust
pub struct AgentCard {
    pub agent_id: AgentIdentity,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<Capability>,
    pub supported_protocols: Vec<String>, // ["A2A", "MCP"]
    pub endpoints: Vec<Endpoint>,
    pub skills: Vec<String>, // Skill names
    pub status: AgentStatus,
}

pub enum AgentStatus {
    Online,
    Busy,
    Offline,
}
```

---

### MCP Bridge

#### STDIO Transport
```rust
pub struct McpClient {
    process: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    request_id: AtomicU64,
    pending: HashMap<u64, oneshot::Sender<Result<McpResponse, McpError>>>,
}

impl McpClient {
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, McpError>;
    pub async fn initialize(&mut self) -> Result<ServerCapabilities, McpError>;
    pub async fn list_tools(&mut self) -> Result<Vec<ToolDefinition>, McpError>;
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Result<Value, McpError>;
    pub async fn shutdown(mut self) -> Result<(), McpError>;
}
```

#### JSON-RPC 2.0 Messages
```rust
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str, // "2.0"
    pub id: u64,
    pub method: String,
    pub params: Option<Value>,
}

pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}
```

#### MCP Error Codes
- `-32700`: Parse error
- `-32600`: Invalid Request
- `-32601`: Method not found
- `-32602`: Invalid params
- `-32603`: Internal error
- `-32001`: Resource not found
- `-32002`: Tool not found

#### Process Lifecycle
- `spawn()` — Create process with piped stdin/stdout/stderr
- `kill_on_drop(true)` — Prevent zombies
- `shutdown()` — Send `shutdown` method, wait 5s, then kill

#### Tool Adapter
```rust
pub struct McpToolAdapter {
    client: Arc<Mutex<McpClient>>,
    tool_name: String,
    definition: ToolDefinition,
}

impl Tool for McpToolAdapter {
    fn name(&self) -> &str { &self.tool_name }
    fn description(&self) -> ToolDescription { /* ... */ }
    fn json_schema(&self) -> Value { self.definition.input_schema.clone() }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        self.client.lock().await.call_tool(&self.tool_name, args).await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))
    }
}
```

---

### Skills System

#### Design Philosophy (Claude/OpenCode Style)

1. **Lazy Loading** — Skills are NOT loaded at startup. Only `name` + `description` are discovered.
2. **On-Demand Activation** — Full SKILL.md content loaded when skill is invoked.
3. **Directory-Based Discovery** — Configuration specifies ONE folder to scan for skills.
4. **SKILL.md Format** — Use standard markdown with YAML frontmatter (like Claude Code).

#### SKILL.md Format

```markdown
---
name: code-review
description: Review code for issues and improvements
---

# Code Review Skill

## What I do
- Analyze code for potential bugs
- Check for style violations
- Suggest improvements

## When to use me
Use this skill when asked to review, audit, or analyze code quality.

## Instructions
1. Read all changed files
2. Check for common anti-patterns
3. Run configured linters
4. Summarize findings
```

#### Directory Structure

```
skills/
  code-review/
    SKILL.md           # Required: skill definition
    references/        # Optional: detailed docs (loaded on-demand)
      style-guide.md
    scripts/           # Optional: executable helpers (never loaded to context)
      run-linter.sh
```

#### Configuration (AgentConfig)

```toml
[agent]
name = "my-agent"
skills_dir = "./skills"  # Single folder to scan

# Skills are discovered from:
# - {skills_dir}/*/SKILL.md
```

#### Skill Loader Implementation

```rust
pub struct SkillLoader {
    skills_dir: PathBuf,
    discovered: HashMap<String, SkillMetadata>,  // name -> (description, path)
}

#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

impl SkillLoader {
    /// Scan skills_dir and load metadata only (lazy).
    pub fn discover(&self) -> Result<HashMap<String, SkillMetadata>, SkillError> {
        let mut skills = HashMap::new();
        for entry in std::fs::read_dir(&self.skills_dir)? {
            let skill_dir = entry?;
            let skill_file = skill_dir.path().join("SKILL.md");
            if skill_file.exists() {
                if let Some(meta) = Self::parse_metadata(&skill_file)? {
                    skills.insert(meta.name.clone(), meta);
                }
            }
        }
        Ok(skills)
    }

    /// Load full SKILL.md content on-demand.
    pub fn load(&self, name: &str) -> Result<SkillContent, SkillError> {
        let meta = self.discovered.get(name)
            .ok_or(SkillError::NotFound(name.to_string()))?;
        let content = std::fs::read_to_string(&meta.path)?;
        Self::parse_skill(content)
    }

    fn parse_metadata(path: &Path) -> Result<Option<SkillMetadata>, SkillError> {
        // Parse YAML frontmatter only
        let content = std::fs::read_to_string(path)?;
        let frontmatter = extract_frontmatter(&content)?;
        let name = frontmatter.get("name")
            .and_then(|v| v.as_str())
            .ok_or(SkillError::InvalidFormat("missing name".into()))?;
        let description = frontmatter.get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(Some(SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            path: path.to_path_buf(),
        }))
    }
}
```

#### Three-Phase Loading

1. **Discovery** — Scan directory, load `name` + `description` only (~50 tokens/skill)
2. **Activation** — When skill invoked, load full SKILL.md content (< 5000 tokens)
3. **Execution** — Load `references/` files if needed (on-demand)

#### Skill Activation Trigger

Skills are activated when the agent's LLM response includes a tool call:

```json
{
  "tool_call": {
    "name": "skill",
    "arguments": { "name": "code-review" }
  }
}
```

The agent runtime intercepts this, loads the skill, and appends to context.

#### Available Skills Injection

Inject available skills into agent context (like OpenCode):

```rust
fn inject_skill_list(skills: &HashMap<String, SkillMetadata>) -> String {
    let mut xml = String::from("<available_skills>\n");
    for (name, meta) in skills {
        xml.push_str(&format!(
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n  </skill>\n",
            name, meta.description
        ));
    }
    xml.push_str("</available_skills>");
    xml
}
```

This is injected into the system prompt so the LLM knows what skills exist.

#### Conflict Resolution

Skills don't define tools directly — they're just prompt fragments. Tool conflicts don't apply at skill level.

---

## File Structure

```
crates/agent/src/
  a2a/
    mod.rs           — A2AMessage, AgentIdentity, TaskState
    envelope.rs      — Serialization, envelope building
    client.rs        — A2AClient, delegate_task, poll_status
    discovery.rs     — AgentCard, A2ADiscovery, announce/list
    idempotency.rs   — IdempotencyStore, TTL cache
  mcp/
    mod.rs           — McpClient, McpError
    transport.rs     — STDIO transport, process spawn
    protocol.rs      — JSON-RPC 2.0 types
    adapter.rs       — McpToolAdapter (Tool trait impl)
  skills/
    mod.rs           — SkillConfig, SkillComposer
    loader.rs        — TOML parsing, skill loading
    composition.rs   — Dependency resolution, conflict detection
    namespace.rs     — Tool namespacing utilities
```

---

## Dependencies

### New Crate Dependencies
- `uuid = { workspace = true }` — for message/task IDs
- No new external crates needed (uses existing tokio, serde_json, rkyv)

### Internal Dependencies
- `bus` — Zenoh RPC, discovery, health
- `agent` (Phase 1) — Tool trait, Agent, LlmClient

---

## Testing Strategy

### A2A Tests
- `test_a2a_envelope_roundtrip` — Serialize/deserialize
- `test_task_state_transitions` — Valid state machine transitions
- `test_idempotency_dedup` — Duplicate request handling
- `test_discovery_announce_list` — AgentCard discovery

### MCP Tests
- `test_jsonrpc_request_serialize` — JSON-RPC format
- `test_jsonrpc_response_parse` — Response parsing
- `test_mcp_client_lifecycle` — spawn/initialize/shutdown (mock)
- `test_mcp_tool_adapter` — Tool trait implementation

### Skills Tests
- `test_skill_from_toml` — Parse TOML config
- `test_skill_dependency_resolution` — Topological sort
- `test_skill_conflict_detection` — Duplicate tool names
- `test_skill_namespace_resolution` — Namespaced tool lookup

---

## Out of Scope (Phase 3+)

- SSE/WebSocket transport for MCP
- Skill marketplace/registry
- A2A authentication/authorization
- Agent orchestration DSL
- Multi-agent conversation protocol
