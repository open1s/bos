# BrainOS Agent Framework

## What This Is

A distributed LLM agent development and scheduling framework built on BrainOS (Zenoh-based pub/sub bus + config system). Provides the primitives for building, composing, and orchestrating AI agents that communicate over the distributed bus. Think "agent orchestration layer" — agents are services, tools are RPC endpoints, skills are composable modules.

## Core Value

Agents can discover each other, call tools, use skills, and delegate work via MCP/A2A — all over the distributed bus with zero configuration.

## Requirements

### v1.0 Complete ✅

- [x] **AGENT-01**: Agent struct — wraps LLM client (OpenAI-compatible), owns system prompt, message history, session
- [x] **AGENT-02**: Tool trait + registry — agents can call typed functions exposed as RPC endpoints on the bus
- [x] **AGENT-03**: MCP client integration — agents can connect to MCP servers, expose their tools locally
- [x] **AGENT-04**: A2A (Agent-to-Agent) protocol — agents discover each other on the bus and delegate tasks
- [x] **AGENT-05**: Skills system — composable, reusable agent capabilities (prompt templates + tools) loadable from config
- [x] **AGENT-06**: Scheduler — orchestrate multi-agent workflows (sequential, parallel, conditional branching)
- [x] **AGENT-07**: Agent session management — persist/restore agent state, conversation history across restarts

### v1.1 Pending

- [ ] **AGENT-08**: Streaming responses — stream LLM token output over the bus in real-time
- [ ] **AGENT-09**: Config-driven agents — define agents, skills, tools, and schedules from TOML/YAML/JSON

### Out of Scope

- LLM provider implementations — use existing clients (reqwest + OpenAI-compatible API)
- Python bindings — separate Phase 4 from BrickOS roadmap
- Persistence layer beyond session state (use a database for long-term memory)
- Built-in auth/authz — defer to Phase 3 of BrickOS
- Web UI or CLI tooling

## Context

**Existing foundation:**
- `crates/bus` — Zenoh pub/sub wrapper (PublisherWrapper, SubscriberWrapper, QueryWrapper, QueryableWrapper, RpcClient, RpcService) with rkyv serialization
- `crates/config` — ConfigLoader for multi-source config (file/directory/inline), TOML/YAML/JSON format support, merge strategies
- Zenoh provides the distributed communication substrate — peer-to-peer, zero-config discovery, any scale
- Workspace at `crates/` — new `crates/agent` (or `crates/brainos`) goes here

**Technical decisions made:**
- rkyv for serialization (zero-copy, unaligned buffers for Zenoh compatibility)
- rkyv_derive for Archive + Serialize + Deserialize
- Tokio for async runtime
- Builder pattern for agent/tool/skill construction

**The idea:** Instead of building agents as monoliths, build them as composable services on the bus. An agent is a QueryableWrapper that accepts task requests and responds with results. Tools are RpcClient calls. MCP is a local proxy. Skills are prompt templates loaded from config. The scheduler coordinates everything via bus messages.

## Milestone Status

### v1.0 ✅ Completed (2026-03-20)

**Scope**: Core Agent + Distributed Integration + Orchestration

Phases Delivered:
1. **Phase 1**: Core Agent (AGENT-01, AGENT-02)
   - Agent struct with LLM client wrapper
   - Tool trait + registry for RPC-based tool calls
   - 3 files created: `agent/mod.rs`, `tool.rs`, `registry.rs`
   - 37 tests passing

2. **Phase 2**: Distributed Integration (AGENT-03, AGENT-04, AGENT-05)
   - MCP client with server discovery and tool introspection
   - A2A protocol with agent discovery and delegation
   - Skills system with prompt templates + tool loading
   - 8 files created across MCP, A2A, skills modules
   - 41 tests passing

3. **Phase 3**: Orchestration & Persistence (AGENT-06, AGENT-07)
   - Workflow scheduler with DSL for sequential/parallel/conditional execution
   - Session persistence with JSON serialization and disk storage
   - 8 files created: 4 scheduler files, 4 session files
   - 51 tests passing

**Total**: 19 files created, 51 tests passing, 3 phases complete

**Technical Decisions**:
- Linear backoff formula: `(attempt + 1) * interval`
- Scheduler decoupled from Agent/A2A for flexibility
- Session storage with optional gzip compression via flate2
- Zenoh 1.8 API compatibility fixes applied

**Git Tag**: v1.0

### v1.1 🚧 Next Major Release

**Scope**: Streaming + Config-Driven Agents

Planned:
- AGENT-08: Streaming LLM responses over bus
- AGENT-09: TOML/YAML/JSON config for agents, skills, tools, schedules

---

- **Tech stack**: Rust only (workspace constraint)
- **No external AI SDK crates**: Use raw reqwest + OpenAI-compatible API
- **Bus dependency**: All agent communication goes through `bus` crate — agents are services on the bus
- **Config dependency**: Agent definitions come from `config` crate — no hardcoded agents
- **Serialization**: rkyv only (no serde for runtime types)

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Agents as bus services | Zenoh already gives discovery + transport — reuse it | — Pending |
| rkyv for agent messages | Zero-copy on bus, compatible with Zenoh buffers | — Pending |
| Tool = RpcClient call | Natural extension of RPC pattern, type-safe | — Pending |
| Skills as config blocks | Declarative, composable, no code changes to add a skill | — Pending |
| No AI SDK crates | Control dependencies, avoid abstraction lock-in | — Pending |

---
*Last updated: 2026-03-20 after v1.0 milestone completion*
