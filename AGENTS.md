# BOS Agent Instructions

High-signal facts for working in this repo.

---

## Workspace Structure

```
crates/
├── agent/          # Core agent: tools, skills, hooks, plugins, sessions, memory
├── bus/            # Pub/sub, queryable, caller/callable
├── config/         # TOML/YAML config loading
├── jsbos/          # Node.js bindings (NAPI-RS)
├── logging/        # Tracing, instrumentation
├── nbos/           # Python bindings (cdylib, maturin)
├── qserde/         # rkyv archive support used by the bus wire format
├── qserde_derive/  # Derive macros for qserde
├── react/          # ReAct engine, LLM integration
└── resource/       # Standalone resource subsystem + `rex` CLI (no dependents)
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
# Build the workspace
cargo build --workspace

# Test single crate / whole workspace
cargo test -p <crate>
cargo test --workspace

# Lint (warnings are errors) and format
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all

# API docs (fails on any missing-docs warning)
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# Standalone resource CLI
cargo run -p rex -- --help

# Python binding (crates/nbos)
cd crates/nbos && maturin develop

# Node.js binding (crates/jsbos)
cd crates/jsbos && npm install && npm run build
```

---

## Crate Dependencies

Each layer depends only on the layers to its right:

```
nbos / jsbos  →  agent  →  react  →  bus  →  logging  →  config
```

- `qserde` / `qserde_derive` provide the rkyv-backed wire format that `bus`
  serializes payloads with.
- `resource` is an independent subsystem; nothing in the agent stack depends on
  it (it pulls `bus`/`config`/`logging` only for its optional `cli` feature).

**Key**: Agent-to-agent messaging and pub/sub flow through `bus`. Direct
request/response work (LLM calls, tool execution) flows through the trait seams
in `react` (`LlmClient`, `Tool`/`AsyncTool`), not the bus.

---

## Python/JS Bindings

| Bindings | Entry | Build |
|----------|-------|-------|
| Python | `crates/nbos/` | `maturin develop` |
| JS | `crates/jsbos/index.js` | `npm run build` |

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
- Current green baselines:
  - Rust (excluding the binding crates): 417 passed, 0 failed, 8 ignored
  - Python (`cd crates/nbos && pytest -m "not llm"`): 229 passed, 4 deselected
  - JS (`cd crates/jsbos && npx ava`): 94 tests (13 parity, 3 native-binding, 21 API-doc, 9 content, 14 memory)

---

## Key Patterns

- **Tools**: Wrap a closure with `FunctionTool` (sync) or `AsyncFunctionTool`
  (async, may await I/O), or implement `Tool`/`AsyncTool` directly; register
  with `ToolRegistry::register`/`register_async`; the agent
  invokes them through the `react` seam (`LlmClient`, `Tool`), never the bus.
- **Bus**: Build a primitive and attach a session.
  - Publisher: `Publisher::new(topic).with_session(session)?.publish(&payload)`
  - Subscriber: `Subscriber::new(topic).with_session(session)` then
    `recv()` / `recv_with_timeout(d)` / `run(handler)`
  - Query: `Query::new(topic).with_session(session)?.query(&req)` against a
    `Queryable::new(topic).with_handler(..)` or `with_stream_handler(..)`
  - RPC: `Caller::new(name, session).call(&req)` against a
    `Callable::new(uri, session).with_handler(..)` then `start()`
  - Raw topic access: `session.publish(topic, &payload)` /
    `session.subscribe(topic, handler)`
- **Memory**: `agent::memory::MemoryStore` stores and recalls text across
  turns; `InMemoryMemory` ranks items by keyword overlap and breaks ties
  toward recency. The trait is object-safe, so a vector or file backend
  implements the same seam. `FileMemory` persists the same ranking to a
  JSON-lines file so memories survive a restart. Attach one with
  `Agent::with_memory` and
  matching items are appended to the system prompt on each run;
  `Agent::recalled_context` exposes the block on its own, and
  `Agent::remember`/`forget`/`recall` are the ergonomic path to the store.
  `MetadataFilter` scopes recall to a metadata key/value and
  `FileMemory::with_max_items` caps a file store. Python and JS
  expose the same store as a `Memory` class, pinned to the Rust ranking
  by the shared `memory_ranking.json` fixture; `AgentBuilder.with_memory`
  (Python) / `withMemory` (JS) prepends recalled matches to each text run,
  bounded by a recall limit. `Memory.search` accepts a metadata filter and
  `with_max_items`/`withMaxItems` caps the store. `Memory.save`/`Memory.load`
  read and write the same JSON-lines format as the Rust `FileMemory`.
- **Bindings**: Python and JS share one canonical vocabulary — `publish_text`/
  `publish_json`, `ask`/`ask_json`, `call`/`call_json`, `handle`/`start`/`run`/
  `run_json`, `recv`/`recv_json`. Python keeps the legacy `create_*`/`publish_*`
  names as deprecated aliases for backward compatibility; never remove them.
  Three static guards in `crates/jsbos/test/` keep the surfaces honest:
  `parity.test.js` compares the wrapper classes across languages,
  `native-bindings.test.js` checks that every wrapper call on a native object
  resolves to a real member of the generated interface, and `api-docs.test.js`
  checks that every public binding member is documented in `docs/api-reference/`.
- **Config**: Use `ConfigLoader.discover()` for auto-loading `~/.bos/conf/config.toml`
- **Documentation**: Every crate carries `#![warn(missing_docs)]` at its root, so
  the workspace must stay at zero missing docs. Document new public items in the
  same change. The sole exception is `resource/src/action.rs`, which suppresses
  the lint for the rkyv-generated resolver enum.
- **Solutions**: `docs/solutions/` — documented solutions (bugs, best practices, patterns), organized by category with YAML frontmatter (`module`, `tags`, `problem_type`)

---

## Last Updated: 2026-10-06
