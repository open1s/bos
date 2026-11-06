# BOS / BSH Architecture

> **BSH is a Distributed Composable Agent & Capability Platform.**  
> Each BSH node is an AI-native composable capability container. Multiple autonomous nodes can federate to discover capabilities, delegate work and collaborate — without merging their runtimes or trust boundaries.

**Architecture principle:** One Runtime. Composable Workspaces. Collaborative Agents. Federated Capabilities. Independent Trust Boundaries.

This document explains the platform through three architecture visuals, then defines the responsibilities and boundaries behind them. It distinguishes the current BOS implementation from the target BSH 2.0 architecture: diagrams describe the intended platform shape; the status section identifies what exists today and what remains under development.

---

## 1. The platform at a glance

![BSH Distributed Composable Agent and Capability Platform](docs/assets/diagrams/bsh-distributed-platform.svg)

A BSH installation is an autonomous node. Inside each node, capabilities are assembled into task-specific workspaces. Across nodes, Federation enables explicitly authorized remote capability use and agent collaboration.

The platform has three scales:

| Scale | What is composed | Main responsibility |
|---|---|---|
| **Capability** | Tools, skills, extensions, services, models and resources | Expose a well-defined operation through a provider contract |
| **Workspace** | Agents, capability views, context, policies and resource grants | Create an isolated environment for a purpose or task |
| **Federation** | Independent BSH nodes and their published capabilities | Discover, delegate and collaborate across node boundaries |

The hierarchy matters: a workspace does not own the whole node registry, and a federation does not become a shared global runtime.

## 2. Inside one BSH node

![Inside a composable BSH node](docs/assets/diagrams/bsh-composable-node.svg)

A node has a local capability plane and a composition control path.

### 2.1 Composition Engine

The Composition Engine is an independent subsystem. It is not part of the ReAct loop and must not be implemented as prompt-side tool selection alone.

Its responsibilities are to:

- Discover candidate capabilities from local providers and explicitly allowed remote peers.
- Resolve capability identity, version, dependencies and compatibility.
- Assemble a versioned composition manifest.
- Produce a workspace-scoped capability view.
- Validate permissions, policy constraints and resource budgets before activation.
- Support deterministic activation, update, rollback and teardown.

**AI proposes; the runtime disposes.** An agent or planner may propose a composition, but deterministic runtime code validates it. Model output is never an authorization decision.

### 2.2 Workspace Runtime

A Workspace is the activated unit of composition. It binds together:

- One or more agents and their session/context boundaries.
- A deliberately scoped set of capabilities.
- Model/provider choices and execution configuration.
- Project context, memory and explicitly granted resources.
- Policy, resource limits, lifecycle and audit context.

A workspace receives a **Capability View**, not unrestricted access to every capability installed on the node. The view is the contract exposed to its agents and tools.

### 2.3 Capability and provider model

A Capability describes an operation that can be discovered, authorized and invoked. A Capability Provider supplies its implementation.

The model must keep these responsibilities distinct:

| Concept | Responsibility |
|---|---|
| **Capability descriptor** | Stable identity, description, input/output contract, version and declared requirements |
| **Provider** | Implements the operation using a supported execution mechanism |
| **Registry** | Tracks known local descriptors and provider registrations |
| **Workspace Capability View** | The subset made visible and available to one workspace |
| **Execution Dispatcher** | Resolves an authorized invocation to its provider and execution boundary |

These are logical responsibilities; they do not imply that each must immediately become a separate crate or service. Avoid creating competing registries with unclear ownership.

### 2.4 Ferris and integrations

Ferris is the extension package/runtime framework used by BSH. It owns extension package validation, dependency/lifecycle management and supported extension execution providers. BSH adapts extension contributions into platform capabilities through explicit host contracts.

MCP is an integration protocol and adapter. It is not the canonical Capability model. MCP tools, resources and prompts may be represented through suitable BSH adapters without making MCP the internal abstraction for every capability.

## 3. Federation: multiple autonomous BSH nodes

![BSH Federation: autonomous nodes and shared work](docs/assets/diagrams/bsh-federation.svg)

Federation connects nodes; it does not merge them. Each node retains authority over its own execution, data and policy.

### 3.1 Federation responsibilities

| Plane | Responsibility |
|---|---|
| **Identity & Trust** | Node identity, enrollment, peer authentication, trust establishment and revocation |
| **Discovery & Control** | Peer discovery, capability advertisements, metadata and optional coordination |
| **Delegation & Collaboration** | Bounded task delegation, remote agent interaction, leases and task state |
| **Data & Execution** | Authorized remote invocation and minimal exchange of task inputs/results |

The topology is **hybrid P2P**: peers can communicate directly; an optional coordinator may assist with rendezvous, discovery or scheduling. A coordinator is not the execution authority for remote nodes.

The protocol is **transport-agnostic**. A transport such as Zenoh may be used as an implementation substrate, but transport connectivity alone does not establish identity, trust or authorization.

### 3.2 Remote capability semantics

A remote capability is a local descriptor/proxy for an operation hosted by another node. It is not a copy of the remote implementation and does not transfer ownership of the provider's resources.

A remote invocation must pass two independent authorization gates:

1. **Caller-side authorization:** the requesting node permits this workspace/agent to invoke the remote capability with the requested scope.
2. **Provider-side authorization:** the provider node independently validates the peer identity, delegation, capability policy and requested operation.

Either side may deny or revoke access. Discovery is not trust; advertisement is not authorization.

### 3.3 Delegation and collaboration

Delegation must be bounded and attributable. A delegation contract should carry a task identity, caller identity, target peer, allowed capability/scope, deadline, resource budget and correlation/idempotency metadata.

Do not assume distributed exactly-once execution. Network failure can leave an invocation outcome unknown. Use idempotency keys, deduplication where appropriate, durable task state, leases and explicit unknown-outcome handling. Sub-delegation should be denied by default unless the grant explicitly permits attenuated delegation.

### 3.4 Data and resource boundaries

Federation should exchange the minimum data needed for a task. Resource access should use explicit grants or leases rather than copying a peer's full environment. Knowledge federation should prefer delegated queries and minimized results over unrestricted replication of source data.

## 4. Agent execution and capability composition

The Agent Runtime and Composition Engine solve different problems:

- **Agent Runtime:** reasons about the task, manages sessions, calls the model and requests capability invocations.
- **Composition Engine:** determines which capabilities are assembled and exposed to a workspace.
- **Execution Dispatcher:** routes a permitted invocation to the correct local or remote provider.
- **Policy layer:** makes the authorization decision at the relevant boundary and records it.

The agent reasoning loop must not silently expand its own privileges. Tool availability is derived from the active workspace capability view and current policy grants.

## 5. BOS implementation foundation

BOS supplies the Rust-native execution foundation used by BSH and the language bindings.

| Component | Current responsibility |
|---|---|
| `crates/react` | ReAct engine, LLM client/provider adapters, tool contracts and execution loop |
| `crates/agent` | Agent assembly, sessions, skills, tools, MCP integration, hooks and plugins |
| `crates/bus` | Messaging abstractions, publish/subscribe and query/RPC patterns |
| `crates/config` | Configuration loading and validation |
| `crates/logging` | Tracing and logging initialization |
| `crates/nbos` | Python bindings over BOS functionality |
| `crates/jsbos` | JavaScript / Node.js bindings over BOS functionality |
| BSH application | GPUI desktop host, workspaces, agent interaction and extension integration |
| Ferris framework | Extension packages, lifecycle and execution-provider abstractions |

The language bindings are interfaces to the shared implementation, not separate agent engines. Lower-level crates should remain independent of higher-level application concerns.

### 5.1 Dependency direction

The intended dependency direction remains layered:

- `react` owns reasoning and LLM execution primitives; it must not depend on `agent`.
- `agent` composes the ReAct layer with sessions, tools, skills, MCP and lifecycle integration.
- `bus`, `config` and `logging` provide infrastructure without depending on agent/application layers.
- `nbos` and `jsbos` marshal calls across language boundaries; business logic remains in Rust crates.
- BSH integrates these crates as a host application.
- Ferris manages extensions; BSH adapts extension contributions into its capability model.

The composition and federation subsystems should be introduced without reversing these dependency rules or embedding platform policy inside the ReAct loop.

## 6. Security and trust model

Security boundaries are architectural boundaries, not UI settings.

1. **Node autonomy:** each node owns its identity, runtime, secrets, filesystem, policy and audit records.
2. **Workspace isolation:** each workspace receives only its declared context, resource grants and capability view.
3. **Capability authorization:** every invocation is checked against the active principal, workspace, capability and scope.
4. **Provider isolation:** Process/WASM/other providers retain their execution-specific isolation controls; a capability grant is not equivalent to OS sandboxing.
5. **Federation authorization:** remote calls require caller-side and provider-side authorization.
6. **Fail closed:** unknown identity, invalid policy, expired grant or unverifiable peer must not silently fall back to permissive execution.
7. **Auditability:** record the caller, workspace, capability, provider, policy decision, peer and outcome without leaking secrets.

Revocation, timeout, cancellation, peer loss and ambiguous remote outcomes are normal protocol states and require explicit handling.

## 7. Architecture decisions

The current BSH 2.0 direction is summarized below.

| ADR | Decision | Consequence |
|---|---|---|
| **ADR-001** | Hybrid composition | Declarative/static composition boundaries plus runtime composition |
| **ADR-002** | Independent Composition Engine | Composition lifecycle is independent of Agent/ReAct reasoning |
| **ADR-003** | Hybrid capability execution | One capability contract over multiple provider/execution models |
| **ADR-004** | Hybrid P2P federation | Direct peer communication with optional coordination |
| **ADR-005** | Transport-agnostic federation protocol | Protocol semantics are not coupled to a single transport |
| **ADR-006** | Independent node runtime and security boundary | No shared authority over peer execution or secrets |
| **ADR-007** | Local composition with explicitly authorized remote capabilities | Remote capabilities enter a workspace only through explicit policy/grant |

## 8. Implementation status and scope

The architecture is intentionally forward-looking, but documentation must not imply that planned features are already shipped.

| Capability | Status framing |
|---|---|
| Rust Agent Runtime, ReAct execution and model/tool integration | Existing BOS foundation |
| Python and JavaScript bindings | Existing BOS interfaces; verify release compatibility separately |
| BSH GPUI desktop host and local agent workspace | Existing product foundation |
| Ferris extension framework and supported extension providers | Existing extension direction; provider-specific isolation varies |
| MCP integration and BOS messaging primitives | Existing integration/communication foundations |
| First-class Capability descriptors and provider contracts | BSH 2.0 architecture work |
| Independent Composition Engine and Workspace Capability View | Target architecture; implement in staged increments |
| BSH node identity, federation discovery and trust enrollment | Target architecture |
| Remote capability invocation and bounded delegation | Target architecture |
| Cross-node multi-agent collaboration and resource federation | Later target architecture |

The architecture diagrams describe the target system shape. They are not a claim that federation, distributed scheduling or every composition lifecycle feature is already available in a released build.

## 9. Recommended implementation sequence

1. **Architecture and source audit:** map current tools, extensions, agents, resources, policies and bus APIs to the target domain model. Record concrete gaps before adding abstractions.
2. **Capability foundation:** define stable descriptors, provider contracts, versioning and invocation context; adapt existing built-ins and extensions.
3. **Composition Engine:** resolve dependencies and compatibility, validate policy, and produce a workspace-scoped capability view.
4. **Workspace Runtime:** activate, isolate, update and tear down compositions with bounded context and resources.
5. **Federation identity and discovery:** implement explicit enrollment, peer authentication, advertisements and revocation.
6. **Remote capability and delegation:** implement dual authorization, bounded requests, leases, idempotency and audit.
7. **Multi-agent and resource collaboration:** add richer coordination only after the two-node security and failure model is verified.

The first federation milestone should remain deliberately small: two enrolled nodes, authenticated discovery, one remote capability, dual authorization, bounded request/response, audit on both sides, and tests for denial, revocation, timeout, peer loss and duplicate requests.

## 10. Related documentation

- [BOS / BSH README](README.md)
- [BSH extension platform](docs/bsh-extension.md)
- [BSH Built-in Tools 2.0](docs/bsh-built-in-tools-2.0.md)
- [BOS / BSH textbook](docs/BOS-BSH-Textbook.md)
- [Security policy](SECURITY.md)
- [Contributor onboarding](ONBOARDING.md)

---

**Design north star:** BSH is not a single agent with a large tool list. It is a platform for assembling purpose-built agent workspaces and connecting autonomous nodes into a governed network of capabilities.
