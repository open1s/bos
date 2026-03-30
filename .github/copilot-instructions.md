# BrainOS (BOS) - Copilot Customization Instructions

> AI Agent guidance for the BrainOS Rust workspace. Links to full documentation rather than duplicating content.

## 🎯 Project Quick Context

**BrainOS** is a modular, event-driven Rust framework for building intelligent AI-powered applications with:
- Multi-agent coordination and LLM integration
- High-performance event streaming (pub/sub)
- Extensible tool systems and resilience patterns
- Cross-session memory persistence

**Repository**: https://github.com/open1s/bos  
**License**: MIT OR Apache-2.0  
**Edition**: 2021 | **Min Rust**: 1.70+

---

## ⚡ Essential Build & Test Commands

### Common Build Tasks
```bash
# Full workspace
cargo build --release                    # Release build (opt-level=3, LTO enabled)
cargo build --all                        # Debug build all crates

# Single crate
cargo build -p <crate-name>              # e.g., cargo build -p react

# Features
cargo build --all-features               # All optional features enabled
cargo build --no-default-features        # Bare minimum dependencies
```

### Testing
```bash
# All tests
cargo test --all                         # Integration + unit tests
cargo test --workspace                   # Alias for --all

# By crate
cargo test -p react --lib                # Unit tests only
cargo test -p agent --all                # All tests (integration + unit)

# Specific test
cargo test -p react test_name -- --nocapture

# Integration tests
cargo test --test '*'                    # Run all integration tests
```

### QA & Quality
```bash
cargo clippy --all                       # Lint checks
cargo fmt --all -- --check               # Format check
cargo test --all --doc                   # Doc tests

cargo bench --all                        # Criterion benchmarks
cargo flamegraph --bin <name>            # Profiling (requires flamegraph)
```

---

## 📁 Workspace Structure & Crate Boundaries

### Five Core Crates

| Crate | Purpose | Key Files |
|-------|---------|-----------|
| **`react`** | ReAct (Reasoning + Acting) AI engine | `src/engine.rs`, `src/llm.rs`, `src/memory.rs` |
| **`agent`** | Agent framework with skills & tools | `src/agent/`, `src/tools/`, `src/skills/` |
| **`bus`** | Event pub/sub messaging backbone | `src/publisher.rs`, `src/subscriber.rs`, `src/queryable.rs` |
| **`config`** | Configuration loading (TOML/YAML/env) | `src/loader.rs`, `src/types.rs` |
| **`logging`** | Tracing & observability setup | `src/lib.rs` (single-file crate) |

**Dependency Graph** (simplified):
```
react ──→ agent ──→ bus
     │      │        ↓
     ├─────→ config  ↓
     │       ↑      ↓
     └─────→ logging
```

**Key Principle**: Each crate has a single responsibility. Cross-crate communication goes through the bus.

---

## 🏗️ Architecture & Design Patterns

### Reference Documentation
- **Full Architecture**: [ARCHITECTURE.md](../../ARCHITECTURE.md) — System layers, data flows, patterns
- **README**: [README.md](../../README.md) — Project overview and getting started

### Core Patterns to Know

| Pattern | Where Used | Example |
|---------|-----------|---------|
| **Registry** | Tool/skill registration | `agent::tools::ToolRegistry` |
| **Factory** | Agent/session creation | `agent::AgentFactory` |
| **Strategy** | Tool execution variants | `DirectExecutor`, `CachedExecutor`, `CircuitBreakerExecutor` |
| **Decorator** | Wrapping tools with resilience | `CircuitBreakerTool<T>` |
| **State** | Circuit breaker, agent states | `CircuitBreakerState::{Closed, Open, HalfOpen}` |
| **Observer** | Event subscriptions | `bus::Subscriber` trait |

### Async-First Model
- **Runtime**: Tokio multi-threaded with work-stealing
- **Concurrency**: Async/await with spawned tasks
- **Communication**: Channel-based (async_channel, tokio::mpsc)
- **Waiting**: Always `.await` on futures; avoid blocking

### Resilience First
- **Circuit Breaker**: Fail-fast on repeated errors
- **Timeouts**: LLM calls and tool execution have configurable timeouts
- **Retries**: Exponential backoff with jitter for transient failures
- **Caching**: LRU cache with TTL for tool results

---

## 💻 Development Conventions

### Branch Naming
Follow BOS conventions from [CONTRIBUTING.md](../../CONTRIBUTING.md):
```
feature/description           # New feature
fix/description              # Bug fix
docs/description             # Documentation
refactor/description         # Code refactoring
test/description             # Test additions/fixes
chore/description            # Build/tool changes
```

### Code Organization
- **Module Structure**: Parallel to filesystem. Public items with doc comments.
- **Error Handling**: Use `thiserror` for custom errors, `anyhow::Result` for propagation.
- **Imports**: Use workspace dependencies from `Cargo.toml`.
- **Concurrency**: Use `tokio::spawn()` for independent work, channels for communication.

### Naming Conventions
- Crate names: `lowercase_with_underscores` (e.g., `circuit_breaker`)
- Traits/Structs: `PascalCase` (e.g., `ToolRegistry`, `LLMProvider`)
- Functions/Methods: `snake_case` (e.g., `execute_tool()`)
- Async methods: prefix rarely, but document `.await` requirement in doc comment
- Test functions: `test_<what_is_being_tested>()` or `<functionality>_test()`

### Comment Requirements
- **Public APIs**: Doc comments on all public items (`///`)
- **Complex Logic**: Inline comments explaining "why", not "what"
- **Safety (unsafe)**: SAFETY comment required (see [unsafe-checker skill](../../../.copilot/skills/unsafe-checker/SKILL.md))
- **TODO/FIXME**: Link to issue if exists: `// TODO: Issue #123 - description`

---

## 🧪 Testing Approach

### Test Organization
```
crates/<name>/
├── src/
│   ├── lib.rs           # Module definitions
│   ├── foo.rs           # Implementation
│   └── #[cfg(test)] mod tests { }  # Unit tests inline
└── tests/
    ├── integration.rs   # Integration tests
    └── fixtures/        # Test data, helper files
```

### Testing Patterns
- **Unit**: Test single functions/methods in-file with `#[test]`
- **Integration**: Test cross-crate interactions in `tests/` directory
- **Fixtures**: Use `tempfile`, `ctor` for setup, store data in `tests/fixtures/`
- **Async Tests**: Use `#[tokio::test]` macro
- **Mocking**: Create `mock_*.rs` files for test doubles

### Example
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_tool_with_timeout() {
        let tool = create_test_tool();
        let result = tool.execute(input).await;
        assert!(result.is_ok());
    }
}
```

---

## 📊 Key Files & Patterns

### Must-Know Files

| File | Purpose |
|------|---------|
| `Cargo.toml` (root) | Workspace definition, shared dependencies |
| `crates/*/Cargo.toml` | Per-crate configuration, local dependencies |
| `crates/*/src/lib.rs` | Crate public API & module tree |
| `crates/*/tests/*.rs` | Integration tests |
| `ARCHITECTURE.md` | System design, data flows, patterns |
| `README.md` | Project overview, getting started |
| `CONTRIBUTING.md` | Contribution guidelines (Chinese) |

### Common Implementation Patterns

**Adding a new tool:**
1. Create `crates/agent/src/tools/<tool_name>.rs`
2. Implement `Tool` trait
3. Register in `ToolRegistry`
4. Add tests in `tests/tool_policy_integration.rs`
5. Document in `ARCHITECTURE.md` if new pattern

**Adding a new agent skill:**
1. Create `crates/agent/src/skills/<skill_name>.rs`
2. Implement `Skill` trait
3. Register in `SkillRegistry`
4. Test cross-crate in integration tests

**Emitting events:**
```rust
let publisher = bus.create_publisher();
publisher.publish("topic/name", event_data).await?;
```

**Subscribing to events:**
```rust
let mut subscriber = bus.create_subscriber();
let receiver = subscriber.subscribe::<EventType>("topic/name");
while let Ok(event) = receiver.recv().await {
    process(event);
}
```

---

## 🔄 Common Development Tasks

### Adding a Feature to React Engine
1. Add method to `ReActEngine` struct in `crates/react/src/engine.rs`
2. Update `Prompt` generation if reasoning needed
3. Add integration test in `crates/react/tests/integration.rs`
4. Run: `cargo test -p react --all`

### Fixing Tool-Related Bugs
1. Locate tool impl in `crates/agent/src/tools/<name>.rs`
2. Check policy enforcement in `crates/agent/src/tools/policy.rs`
3. Verify circuit breaker logic in `crates/agent/src/tools/circuit_breaker.rs`
4. Add regression test
5. Run: `cargo test -p agent --all`

### Modifying Event Bus Contract
1. Update message types in `crates/bus/src/query.rs` or `codec.rs`
2. Update serialization logic
3. Run all tests: `cargo test --all`
4. Update `ARCHITECTURE.md` if new pattern

### Performance Optimization
1. Identify bottleneck with: `cargo flamegraph --bin <target>`
2. Check existing benchmarks: `crates/*/tests/bench_*.rs`
3. Add criterion benchmark: `cargo bench -p <crate>`
4. Verify optimization doesn't break tests

---

## 🚀 When to Call Subagents

Use the **Explore** subagent for complex codebase questions:

```
"Explore how circuit breaker state transitions work in the tool execution flow"
"Explore the memory persistence layer for cross-session state"
"Explore how the ReAct engine handles tool timeouts"
```

Suitable for:
- Understanding undocumented portions
- Finding where a specific concept is implemented
- Tracing data flow across multiple files
- Impact analysis before changes

---

## ⚠️ Common Pitfalls & Anti-Patterns

### Don't
- ❌ **Block in async code**: Use `.await` instead of `.wait()` or `.join()`
- ❌ **Ignore error types**: Use `thiserror` for custom errors, `?` for propagation
- ❌ **Clone on every value**: Prefer references, Arc, or channels
- ❌ **Unsafe without SAFETY**: All unsafe blocks need explanatory comments
- ❌ **Spawn unbounded tasks**: Bounded channels and semaphores for concurrency limits
- ❌ **Hardcode timeouts**: Use `CircuitBreakerConfig` for tunable timeouts
- ❌ **Skip tests for "simple" code**: Even simple tools need policy tests

### Do
- ✅ **Use workspace dependencies**: Add to `Cargo.toml`, re-export from workspace
- ✅ **Document trait requirements**: SAFETY, INVARIANTS, requirements in doc comments
- ✅ **Test error paths**: `#[should_panic]`, `assert!(result.is_err())`
- ✅ **Add observability**: Use `tracing::span!`, `tracing::instrument!`
- ✅ **Cache computed values**: Use LRU or memoization for expensive operations
- ✅ **Validate inputs**: Check policy before tool execution

---

## 📚 Related Skills & Resources

| Skill | When to Use |
|-------|------------|
| [rust-router](../../../.copilot/skills/rust-router/SKILL.md) | General Rust questions, errors, design decisions |
| [m04-zero-cost](../../../.copilot/skills/m04-zero-cost/SKILL.md) | Generics, traits, monomorphization |
| [m07-concurrency](../../../.copilot/skills/m07-concurrency/SKILL.md) | Thread safety, Send/Sync, deadlocks |
| [domain-web](../../../.copilot/skills/domain-web/SKILL.md) | HTTP tools, REST API integration |
| [unsafe-checker](../../../.copilot/skills/unsafe-checker/SKILL.md) | Raw pointers, FFI, unsafe Rust review |
| [m10-performance](../../../.copilot/skills/m10-performance/SKILL.md) | Benchmarking, profiling, optimization |

---

## 🎓 Suggested Next Prompts to Try

1. **"Explain the ReAct execution loop and how timeouts are handled"**
   → Explores reasoning + acting coordination

2. **"Help me implement a new tool for database queries"**
   → Practices tool registration and policy enforcement

3. **"Debug why the circuit breaker isn't tripping on failures"**
   → Deep dive into resilience patterns

4. **"How do I add metrics collection to the tool execution?"**
   → Observability and telemetry integration

---

## 🔧 Workspace-Specific Configuration

### Rust Toolchain
- **Edition**: 2021 (no 2024 migration yet)
- **Min Version**: 1.70+
- **Clippy**: Run before PRs
- **Fmt**: Enforced in CI

### Profile Settings
```toml
[profile.release]
opt-level = 3          # Maximum optimization
lto = true            # Link-time optimization
codegen-units = 1     # Single codegen unit for better optimization

[profile.bench]
inherits = "release"
debug = true          # Keep debug symbols for flame graphs
```

### Dependencies Strategy
- Workspace-wide versions in root `Cargo.toml` → All crates use same versions
- Path dependencies for internal crates → No version drift
- Minimal per-crate overrides → Prefer workspace defaults

---

## 📞 Support & Examples

- **Full examples**: See `crates/react/tests/integration.rs`, `crates/agent/tests/tool_policy_integration.rs`
- **Quick feedback**: Run tests incrementally as you develop
- **Documentation**: All patterns explained in [ARCHITECTURE.md](../../ARCHITECTURE.md)

---

**Last Updated**: 2026-03-31  
**Maintained by**: BrickOS Team
