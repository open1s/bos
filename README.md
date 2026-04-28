English | [中文版本](./README-ZH.md)
# BrainOS (BOS)

A modular Rust-based operating system and runtime framework for building intelligent AI-powered applications with support for multi-agent coordination, event streaming, and extensible tool systems.

## Key Features

- **🤖 Agent Framework**: Multi-agent coordination with LLM integration and skill management
- **🚌 Event Bus**: High-performance pub/sub messaging with query/response, RPC patterns
- **⚙️ Configuration**: Flexible config loading from TOML, YAML, and environment variables
- **🧠 ReAct Engine**: Reasoning + Acting loop scaffold for AI agent workflows
- **🐍 Python Bindings**: `pip install brainos` - unified high-level Python API
- **📦 Node.js Bindings**: `npm install @open1s/jsbos` - unified high-level JavaScript API
- **🔄 Memory Persistence**: Cross-session memory support for agents
- **🔌 MCP Client**: Connect to Model Context Protocol servers

---

## Quick Start

### Python (brainos)

```python
from brainos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("What is 2+2?")
```

### JavaScript (@open1s/jsbos / brainos-js)

```javascript
const { BrainOS, tool } = require('@open1s/jsbos/brainos');

class AddTool {
  @tool('Add two numbers')
  add(a, b) {
    return a + b;
  }
}

const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').withTools(new AddTool());
const result = await agent.ask('What is 2+2?');
```

### Rust (agent crate)

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default().name("assistant");
let agent = Agent::builder().config(config).build()?;
let result = agent.run_simple("Hello").await?;
```

---

## Project Structure

```
bos/
├── crates/
│   ├── agent/          # AI agent framework with LLM, skills, tools, MCP
│   ├── bus/            # Pub/sub, queryable, caller/callable
│   ├── config/         # TOML/YAML config loading
│   ├── logging/        # Tracing and instrumentation
│   ├── react/          # ReAct reasoning engine
│   ├── pybos/          # Python bindings (brainos package)
│   │   └── brainos/    # High-level Python wrapper
│   └── jsbos/          # Node.js bindings (@open1s/jsbos)
│       └── brainos.js  # High-level JavaScript wrapper
├── docs/               # User guides
│   ├── python-user-guide.md
│   ├── javascript-user-guide.md
│   └── rust-user-guide.md
└── Cargo.toml          # Workspace
```

---

## Crates

| Crate | Description | Install |
|-------|-------------|---------|
| `agent` | Core agent with LLM integration, tools, skills, MCP | `cargo add agent` |
| `bus` | Pub/sub, query/response, RPC messaging | `cargo add bus` |
| `config` | Config loading from TOML, YAML, env vars | `cargo add config` |
| `logging` | Tracing and observability | `cargo add logging` |
| `react` | ReAct reasoning + acting engine | `cargo add react` |
| `pybos` | Python bindings | `pip install brainos` |
| `jsbos` | Node.js bindings | `npm install @open1s/jsbos` |

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

# Python bindings (low-level pybos)
cd crates/pybos && maturin develop

# Node.js bindings (low-level jsbos)
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

The `brainos` package (Python) and `@open1s/jsbos/brainos.js` (JavaScript) provide consistent high-level APIs:

| Feature | Python | JavaScript |
|---------|--------|------------|
| Import | `from brainos import BrainOS, tool` | `const { BrainOS, tool } = require('@open1s/jsbos/brainos')` |
| Create brain | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| Create agent | `brain.agent("name")` | `brain.agent("name")` |
| Fluent config | `.with_model("gpt-4")` | `.withModel("gpt-4")` |
| Register tools | `.register(tool)` | `.withTools(toolDef)` |
| Run | `await agent.ask("...")` | `await agent.ask("...")` |
| Bus factory | `BusManager()` | `BusManager.create()` |

### Low-level Bindings

For direct access to Rust bindings:

| Language | Package | Import |
|----------|---------|--------|
| Python | `pybos` | `from pybos import Agent, Bus, ...` |
| JavaScript | `@open1s/jsbos` | `const { Agent, Bus } = require('@open1s/jsbos')` |

---

## Configuration

Create `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

[bus]
mode = "peer"
listen = ["127.0.0.1:7890"]
```

Or use environment variables: `OPENAI_API_KEY`, `LLM_BASE_URL`, `LLM_MODEL`

---

## Examples

See the examples directories:

- Python: `crates/pybos/examples/`
- JavaScript: `crates/jsbos/examples/`

---

## License

MIT OR Apache-2.0

---

**Version**: 1.2.0 | **Last Updated**: 2026-04-28