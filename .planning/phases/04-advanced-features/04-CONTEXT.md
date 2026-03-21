# Phase 04: Advanced Features - Context

**Gathered:** 2026-03-21
**Status:** Ready for planning

<domain>
## Phase Boundary

Validate streaming, scheduler, skills, and MCP integration through demos. The focus is on proving existing implementations work in real scenarios: streaming tokens over Zenoh, executing workflows end-to-end, loading skills from config, and connecting MCP servers.

</domain>

<decisions>
## Implementation Decisions

### Streaming Validation
- Use real OpenAI API for streaming (not simulated responses)
- Validate end-to-end: LLM → SseDecoder → TokenPublisher → Zenoh → Subscriber
- Run ignored integration tests with Zenoh router
- Demonstrate backpressure adaptation under load
- Create subscriber component for token reception

**Gap Analysis:**
- Core components 100% complete (SseDecoder, TokenPublisher, backpressure)
- **Missing:** Subscriber component, real validation, WeChat demo upgrade

### Scheduler Validation
- Implement executor: sequential, parallel, conditional execution
- Integrate with A2A client for remote delegation
- Add timeout enforcement with tokio::time::timeout
- Wrap step execution in retry logic
- Create workflow examples (sequential, parallel, conditional, mixed)

**Gap Analysis:**
- Data model + DSL 100% complete
- **Missing:** Execution engine (currently stub), A2A integration, examples

### Skills Validation
- Use existing SkillLoader for YAML/TOML skill discovery
- Create example skills demonstrating categories (Code, Domain, etc.)
- Demonstrate skill composition (multiple skills loaded together)
- Validate skill injection into agent system prompt
- Test skill dependency resolution

**Gap Analysis:**
- Skills system 100% complete (loader, injector, metadata)
- **Missing:** Example skills, demonstration, e2e validation

### MCP Validation
- Implement resource and prompt support (currently tool-only)
- Integrate McpToolAdapter with ToolRegistry
- Add MCP server configuration to TomlAgentConfig
- Create MCP server discovery mechanism
- End-to-end test with mcp-everything or tree-sitter server

**Gap Analysis:**
- MCP client + tool adapter 100% complete
- **Missing:** Resources/prompts, ToolRegistry integration, config, examples

### Claude's Discretion
- Specific streaming token rate configuration (currently 100/sec)
- Workflow DSL syntax refinements (if any ergonomic improvements needed)
- Skill namespacing strategy (already implemented in ToolRegistry)
- MCP server lifecycle management (start/stop/restart)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase 4 Scope
- `.planning/ROADMAP.md` §531-700 — Phase 4 plans 04-01 through 04-03 with test cases
- `.planning/REQUIREMENTS.md` §23-28 — STRM-01 through STRM-03 streaming requirements
- `.planning/REQUIREMENTS.md` §49-54 — SCHD-01 through SCHD-04 scheduler requirements
- `.planning/REQUIREMENTS.md` §42-48 — SKIL-01 through SKIL-04 skills requirements
- `.planning/REQUIREMENTS.md` §29-34 — MCP-01 through MCP-03 MCP requirements

### Prior Context
- `.planning/phases/01-core-agent/01-CONTEXT.md` — Core agent decisions (tool system, streaming architecture)
- `.planning/phases/02-agent-protocols/02-CONTEXT.md` — A2A protocol, skills system decisions
- `.planning/phases/03-orchestration-persistence/03-CONTEXT.md` — Scheduler DSL, session decisions

### Project Context
- `.planning/PROJECT.md` — Core value, v1.0 status, v1.1 pending (AGENT-08, AGENT-09)
- `.planning/STATE.md` — Phase status, completed features, technical decisions

</canonical_refs>

<code_context>
## Existing Code Insights

### Streaming Components
- `crates/agent/src/streaming/mod.rs` — SseDecoder with unit tests (105 lines)
- `crates/agent/src/streaming/publisher.rs` — TokenPublisherWrapper with batching (199 lines)
- `crates/agent/src/streaming/backpressure.rs` — TokenBatch, RateLimiter, BackpressureController (405 lines)
- `crates/agent/src/streaming/integration_tests.rs` — 4 tests ignored (need Zenoh router)
- `examples/wechat-demo/src/simulator.rs` — Simulate streaming (replace with real)

**Reusable Assets:**
- Agent::run_streaming_with_tools() in agent/mod.rs
- OpenAiClient::stream_complete() uses SseDecoder
- All streaming primitives ready for bus integration

**Established Patterns:**
- SSE format: `data: <json>\n\n` with `[DONE]` marker
- Batching: 10-50 tokens, 50ms timeout
- Rate limiting: Token bucket algorithm (default 100/sec)
- Backpressure: Adaptive rate adjustment (80% load → reduce, 50% load → increase)

### Scheduler Components
- `crates/agent/src/scheduler/mod.rs` — Core types (Workflow, Step, StepType, etc.) (107 lines)
- `crates/agent/src/scheduler/dsl.rs` — WorkflowBuilder, StepBuilder (174 lines)
- `crates/agent/src/scheduler/retry.rs` — Backoff strategies (126 lines)
- `crates/agent/src/scheduler/executor.rs` — Scheduler::execute_workflow() STUB (39 lines)
- `crates/agent/src/scheduler/tests.rs` — 11 tests (stub validation only) (267 lines)

**Reusable Assets:**
- With retry logic wrapper in retry.rs
- Condition evaluation: evaluate_condition(JsonPath)
- Step.agent_id: Option<String> for A2A delegation

**Established Patterns:**
- Linear backoff: `(attempt + 1) * interval`
- Exponential: `base * 2^attempt` (capped at max)
- Builder pattern workflow definition

**Integration Points:**
- Connect executor to A2AClient::delegate_task() when agent_id is Some
- Use tokio::time::timeout for step timeout enforcement

### Skills Components
- `crates/agent/src/skills/mod.rs` — Type exports, SkillError (87 lines)
- `crates/agent/src/skills/metadata.rs` — SkillCategory, SkillVersion, SkillMetadata (304 lines)
- `crates/agent/src/skills/loader.rs` — SkillLoader, SkillStats (396 lines)
- `crates/agent/src/skills/injector.rs` — SkillInjector, injection (237 lines)

**Reusable Assets:**
- SKILL.md format with YAML frontmatter (--- delimited)
- Category system: Analysis, Code, Communication, Data, Domain, Security, Testing, Utility
- Dependency validation and circular dependency detection
- Injection formats: Compact, Standard, Verbose

**Established Patterns:**
- Skill directory structure: skills/{name}/SKILL.md, skills/{name}/references/
- Lazy discovery: SkillLoader.discover() → load() → inject
- XML-based injection: `<available_skills><skill>...</skill></available_skills>`

**Integration Points:**
- AgentBuilder should accept skills list
- SkillInjector injects into system prompt

### MCP Components
- `crates/agent/src/mcp/mod.rs` — Module exports (17 lines)
- `crates/agent/src/mcp/client.rs` — McpClient with JSON-RPC 2.0 (171 lines)
- `crates/agent/src/mcp/adapter.rs` — McpToolAdapter (Tool trait) (82 lines)
- `crates/agent/src/mcp/protocol.rs` — JSON-RPC types (125 lines)
- `crates/agent/src/mcp/transport.rs` — StdioTransport (114 lines)
- `crates/agent/src/mcp/tests.rs` — Protocol unit tests (213 lines)

**Reusable Assets:**
- McpClient::spawn(), initialize(), list_tools(), call_tool()
- McpToolAdapter implements Tool trait
- Protocol version 2025-03-26 support
- Atomic request ID generation

**Established Patterns:**
- Line-based JSON over STDIO (newline-delimited JSON)
- MCP tool → BrainOS Tool adapter pattern

**Integration Points:**
- ToolRegistry should register McpToolAdapter instances
- TomlAgentConfig should include MCP server configuration

</code_context>

<specifics>
## Specific Ideas

### Streaming
- "I want to see tokens arrive in real-time over the bus, not just log messages"
- "Backpressure should visibly slow down when the bus is congested"
- Demonstrate with tools/basic-communication example using real LLM streaming

### Scheduler
- "Sequential steps should execute in order, each receiving the previous step's output"
- "Parallel steps should run simultaneously and complete when all finish"
- "Conditionals should branch based on JsonPath evaluation of previous output"
- "A2A delegation should spawn tasks on remote agents, poll for results, handle timeout"

### Skills
- "Skills should be loadable from YAML without code changes"
- "Multiple skills can be composed together (e.g., 'code-review' uses 'code-analysis' + 'security')"
- "Skills inject into system prompt without manual editing"

### MCP
- "MCP servers should be listed in config like any agent settings"
- "MCP tools appear in ToolRegistry just like local tools"
- "Skills can reference MCP tools — seamless integration"

</specifics>

<deferred>
## Deferred Ideas

- Multi-provider LLM (Anthropic) — v2 milestone
- Workflow DAG and subworkflows — v2 ORCH-01, ORCH-02
- Agent telemetry and A2A tracing — v2 OBS-01, OBS-02
- Vector database and long-term memory — out of scope

</deferred>

---

*Phase: 04-advanced-features*
*Context gathered: 2026-03-21*
