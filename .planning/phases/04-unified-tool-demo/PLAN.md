# Phase 4: Unified Tool Demo - Multi-Agent LLM Integration

**Goal:** Create a comprehensive demo showing multiple agents using LLM to call tools across different sources (local, RPC, MCP, A2A) and complete a task via Skills.

---

## Overview

This phase demonstrates the unified tool calling architecture through a practical multi-agent scenario:

1. **Alice Agent** - Coordinator with LLM, discovers and calls tools from other agents
2. **Bob Agent** - Tool provider, exposes calculator tools via RPC
3. **Charlie Agent** - Skill executor, uses skills to complete coding tasks
4. **Task:** Generate Python quick sort implementation via Skill system

---

## Requirements

- [x] AGENT-01: Agent struct (from Phase 1)
- [x] AGENT-02: Tool registry (from Phase 1)
- [x] AGENT-04: A2A protocol (from Phase 2)
- [x] AGENT-05: Skills system (from Phase 2)
- [ ] AGENT-10: Unified tool discovery and registration
- [ ] AGENT-11: Multi-agent LLM coordination

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Alice (Coordinator)                      │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ LLM (GPT-4) ←→ UnifiedToolRegistry                      │   │
│  │                    ├── Local tools (add, concat)        │   │
│  │                    ├── RPC tools (from Bob)             │   │
│  │                    ├── MCP tools (filesystem)           │   │
│  │                    └── A2A tools (from Charlie)         │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ Zenoh Bus
                              │
        ┌─────────────────────┴─────────────────────┐
        │                                           │
        ▼                                           ▼
┌───────────────────┐                     ┌───────────────────┐
│   Bob (Provider)  │                     │ Charlie (Skill)   │
│                   │                     │                   │
│ ┌───────────────┐ │                     │ ┌───────────────┐ │
│ │ RPC Service   │ │                     │ │ Skill: coder  │ │
│ │ - add(a, b)   │ │                     │ │ - generate()  │ │
│ │ - mul(a, b)   │ │                     │ │ - execute()   │ │
│ └───────────────┘ │                     │ └───────────────┘ │
│                   │                     │                   │
│ Tools exposed via │                     │ A2A capability:   │
│ agent/bob/tools/* │                     │ "code-generation" │
└───────────────────┘                     └───────────────────┘
```

---

## Plan 04-01: Unified Tool Discovery & Registration

### Goal
Implement complete unified tool discovery system that aggregates tools from all sources.

### Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `examples/unified-tool-demo/Cargo.toml` | Update | Add all dependencies |
| `examples/unified-tool-demo/src/bin/alice.rs` | Create | Coordinator agent |
| `examples/unified-tool-demo/src/bin/bob.rs` | Create | Tool provider agent |
| `examples/unified-tool-demo/src/bin/charlie.rs` | Create | Skill executor agent |
| `examples/unified-tool-demo/src/common/mod.rs` | Create | Shared utilities |
| `examples/unified-tool-demo/src/common/tool_setup.rs` | Create | Tool registration helpers |

### Test Cases

| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Local tool registration | Register FunctionTool | Tool callable | `cargo test --bin alice test_local_tool` |
| RPC discovery | Start Bob, discover | Tools found | `cargo test --bin alice test_rpc_discovery` |
| A2A discovery | Start Charlie, discover | Capabilities found | `cargo test --bin alice test_a2a_discovery` |
| Unified registry | All sources | All tools registered | `cargo test test_unified_registry` |

### Tasks

1. **Create shared utilities module**
   - `common/mod.rs` - Module exports
   - `common/tool_setup.rs` - Tool registration helpers

2. **Create Bob agent (Tool Provider)**
   - Expose calculator tools via RPC
   - Announce tools for discovery
   - Handle tool execution requests

3. **Create Charlie agent (Skill Executor)**
   - Load coder skill
   - Announce via A2A discovery
   - Handle task delegation requests

4. **Create Alice agent (Coordinator)**
   - Connect LLM client
   - Discover all tool sources
   - Register unified tools
   - Run LLM loop with tool calling

---

## Plan 04-02: Multi-Agent LLM Coordination

### Goal
Demonstrate LLM-driven multi-agent coordination with tool calling.

### Task Scenario

**User Request:** "Use the code generation skill to write a Python quick sort implementation"

**Expected Flow:**
```
Alice receives request
    │
    ▼
LLM analyzes: needs skill execution
    │
    ▼
Alice calls A2A tool: a2a/charlie/code_generate
    │
    ▼
Charlie receives task via A2A
    │
    ▼
Charlie uses skill: coder.generate("python quicksort")
    │
    ▼
Charlie returns result to Alice
    │
    ▼
Alice formats response to user
```

### Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `examples/unified-tool-demo/src/skills/coder/mod.rs` | Create | Code generation skill |
| `examples/unified-tool-demo/src/skills/coder/generate.rs` | Create | Python code generation |
| `examples/unified-tool-demo/skills/coder.toml` | Create | Skill configuration |

### Test Cases

| Case | Input | Expected | Verification |
|------|-------|----------|--------------|
| Skill loading | Load coder skill | Skill available | `cargo test test_skill_load` |
| Code generation | "python quicksort" | Valid Python code | `cargo test test_code_gen` |
| A2A delegation | Task to Charlie | Response received | `cargo test test_a2a_call` |
| E2E flow | Full request | Quick sort code | Manual: run demo |

---

## Success Criteria

1. **Discovery Working**
   - [ ] Local tools registered and callable
   - [ ] RPC tools discovered from Bob
   - [ ] A2A capabilities discovered from Charlie
   - [ ] Unified registry aggregates all sources

2. **Tool Calling Working**
   - [ ] LLM can call local tools
   - [ ] LLM can call RPC tools (via BusToolClient)
   - [ ] LLM can delegate to A2A agent

3. **Skill System Working**
   - [ ] Skill loaded successfully
   - [ ] Skill generates valid Python code
   - [ ] Skill result returned to user

4. **End-to-End Demo**
   - [ ] Three agents running simultaneously
   - [ ] Alice receives user request
   - [ ] Alice discovers and calls appropriate tools
   - [ ] Charlie generates quick sort code
   - [ ] Result returned to user

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
2. `feat(demo): implement Bob agent with RPC tool server`
3. `feat(demo): implement Charlie agent with skill executor`
4. `feat(demo): implement Alice agent with LLM coordination`
5. `feat(demo): add coder skill for Python generation`
6. `test(demo): add integration tests for multi-agent flow`
7. `docs(demo): add README with usage instructions`

---

## How to Run

```bash
# Terminal 1: Start Zenoh router
zenohd

# Terminal 2: Start Bob (Tool Provider)
cd examples/unified-tool-demo
cargo run --bin bob

# Terminal 3: Start Charlie (Skill Executor)
cd examples/unified-tool-demo
cargo run --bin charlie

# Terminal 4: Start Alice (Coordinator with LLM)
cd examples/unified-tool-demo
export OPENAI_API_KEY="your-key"
cargo run --bin alice
```

---

## Expected Output

```
=== Alice Agent Started ===
Discovering tools from all sources...
  - Local: add, concat
  - RPC (bob): rpc/bob/add, rpc/bob/multiply
  - A2A (charlie): a2a/charlie/code_generate

User: Use the code generation skill to write a Python quick sort implementation

Alice: I'll use Charlie's code generation capability to create that for you.
[Calling a2a/charlie/code_generate with {"language": "python", "task": "quicksort"}]

Result:
```python
def quicksort(arr):
    if len(arr) <= 1:
        return arr
    pivot = arr[len(arr) // 2]
    left = [x for x in arr if x < pivot]
    middle = [x for x in arr if x == pivot]
    right = [x for x in arr if x > pivot]
    return quicksort(left) + middle + quicksort(right)
```

This quick sort implementation uses Python list comprehensions...
```
