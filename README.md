English | [中文版本](./README-ZH.md)
# BrainOS (BOS)

A modular Rust-based operating system and runtime framework for building intelligent AI-powered applications with support for multi-agent coordination, event streaming, and extensible tool systems.

## Key Features

- **🤖 Agent Framework**: Multi-agent coordination with LLM integration and skill management
- **🚌 Event Bus**: High-performance pub/sub messaging with query/response, RPC patterns
- **⚙️ Configuration**: Flexible config loading from TOML, YAML, and environment variables
- **🧠 ReAct Engine**: Reasoning + Acting loop scaffold for AI agent workflows
- **🐍 Python Bindings**: `pip install brainos` - unified Python API
- **📦 Node.js Bindings**: `npm install brainos` - unified JavaScript API
- **🔄 Memory Persistence**: Cross-session memory support for agents

---

## Quick Start

### Rust

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default().name("assistant");
let agent = Agent::builder().config(config).build()?;
let result = agent.run_simple("Hello").await?;
```

### Python

```python
from brainos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("What is 2+2?")
```

### JavaScript

```javascript
const { BrainOS, ToolDef } = require('brainos');

const addTool = new ToolDef('add', 'Add', (args) => args.a + args.b, ...);
const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').register(addTool);
const result = await agent.ask('What is 2+2?');
```

---

## Project Structure

```
bos/
├── crates/
│   ├── agent/      # AI agent framework with LLM, skills, tools
│   ├── bus/        # Pub/sub, queryable, caller/callable
│   ├── config/     # TOML/YAML config loading
│   ├── logging/    # Tracing and instrumentation
│   ├── react/      # ReAct reasoning engine
│   ├── pybos/      # Python bindings (brainos package)
│   └── jsbos/      # Node.js bindings (brainos package)
├── docs/           # User guides
│   ├── python-user-guide.md
│   ├── javascript-user-guide.md
│   └── rust-user-guide.md
└── Cargo.toml      # Workspace
```

---

## Crates

| Crate | Description |
|-------|-------------|
| `agent` | Core agent with LLM integration, tools, skills, MCP |
| `bus` | Pub/sub, query/response, RPC messaging |
| `config` | Config loading from TOML, YAML, env vars |
| `logging` | Tracing and observability |
| `react` | ReAct reasoning + acting engine |
| `pybos` | Python bindings (`pip install brainos`) |
| `jsbos` | Node.js bindings (`npm install brainos`) |

---

## Commands

```bash
# Build all
cargo build --all

# Test all
cargo test --all

# Lint
cargo clippy --all
cargo fmt --all

# Python bindings
cd crates/pybos && maturin develop

# Node.js bindings
cd crates/jsbos && npm install && npm run build
```

---

## User Guides

- **Python**: [docs/python-user-guide.md](docs/python-user-guide.md)
- **JavaScript**: [docs/javascript-user-guide.md](docs/javascript-user-guide.md)  
- **Rust**: [docs/rust-user-guide.md](docs/rust-user-guide.md)
- **中文**: [README-ZH.md](README-ZH.md)

---

## Unified API

Python and JavaScript APIs are consistent:

| Feature | Python | JavaScript |
|---------|--------|------------|
| Create agent | `brain.agent("name")` | `brain.agent("name")` |
| Fluent config | `.with_model("gpt-4")` | `.withModel("gpt-4")` |
| Register tools | `.register(tool)` | `.register(toolDef)` |
| Run | `await agent.ask("...")` | `await agent.ask("...")` |
| Bus factory | `BusManager()` | `BusManager.create()` |

---

## License

MIT OR Apache-2.0

---

**Version**: 0.1.0 | **Last Updated**: 2026-04-05