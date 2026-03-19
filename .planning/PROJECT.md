# BrainOS Agent Framework

## What This Is

A distributed LLM agent development and scheduling framework built on BrainOS (Zenoh-based pub/sub bus + config system). Provides the primitives for building, composing, and orchestrating AI agents that communicate over the distributed bus. Think "agent orchestration layer" — agents are services, tools are RPC endpoints, skills are composable modules.

## Core Value

Agents can discover each other, call tools, use skills, and delegate work via MCP/A2A — all over the distributed bus with zero configuration.

## Requirements

### Active

- [ ] **AGENT-01**: Agent struct — wraps LLM client (OpenAI-compatible), owns system prompt, message history, session
- [ ] **AGENT-02**: Tool trait + registry — agents can call typed functions exposed as RPC endpoints on the bus
- [ ] **AGENT-03**: MCP client integration — agents can connect to MCP servers, expose their tools locally
- [ ] **AGENT-04**: A2A (Agent-to-Agent) protocol — agents discover each other on the bus and delegate tasks
- [ ] **AGENT-05**: Skills system — composable, reusable agent capabilities (prompt templates + tools) loadable from config
- [ ] **AGENT-06**: Scheduler — orchestrate multi-agent workflows (sequential, parallel, conditional branching)
- [ ] **AGENT-07**: Agent session management — persist/restore agent state, conversation history across restarts
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

## Constraints

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
*Last updated: 2026-03-19 after initialization*
