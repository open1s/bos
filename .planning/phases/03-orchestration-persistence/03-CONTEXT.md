# Phase 03: Orchestration & Persistence

**Goal:** Multi-agent workflows and durable agent sessions.

## Context

Project context from upstream research and analysis:

**Completed:**
- Phase 1 (Core Agent): LLM client, agent reasoning loop, tool system, streaming
- Phase 2 (Distributed Integration): MCP bridge, A2A protocol, skills system, token streaming

**Current State:**
- Single agent can execute tasks with tools
- Agents can discover and delegate to other agents via A2A
- Skills system for composable capability
- Token streaming with backpressure

**What's Missing:**
- Multi-step workflow orchestration (sequential/parallel/conditional)
- Step timeout and retry with backoff
- Agent session persistence (save/restore conversation state)
- Session management (list/delete sessions)

## Technical Decisions

**Workflow Execution:**
```yaml
scheduler_type: hierarchical  # Recommended approach
parallel_execution: async tasks via tokio::spawn
branching: pattern match on JSON output
retry: exponential backoff with max retries
```

**Session Storage:**
```yaml
storage_format: JSON
location: .bos/sessions/{agent_id}.json
fields: message_log, state, metadata, timestamps
compression: optional gzip for large sessions
```

**Thread Safety:**
```yaml
session_manager: Arc<RwLock<SessionManager>>
state_access: RwLock for read-heavy workloads
persistence: async via tokio::fs
```

## Success Criteria

From ROADMAP.md Phase 3:

1. Sequential workflow: A → B → C passes output as input
2. Parallel workflow: A, B, C run simultaneously, collect results
3. Conditional branching: branch based on output pattern
4. Step timeout: configurable per step with retry (exponential backoff)
5. Session serialization: message history + context → JSON
6. Session restore: load from disk, continue conversation
7. Session management: list, delete by agent_id

## Requirements Coverage

| Requirement | Description |
|-------------|-------------|
| SCHD-01      | Sequential workflow execution (A → B → C) |
| SCHD-02      | Parallel workflow execution (A, B, C simultaneous) |
| SCHD-03      | Conditional branching based on output |
| SCHD-04      | Step timeout and retry with exponential backoff |
| SESS-01      | AgentState serialization to JSON |
| SESS-02      | Session restore from disk |
| SESS-03      | Session management (list/delete) |

## Integration Points

**Upstream Dependencies:**
- `crates/agent/src/agent/mod.rs` — Agent struct, MessageLog
- `crates/agent/src/llm/mod.rs` — LlmClient trait
- `crates/agent/src/a2a/client.rs` — A2A delegation (for workflow steps)

**Downstream Consumers:**
- CLI tooling (Phase 4+) — session list/restore commands
- Orchestrator daemon — workflow execution engine

## Design Patterns

**Scheduler:**
- Visitor pattern for workflow steps
- Builder pattern for workflow DSL
- Strategy pattern for step execution types

**Session Manager:**
- Repository pattern for session CRUD
- Builder pattern for session construction
- Observer pattern for session lifecycle events

## Anti-Patterns to Avoid

- ❌ Blocking executors — use async/await throughout
- ❌ Manual thread spawning — use tokio::task::spawn
- ❌ Session memory leaks — use weak references for long-lived sessions
- ❌ Infinite retry loops — enforce max_retries limit
- ❌ JSON parse errors — use rigorous validation on restore

## Open Questions

1. Should workflow steps support A2A delegation natively?
   **Decision:** Yes - workflow steps can be local (tools) or remote (A2A agents)

2. Session TTL/expiration?
   **Decision:** Add optional TTL field, implement cleanup task

3. Workflow DAG vs hierarchical?
   **Decision:** Start with hierarchical (simpler), DAG in v2

## Key Artifacts to Create

**Plan 03-01: Scheduler**
- `crates/agent/src/scheduler/mod.rs` — Scheduler, Workflow, Step types
- `crates/agent/src/scheduler/executor.rs` — Sequential/parallel/conditional execution
- `crates/agent/src/scheduler/retry.rs` — Exponential backoff
- `crates/agent/src/scheduler/dsl.rs` — Workflow builder DSL

**Plan 03-02: Session Persistence**
- `crates/agent/src/session/mod.rs` — Session, AgentState types
- `crates/agent/src/session/serializer.rs` — JSON serialization
- `crates/agent/src/session/manager.rs` — Session manager (CRUD)
- `crates/agent/src/session/storage.rs` — Disk I/O

## Next Steps

Proceed to Plan 03-01 (Scheduler) to implement workflow execution engine.
