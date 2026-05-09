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
- **🔌 MCP Client**: Connect to Model Context Protocol servers (stdio & HTTP)
- **📚 Skills System**: Load agent capabilities from directory-based skill definitions

---

## Quick Start

### Python (brainos)

```python
from brainos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").with_tools(add)
    result = await agent.ask("What is 2+2?")
```

### JavaScript (@open1s/jsbos / brainos-js)

```javascript
const { BrainOS, ToolDef } = require('@open1s/jsbos/brainos');

// Create tool using ToolDef
const addTool = new ToolDef(
  'add',
  'Add two numbers',
  (args) => (args.a || 0) + (args.b || 0),
  { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
  { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] }
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent('assistant')
  .register(addTool)
  .start();
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

## Skills System

Agents can load capabilities from directory-based skill definitions:

```python
# Create skills directory with skill folders
skills_dir = "/path/to/skills"
agent.register_skills_from_dir(skills_dir)
```

### Skill Format

Each skill is a directory containing a `SKILL.md` file with YAML frontmatter:

```
skills/
├── python-coding/
│   └── SKILL.md
├── api-design/
│   └── SKILL.md
└── database-ops/
    └── SKILL.md
```

**SKILL.md format:**
```markdown
---
name: python-coding
description: Python coding conventions for this project
category: coding
version: 1.0.0
---

# Python Coding Conventions

Your skill instructions here...
```

The agent's LLM receives available skills in the system prompt and can call `load_skill` to retrieve full instructions.

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
| Import | `from brainos import BrainOS, tool` | `const { BrainOS, ToolDef } = require('@open1s/jsbos/brainos')` |
| Create brain | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| Create agent | `brain.agent("name")` | `brain.agent("name")` |
| Fluent config | `.with_model("gpt-4")` | `.withModel("gpt-4")` |
| Register tools | `.with_tools(tool)` | `.register(toolDef)` |
| Run | `await agent.ask("...")` | `await agent.ask("...")` |
| Bus factory | `BusManager()` | `BusManager.create()` |

### Low-level Bindings

For direct access to Rust bindings:

| Language | Package | Import |
|----------|---------|--------|
| Python | `pybos` | `from pybos import Agent, Bus, McpClient, ...` |
| JavaScript | `@open1s/jsbos` | `const { Agent, Bus, McpClient } = require('@open1s/jsbos')` |

---

## MCP Client

Connect to MCP servers via stdio or HTTP transport:

### Python

```python
from pybos import McpClient

# Process-based server
client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
await client.initialize()

# HTTP server
client = McpClient.connect_http("http://127.0.0.1:8000/mcp")
await client.initialize()

# Use tools
tools = await client.list_tools()
result = await client.call_tool("echo", '{"text": "hello"}')
```

### JavaScript

```javascript
const { McpClient } = require('@open1s/jsbos');

// Process-based server
const client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]);
await client.initialize();

// HTTP server
const client = McpClient.connectHttp("http://127.0.0.1:8000/mcp");
await client.initialize();

// Use tools
const tools = await client.listTools();
const result = await client.callTool("echo", '{"text": "hello"}');
```

### HTTP Server Example

```bash
# Start an MCP HTTP server
python3 crates/examples/mcp_http_server.py
# Server runs on http://127.0.0.1:8000/mcp
```

---

## Configuration

Create `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

# Or use NVIDIA NIM
[global_model]
api_key = "nv-..."
base_url = "https://api.nvidia.com/v"
model = "nvidia/llama-3.1-nemotron-70b-instruct"

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
- Rust: `crates/examples/` (includes `agent_skill_demo.rs`)

### MCP Demos

```bash
# JavaScript MCP HTTP demo
node crates/jsbos/examples/mcp_http_agent_demo.cjs

# Python MCP HTTP demo (run server first, then use)
python3 crates/examples/mcp_http_server.py
```

---

## License

MIT OR Apache-2.0

---

**Version**: 2.0.5 | **Last Updated**: 2026-05-08