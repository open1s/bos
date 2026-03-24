# Phase 4: Unified Tool Demo - Tool Definition Patterns

**Goal:** Create a comprehensive demo showing multi-agent LLM coordination with tools from multiple sources (local, RPC, function, A2A, skills) in the bos framework.

---

## Overview

This phase demonstrates multi-agent LLM coordination with tools from multiple sources in the bos framework:

1. **Single binary** with coordinator and provider roles spawned as async tasks via `tokio::spawn`
2. **Coordinator role** - Discovers tools from all sources, calls them via LLM
3. **Provider role** - Exposes tools via RPC and A2A
4. **Tool types demonstrated**: Local (full Tool trait), RPC (service discovery), Function (FunctionTool), A2A (full workflow)
5. **Skills system**: Demonstrateates the use of skills to extend the capabilities of the LLM

---

## Requirements

- [x] AGENT-01: Agent struct (from Phase 1)
- [x] AGENT-02: Tool registry (from Phase 1)
- [x] AGENT-04: A2A protocol (from Phase 2)
- [x] AGENT-05: Skills system (from Phase 2)
- [x] AGENT-10: Unified tool discovery and registration
- [x] AGENT-11: Multi-agent LLM coordination

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Single Binary (unified-tool-demo) │ │
│ ┌─────────────────────────────────────────────────────────┐ │ │
│ │ tokio::spawn(coordinator_task) │ │ │
│ │ ┌─────────────────────────────────────────────────────┐ │ │ │
│ │ │ Coordinator │ │ │ │ │
│ │ │ LLM ←→ UnifiedToolRegistry │ │ │ │ │
│ │ │ ├── Local (Tool trait impl) │ │ │ │ │
│ │ │ ├── RPC (ZenohRpcDiscovery) │ │ │ │ │
│ │ │ ├── Function (FunctionTool) │ │ │ │ │
│ │ │ └── A2A (A2AToolDiscovery) │ │ │ │ │
│ │ └─────────────────────────────────────────────────────┘ │ │ │
│ └─────────────────────────────────────────────────────────┘ │ │
│ │ │
│ tokio::spawn(provider_task) │ │
│ ┌─────────────────────────────────────────────────────────┐ │ │
│ │ Provider │ │ │
│ │ ┌───────────────┐ ┌───────────────┐ │ │ │
│ │ │ RPC Service │ │ A2A Handler │ │ │ │
│ │ │ - add, mul │ │ - code_gen │ │ │ │
│ │ └───────────────┘ └───────────────┘ │ │ │
│ └─────────────────────────────────────────────────────────┘ │ │
└─────────────────────────────────────────────────────────────────┘
│ Zenoh Bus
└─────────────────────────────────────────────────────────────────┘
```

---

## Plan 04-01: Unified Tool Discovery & Registration

### Goal
Implement complete unified tool discovery system that aggregates tools from all sources.

### Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `examples/unified-tool-demo/Cargo.toml` | Create | Demo project with dependencies |
| `examples/unified-tool-demo/src/main.rs` | Create | Entry point, spawns both roles |
| `examples/unified-tool-demo/src/lib.rs` | Create | Shared code, tool definitions |
| `examples/unified-tool-demo/src/roles/mod.rs` | Create | Role module |
| `examples/unified-tool-demo/src/roles/coordinator.rs` | Create | Coordinator task with UnifiedToolRegistry |
| `examples/unified-tool-demo/src/roles/provider.rs` | Create | Provider task with RPC + A2A services |

### Test Cases

| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Local tool registration | Register FunctionTool | Tool callable | `cargo test --bin alice test_local_tool` |
| RPC discovery | Start Bob, discover | Tools found | `cargo test --bin alice test_rpc_discovery` |
| A2A discovery | Start Charlie, discover | Capabilities found | `cargo test --bin alice test_a2a_discovery` |
| Unified registry | All sources | All tools registered | `cargo test test_unified_registry` |

### Tasks

1. **Create project structure**
   - `Cargo.toml` - Dependencies: agent, bus, tokio, async-trait
   - `src/lib.rs` - Tool definitions (local, function tools)

2. **Create coordinator role**
   - Uses `UnifiedToolRegistry` with auto-discovery
   - Discovers Local, ZenohRpc, A2A tools
   - Shows full A2A workflow (discovery + delegation + response)
   - Calls tools via LLM

3. **Create provider role**
   - Exposes local tools via RPC service (ZenohRpcDiscovery)
   - Exposes tools via A2A protocol
   - Demonstrates service discovery integration

4. **Create main entry point**
   - Spawns both roles via `tokio::spawn`
   - Shows concurrent execution pattern
   - Unified error handling

---

## Success Criteria

1. **Tool Definition Patterns Demonstrated**
   - [ ] Local tools with full Tool trait implementation
   - [ ] RPC tools with service discovery integration
   - [ ] Function tools with FunctionTool::numeric() and custom schema
   - [ ] A2A tools with full workflow (discovery + delegation + response)

2. **Discovery Working**
   - [ ] Local tools registered and callable
   - [ ] RPC tools discovered via ZenohRpcDiscovery
   - [ ] A2A capabilities discovered via A2AToolDiscovery
   - [ ] UnifiedToolRegistry aggregates all sources

3. **Demo Structure**
   - [ ] Single binary with both roles
   - [ ] Roles spawned via tokio::spawn
   - [ ] Concurrent execution works

4. **Error Handling**
   - [ ] Unified error wrapper shows source
   - [ ] Errors from all tool types handled gracefully

---

## Potential Issues to Address

1. **Async Coordination**
   - Problem: Multiple agents need to start before discovery
   - Solution: Startup sequence with health checks

2. **Tool Name Collision**
   - Problem: Multiple sources may have same tool name
   - Solution: Namespace prefix (local/, rpc/bob/, a2a/charlie/)

3. **Timeout Handling**
   - Problem: A2A calls may timeout
   - Solution: Configurable timeout with retry

4. **Error Propagation**
   - Problem: Errors from remote tools need clear messages
   - Solution: Structured error types with source info

5. **LLM Context Limit**
   - Problem: Too many tools exceed context
   - Solution: Progressive tool discovery

---

## Atomic Commits

1. `feat(demo): create unified-tool-demo project structure`
2. `feat(demo): implement local tool with Tool trait`
3. `feat(demo): implement RPC tool with service discovery`
4. `feat(demo): implement function tool patterns`
5. `feat(demo): implement A2A tool workflow`
6. `feat(demo): create coordinator with UnifiedToolRegistry`
7. `feat(demo): create provider with RPC + A2A services`
8. `feat(demo): wire up tokio::spawn for both roles`
9. `test(demo): add tool discovery and calling tests`

---

## How to Run

```bash
# Terminal 1: Start Zenoh router
zenohd

# Terminal 2: Run the demo (single binary with both roles)
cd examples/unified-tool-demo
export OPENAI_API_KEY="your-key"
cargo run
```

---

## Expected Output

```
=== Unified Tool Demo Started ===
Spawning coordinator and provider tasks via tokio::spawn...

[Provider] RPC service started: agent/demo/tools
[Provider] A2A handler started: agent/demo/a2a

[Coordinator] UnifiedToolRegistry initialized
[Coordinator] Discovering tools from all sources...
[Coordinator] - Local: add, multiply (Tool trait impl)
[Coordinator] - RPC: rpc/demo/add, rpc/demo/multiply (discovered)
[Coordinator] - Function: echo, greeting (FunctionTool)
[Coordinator] - A2A: a2a/demo/code_generate (discovered)

[Coordinator] All tools registered: 8 total

Tool call test:
- Calling local::add(5, 3) = 8
- Calling rpc::demo/multiply(4, 7) = 28
- Calling function::echo("hello") = "hello"
- Calling a2a::demo/code_generate(...) = "..."
```
