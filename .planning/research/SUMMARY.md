# Project Research Summary

**Project:** BOS - Distributed LLM Agent Orchestration Framework
**Domain:** Distributed Systems / AI Agent Framework (Rust/Zenoh)
**Researched:** 2026-03-19
**Confidence:** MEDIUM-HIGH

## Executive Summary

This project builds a distributed LLM agent orchestration framework in Rust using Zenoh as the communication substrate. Unlike single-agent frameworks, this architecture treats agents as services on a pub/sub bus where they can discover each other, delegate tasks, and coordinate workflows without a central coordinator. The key insight from research: the "bus-native" approach means zero-configuration multi-agent systems where agents are QueryableWrappers on Zenoh and tools are RpcClients.

The recommended approach is a minimal, custom implementation avoiding external AI SDK crates (using raw reqwest for HTTP), with rkyv for zero-copy serialization on the bus. Build the LLM client trait first—provider abstraction is the critical foundation that everything else depends on. The top risks are mega-prompts breaking production behavior, tight coupling to specific LLM providers, and underestimating distributed coordination complexity in A2A protocols.

## Key Findings

### Recommended Stack

**Core technologies:**
- **reqwest** — HTTP client for OpenAI-compatible API calls. Single Client instance with connection pooling, tokio runtime for async.
- **rkyv** — Zero-copy deserialization for agent messages on the Zenoh bus. Already in workspace.
- **tokio + tokio_stream** — Async runtime and SSE streaming handling. Industry standard in Rust.
- **serde_json** — JSON serialization for LLM API calls (not for bus messages—use rkyv only).
- **Zenoh (via bus crate)** — Distributed communication substrate, already dependency.

**What NOT to use and why:**
| Library | Reason |
|---------|--------|
| `llm` (graniet) | Too heavy; build minimal client |
| `cloudllm` | Bundles everything; build lean |
| `langchain-rust` | Chain-centric, doesn't fit bus architecture |
| `eventsource-stream` | SSE parsing simple enough to hand-roll |
| `async-openai` | OpenAI-only; need multi-provider |

### Expected Features

**Must have (table stakes):**
- **Agent primitive** — Configurable agent with message history, reasoning loop (AGENT-01)
- **Tool calling** — Define tools with JSON Schema, LLM decides when to call (AGENT-02)
- **LLM provider abstraction** — OpenAI-compatible + at least one other (AGENT-01)
- **Streaming** — Token-by-token async streams (AGENT-08)
- **Config-driven setup** — TOML/YAML agent definitions (AGENT-09)

**Should have (differentiators):**
- **A2A protocol** — Agent-to-agent delegation via Zenoh, task state machine (AGENT-04)
- **Skills system** — Composable prompt fragments + tools from config (AGENT-05)
- **MCP integration** — Bridge MCP STDIO servers to bus RPC (AGENT-03)
- **Workflow scheduling** — Sequential, parallel, conditional multi-agent orchestration (AGENT-06)
- **Session persistence** — Serialize/restore agent state (AGENT-07)

**Defer (v2+):**
- Built-in vector database
- UI/CLI tooling
- Multi-modal (vision, audio)
- Agent memory beyond session
- Python bindings

### Architecture Approach

The architecture treats agents as services on Zenoh. Agent Core owns the reasoning loop, Tool System defines and executes tools, MCP Bridge wraps STDIO MCP servers as bus RPC, A2A Protocol handles agent-to-agent delegation, and Scheduler orchestrates multi-step workflows.

**Major components:**
1. **Agent Core** — Reasoning loop: receive task → call LLM → parse → handle tools → return
2. **Tool System** — Tool trait + registry + schema translator for provider differences
3. **MCP Bridge** — JSON-RPC over STDIO → bus RPC proxy
4. **A2A Protocol** — Task state machine (Submitted→Working→Completed/Failed/InputRequired)
5. **Scheduler** — Sequential/Parallel/Conditional workflow execution

### Critical Pitfalls

1. **Mega-prompts** — LLMs don't execute 500 instructions reliably. Keep prompts under 2000 tokens, use Skills to compose capabilities.
2. **Provider lock-in** — Hardcoding OpenAI assumptions. Build LlmClient trait from day one.
3. **Poor streaming** — Blocking or silent failures. Implement true streaming with backpressure from the start.
4. **Schema mismatches** — JSON validation failures causing endless loops. Add validation with recovery and provider-specific translation.
5. **A2A state ignoring** — No timeout, no task tracking, silent failures. Implement full state machine with explicit failure states.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: Core Agent Foundation
**Rationale:** Every other feature depends on having a working agent. Can't build A2A without agents, can't test MCP without tool system.
**Delivers:** LlmClient trait, Tool trait + registry, Agent core, Basic streaming, Config-driven loading.
**Addresses:** AGENT-01, AGENT-02, AGENT-08, AGENT-09
**Avoids:** Pitfall 1 (mega-prompts via skill composition), Pitfall 2 (provider lock-in), Pitfall 4 (schema mismatches)

### Phase 2: Distributed Integration
**Rationale:** Once agents exist, integrate them with the distributed bus and external protocols.
**Delivers:** MCP bridge, A2A protocol, Skills system.
**Addresses:** AGENT-03, AGENT-04, AGENT-05
**Avoids:** Pitfall 3 (poor streaming), Pitfall 5 (MCP scale issues), Pitfall 6 (A2A state), Pitfall 7 (skill conflicts)
**Research Flag:** A2A protocol is newer (late 2024/2025) — may need deeper research on failure handling patterns.

### Phase 3: Orchestration & Persistence
**Rationale:** Multi-agent workflows and durability depend on working single agents and A2A.
**Delivers:** Scheduler, Session persistence, Error handling polish.
**Addresses:** AGENT-06, AGENT-07
**Avoids:** A2A task stuck in "working" state, session loss on restart

### Phase Ordering Rationale
- **LLM Client first** — Provider abstraction is the foundation; everything depends on it
- **Agent Core second** — Must have single-agent working before distributed coordination
- **Integrations parallel** — MCP, Skills, Streaming can be developed somewhat independently once Agent Core exists
- **Orchestration last** — Scheduler requires A2A working first

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 2 (A2A Protocol):** Protocol is still evolving (late 2024/2025), need to validate failure handling patterns
- **Phase 2 (MCP Integration):** MCP was designed for single-machine use; bridging to distributed bus is non-trivial

Phases with standard patterns (skip research-phase):
- **Phase 1 (LLM Client):** reqwest + tokio streams is industry standard
- **Phase 3 (Scheduler):** Sequential/parallel/conditional is well-understood pattern

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | reqwest, tokio, rkyv are proven technologies already in workspace |
| Features | MEDIUM-HIGH | Clear feature boundaries, well-documented dependencies |
| Architecture | MEDIUM-HIGH | Component boundaries solid, build order validated by dependencies |
| Pitfalls | HIGH | Researched from multiple industry sources (2025-2026) |

**Overall confidence:** MEDIUM-HIGH

### Gaps to Address

- **A2A failure handling:** Protocol is new, limited production examples. Validate during Phase 2 implementation.
- **MCP at scale:** MCP wasn't designed for distributed systems. May need design iteration.
- **Provider schema translation:** Each LLM has different tool format. Need to validate translation logic with 2+ providers.

## Sources

### Primary (HIGH confidence)
- Zenoh documentation — bus communication substrate
- reqwest crate docs — HTTP client patterns
- rkyv crate docs — zero-copy serialization

### Secondary (MEDIUM confidence)
- Anthropic A2A Protocol spec — agent-to-agent patterns
- MCP Protocol spec — JSON-RPC over STDIO
- cloudllm crate (study only, don't depend) — provider abstraction patterns

### Tertiary (LOW confidence)
- Industry reports on agent failures (Composio 2025, Softcery 2025) — patterns but need validation

---

*Research completed: 2026-03-19*
*Ready for roadmap: yes*