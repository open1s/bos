# BrainOS Agent Framework Roadmap

**Project:** BrainOS Agent Framework - Distributed LLM agent orchestration on Zenoh
**Crates:** `crates/agent` (new)

---

## Milestone: Agent v1

**Goal:** A distributed LLM agent framework where agents are services on the Zenoh bus. Agents can call tools, use skills, delegate to other agents, and run orchestrated workflows.

---

## Phase 1: Core Agent Foundation

**Goal:** A working single agent that calls tools, streams output, and loads from config.

**Deliverables:**
- `crates/agent` crate scaffold — workspace member, depends on `bus` + `config`
- `LlmClient` trait — `complete()` + `stream_complete()`, OpenAI-compatible impl with reqwest
- `Agent` struct — wraps LlmClient, owns message history, config-driven construction
- Agent reasoning loop — receive task → call LLM → parse response → execute tools → return
- `Tool` trait + `ToolRegistry` — `name()`, `description()`, `json_schema()`, `execute()`
- Schema translator — OpenAI tool format, minimal schema validation
- SSE decoder — parse OpenAI SSE streaming, yield tokens via `tokio_stream`
- Config-driven agent loading — define agent from TOML/YAML via ConfigLoader

**Requirements:** CORE-01, CORE-02, CORE-03, CORE-04, TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05, STRM-01

**Success Criteria:**
1. Agent constructed from config can be given a task and returns a response
2. Agent can call a registered tool and use the result in its next turn
3. Token stream works — tokens arrive as they're generated, not all at once
4. Tool schema mismatches produce clear errors, not silent failures
5. Agent, tools, and streaming all compile with `cargo build -p agent`

**Status:** Complete

Plans:
- [x] 01-01-PLAN.md — LlmClient, Agent Core & Reasoning Loop ✅
- [x] 01-02-PLAN.md — Tool trait, registry, schema translator, error recovery ✅
- [x] 01-03-PLAN.md — SSE streaming, config-driven loading, integration tests ✅

---

## Phase 2: Distributed Integration

**Goal:** Agents on the bus can discover each other, use MCP tools, and compose skills.

**Deliverables:**
- MCP STDIO client — spawn process, JSON-RPC 2.0 over stdin/stdout
- MCP tool adapter — convert MCP tool definitions to brainos Tool trait
- MCP bridge — MCP tools registered in ToolRegistry, transparent to agent
- A2A message envelope — JSON-RPC over Zenoh: `{method, params, task_id, reply_to}`
- Task state machine — `Submitted → Working → Completed | Failed | InputRequired`
- Agent discovery — publish/subscribe `agents/capabilities/{id}` on bus
- A2A delegation — delegate task to remote agent, poll result, handle timeout
- Skill definition — TOML/YAML: name, prompt fragment, tool refs, dependencies
- Skill registry + composer — load skills, merge prompts, detect tool name conflicts
- Token streaming over bus — PublisherWrapper for per-token broadcast
- Bus backpressure — batch/rate-limit token messages to avoid flooding

**Requirements:** STRM-02, STRM-03, MCP-01, MCP-02, MCP-03, A2A-01, A2A-02, A2A-03, A2A-04, SKIL-01, SKIL-02, SKIL-03, SKIL-04

**Success Criteria:**
1. Agent can discover other agents on the bus and delegate a task
2. MCP server tools appear in agent's tool registry and are callable
3. Skills loaded from config compose without tool name conflicts
4. Task delegation tracks state and times out properly
5. Streaming tokens reach bus subscribers without flooding the network

**Status:** Planned

**Plans:** 4 plans

Plans:
- [x] 02-01-PLAN.md — A2A protocol: envelope, state machine, discovery, delegation
- [x] 02-02-PLAN.md — MCP bridge: STDIO client, tool adapter, bus integration
- [x] 02-03-PLAN.md — Skills system: definition schema, registry, composer, namespacing
- [x] 02-04-PLAN.md — Streaming over bus, backpressure, integration tests

---

## Phase 3: Orchestration & Persistence

**Goal:** Multi-agent workflows and durable agent sessions.

**Deliverables:**
- Sequential workflow — run steps A → B → C, pass output as input
- Parallel workflow — run A, B, C simultaneously, collect results
- Conditional branching — branch based on output pattern match
- Step timeout and retry — configurable per step, exponential backoff
- AgentState serialization — message history + context → JSON
- Session save/restore — persist to disk, reload, continue conversation
- Session manager — list, delete, manage sessions by agent_id

**Requirements:** SCHD-01, SCHD-02, SCHD-03, SCHD-04, SESS-01, SESS-02, SESS-03

**Success Criteria:**
1. Multi-step workflow executes correctly with sequential, parallel, and conditional branches
2. Failed step times out and retries with backoff, then fails the workflow
3. Agent session survives restart — same agent_id restores message history
4. Session list shows available sessions with metadata

**Status:** Planned

Plans:
- [ ] 03-01-PLAN.md — Scheduler: workflow DSL, sequential/parallel/conditional execution
- [ ] 03-02-PLAN.md — Session persistence: state serialization, save/restore/manage

---

## v1 Requirements Coverage

| Phase | Requirements | Count |
|-------|-------------|-------|
| Phase 1 | CORE-01, CORE-02, CORE-03, CORE-04, TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05, STRM-01 | 10 |
| Phase 2 | STRM-02, STRM-03, MCP-01, MCP-02, MCP-03, A2A-01, A2A-02, A2A-03, A2A-04, SKIL-01, SKIL-02, SKIL-03, SKIL-04 | 13 |
| Phase 3 | SCHD-01, SCHD-02, SCHD-03, SCHD-04, SESS-01, SESS-02, SESS-03 | 7 |
| **Total** | | **30** |

---
*Created: 2026-03-19*
