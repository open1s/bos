<p align="right">
  <b>English</b> · <a href="README-ZH.md">中文</a>
</p>

<p align="center">
  <img src="docs/assets/bos-logo.png" alt="BOS — BrainOS" width="440">
</p>

<p align="center">
  <strong>An agent runtime in Rust — and the desktop app that runs on it.</strong>
</p>

<p align="center">
  <b>BOS</b> (BrainOS) is the runtime: agents, tools, skills, memory, MCP, and a multi-agent bus,<br>
  with first-class Python and JavaScript bindings over one Rust core.<br>
  <b>BSH</b> (BOS Harness) is the native desktop app built on that same runtime: workspaces, chat,<br>
  terminal, browser, editor — and an extension platform that reaches every one of them.
</p>

<p align="center">
  <a href="https://pypi.org/project/nbos"><img src="https://img.shields.io/pypi/v/nbos?label=python&logo=pypi&color=3776AB" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@open1s/jsbos"><img src="https://img.shields.io/npm/v/@open1s/jsbos?label=javascript&logo=npm&color=CB3837" alt="npm"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/rust.yml"><img src="https://github.com/open1s/bos/actions/workflows/rust.yml/badge.svg" alt="Rust CI"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/binding-parity.yml"><img src="https://github.com/open1s/bos/actions/workflows/binding-parity.yml/badge.svg" alt="Binding parity"></a>
  <a href="https://github.com/open1s/bos/blob/main/LICENSE"><img src="https://img.shields.io/github/license/open1s/bos?color=blue" alt="License"></a>
  <a href="docs/BOS-BSH-Textbook.md"><img src="https://img.shields.io/badge/docs-34%20chapters%20%C2%B7%20second%20edition-blue" alt="Textbook"></a>
</p>

---

## Two things, one repository

| | What it is | Start with |
|---|---|---|
| **BOS**<br>the runtime | A Rust core (`agent`, `react`, `bus`, `config`, `logging`) plus Python (`nbos`) and JavaScript (`@open1s/jsbos`) bindings. It is what you embed when you want an agent in *your* program. | `pip install nbos`<br>`npm install @open1s/jsbos`<br>`agent = { path = "crates/agent" }` |
| **BSH**<br>the harness | The native desktop application. It is what you open when you want to *use* an agent: sessions, workspaces, a terminal, a browser, an editor, subagents, scheduled tasks, and extensions. | `cargo build -p bsh` |

BSH is not a second product with its own stack. It is the first consumer of BOS: the same
agent loop, the same tools, the same sandbox, the same MCP client. When BSH gets a
capability, the crates get it too — and the bindings follow.

---

## The 30-second taste

**Python — an agent with a tool:**

```bash
pip install nbos
```

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").with_tools(add)
    print(await agent.ask("What is 2+2?"))
```

**JavaScript — the same agent, the same vocabulary:**

```bash
npm install @open1s/jsbos
```

```javascript
import { BrainOS, ToolDef } from '@open1s/jsbos';

const add = new ToolDef(
  'add', 'Add two numbers',
  (args) => (args.a || 0) + (args.b || 0),
  { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
  { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] },
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent('assistant').register(add).start();
console.log(await agent.runSimple('What is 2+2?'));
```

**Rust — the engine underneath both:**

```rust
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() {
    let config = AgentConfig::default()
        .name("assistant")
        .model("openai/gpt-4o")          // the vendor is inferred from the prefix
        .api_key(std::env::var("OPENAI_API_KEY").unwrap_or_default());
    let agent = Agent::from_config(config);
    println!("{}", agent.run_simple("Hello").await.expect("agent run"));
}
```

**The app — a window instead of a REPL:**

```bash
git clone https://github.com/open1s/bos && cd bos
cargo build --release -p bsh
./target/release/bsh                  # pick a model in Settings → Models
./crates/bsh/make-dev-app.sh          # macOS: the same binary, wrapped in BSH.app
```

BSH needs Rust 1.97+ and **Zig 0.16** (the terminal pane compiles Ghostty). On Linux, the
CI install list in [`.github/workflows/rust.yml`](.github/workflows/rust.yml) is the exact
set of GTK, WebKitGTK, Wayland and X11 headers the build wants. `make-dev-app.sh` builds a
debug binary and wraps it in `target/debug/BSH.app`.

### Point it at a model

**BOS** reads `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

# ...or name the vendors you use and let the model prefix pick one
[llm.nvidia]
api_key = "nv-..."
base_url = "https://integrate.api.nvidia.com/v1"
model = "nvidia/llama-3.1-nemotron-70b-instruct"

[llm.google]
api_key = "google-api-key"
base_url = "https://generativelanguage.googleapis.com/v1"
model = "gemini-pro"

[llm.openrouter]
api_key = "or-..."
base_url = "https://openrouter.ai/api/v1"
model = "anthropic/claude-3-haiku"
```

Environment variables work too: `OPENAI_API_KEY`, `LLM_BASE_URL`, `LLM_MODEL`.

**BSH** has the same providers as rows you can edit: **Settings → Models**. Its settings live
in `~/.bos/bsh/settings.toml`, and the values that must not sit there in plain text go into the
encrypted vault beside it.

---

## Why this instead of another agent framework

| What you need | What is here | What people usually settle for |
|---|---|---|
| **One behavior, three languages** | A Rust core with `nbos` and `@open1s/jsbos` over it; a CI guard fails the build when the two binding surfaces drift | A Python framework and a half-ported SDK |
| **Agents that talk to each other** | `bus`: publish/subscribe, query/response and caller/callable RPC, with an rkyv wire format | Ad-hoc HTTP between processes |
| **Tools you can actually run** | One sandbox implementation (macOS SBPL, Linux Landlock + Seccomp, Windows AppContainer) behind the tool *and* terminal seams | "The model asked nicely, so it is fine" |
| **Domain knowledge** | Directory skills — a `SKILL.md` with frontmatter, loaded on demand by the model | One system prompt that keeps growing |
| **External tool servers** | An MCP client over stdio and HTTP, with tools, resources and prompts mapped in | A bespoke adapter per server |
| **Context that survives** | In-memory and file-backed memory, metadata filters, `{{memory}}` in the prompt | RAG glue rebuilt per project |
| **Production behavior** | Retry classification, rate limiter, circuit breaker, per-run token/tool/time metrics | Hope, then a postmortem |
| **A face** | A GPU-rendered native window (GPUI), not an Electron shell | A web UI bolted onto a CLI |

---

## Inside the runtime (BOS)

- **Agent loop** — `crates/react` runs the ReAct cycle: stream, call tools, observe, continue.
  Hooks fire at the seams (`before_tool_call`, `after_llm_call`, `on_complete`, `on_error`, …)
  so a host can observe or gate a step without forking the engine.
- **Providers** — any OpenAI-compatible endpoint, the Anthropic protocol, NVIDIA NIM, Google
  AI and OpenRouter, selected by config or by the model's vendor prefix.
- **Tools** — wrap a closure with `FunctionTool`/`AsyncFunctionTool`, or implement
  `Tool`/`AsyncTool` and register it; the agent reaches them through the `react` seam, never
  through the bus.
- **Skills** — point the agent at a directory of `SKILL.md` folders and the model gets the
  catalogue in its system prompt, then calls `load_skill` for the full instructions.
- **Memory** — `InMemoryMemory` ranks by keyword overlap and recency; `FileMemory` persists the
  same ranking to JSON lines so it survives a restart. Filters scope recall; `{{memory}}` places
  it in the prompt.
- **MCP** — connect to servers you already have, over stdio or HTTP, and use their tools,
  resources and prompts as if they were native.
- **The bus** — `Publisher`/`Subscriber` for events, `Queryable` for request/response, and
  `Callable`/`Caller` for RPC. A session attaches all of them to one transport.
- **Sandbox** — `heel` is the one sandbox implementation: the workspace is the only writable place,
  executable bytes are written nowhere, reads are opt-in above a baseline, and network goes
  through a policy.
- **Observability** — tracing through `crates/logging`, plus call metrics (tokens, tool calls,
  wall time, rate-limit waits) that both bindings expose.

### MCP, without a wrapper library

```python
from nbos import McpClient

client = await McpClient.spawn("npx", ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
await client.initialize()

tools = await client.list_tools()
print(await client.call_tool("echo", '{"text": "hello"}'))
```

The JavaScript client has the same calls in camelCase (`listTools`, `callTool`,
`connectHttp`), and `agent` exposes the same client to the Rust host.

### The bindings stay in step

`nbos` and `@open1s/jsbos` are not translations of each other — they are two surfaces over the
same engine, held to one vocabulary (`ask`, `call`, `publish_text`, `recv`, `handle`, …) by
three static guards that run on every pull request: [`parity.test.js`](crates/jsbos/test/parity.test.js)
compares the Python and JavaScript classes, [`native-bindings.test.js`](crates/jsbos/test/native-bindings.test.js)
proves every wrapper call resolves to a real native member, and
[`api-docs.test.js`](crates/jsbos/test/api-docs.test.js) proves every public member is documented.

| | Python | JavaScript |
|---|---|---|
| Import | `from nbos import BrainOS, tool` | `import { BrainOS, ToolDef } from '@open1s/jsbos'` |
| Create the runtime | `async with BrainOS() as brain:` | `const brain = new BrainOS(); await brain.start()` |
| Create an agent | `brain.agent("name")` | `brain.agent("name")` |
| Choose a model | `.with_model("gpt-4")` | `.model("gpt-4")` |
| Register tools | `.with_tools(add)` | `.register(toolDef)` |
| Run | `await agent.ask("…")` | `await agent.runSimple("…")` |
| Multimodal | `Content.parts([...])` | `Content.parts([...])` |
| Low level | `from nbos import Agent, Bus, McpClient` | `import { Agent, Bus, McpClient } from '@open1s/jsbos'` |

---

## Inside the harness (BSH)

`bsh` is one native binary. Everything below is in it, backed by the crates in this repo —
none of it is a mock surface:

- **Workspaces and sessions** — each workspace is a directory and a boundary; sessions are
  stored, searched, pinned, archived, forked and restored on launch.
- **The conversation** — streaming turns, a turn rail for jumping back, a to-do panel fed by
  the agent's own plan store, a queue for messages typed mid-turn, subagent runs listed while
  they stream, and scheduled tasks that fire into a session on a cadence.
- **A real terminal** — Ghostty's Metal surface inside the window, with each submitted line
  checked by the terminal policy before the shell sees it.
- **A browser pane** — a WebView (WKWebView on macOS, WebKitGTK on Linux) for pages, and
  `bsh --browser=URL` to land on one directly.
- **An editor** — file tree, tabs, diffs, and tree-sitter grammars for the languages people
  actually open.
- **Settings that mean something** — account, vault, general, models, plugins and agent
  presets; a `settings.toml` that is read once per launch, and an encrypted vault
  (Argon2id + XChaCha20-Poly1305) for the values that must not sit in it in plain text.
- **Two languages** — English and Chinese, translated pane by pane rather than in the menu only.
- **Extensions** — see below. The app is designed so a package can add a tool, a command, a
  panel, a prompt, a context, a skill, a theme, an icon, a language service, an agent role, a
  workflow, storage, events, or an observation hook.

---

## Extensions: one manifest, six runtimes

A package is a directory with an `extension.toml` and the contributions it declares. The host
validates it, hashes it, installs it into a content-addressed store, resolves its dependencies,
runs its lifecycle, and then dispatches each call through a capability policy that decides —
per call, with scopes — whether the package may do the thing it asked for.

<p align="center">
  <img src="docs/assets/charts/architecture.png" alt="From package to contribution: validate, hash, install, resolve, gate, dispatch, run" width="820">
</p>

| Runtime | Use it when | The guest is |
|---|---|---|
| **declarative** | The work is JSON: tools, panels, prompts, themes | No code |
| **process** | The work is an existing program or CLI | A child process speaking line-delimited JSON-RPC |
| **wasm-component** | It is new third-party code that needs a hard sandbox | A WASM component against the WIT contract |
| **extism** | You already have an Extism plugin | A plugin, with a `bsh_host` import surface |
| **mcp** | You already have an MCP server | That server, stdio |
| **built-in** | You are first-party and linked in | Rust in this repository |

<p align="center">
  <img src="docs/assets/charts/call-path.png" alt="Every tool call: authorize each declared capability, deny with a reason and an audit row, or call with a deadline" width="700">
</p>

```bash
# Build the app first, then use its developer CLI.
export BSH=./target/release/bsh

$BSH ext new com.acme.hello --dir hello   # scaffold a package
$BSH ext validate hello                   # schema, ids, files, compatibility
$BSH ext mock hello echo '{"text":"hi"}'  # run a tool against a permissive host
$BSH ext install hello                    # stage, hash, authorize, activate
$BSH ext list                             # what the host believes is installed
$BSH ext doctor                           # hash drift, dependency and policy gaps
$BSH ext grant com.acme.hello workspace.write --scope workspace
$BSH ext logs com.acme.hello              # the audit trail, decision by decision
```

Start from a working package: [`examples/extensions/hello`](examples/extensions/hello) is
declarative, [`process-echo`](examples/extensions/process-echo) speaks JSON-RPC,
[`wasm-echo`](examples/extensions/wasm-echo) and [`extism-echo`](examples/extensions/extism-echo)
are sandboxed, [`mcp-echo`](examples/extensions/mcp-echo) reuses a server you already have, and
[`extism-hook`](examples/extensions/extism-hook) observes the model and HTTP traffic. CI verifies
every one of them on every push ([`scripts/ci/verify-bsh-examples.sh`](scripts/ci/verify-bsh-examples.sh)).

- **Normative contract:** [`docs/bsh-extension-standard.md`](docs/bsh-extension-standard.md) · **walkthrough:**
  [`docs/bsh-extension-dev-guide.md`](docs/bsh-extension-dev-guide.md) · **WIT:**
  [`crates/bsh/wit/extension.wit`](crates/bsh/wit/extension.wit)
- **Twenty capabilities**, from `workspace.write` and `network.http` to `secrets.read`,
  `process.execute` and `hooks.observe`, each grantable with a scope and each decision written to
  an audit log.
- **Hooks are not events:** an event is after the fact and has many subscribers; a hook is a
  declared seam call with one receiver class. [`docs/bsh-hook-system.md`](docs/bsh-hook-system.md)
- **Signing and trust:** `bsh ext keygen`/`sign`/`trust`/`verify`, a dependency lockfile, and a
  market that can point at any Git repository.

---

## Repository map

```
crates/
├── agent/        agents, tools, skills, memory, MCP, sessions, sandbox, policies
├── react/        the ReAct engine and LLM client seam
├── bus/          pub/sub, query, caller/callable
├── config/       TOML/YAML/env loading, discovery
├── logging/      tracing and instrumentation
├── qserde/       rkyv-backed wire format used by the bus
├── resource/     standalone resource subsystem and the `rex` CLI
├── bsh/          the BSH desktop harness (binary + extension platform)
├── nbos/         Python bindings -> pip install nbos
└── jsbos/        Node.js bindings -> npm install @open1s/jsbos
```

Layers flow one way: `nbos`/`jsbos` → `agent` → `react` → `bus` → `logging` → `config`.
`resource` stands alone. The [ARCHITECTURE.md](ARCHITECTURE.md) has the long version.

---

## Examples

| | |
|---|---|
| **Python** | [`crates/examples/`](crates/examples) — agents, the bus, MCP (stdio and HTTP), hooks, plugins; [`crates/nbos/examples/`](crates/nbos/examples) for multimodal content and audio |
| **JavaScript** | [`crates/jsbos/examples/`](crates/jsbos/examples) — the same set, plus an MCP-over-HTTP agent |
| **Rust** | [`crates/agent/examples/`](crates/agent/examples) — quickstart, hooks and plugins, resilience; [`crates/react/examples/`](crates/react/examples) for a bare LLM call |
| **Extensions** | [`examples/extensions/`](examples/extensions) — one package per runtime, all verified in CI |

---

## Documentation

| Start here | |
|---|---|
| [**BSH by Example**](docs/BOS-BSH-Textbook.md) | A 34-chapter working book on the app and its extension framework — second edition |
| [Python guide](docs/python-user-guide.md) · [JavaScript guide](docs/javascript-user-guide.md) · [Rust guide](docs/rust-user-guide.md) | The three surfaces, with complete examples |
| [Extension standard](docs/bsh-extension-standard.md) · [dev guide](docs/bsh-extension-dev-guide.md) | The normative contract, and the shortest path to a package |
| [Architecture](ARCHITECTURE.md) · [Onboarding](ONBOARDING.md) | How the layers fit, and how to get productive in this repo |
| [Settings spec](docs/bsh-settings-spec.md) · [Terminal policy](docs/bsh-terminal-policy.md) · [Hooks](docs/bsh-hook-system.md) · [Observability capture](docs/bsh-observability-capture.md) | The behaviour behind specific panes |
| [`rex` CLI](docs/rex-user-guide.md) · [Diagnostics](docs/diagnostics.md) | The standalone resource tool, and what to do when something misbehaves |
| [Wiki](https://github.com/open1s/bos/wiki) · [Changelog](CHANGELOG.md) | Guides and release notes |

---

## Build, test, contribute

```bash
cargo build --workspace                  # everything
cargo test --workspace                   # the core workspace (bindings have their own suites)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps

# bindings
cd crates/nbos  && maturin develop && pytest -m "not llm"
cd crates/jsbos && npm install && npx ava
```

- **CI** runs the workspace tests, clippy, rustdoc, the extension-example verifier, and the two
  binding pipelines: [`.github/workflows/`](.github/workflows/)
- **Contribute:** [CONTRIBUTING.md](CONTRIBUTING.md) · **security:** [SECURITY.md](SECURITY.md) ·
  **versioning:** this repo uses [jj](https://github.com/jj-vcs/jj) — every change is a stack you
  are expected to curate before pushing.
- **Good first contributions:** write an extension for a tool you already use, add a grammar or a
  language service, or pick up an issue labelled `good first issue`.

If BSH or BOS saves you a day, **star the repository** — it is the signal that keeps this work
funded, and it is how the next person finds it.

---

## License

MIT OR Apache-2.0 · **Version** 3.0.2 · [Releases](https://github.com/open1s/bos/releases)
