# BOS Agent Instructions

High-signal facts for working in this repo.

---

## Workspace Structure

```
crates/
├── agent/      # Core agent with tools, skills, LLM providers
├── bus/        # Pub/sub, queryable, caller/callable
├── config/    # TOML/YAML config loading
├── logging/   # Tracing, instrumentation
├── nbos/     # Python bindings (cdylib, maturin)
├── jsbos/     # Node.js bindings (NAPI-RS)
└── react/     # ReAct engine, LLM integration
```
---
## Version Control
This repo use jj(jujutsu) for version management
Core Concepts
- change_id: Unique identifier for a change (use this, NOT commit hash)
- stack: Ordered list of changes (your working history)
- working copy: Always attached to a change

Key difference from Git:
jj is stack-based, not branch-based. You are expected to edit, reorder, and clean history before pushing.
```bash
# Status
jj status

# Create new change
jj new

# Describe change
jj describe -m "<crate>: <title>"

# View stack
jj log

# Edit change
jj edit <change_id>

# Split change
jj split

# Squash into parent
jj squash

# Reorder changes
jj rebase -r <change_id> -d <destination>
```
---

## Essential Commands

```bash
# Build all
cargo build --all

# Test single crate
cargo test -p <crate>

# Lint
cargo clippy --all
cargo fmt --all

# Python binding (crates/nbos)
cd crates/nbos && maturin develop

# Node.js binding (crates/jsbos)
cd crates/jsbos && npm install && npm run build
```

---

## Crate Dependencies

- `agent` depends on: `bus`, `config`, `logging`, `react`
- `nbos` depends on: `agent`, `bus`, `config`
- `jsbos` depends on: `agent`, `bus`, `config`

**Key**: All cross-crate communication flows through `bus`.

---

## Python/JS Bindings

| Bindings | Entry | Build |
|----------|-------|-------|
| Python | `crates/pybos/brainos/` | `maturin develop` |
| JS | `crates/jsbos/brainos.js` | `npm run build` |

**User guides**: `docs/python-user-guide.md`, `docs/javascript-user-guide.md`, `docs/rust-user-guide.md`

---

## Unified API (Python ↔ JS)

Python and JavaScript APIs are designed to be consistent:

```python
# Python
from nbos import BrainOS, tool

@tool("Add")
def add(a, b): return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    result = await agent.ask("What is 2+2?")
```

```javascript
// JavaScript
const { BrainOS, ToolDef } = require('brainos');

const addTool = new ToolDef('add', 'Add', (args) => args.a + args.b, ...);
const brain = new BrainOS();
await brain.start();
const agent = brain.agent('assistant').register(addTool);
const result = await agent.ask('What is 2+2?');
```

---

## Testing Notes

- Use `#[tokio::test]` for async tests
- Run with: `cargo test -p <crate> name -- --nocapture`
- Set `RUST_LOG=debug` for tracing output

---

## Key Patterns

- **Tools**: Implement `Tool` trait, register via `ToolRegistry`
- **Bus**: `create_publisher()`, `create_subscriber()`, `create_query()`, `create_caller()`
- **Config**: Use `ConfigLoader.discover()` for auto-loading `~/.bos/conf/config.toml`
- **Solutions**: `docs/solutions/` — documented solutions (bugs, best practices, patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`)

---

## Last Updated: 2026-04-09
