# BrainOS Onboarding Guide

A modular Rust-based operating system and runtime framework for building intelligent AI-powered applications with support for multi-agent coordination, event streaming, and extensible tool systems.

---

## Developer Experience

You consume BrainOS as a developer tool -- install it via pip or npm, then write code that creates agents, registers tools, and asks questions.

**Python**:

```python
from brainos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("What is 2+2?")
```

**JavaScript**:

```javascript
const { BrainOS, ToolDef } = require('brainos');

const addTool = new ToolDef('add', 'Add', (args) => args.a + args.b, ...);
const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').register(addTool);
const result = await agent.ask('What is 2+2?');
```

The Python and JavaScript APIs are intentionally consistent -- `brain.agent("name")`, `.with_model()` vs `.withModel()`, `.register()`, `await agent.ask()`. See [docs/python-user-guide.md](docs/python-user-guide.md) and [docs/javascript-user-guide.md](docs/javascript-user-guide.md) for full guides.

---

## How Is It Organized?

### System Architecture

```
  Application Code (Python/JS/Rust)
  |
  | Tool calls, agent.ask()
  v
+------------------------+
| Python/JS Bindings     |
| pybos / jsbos          |
+------------------------+
  |
  | Calls through bus
  v
+------------------------+
| ReAct Engine           |
| crates/react           |
+------------------------+
  |
  | Skills, Tools, LLM
  v
+----------------+------------------+
| Agent Framework | Tool Registry   |
| crates/agent    | (extensible)    |
+----------------+------------------+
  |
  | All cross-crate communication
  v
+------------------------+
| Event Bus              |
| crates/bus             |
| Pub/Sub, Query, RPC    |
+------------------------+
  |
  | Config, Logging, Memory
  v
+------------------------+
| Infrastructure Layer   |
| config/ logging/ react |
+------------------------+
```

### Crate Structure

```
bos/
├── crates/
│   ├── agent/      # Agent lifecycle, skills, tools, LLM providers
│   ├── bus/        # Pub/sub, query/response, RPC messaging
│   ├── config/     # TOML/YAML/env config loading
│   ├── logging/    # Tracing and observability
│   ├── react/      # ReAct reasoning + acting engine
│   ├── pybos/      # Python bindings (pip install brainos)
│   ├── jsbos/      # Node.js bindings (npm install brainos)
│   └── qserde/     # Serialization utilities
├── docs/           # User guides and API references
├── examples/       # Usage examples
└── Cargo.toml      # Workspace manifest
```

| Module | Responsibility |
|--------|----------------|
| `crates/agent/` | Agent lifecycle, skill registry, tool dispatch, LLM integration |
| `crates/bus/` | Async pub/sub, query/response patterns, RPC, session scoping |
| `crates/react/` | ReAct loop: reasoning, action dispatch, memory, resilience |
| `crates/config/` | Config file loading, env var overrides, schema validation |
| `crates/logging/` | Tracing spans, structured logs, metrics exporters |
| `crates/pybos/` | Python cding (maturin) exposing Rust API to Python |
| `crates/jsbos/` | Node.js bindings (NAPI-RS) exposing Rust API to JS |

### Crate Dependencies

All cross-crate communication flows through `bus`.

```
agent ──► bus, config, logging, react
pybos ──► agent, bus, config
jsbos ──► agent, bus, config
react  ──► agent, bus, config, logging
```

### External Integrations

| Dependency | What it's used for | Configured via |
|------------|-------------------|----------------|
| LLM Providers | OpenAI GPT, Anthropic Claude | `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` env vars |
| Tokio | Async runtime | Default in all crates |
| Zenoh | Distributed messaging | `ZENOH_CONFIG` env var (optional) |

This project appears self-contained with no required external service dependencies beyond LLM API keys.

---

## Key Concepts and Abstractions

| Concept | What it means in this codebase |
|---------|-------------------------------|
| `Agent` | An AI agent with lifecycle, tools, and memory |
| `Tool` | A callable function with name, description, execute logic |
| `Skill` | A composable unit of agent capability, loaded dynamically |
| `ToolRegistry` | Central registry mapping tool names to implementations |
| `Bus` | Event pub/sub system -- all crate communication flows through here |
| `Publisher` | Emits events to a topic |
| `Subscriber` | Receives events from a topic |
| `Queryable` | Request/response pattern on a topic |
| `ReAct Engine | Reasoning + Acting loop that orchestrates agent execution |
| `Session` | Isolated execution context with scoped bus communication |
| Circuit Breaker | Resilience pattern that fails fast after repeated failures |
| `ConfigLoader.discover()` | Auto-loads `~/.bos/conf/config.toml` |

### Architectural Patterns

- **Registry Pattern**: Tool and skill registration via `ToolRegistry` and `SkillRegistry`
- **Factory Pattern**: Agent creation through `Agent::builder()` with fluent config
- **Strategy Pattern**: Tool execution has pluggable strategies (direct, cached, circuit-broken)
- **Decorator Pattern**: Tools wrapped with circuit breakers, caching, timeouts

---

## Primary Flows

### Agent Ask Flow

```
User calls agent.ask("What is 2+2?")
|
v
Python/JS binding (pybos/jsbos)
|
v
ReAct Engine.run() - starts reasoning loop
|
v
LLM Call (OpenAI/Anthropic) - reasoning phase
|
v
Parse thought/action from LLM response
|
v
Tool Registry lookup by action name
|
v
Circuit breaker check, cache lookup
|
v
Tool.execute() with timeout
|
v
Memory update (store interaction)
|
v
Check stopping criteria or loop
|
v
Return result to user
```

### Tool Registration Flow

```
User defines function with @tool decorator (Python)
|
v
ToolDef created with name, description, function pointer
|
v
brain.agent("name").register(toolDef)
|
v
Tool added to ToolRegistry (HashMap<String, Arc<dyn Tool>>)
|
v
Agent uses tool at runtime via ReAct engine
```

---

## Developer Guide

### Setup

Prerequisites: Rust 1.70+, Python 3.10+ or Node.js 18+, maturin (for Python bindings)

```bash
# Clone and enter the project
git clone <repo> && cd bos

# Build all crates
cargo build --all

# Run tests
cargo test --all

# Lint
cargo clippy --all
cargo fmt --all

# Python bindings (from pybos directory)
cd crates/pybos && maturin develop

# Node.js bindings (from jsbos directory)
cd crates/jsbos && npm install && npm run build
```

### Running and Testing

```bash
# Test a single crate
cargo test -p agent

# Test with output (for async tests)
cargo test -p react -- --nocapture

# Debug logging
RUST_LOG=debug cargo test -p agent
```

### Version Control

This project uses **jj (jujutsu)** instead of git. Key commands:

```bash
jj status        # See working tree status
jj new           # Create a new change
jj describe -m "<crate>: <description>"  # Describe your change
jj log           # View the change stack
jj edit <change_id>   # Edit an existing change
jj squash        # Squash into parent change
jj rebase -r <change_id> -d <destination>  # Reorder changes
```

### Common Change Patterns

**Add a new tool**:
1. Implement `Tool` trait in `crates/agent/src/tools/`
2. Register in `ToolRegistry::register()`
3. Add tests in the same crate

**Add a new LLM provider**:
1. Implement `LLMProvider` trait in `crates/agent/src/llm/`
2. Add provider selection logic in the ReAct engine

**Add a new crate**:
1. Create directory in `crates/`
2. Add to workspace members in `Cargo.toml`
3. Add dependencies in workspace section

### Key Files to Start With

| Area | File | Why |
|------|------|-----|
| Agent core | `crates/agent/src/agent/` | Core agent traits and builder |
| Tool system | `crates/agent/src/tools/` | Tool registration and execution |
| ReAct loop | `crates/react/src/engine.rs` | Main reasoning + acting orchestration |
| Event bus | `crates/bus/src/` | Pub/sub, query, RPC patterns |
| Config loading | `crates/config/src/loader.rs` | TOML/YAML/env config |

### Practical Tips

- All cross-crate calls go through `bus` -- don't import crates directly in tight loops
- Use `ConfigLoader.discover()` to auto-load config from `~/.bos/conf/config.toml`
- For async tests, use `#[tokio::test]` and run with `--nocapture` to see output
- The `react` crate depends on `agent`, so changes to agent may require rebuilds of react

---

## Documentation

- [README.md](README.md) -- Project overview and quick start
- [ARCHITECTURE.md](ARCHITECTURE.md) -- Detailed system architecture (747 lines)
- [docs/python-user-guide.md](docs/python-user-guide.md) -- Python API guide
- [docs/javascript-user-guide.md](docs/javascript-user-guide.md) -- JavaScript API guide
- [docs/rust-user-guide.md](docs/rust-user-guide.md) -- Rust API guide
- [docs/solutions/](docs/solutions/) -- Documented patterns, best practices, bug fixes