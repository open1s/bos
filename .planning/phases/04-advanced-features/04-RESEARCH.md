# Phase 4: Advanced Features - Research

**Researched:** 2026-03-21
**Status:** Ready for planning

---

## Executive Summary

Phase 4 validates four major subsystems that were **architected in Phases 1-3** but not fully exercised: streaming tokens over Zenoh, executing workflows end-to-end, loading skills from config, and integrating MCP servers.

**Key Finding:** All core data structures and APIs are complete. The gaps are in **execution engines** and **validation scenarios**, not foundational functionality.

---

## 1: Streaming Validation

### 1.1 Current State

**Complete Components:**
- `SseDecoder` (105 lines) — Parses SSE format `data: <json>\n\n` with `[DONE]` marker
- `TokenPublisherWrapper` (199 lines) — Batching (10-50 tokens, 50ms timeout), rate limiting (100 tokens/sec default)
- `BackpressureController` (405 lines) — Token bucket algorithm, adaptive rate adjustment
- `BackpressureController` supports reporting bus load and `current_rate()` for monitoring

**Integration Points:**
- `Agent::run_streaming_with_tools()` in `agent/mod.rs` — Uses OpenAiClient::stream_complete() and SseDecoder
- `OpenAiClient::stream_complete()` — Yields StreamToken (Text, ToolCall, Done)

**Gap: End-to-End Validation**
- Integration tests in `streaming/integration_tests.rs` are **ignored** (require Zenoh router)
- No demonstration of Zenoh bus streaming in examples yet
- WeChat demo uses simulated streaming (`examples/wechat-demo/src/simulator.rs`)

### 1.2 Implementation Patterns

**Token Publishing Flow:**
```
LLM API → SseDecoder → StreamToken → TokenPublisherWrapper → Zenoh
                                        ↓
                                    BackpressureController
                                        ↓
                                    TokenBatch
```

1. **SSE Decoding:** OpenAiClient stream returns byte chunks → SseDecoder::decode_chunk() → Vec<SseEvent>
2. **Token Serialization:** `serialize_token(task_id, token)` → SerializedToken { task_id, token_type, content }
3. **Rate Limiting:** `BackpressureController::should_publish()` checks token bucket
4. **Batching:** Tokens accumulate in TokenBatch until:
   - 50 tokens collected OR
   - 50ms timeout elapsed OR
   - Backpressure threshold reached
5. **Bus Publishing:** `BusPublisher::publish_raw()` sends batch as bytes to topic `{prefix}/{agent_id}/tokens/stream`
6. **Adaptive Backpressure:** `report_bus_load(f64)` adjusts rate:
   - Load > 0.8 → Reduce rate
   - Load < 0.5 → Increase rate

### 1.3 Demo Design (Plan 04-01)

**Demo Structure:** `examples/demo-streaming/`

**Test Cases from ROADMAP:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| SSE decode | Parse SSE stream | Tokens yielded | `cargo test demo_sse_decode` |
| Token publisher | Publish tokens | Received | `cargo test demo_token_publish` |
| Rate limiter | 1000 req/s | Throttled | `cargo test demo_rate_limit` |
| Backpressure | Burst of data | Buffered | `cargo test demo_backpressure` |

**Demo Binary Implementation:**
```rust
// main.rs outline:
1. Setup OpenAiClient with real API key
2. Create Zenoh session
3. Create TokenPublisherWrapper
4. Start subscriber on `tokens/stream` topic
5. Call OpenAiClient.stream_complete() on a prompt
6. For each token: publisher.publish_token(task_id, token).await
7. Subscriber receives and displays tokens in real-time
8. Flush at end
```

**Subscriber Component Needed:**
```rust
// streaming/subscriber.rs (new file):
pub struct TokenSubscriber {
    session: Arc<Session>,
    agent_id: String,
    topic_prefix: String,
}

impl TokenSubscriber {
    pub async fn subscribe_tokens<F>(&self, callback: F) -> Result<()>
    where F: Fn(SerializedToken) -> ()
    {
        // Subscribe to {prefix}/{agent_id}/tokens/stream
        // Deserialize bytes → SerializedToken
        // Call callback for each token
    }

    pub fn verify_token_order(&self) -> bool {
        // Check sequence numbers/arrival order
    }
}
```

**Zenoh Router Requirement:**
- Integration tests need `zenohd` running
- Demo can use standalone Zenoh (router embedded in session)
- Test setup in `.github/workflows/integration-tests.yml`

### 1.4 Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| OpenAI API rate limits | Medium | Use smaller tokens, catch rate limit errors |
| Zenoh routing delay | Low | Document expected latency (<50ms) |
| Token order on bus | Medium | Add sequence numbers to SerializedToken |
| Backpressure tuning | Low | Document default rate (100/sec) is conservative |

---

## 2: Scheduler Validation

### 2.1 Current State

**Complete Components:**
- `Workflow` struct (107 lines) — Steps, defaults, metadata
- `WorkflowBuilder`, `StepBuilder` (174 lines) — Fluent DSL
- `BackoffStrategy` (126 lines) — Linear, Exponential, Fixed formulas
- `Scheduler::evaluate_condition()` — JsonPath evaluation for ConditionType
- Backoff calculation with retries in `retry.rs`

**Missing Executor:**
```rust
// executor.rs (39 lines) — STUB
pub async fn execute_workflow(&self, _workflow: &Workflow) -> WorkflowResult {
    WorkflowResult { status: Completed, .. } // Stub!
}
```

**Integration Points:**
- `Step.agent_id: Option<String>` — Remote delegation when set
- ConditionType::JsonPath{path, expected} — Evaluates against previous step output
- Retry logic exists in `BackoffStrategy::calculate_backoff(attempt)`

### 2.2 Executor Architecture

**Execution Engine Design:**

```rust
// executor.rs - Proposed implementation
impl Scheduler {
    pub async fn execute_workflow(
        &self,
        workflow: &Workflow,
        a2a_client: Option<A2AClient>,
    ) -> WorkflowResult
    {
        let step_results = Vec::new();
        let mut errors = Vec::new();

        // Flatten steps to handle nested sequential/parallel
        let flattened = self.flatten_workflow(workflow);
        for step in flattened {
            match &step.step_type {
                StepType::Sequential => {
                    // Pass previous output as input
                    let output = self.execute_step(&step, prev_output).await;
                }
                StepType::Parallel => {
                    // Spawn all steps concurrently, join futures
                    let outputs = futures::future::join_all(vec_of_tasks).await;
                }
                StepType::Conditional { condition } => {
                    // Evaluate condition against prev_output
                    if self.evaluate_condition(condition, &prev_output) {
                        self.execute_step(&step, prev_output).await;
                    }
                }
            }
        }
    }

    async fn execute_step(
        &self,
        step: &Step,
        input: Option<serde_json::Value>,
        a2a_client: &Option<A2AClient>,
    ) -> StepResult
    {
        let mut attempts = 0;

        loop {
            let step_output = tokio::time::timeout(step.timeout, async {
                match &step.agent_id {
                    None => self.execute_locally(&step, input).await,
                    Some(agent_id) => self.execute_remotely(a2a_client, agent_id, input).await,
                }
            }).await;

            match step_output {
                Ok(result) => return result,
                Err(_) if attempts < step.max_retries => {
                    let backoff = step.backoff.calculate_backoff(attempts);
                    tokio::time::sleep(backoff).await;
                    attempts += 1;
                }
                Err(e) => return StepResult {
                    status: StepStatus::TimedOut,
                    error: e.to_string(),
                    ..
                },
            }
        }
    }
}
```

**Key Design Decisions:**

1. **Sequential Pass-Through:** `input` → output → next step's input
2. **Parallel Isolation:** Each parallel step starts with same input
3. **Timeout Enforcement:** `tokio::time::timeout(step.timeout, ...)` wraps all execution
4. **Retry Loop:** Backoff between retries, track attempt count in results
5. **A2A Delegation:** `execute_remotely()` calls `A2AClient::delegate_task()`

**JsonPath Evaluation:**
```rust
// ConditionType::JsonPath
output.get(path) == Some(expected)

// Examples:
path: "$.output.result"
expected: 42 → output["output"]["result"] == 42

path: "$.status"
expected: "success"

// For now, simple JSON key lookup. More complex JsonPath in v2 (ORCH-01)
```

### 2.3 A2A Integration

**Remote Execution Pattern:**

```rust
async fn execute_remotely(
    &self,
    a2a_client: &A2AClient,
    agent_id: &str,
    input: Option<serde_json::Value>,
) -> StepResult
{
    let identity = AgentIdentity::new(
        format!("scheduler-{}", uuid::Uuid::new_v4()),
        "Scheduler".to_string(),
        "1.0.0".to_string(),
    );

    // Delegate task to remote agent
    let task = Task::new(
        uuid::Uuid::new_v4().to_string(),
        input.unwrap_or(serde_json::json!({})),
    );

    match a2a_client.delegate_task(&AgentIdentity::new(
        agent_id.to_string(), agent_id.to_string(), "1.0.0".to_string()
    ), task).await {
        Ok(response_task) => StepResult {
            status: StepStatus::Completed,
            output: response_task.output,
            ..
        },
        Err(e) => StepResult {
            status: StepStatus::Failed { error: e.to_string() },
            ..
        },
    }
}
```

**Integration Points:**
- Existing `A2AClient::delegate_task()` returns `Task` with output
- Agent identity needed for A2A delegation
- Task timeout → A2A timeout (map step timeout to task timeout)

### 2.4 Demo Design (Plan 04-02)

**Demo Structure:** `examples/demo-scheduler/`

**Test Cases from ROADMAP:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Sequential | 3 steps A→B→C | A→B→C order | `cargo test demo_sched_seq` |
| Parallel | 3 steps parallel | All complete | `cargo test demo_sched_par` |
| Conditional | Branch on output | Correct branch | `cargo test demo_sched_cond` |
| Retry | Fail 2 times then succeed | 3 attempts | `cargo test demo_sched_retry` |
| Timeout | Step takes too long | Timeout error | `cargo test demo_sched_timeout` |

**Demo Binary Implementation:**
```rust
// main.rs outline:
1. Create Zenoh session + A2AClient
2. Define local step (simple calculation)
3. Define remote step (requires second agent)
4. Build workflows:
   - Sequential: add → multiply → subtract
   - Parallel: 3 calculations concurrent
   - Conditional: if output > 10 → branch A else branch B
   - Retry with exponential backoff
5. Execute workflows, print results
6. Show step_results with duration, retry_count
```

**Workflow Examples:**

```rust
// Sequential
let workflow = WorkflowBuilder::new("calc")
    .add_step(StepBuilder::new("add")
        .sequential()
        .timeout(Duration::from_secs(5))
    )
    .add_step(StepBuilder::new("multiply")
        .sequential()
        .timeout(Duration::from_secs(5))
    )
    .build();

// Parallel
let workflow = WorkflowBuilder::new("parallel")
    .add_step(StepBuilder::new("task_a").parallel())
    .add_step(StepBuilder::new("task_b").parallel())
    .add_step(StepBuilder::new("task_c").parallel())
    .build();

// Conditional
 let workflow = WorkflowBuilder::new("conditional")
    .add_step(StepBuilder::new("calculate").sequential())
    .add_step(StepBuilder::new("high_value")
        .conditional(ConditionType::JsonPath {
            path: "value".to_string(),
            expected: serde_json::json!(100),
        })
    ).build();
```

### 2.5 Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| A2A timeout vs step timeout | High | Ensure A2A respects step timeout |
| Parallel resource contention | Medium | Document semaphore/limits in examples |
| Condition evaluation edge cases | Low | Keep JsonPath simple for v1 |
| State management across retries | Medium | Ensure stateless step execution |

---

## 3: Skills Validation

### 3.1 Current State

**Complete Components:**
- `SkillLoader` (396 lines) — YAML frontmatter parsing, lazy discovery, validation
- `SkillInjector` (237 lines) — XML-based injection, 3 formats (Compact, Standard, Verbose)
- `SkillMetadata`, `SkillContent` — Types for skills
- Circular dependency detection in `validate_all()`
- Category system: 8 categories (Analysis, Code, Communication, Data, Domain, Security, Testing, Utility)

**Gap: No Example Skills**
- No `.skills/` directory with example SKILL.md files
- No demonstration of skill loading in examples
- No integration test showing skill → agent prompt flow

**Skill File Format:**
```markdown
---
name: code-review
description: Review code for security and quality
version: 1.0.0
category: Code
author: BrainOS
tags: [quality, security]
requires: [security-analysis]
provides: [quality-report]
---

<prompt>
You are a code review expert. Check for:
1. Security vulnerabilities
2. Performance issues
3. Code style violations
</prompt>
```

### 3.2 Skill Loading Flow

```
skills/{name}/SKILL.md
    ↓
SkillLoader::discover()
    ↓
Parse YAML frontmatter → SkillMetadata
    ↓
Validate: missing fields, circular deps, not found
    ↓
SkillLoader.load(name)
    ↓
Load SKILL.md → extract frontmatter + body
    ↓
Load references/{file} → ReferenceFile[]
    ↓
Return SkillContent { metadata, instructions, references }
```

**Lazy Discovery:**
- `discover()` scans `skills/` directory for `SKILL.md` files
- Parses only frontmatter (metadata), not full content
- Validates structure but doesn't load references
- `load()` lazily loads full content when needed

### 3.3 Skill Injection

**Injection Formats:**

```rust
// Compact (minimal metadata)
<available_skills>
<skill name="code-review" />
</available_skills>

// Standard (recommended)
<available_skills>
<skill name="code-review" description="Review code" category="Code">
  <instructions>
    Check for security vulnerabilities...
  </instructions>
</skill>
</available_skills>

// Verbose (everything)
<available_skills>
<skill name="code-review" version="1.0.0" author="BrainOS"
        description="Review code" category="Code">
  <tags>
    <tag>quality</tag>
    <tag>security</tag>
  </tags>
  <requires>
    <skill>security-analysis</skill>
  </requires>
  <provides>
    <feature>quality-report</feature>
  </provides>
  <instructions>
    ...
  </instructions>
  <references>
    <file name="checklist.md">/full/path/to/checklist.md</file>
  </references>
</skill>
</available_skills>
```

**Integration with Agent:**
```rust
// In AgentBuilder:
impl AgentBuilder {
    pub fn with_skills(mut self, skills_dir: PathBuf) -> Self {
        let mut loader = SkillLoader::new(skills_dir);
        loader.discover().expect("skills discovery failed");

        let skill_contents: Vec<SkillContent> = loader.list()
            .iter()
            .map(|m| loader.load(&m.name).unwrap())
            .collect();

        let injector = SkillInjector::new();
        let skills_xml = injector.inject_available(&skill_contents);

        self.config.system_prompt.push_str("\n\n");
        self.config.system_prompt.push_str(&skills_xml);
        self
    }
}
```

**Reference File Loading:**
- `references/` subdirectory in skill directory
- Loaded as strings, not parsed
- Injected into instructions with full path references

### 3.4 Demo Design (Plan 04-03 part 1)

**Demo Structure:** `examples/demo-skills/`

**Skills to Create:**
1. `skills/basic-communication/SKILL.md` — Simple chat patterns
2. `skills/code-analysis/SKILL.md` — Code review, with references/checklist.md
3. `skills/security/SKILL.md` — Security scanning
4. `skills/composite/SKILL.md` — Uses code-analysis + security

**Test Cases from ROADMAP:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Load skill | Load from YAML | Skill loaded | `cargo test demo_skill_load` |
| Compose skills | Load 2 skills | No conflicts | `cargo test demo_skill_compose` |
| Inject skill | Inject into prompt | Injected | `cargo test demo_skill_inject` |

**Demo Binary Implementation:**
```rust
// main.rs outline:
1. Create examples/demo-skills/skills/ directory
2. Create 4 example SKILL.md files
3. Create references/checklist.md for code-analysis
4. Load skills with SkillLoader::discover()
5. Validate: check_dependencies(), detect_circular_deps()
6. Load full content with .load()
7. Inject with SkillInjector (all 3 formats)
8. Create AgentBuilder.with_skills() and show system prompt with skills
```

### 3.5 Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Skill metadata too large | Low | Document size limits in examples |
| Circular dependencies | Medium | Validation already implemented |
| Reference file paths | Low | Use absolute paths, test on different OS |
| Skill conflicts | Low | Namespacing already in ToolRegistry |

---

## 4: MCP Validation

### 4.1 Current State

**Complete Components:**
- `McpClient` (171 lines) — JSON-RPC 2.0 over STDIO, spawns server process
- `McpToolAdapter` (82 lines) — Implements Tool trait, wraps MCP tool calls
- `StdioTransport` (114 lines) — Line-based JSON over stdin/stdout
- Protocol types: `JsonRpcRequest`, `JsonRpcResponse`, `ServerCapabilities`, `ToolDefinition`

**Gap: Tool-Only Support**
- MCP protocol supports **tools, resources, prompts** (2025-03-26 spec)
- Current implementation only handles `tools/list` and `tools/call`
- No resource/prompt methods

**Capabilities Detection:**
```rust
// In McpClient::initialize()
let capabilities = resp.result.and_then(|v| v.get("capabilities")).map(|v| {
    serde_json::json!({
        "tools": v["tools"].as_bool(),
        "resources": v["resources"].as_bool(),
        "prompts": v["prompts"].as_bool()
    })
});
```

### 4.2 MCP Protocol Flow

```
1. Spawn process: McpClient::spawn("tree-sitter", ["--stdio"])
2. Initialize protocol: initialize → ServerCapabilities
3. List tools: tools/list → Vec<ToolDefinition>
4. Create adapters: For each tool, create McpToolAdapter
5. Register in ToolRegistry
6. Execute: agent calls tool → McpToolAdapter.execute() → call_tool() → JSON-RPC response

// Future (v1.2):
- List resources: resources/list
- Read resource: resources/read { uri }
- List prompts: prompts/list
- Get prompt: prompts/get { name, args }
```

**JSON-RPC Message Format:**
```json
// Request
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "search_code",
    "arguments": {
      "query": "def main"
    }
  }
}

// Response
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "content": [
      { "type": "text", "text": "Found 3 matches..." }
    ]
  }
}
```

### 4.3 ToolRegistry Integration

**MCP Tools as BrainOS Tools:**

```rust
// In Agent configuration:
let mcp_client = Arc::new(McpClient::spawn("tree-sitter", []).await?);
mcp_client.initialize().await?;

let tools = mcp_client.list_tools().await?;
for tool_def in tools {
    let adapter = McpToolAdapter::new(
        mcp_client.clone(),
        tool_def.name.clone(),
        tool_def.description.clone(),
        tool_def.input_schema.clone(),
    );
    tool_registry.register(adapter).await?;
}
```

**Config Integration:**

```yaml
# agent-config.toml
[mcp_servers.tree-sitter]
command = "tree-sitter"
args = ["--stdio"]
enabled = true

[mcp_servers.filesystem]
command = "mcp-server-filesystem"
args = ["/path/to/read"]
enabled = true
```

```rust
// TomlAgentConfig extension:
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
}

pub struct TomlAgentConfig {
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

impl AgentBuilder {
    pub async fn with_mcp_servers(mut self, config: &TomlAgentConfig) -> Result<Self> {
        for (name, server_cfg) in &config.mcp_servers {
            if server_cfg.enabled {
                let client = McpClient::spawn(&server_cfg.command, &server_cfg.args).await?;
                client.initialize().await?;

                let tools = client.list_tools().await?;
                for tool_def in tools {
                    let adapter = McpToolAdapter::new(
                        client.clone(),
                        tool_def.name.clone(),
                        tool_def.description.clone(),
                        tool_def.input_schema.clone(),
                    );
                    self.tool_registry.register(adapter).await?;
                }
            }
        }
        Ok(self)
    }
}
```

### 4.4 Demo Design (Plan 04-03 part 2)

**Demo Structure:** `examples/demo-mcp/`

**Test Cases from ROADMAP:**
| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| MCP adapter | MCP tool → Tool | Conversion works | `cargo test demo_mcp_adapter` |
| MCP client | Connect to server | Tools available | `cargo test demo_mcp_client` |

**Pre-built MCP Servers:**
- `mcp-server-everything` — Test server with sample tools/resources/prompts
- `vscode-tree-sitter-mcp` — Real world server for code AST queries

**Demo Binary Implementation:**
```rust
// main.rs outline:
1. Spawn mcp-everything server (path in PATH or bundled)
2. Create McpClient
3. Initialize protocol
4. List tools, resources, prompts
5. Create McpToolAdapter for each tool
6. Register in ToolRegistry
7. Create Agent with tools
8. Execute agent task that calls MCP tool
9. Print tool results
```

**Config File Example:**
```toml
[[tool]]
name = "echo"
description = "Echo back input"

[mcp_servers.everything]
command = "mcp-everything"
args = []
enabled = true

[mcp_servers.tree_sitter]
command = "tree-sitter"
args = ["--stdio"]
enabled = false  # Optional: require user to install
```

### 4.5 Resource/Prompt Support (Future)

**Protocol Extensions:**
```rust
// In McpClient (v1.2):
pub async fn list_resources(&self) -> Result<Vec<Resource>>
pub async fn read_resource(&self, uri: &str) -> Result<ReaderResult>
pub async fn list_prompts(&self) -> Result<Vec<Prompt>>
pub async fn get_prompt(&self, name: &str, args: serde_json::Value) -> Result<GetPromptResult>
```

**Resource Types:**
- Text files
- Binary files (images)
- Folders (collections)

**Prompt Templates:**
- Pre-defined prompt fragments
- Variables substitution
- Multi-message prompts

### 4.6 Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| MCP protocol changes | Medium | Pin to specific version in spec |
| Server version mismatches | Low | Graceful error handling |
| Stdio buffering issues | Low | Line-based JSON already uses newlines |
| MCP server not installed | Low | Document installation, skip if missing |

---

## 5: Tech Stack Decisions

### 5.1 Streaming

| Component | Stack | Rationale |
|-----------|-------|-----------|
| SSE parsing | Custom `SseDecoder` | Simple format, existing code |
| Token batching | `TokenBatch` struct | Existing implementation |
| Rate limiting | Token bucket algorithm | Standard approach |
| Backpressure | Adaptive rate adjustment | Existing implementation |
| Bus transport | Zenoh `PublisherWrapper` | Already in project |

### 5.2 Scheduler

| Component | Stack | Rationale |
|-----------|-------|-----------|
| DSL builder | Fluent API (`.add_step()`) | Existing implementation |
| Parallel execution | `futures::future::join_all` | Standard Rust async |
| Timeout | `tokio::time::timeout` | Existing tokio patterns |
| Retry | Existing `BackoffStrategy` | Reuse scheduler retry logic |
| A2A integration | Existing `A2AClient` | Must integrate with A2A |

### 5.3 Skills

| Component | Stack | Rationale |
|-----------|-------|-----------|
| Frontmatter | yaml-rust (`serde_yaml`) | Already in dependencies |
| File structure | `skills/{name}/SKILL.md` | Follow Claude pattern |
| Injection format | XML tags `<available_skills>` | Claude standard |
| Validation | Custom logic | Circular dependency detection |
| Lazy discovery | Parse metadata only | Efficient for many skills |

### 5.4 MCP

| Component | Stack | Rationale |
|-----------|-------|-----------|
| Protocol version | 2025-03-26 | MCP spec |
| Transport | Stdio (stdin/stdout) | MCP standard |
| JSON-RPC | Custom types | Lightweight |
| Tool adapter | `McpToolAdapter` | Implements Tool trait |
| Server spawning | `StdioTransport::spawn` | tokio::process |

---

## 6: Testing Strategy

### 6.1 Streaming Tests

**Unit Tests:**
```rust
// streaming/mod.rs - Existing
#[test] fn test_sse_decoder_single_event()
#[test] fn test_sse_decoder_delta_chunk()
#[test] fn test_sse_decoder_empty_input()

// streaming/integration_tests.rs - Ignored (require Zenoh)
#[test] #[ignore] fn test_sse_to_bus_tokens()
#[test] #[ignore] fn test_backpressure_adaptive()
```

**Integration Tests (demo-streaming):**
- Verify tokens arrive in order
- Verify batching works (check batch sizes in logs)
- Verify rate limiting (measure token throughput)
- Verify backpressure (simulated load at 80%, rate should decrease)

### 6.2 Scheduler Tests

**Unit Tests (existing):**
```rust
// scheduler/tests.rs — Stub validation only
#[test] fn test_workflow_builder()
#[test] fn test_backoff_calculation()
```

**Integration Tests (demo-scheduler):**
- Sequential: Output of step A is input to step B
- Parallel: All steps complete, order independent
- Conditional: Branch taken matches condition
- Retry: Attempt counter increases, backoff delays work
- Timeout: Step fails after timeout
- Remote: A2A delegation works (requires second agent)

### 6.3 Skills Tests

**Unit Tests:**
```rust
// skills/loader.rs - Existing validation
#[test] fn test_parse_skill_metadata()
#[test] fn test_circular_dependency_detection()
#[test] fn test_validate_name()

// skills/injector.rs
#[test] fn test_inject_compact()
#[test] fn test_inject_standard()
#[test] fn test_inject_verbose()
```

**Integration Tests (demo-skills):**
- Load skills from disk
- Validate all dependencies present
- Inject into agent prompt
- Agent respects skill instructions
- Reference files loaded correctly

### 6.4 MCP Tests

**Unit Tests (existing):**
```rust
// mcp/tests.rs
#[test] fn test_jsonrpc_request_serialization()
#[test] fn test_protocol_parsing()
```

**Integration Tests (demo-mcp):**
- Initialize mcp-everything server
- List tools > 0
- Call tool, get response
- Create adapter, register in ToolRegistry
- Agent can call MCP tool

---

## 7: Common Pitfalls

### 7.1 Streaming

**Pitfall:** Flooding bus with individual tokens
**Solution:** Use TokenBatch (10-50 tokens, 50ms timeout)

**Pitfall:** Rate limit too aggressive (tokens stuck in buffer)
**Solution:** BackpressureController adapts rate based on bus load

**Pitfall:** Token order lost in batches
**Solution:** Add sequence numbers to SerializedToken

### 7.2 Scheduler

**Pitfall:** Sequential steps don't receive previous output
**Solution:** Store `prev_output` and pass to next step

**Pitfall:** Parallel steps share mutable state
**Solution:** Each parallel step gets input copy, output isolated

**Pitfall:** Timeout ignored by long-running step
**Solution:** `tokio::time::timeout()` is mandatory in executor

**Pitfall:** A2A delegation hangs forever
**Solution:** Map step timeout to A2A task timeout

### 7.3 Skills

**Pitfall:** Circular dependencies crash loader
**Solution:** `detect_circular_deps()` already implemented

**Pitfall:** Reference file paths are relative (break on different CWD)
**Solution:** Use absolute paths from skill directory

**Pitfall:** Skill instructions too long in system prompt
**Solution:** Use Compact format, limit to top N skills

### 7.4 MCP

**Pitfall:** Server not installed, demo fails
**Solution:** Check `command --version`, skip if not found

**Pitfall:** Stdio buffering lines together
**Solution:** Line-based JSON already handles with newline delimiter

**Pitfall:** MCP protocol version mismatch
**Solution:** Pin to 2025-03-26 in initialization handshake

---

## 8: Validation Architecture

### 8.1 Testing Hierarchy

```
┌─────────────────────────────────────────┐
│  E2E Demos (examples/demo-xxxx)         │
│  - Real LLM API (OpenAI)                │
│  - Real Zenoh router                    │
│  - Real MCP servers                     │
└─────────────────────────────────────────┘
           │ validates
           ↓
┌─────────────────────────────────────────┐
│  Integration Tests (tests/xxxx_test.rs) │
│  - With Zenoh router (ignored without)  │
│  - With mock MCP server (spawn)         │
└─────────────────────────────────────────┘
           │ validates
           ↓
┌─────────────────────────────────────────┐
│  Unit Tests (src/xxx/tests.rs)          │
│  - Pure logic, no external deps         │
│  - Fast, run on every commit            │
└─────────────────────────────────────────┘
```

### 8.2 Nyquist Validation

**Requirement:** Every task must include `<read_first>` and `<acceptance_criteria>` with grep-verifiable conditions.

**Examples:**

```xml
<task type="auto">
  <name>Task: Implement scheduler executor</name>
  <read_first>
    crates/agent/src/scheduler/mod.rs
    crates/agent/src/scheduler/executor.rs
    crates/agent/src/a2a/client.rs
  </read_first>
  <action>
    Replace STUB execute_workflow() with full implementation:
    1. Flatten workflow steps
    2. For Sequential: execute in order, pass prev_output as input
    3. For Parallel: spawn all, join_all for results
    4. For Conditional: evaluate condition, execute if true
    5. Wrap execution in tokio::time::timeout(step.timeout)
    6. Retry loop with BackoffStrategy::calculate_backoff(attempt)
    7. If agent_id present, call A2AClient::delegate_task()
  </action>
  <acceptance_criteria>
    - crates/agent/src/scheduler/executor.rs contains 'async fn execute_workflow'
    - crates/agent/src/scheduler/executor.rs contains 'execute_step' with tokio::time::timeout
    - crates/agent/src/scheduler/executor.rs contains 'tokio::time::sleep(backoff)' in retry loop
    - crates/agent/src/scheduler/executor.rs contains 'A2AClient' import
  </acceptance_criteria>
</task>
```

### 8.3 Success Metrics

**Quantitative:**
- Streaming: >100 tokens/sec throughput, <50ms Zenoh latency, backpressure adaptation visible
- Scheduler: Sequential preserves order, Parallel completes all, Conditional branches correctly
- Skills: Load N skills in <100ms, inject XML <1MB for 10 skills
- MCP: Connect to mcp-everything in <1s, list tools >5, call tool <500ms

**Qualitative:**
- Demonstration binaries run without Zenoh router installation instructions obvious
- Logs show token arrival timestamps
- Workflow output JSON includes duration and retry_count
- Skills appear as `<available_skills>` in agent system prompt
- MCP tools appear in ToolRegistry.list()

---

## 9: Standard Stack Reference

This phase validates existing implementations. No new stack decisions needed.

### 9.1 External Dependencies

| Crate | Version | Usage |
|-------|---------|-------|
| `tokio` | 1.x | Async runtime, timeout, sleep |
| `zenoh` | Latest | Pub/sub, RPC, discovery |
| `serde_json` | Latest | JSON serialization |
| `serde_yaml` | Latest | YAML frontmatter parsing |

### 9.2 Internal Crates

| Crate | Features Used |
|-------|---------------|
| `bus` | PublisherWrapper, SubscriberWrapper, RpcService |
| `agent` | streaming, scheduler, skills, mcp modules |

### 9.3 Don't Hand Roll

✅ **DO use:**
- SseDecoder (existing)
- TokenPublisherWrapper (existing)
- BackpressureController (existing)
- Scheduler DSL (existing)
- BackoffStrategy (existing)
- SkillLoader (existing)
- McpClient (existing)
- McpToolAdapter (existing)

❌ **DON'T implement from scratch:**
- SSE parsing (SseDecoder)
- Token bucket algorithm (BackpressureController)
- Workflow builder (WorkflowBuilder)
- Skill discovery (SkillLoader.discover)

---

## Phase 4 Research Summary

**What's Complete:** All data structures, APIs, and core logic. This is a **validation phase**, not a new feature phase.

**What's Needed:**

1. **Streaming:** Create subscriber component + end-to-end demo with real OpenAI API
2. **Scheduler:** Implement executor engine (currently stub) + workflow examples
3. **Skills:** Create example SKILL.md files + demonstration of loading/injection
4. **MCP:** Add resources/prompts support (optional for v1.1) + ToolRegistry integration + demo with mcp-everything

**Complexity:** Medium. Existing code is solid, gaps are in execution/viability validation.

**Estimated Effort:** 3 plans, ~2 days total work.

**Resources Needed:** OpenAI API key, Zenoh router (embedded ok), MCP servers (mcp-everything, tree-sitter optional)

---

*Research complete: 2026-03-21*
*Next: Create PLAN.md files for 04-01, 04-02, 04-03*
