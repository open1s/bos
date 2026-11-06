<p align="right">
  <b>English</b> · <a href="README-ZH.md">中文</a>
</p>

<p align="center">
  <img src="docs/assets/bos-logo.png" alt="BOS — Rust-Native Agent Runtime and Distributed Platform" width="400">
</p>

<h1 align="center">BOS — Brain Operating System</h1>

<p align="center">
  <strong>Distributed Composable Agent &amp; Capability Platform</strong>
</p>

<p align="center">
  Compose agents, tools, runtimes and resources into purpose-built workspaces.<br>
  Connect independent BSH nodes to collaborate across machines and trust boundaries.
</p>

<p align="center">
  <img src="docs/assets/diagrams/bsh-product-hero.svg" alt="Conceptual BSH product visual: a task-specific workspace composes scoped capabilities and collaborates with independently governed remote nodes. Not an application screenshot." width="100%">
</p>

<p align="center">
  <a href="https://pypi.org/project/nbos"><img src="https://img.shields.io/pypi/v/nbos?label=Python%20SDK&logo=pypi&color=3776AB" alt="Python SDK"></a>
  <a href="https://www.npmjs.com/package/@open1s/jsbos"><img src="https://img.shields.io/npm/v/@open1s/jsbos?label=JavaScript%20SDK&logo=npm&color=CB3837" alt="JavaScript SDK"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/rust.yml"><img src="https://github.com/open1s/bos/actions/workflows/rust.yml/badge.svg" alt="Rust CI"></a>
  <a href="https://github.com/open1s/bos/actions/workflows/binding-parity.yml"><img src="https://github.com/open1s/bos/actions/workflows/binding-parity.yml/badge.svg" alt="Binding parity"></a>
  <a href="https://img.shields.io/github/license/open1s/bos"><img src="https://img.shields.io/github/license/open1s/bos" alt="License"></a>
</p>

<p align="center">
  <a href="#what-is-bos">What is BOS?</a> ·
  <a href="#bsh-platform">BSH platform</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="#documentation">Documentation</a>
</p>

---

## What is BOS?

**BOS is a Rust-native foundation for building, composing and operating AI agents as a distributed system.**

At its center is **BSH**: not merely a desktop agent workspace, but a **Distributed Composable Agent & Capability Platform**. A BSH node is a capability container: it assembles agents, tools, extensions, models, context and resources into purpose-built workspaces. Multiple independent BSH nodes can then be connected as a federation, allowing agents and capabilities to be discovered, delegated to and composed across machines—without collapsing their runtime or security boundaries.

BOS provides the shared execution foundation and developer interfaces. BSH turns that foundation into an environment for composing and operating local and distributed agent systems.

| Layer | Responsibility |
|---|---|
| **BOS Runtime** | Rust-native agent execution, tool dispatch, model integration, messaging and runtime controls |
| **BSH Node** | The desktop/runtime host for workspaces, agents, extensions and local resources; the full composable container is the BSH 2.0 target |
| **Composition Engine** | Target subsystem: resolves, validates and activates task-specific capability sets independently of agent reasoning |
| **Ferris Extension Framework** | Packages, loads and governs extensions across supported execution providers |
| **Federation** | Target subsystem: connects independent nodes and exposes explicitly authorized remote capabilities and agent services |
| **Rust / NBOS / JSBOS** | Native, Python and JavaScript interfaces to the shared BOS implementation |

### The platform in one sentence

**One Runtime. Composable Workspaces. Collaborative Agents. Federated Capabilities. Independent Trust Boundaries.**

## BSH platform

BSH has two complementary roles.

<p align="center"><img src="docs/assets/diagrams/bsh-platform-value.svg" alt="BOS and BSH value proposition: Rust-native foundation, composable workspaces and federated autonomous nodes." width="100%"></p>

BSH turns the BOS foundation into a practical operating environment: compose capabilities for a task, run them in a governed workspace, extend the platform, and connect independent nodes when collaboration requires it.

### 1. A composable capability container

A BSH node assembles only the capabilities needed for a particular purpose. A workspace can receive a tailored capability view instead of exposing every installed tool to every agent.

A composition may include:

- **Agents** — task-focused agents, specialist roles and delegated workers.
- **Tools and skills** — built-in tools, user-defined tools, skills and MCP integrations.
- **Extensions** — host contributions and isolated extension packages managed through Ferris.
- **Models and policies** — model providers, routing choices, permissions and execution constraints.
- **Context and resources** — project files, memory, services and other explicitly granted resources.
- **Workflows** — reusable task compositions and coordinated agent activities.

Composition is a first-class platform concern, not a side effect of agent prompting. The Composition Engine is designed to remain independent of the ReAct/agent reasoning engine. AI can propose a composition; the runtime validates compatibility, dependencies, permissions and resource limits before activation.

<p align="center"><img src="docs/assets/diagrams/bsh-composable-workspace-visual.svg" alt="How BSH composes agents, models, tools, extensions, context and resources into a validated task-specific workspace." width="100%"></p>

### 2. A distributed agent and capability platform

A BSH installation is an autonomous node. Multiple nodes can form a federation and collaborate while retaining their own identity, runtime, policies, secrets, files and audit records.

The intended federation model supports:

- **Node and capability discovery** — discover peers and the capabilities they advertise.
- **Remote capability invocation** — use a remote capability through a local, policy-aware interface.
- **Agent delegation** — assign a bounded task to an agent on another node and receive its result.
- **Multi-agent collaboration** — coordinate specialists across nodes when a task benefits from distributed expertise or resources.
- **Resource-aware collaboration** — request access to selected services or resources without copying an entire node's environment.
- **Independent governance** — authorize at both the requesting node and the provider node; either side can deny or revoke access.

Federation is not a shared super-process. A coordinator may help with rendezvous, discovery or scheduling, but it does not become the execution authority for peer nodes. Remote capabilities remain remote, and discovery never implies trust.

<p align="center"><img src="docs/assets/diagrams/bsh-federation-value.svg" alt="Federation connects autonomous BSH nodes through bounded requests and independent authorization at both ends." width="100%"></p>

> **Implementation status:** BOS already provides the Rust runtime, agent/tool execution, messaging foundations, BSH desktop host, Ferris extension framework and MCP integration. The full BSH composition and federation model is the platform's architectural direction and is being developed in stages. Do not interpret the target federation capabilities above as a claim that every feature is already generally available.

## What can you build with BSH?

BSH is designed around **purpose-built workspaces**, not one universal agent with every tool enabled. The following scenarios show how the same BOS foundation can be composed for different jobs. They are illustrative composition blueprints, not claims that these exact presets ship out of the box.

### Research Workspace

<p align="center"><img src="docs/assets/diagrams/scenario-research-workspace.svg" alt="Research workspace blueprint combining a research agent, project sources, document tools, analysis capabilities and evidence-led outputs." width="100%"></p>

Bring a research brief, project documents and selected source integrations together with an agent and analysis tools. The workspace can be configured to collect evidence, extract information, compare findings and produce a structured report. Source search and specialist analysis integrations are supplied by the tools and extensions configured for that workspace.

### Software Engineering Workspace

<p align="center"><img src="docs/assets/diagrams/scenario-software-engineering-workspace.svg" alt="Software engineering workspace blueprint combining repository context, an engineering agent, file tools, terminal, browser and extensions." width="100%"></p>

Give a coding agent the repository context and a focused toolset: file discovery and batch reads, structured edits, terminal operations, browser access and relevant extensions. BSH's desktop already provides the operator surfaces — editor, diffs, terminal and browser — while dedicated tools and execution policies govern what the agent can do. A developer remains responsible for reviewing changes and test results.

### Distributed Multi-Agent Workspace

<p align="center"><img src="docs/assets/diagrams/scenario-distributed-multi-agent.svg" alt="Target distributed multi-agent scenario with a requester node delegating bounded tasks to autonomous research and engineering provider nodes." width="100%"></p>

A coordinating workspace can delegate bounded research or engineering tasks to specialist agents hosted by other BSH nodes, then combine their results. Each node keeps its own runtime, secrets, policies and audit trail; remote work requires explicit authorization at both ends.

<blockquote>
  **Roadmap status:** the distributed scenario illustrates the BSH Federation target architecture. Node enrollment, discovery, remote capability invocation and cross-node delegation are being developed in stages; this is not a claim that the complete workflow is currently available.
</blockquote>

## Architecture

BOS separates the shared execution foundation, local composition and cross-node collaboration.

<p align="center">
  <img src="docs/assets/diagrams/bsh-distributed-platform.svg" alt="BSH distributed platform: autonomous nodes contain composable workspaces and collaborate through federation while retaining independent trust boundaries." width="100%">
</p>

### Local node architecture

Within one BSH node, the platform is organized around distinct responsibilities:

<p align="center"><img src="docs/assets/diagrams/bsh-composable-node.svg" alt="Internal architecture of one composable BSH node, from composition and workspace runtime to scoped capabilities and providers." width="100%"></p>

| Subsystem | Responsibility |
|---|---|
| **Agent Runtime** | Runs agents and reasoning loops, manages sessions and coordinates tool calls |
| **Capability System** | Describes available operations and mediates access to providers |
| **Composition Engine** | Target: resolves a task-specific composition and produces a workspace capability view |
| **Workspace Runtime** | Target: activates and governs a composition with its context, resources and lifecycle |
| **Ferris** | Manages extension packages, providers, lifecycle and execution boundaries |
| **Policy and Audit** | Applies current authorization and execution controls; the unified composition-wide policy/audit model is a target |
| **Bus and Adapters** | Provides messaging and integration seams for local and external systems |

### Federation architecture

Federation connects nodes; it does not merge them.

| Plane | Responsibility |
|---|---|
| **Identity and Trust** | Node identity, enrollment, trust establishment, revocation and peer authentication |
| **Discovery and Control** | Peer discovery, capability advertisements, policy metadata and coordination |
| **Delegation and Collaboration** | Bounded task delegation, remote agent interaction, leases and task state |
| **Data and Execution** | Authorized remote invocation and minimal exchange of task inputs and results |

The federation protocol is transport-agnostic. Transports such as Zenoh may be used as implementation substrates, but transport connectivity alone does not establish identity, authorization or trust.

### BSH extension and agent model

<p align="center">
  <img src="docs/assets/diagrams/bsh-extension-agent-model.svg" alt="BSH extension framework and agent execution model, showing host services, extension runtimes, agent engine, context, tools, runtime environment and governance." width="100%">
</p>

The extension framework is a host-integrated subsystem. It supplies capabilities to BSH through explicit contracts; it is not a second agent runtime. MCP is an integration protocol and adapter, not the canonical capability model of the platform.

## Why BOS?

- **Rust-native execution.** A shared implementation underpins native applications and language bindings.
- **Composable by design.** Assemble task-specific workspaces from agents, tools, extensions, models, context and resources.
- **Agent-oriented runtime.** ReAct execution coordinates model responses, tool calls, observations and lifecycle hooks.
- **LLM-aware tools.** Tool contracts, schemas and bounded outputs help models select and use capabilities effectively.
- **Extensible execution.** Ferris supports multiple extension providers, including declarative, process, WebAssembly Component, Extism, MCP and built-in integrations.
- **Distributed communication foundation.** The bus provides publish/subscribe, query/response and RPC patterns for system components.
- **Policy at execution boundaries.** Capability authorization, sandboxing, limits and audit controls are treated as runtime responsibilities.
- **Language interoperability.** Rust, Python and JavaScript interfaces use the same underlying implementation.

## BOS developer foundation

BOS keeps the core implementation in Rust and exposes it through native Rust, Python and JavaScript interfaces. BSH is the operator workspace built on the same foundation.

<p align="center"><img src="docs/assets/diagrams/bos-developer-foundation.svg" alt="BOS shared Rust-native foundation powering Rust, Python and JavaScript integrations and the BSH application." width="100%"></p>

## Quick start

Choose the interface that fits your application.

### Python

```bash
pip install nbos
```

```python
from nbos import BrainOS, tool

@tool("Add two numbers")
def add(a: int, b: int) -> int:
    return a + b

async with BrainOS() as brain:
    agent = brain.agent("assistant").register(add)
    print(await agent.ask("What is 2 + 2?"))
```

Guide: [Python user guide](docs/python-user-guide.md)

### JavaScript / Node.js

```bash
npm install @open1s/jsbos
```

```javascript
import { BrainOS, ToolDef } from "@open1s/jsbos";

const add = new ToolDef(
  "add",
  "Add two numbers",
  (args) => args.a + args.b,
  { type: "object", properties: { a: { type: "number" }, b: { type: "number" } }, required: ["a", "b"] },
);

const brain = new BrainOS();
await brain.start();
const agent = await brain.agent("assistant").register(add).start();
console.log(await agent.runSimple("What is 2 + 2?"));
await brain.stop();
```

Guide: [JavaScript user guide](docs/javascript-user-guide.md)

### Rust

Use the runtime crates directly from the workspace:

```rust
use agent::{Agent, AgentConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = AgentConfig::default()
        .name("assistant")
        .model("gpt-4o")
        .base_url("https://api.openai.com/v1")
        .api_key(std::env::var("OPENAI_API_KEY")?);

    let agent = Agent::from_config(config);
    println!("{}", agent.run_simple("Hello").await?);
    Ok(())
}
```

Guide: [Rust user guide](docs/rust-user-guide.md)

### Run BSH

```bash
git clone https://github.com/open1s/bos
cd bos
cargo build --release -p bsh
./target/release/bsh
```

BSH requires Rust 1.97+ and Zig 0.16 to build its Ghostty terminal integration.
On macOS, a development app can also be packaged with
`./crates/bsh/make-dev-app.sh`. See the
[Rust workflow](.github/workflows/rust.yml) for Linux build dependencies.

### Install BSH

Prebuilt installers are published on the
[releases page](https://github.com/open1s/bos/releases). Available formats and
platform coverage are listed with each release. Each release carries
`SHA256SUMS`.

The tag-driven build, signing and notarization setup is documented in
[docs/bsh-release.md](docs/bsh-release.md).

## BSH desktop environment

BSH provides a native GPUI environment for developing, using and operating agents. It is also the operator interface for local and, as federation capabilities mature, distributed workspaces.

- **Workspaces and sessions** — organize projects and persistent conversations.
- **Agent-first interaction** — streaming responses, task plans, queued messages and visible subagent activity.
- **Integrated terminal** — Ghostty terminal surface with command-policy checks.
- **Browser pane** — embedded WebView for browsing alongside an agent.
- **Code editor** — file tree, tabs, diffs and tree-sitter language grammars.
- **Settings and credentials** — provider configuration and an encrypted vault.
- **Bilingual interface** — English and Chinese.
- **Extensions** — add tools, commands, panels, prompts, contexts, skills, themes, language services, agent roles, workflows, storage and hooks.

### BSH interface preview

> **Screenshot placeholders — not real application captures.** Replace these assets with screenshots from a running BSH build before using them as product evidence.

| Main workspace | Agent session |
|---|---|
| ![BSH workspace placeholder](docs/assets/bsh-showcase/workspace.png) | ![BSH agent session placeholder](docs/assets/bsh-showcase/agent-session.png) |

| Extension manager | Model and settings |
|---|---|
| ![BSH extension manager placeholder](docs/assets/bsh-showcase/extensions.png) | ![BSH settings placeholder](docs/assets/bsh-showcase/settings.png) |

Replacement instructions: [BSH screenshot guide](docs/assets/bsh-showcase/README.md).

## Ferris Extension Framework

[Ferris](crates/ferris-core/) is the extension framework used by BSH. It separates extension package and lifecycle management from the host's agent reasoning engine. BSH adapts extension-provided functionality into host capabilities through explicit contracts and permission checks.

| Provider | Intended use |
|---|---|
| **Declarative** | JSON-defined tools, panels, prompts and themes |
| **Process** | Existing programs and services communicating through JSON-RPC |
| **WASM Component** | Portable components using the WebAssembly Component Model and WIT |
| **Extism** | Existing Extism plugins |
| **MCP** | Reuse an MCP server through an adapter |
| **Built-in** | First-party Rust functionality linked into the host |

Create and validate a package:

```bash
bsh ext new com.acme.hello --dir hello
bsh ext validate hello
bsh ext mock hello echo '{"text":"hi"}'
bsh ext install hello
bsh ext list
bsh ext doctor
```

Start here:
- [BSH extension platform](docs/bsh-extension.md)
- [WIT interface](crates/bsh/wit/extension.wit)
- [Extension examples](examples/extensions/)

**Security note:** extension capability authorization and operating-system isolation are separate controls. Enforcement differs by operating system and runtime. Consult the [BSH extension platform](docs/bsh-extension.md) and [security policy](SECURITY.md) for the current contract and limitations.

## Model configuration

BOS supports OpenAI-compatible endpoints, Anthropic, NVIDIA NIM, Google AI
and OpenRouter. Configure a default model in `~/.bos/conf/config.toml`:

```toml
[global_model]
api_key = "your-api-key"
base_url = "https://api.openai.com/v1"
model = "gpt-4"

[llm.nvidia]
api_key = "nv-..."
base_url = "https://integrate.api.nvidia.com/v1"
model = "nvidia/your-model"
```

If the call and configuration file do not provide a key, the bindings read
`OPENAI_API_KEY`. BSH manages providers in **Settings → Models** and stores
sensitive credentials in its encrypted vault.

## Documentation

| Resource | Description |
|---|---|
| [BOS / BSH textbook](docs/BOS-BSH-Textbook.md) | In-depth guide to the runtime, desktop host and extension framework |
| [Architecture](ARCHITECTURE.md) | Current component responsibilities and architecture |
| [BSH 2.0 workspace acceptance](docs/bsh-2.0-e2e-acceptance.md) | Code-backed implementation gap, golden-path demo and end-to-end security acceptance |
| [Rust user guide](docs/rust-user-guide.md) | Embed and use BOS from Rust |
| [Python user guide](docs/python-user-guide.md) | Use BOS from Python |
| [JavaScript user guide](docs/javascript-user-guide.md) | Use BOS from Node.js and JavaScript |
| [BSH extension platform](docs/bsh-extension.md) | Manifest, providers, lifecycle, capabilities, CLI and security |
| [BSH Built-in Tools 2.0](docs/bsh-built-in-tools-2.0.md) | LLM-oriented tool contracts, batch file operations, output budgets and benchmark plan |
| [Contributor onboarding](ONBOARDING.md) | Set up a development environment |
| [Settings](docs/bsh-settings-spec.md) · [Terminal policy](docs/bsh-terminal-policy.md) · [Hooks](docs/bsh-hook-system.md) | Subsystem specifications |
| [BSH release pipeline](docs/bsh-release.md) | Installer artifacts, signing and release workflow |
| [REX guide](docs/rex-user-guide.md) · [Diagnostics](docs/diagnostics.md) | Resource tooling and troubleshooting |
| [Wiki](https://github.com/open1s/bos/wiki) · [Changelog](CHANGELOG.md) | Guides and release history |

## Build, test and contribute

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Python and JavaScript binding tests:

```bash
cd crates/nbos  && maturin develop && pytest -m "not llm"
cd crates/jsbos && npm install && npx ava
```

- [Contributing guide](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [GitHub issues](https://github.com/open1s/bos/issues)
- [Good first issues](https://github.com/open1s/bos/labels/good%20first%20issue)

If BOS is useful to you, consider starring the repository and sharing what you
build with it. Feedback and real-world examples help guide the platform.

## License

MIT OR Apache-2.0 · [Releases](https://github.com/open1s/bos/releases)
