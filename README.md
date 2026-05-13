English | [中文版本](./README-ZH.md)

<p align="center"> BrainOS — Multi-language AI agent runtime. One framework — Rust core, Python & JS bindings.
</p>

<p align="center">
  <a href="https://pypi.org/project/nbos"><img src="https://img.shields.io/pypi/v/nbos?label=python&logo=pypi&color=3776AB" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@open1s/jsbos"><img src="https://img.shields.io/npm/v/@open1s/jsbos?label=javascript&logo=npm&color=CB3837" alt="npm"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/jsbos.yml"><img src="https://github.com/open1s/bos/actions/workflows/jsbos.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/open1s/bos/wiki"><img src="https://img.shields.io/badge/docs-wiki-blue?logo=github" alt="Wiki"></a>
  <a href="https://github.com/open1s/bos/blob/main/LICENSE"><img src="https://img.shields.io/github/license/open1s/bos?color=blue" alt="License"></a>
</p>

---

**You have an LLM. You want it to use tools, talk to other agents, remember conversations, and connect to MCP servers — in Python, JavaScript, or Rust.**

BOS is the runtime that makes this work out of the box. One `pip install nbos` or `npm install @open1s/jsbos` gets you agents with tools, a pub/sub event bus for multi-agent coordination, MCP client for external tools, skill loading for domain-specific capabilities, and cross-session memory — all backed by a performant Rust core.

<p align="center">
  <img src="https://github.com/open1s/bos/blob/main/docs/assets/bos-hero.png" alt="BrainOS demo" width="700">
</p>

```bash
# 30-second win — copy, paste, run
pip install nbos && python -c "
from nbos import BrainOS
import asyncio
print(asyncio.run(BrainOS().agent('assistant').ask('say hi')))
"
```

---

## Quick Start

### Python (brainos)

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").with_tools(add)
    result = await agent.ask("What is 2+2?")
```

### JavaScript (@open1s/jsbos / brainos-js)

```javascript
import { BrainOS, ToolDef } from '@open1s/jsbos';

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
const result = await agent.runSimple('What is 2+2?');
```

### Rust (agent crate)

```rust
use agent::{Agent, AgentConfig};

let config = AgentConfig::default().name("assistant");
let agent = Agent::builder().config(config).build()?;
let result = agent.run_simple("Hello").await?;
```

---

## Why BOS?

BOS is not another LangChain wrapper or Python-only framework. It's a **multi-language runtime** built from the ground up for production AI agents.

| Need | BOS | Typical alternative |
|------|-----|-------------------|
| **Language choice** | Rust core + Python `nbos` + JavaScript `@open1s/jsbos` | Python-only |
| **Multi-agent** | Built-in event bus (pub/sub, query/RPC, caller/callable) | Ad-hoc or single-process |
| **External tools** | Native MCP client (stdio + HTTP) | Roll your own |
| **Agent capabilities** | Directory-based skills system — load domain expertise on demand | Hardcoded prompts |
| **Memory** | Cross-session persistence built in | Plugin or DIY |
| **Production** | Circuit breaker, rate limiter, configurable resilience | Often absent |
| **Performance** | Rust zero-cost abstractions, async Tokio runtime | GIL-bound Python |

**If you want an agent that speaks more than one language, talks to other agents, and works in production — not just a notebook — BOS is the runtime.**

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
│   ├── nbos/           # Python bindings (nbos package)
│   └── jsbos/          # Node.js bindings (@open1s/jsbos)
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
| `nbos` | Python bindings | `pip install nbos` |
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

# Python bindings (nbos)
cd crates/nbos && maturin develop

# Node.js bindings (jsbos)
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

The `nbos` package (Python) and `@open1s/jsbos` (JavaScript) provide consistent high-level APIs:

| Feature | Python | JavaScript |
|---------|--------|------------|
| Import | `from nbos import BrainOS, tool` | `import { BrainOS, ToolDef } from '@open1s/jsbos'` |
| Create brain | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| Create agent | `brain.agent("name")` | `brain.agent("name")` |
| Fluent config | `.with_model("gpt-4")` | `.model("gpt-4")` |
| Register tools | `.with_tools(tool)` | `.register(toolDef)` |
| Run | `await agent.ask("...")` | `await agent.runSimple("...")` |
| Bus factory | `BusManager()` | `BusManager.create()` |

### Low-level Bindings

For direct access to Rust bindings:

| Language | Package | Import |
|----------|---------|--------|
| Python | `nbos` | `from nbos import Agent, Bus, McpClient, ...` |
| JavaScript | `@open1s/jsbos` | `import { Agent, Bus, McpClient } from '@open1s/jsbos'` |

---

## MCP Client

Connect to MCP servers via stdio or HTTP transport:

### Python

```python
from nbos import McpClient

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
import { McpClient } from '@open1s/jsbos';

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

- Python: `crates/nbos/examples/`
- JavaScript: `crates/jsbos/examples/`
- Rust: `crates/examples/` (includes `agent_skill_demo.rs`)

### MCP Demos

```bash
# JavaScript MCP HTTP demo
node crates/jsbos/examples/mcp_http_agent_demo.js

# Python MCP HTTP demo (run server first, then use)
python3 crates/examples/mcp_http_server.py
```

---

## License

MIT OR Apache-2.0

---

**Version**: 2.0.6 | **Last Updated**: 2026-05-10
