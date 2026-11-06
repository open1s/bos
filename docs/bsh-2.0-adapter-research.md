# BSH 2.0 — How to Adapter

*Source-level gap analysis and provider-adapter design*

- **Status:** Research / proposal (input to RFC phase 0)
- **Date:** 2026-11-06
- **Relates to:** [bsh-2.0-architecture-rfc.md](bsh-2.0-architecture-rfc.md), ADR-001 through ADR-007
- **Accepted as:** [ADR-008 and ADR-009](bsh-2.0-architecture-rfc.md) in the RFC (2026-11-06).
- **Transport decision:** Zenoh carries the federation protocol (§5.7). ADR-005 still holds: the
  protocol is defined above the transport, and the transport may not define authorization.
- **Wire decisions:** JSON envelope; resource bytes ride Zenoh too. One federation endpoint per
  node, one encoding, one identity.
- **Question:** the RFC says the next step is a source-level gap analysis, and phase 1 names a
  "Provider contract and adapter boundary". What exactly has to be adapted, what already exists,
  and what does the adapter look like in this repository?

---

## 0. "Adapter" means three different things in the RFC

Before designing anything, the word has to be split. The RFC uses *adapter* for three unrelated
seams, and they have different owners, different lifetimes, and different risks.

| # | Adapter | RFC | Direction | This document |
|---|---|---|---|---|
| A | **Provider adapter** — lowers an existing implementation surface (built-in tool, Ferris runtime, MCP server, remote node, resource) into one Capability contract | §6.2, §17.2, §17.3, §18 | implementation → contract | **in scope, the main subject** |
| B | **Transport adapter** — carries federation protocol messages over Zenoh/QUIC/HTTP2/WS | §4.8, §15.4, ADR-005 | protocol → wire | **decided: Zenoh** (§5.7); the protocol stays independent of it |
| C | **Workspace view adapter** — turns a resolved, authorized capability set into the tool list one Agent turn sees | §6.4, §10.3, §17.1 | contract → Agent/ReAct | **in scope, because A is useless without it** |

A is the hard part and C is the part that keeps the change honest: if the adapter only produces
descriptors and the engine still sees the whole global registry, nothing in §10.3 has been
delivered. B now has a decision rather than an open question — Zenoh — but the transport must not
leak into protocol semantics, so its scope is bounded by §5.7 instead of expanding into the whole
federation surface.

---

## 1. Method

- Read the RFC end to end, extracting every normative statement about providers, registries,
  invocation, and authorization.
- Audited every surface that can execute something on the model's behalf: `react` tool traits,
  `agent` tool registry, `agent::mcp`, `agent::bus`, `agent::extensions`, `ferris-core`,
  `ferris-runtime`, `resource`, and the host composition in `crates/bsh`.
- For each surface recorded: what contract it already honors, who owns its lifecycle, and which
  RFC field it cannot supply today.
- Designed the adapter as a *lowering* over those surfaces, not a replacement, because §17 and
  §28 both make regression of existing behavior an acceptance criterion.

No code was changed by this research.

---

## 2. What already exists

The repository is further along than the RFC assumes. Five of the seven provider categories have a
working implementation seam; what is missing is the shared descriptor/identity/context layer above
them, not the runtimes.

| Surface | Location | What it is | What it cannot supply |
|---|---|---|---|
| `react::Tool` / `react::AsyncTool` | `crates/react/src/tool/registry.rs:8`, `:60` | The engine seam: name, description, JSON schema, run, cancel, streaming, deferred, proxy target | Identity, version, provider, permissions, side effects, deadline, principal |
| `agent::tools::ToolRegistry` | `crates/agent/src/tools/registry.rs:9` | Name-keyed dispatch registry, sync + async maps, schema cache, MCP marker | Authority — it is a dispatch map, not a governed catalogue |
| `Capability` enum (extensions) | `crates/agent/src/extensions/mod.rs:172` | 7 coarse package capabilities (Tools/Hooks/Commands/Skills/Prompts/Panels/Models) | Per-operation scope; it describes a package, not an invocation |
| `ExtensionRegistry` | `crates/agent/src/extensions/registry.rs:93` | Package → `Contribution::Tool(Arc<dyn Tool>)` lowering, conflict check, apply to Agent | Versioned capability IDs, per-call policy, provider identity |
| Ferris `Runtime` + `ToolDescriptor` | `crates/ferris-runtime/src/contract.rs:31`, `:241` | The closest thing to a provider contract: tools(), call_tool(), CallContext (extension, correlation, workspace, timeout), ToolResult with MCP-shaped content | A provider registry; per-call principal; capability ID separate from contribution id |
| Ferris capability model | `crates/ferris-core/src/capability.rs:40`, `:231`, `:545`, `:813` | 21 named capabilities, Risk, CallScope, GrantStore, PolicyEngine::authorize_call | Workspace scope; it is extension-scoped, and the subject is an extension, not a workspace |
| `ExtensionTool` | `crates/bsh/src/extension/agent.rs:225` | **The reference adapter**: qualified name, per-call policy, call admission + cancellation (CallTracker), timeout, output cap, streaming, event dispatch, revocation lease | Nothing structural — it is the template, coupled to the host crate |
| `McpToolAdapter` | `crates/agent/src/mcp/adapter.rs:10` | MCP tool → Tool + AsyncTool, schema passthrough | Server identity in the ID, version, risk, deadline; sync path spawns a thread + runtime per call |
| `McpPolicy` | `crates/agent/src/mcp/policy.rs:88` | Allow/deny/ask per (server, tool) | Enforcement is a pre-filter at registration, not an invocation decision |
| `AgentCallerTool` | `crates/agent/src/bus/tool.rs:13` | Remote agent tool over bus RPC | Grants, provenance, idempotency, dual authorization — it is agent RPC, not a capability proxy |
| `ResourceTool` | `crates/resource/src/tool.rs:13` | Resource URI → JSON ResourceAction invocation | It does not implement `react::Tool`; no schema; no registry entry |
| Discovery (find_tool / call_tool) | `crates/agent/src/agent/agentic/discovery.rs:768` | On-demand tool shortlist + dispatcher, Withheld set | It is **explicitly not an authorization boundary** (`:17`) |
| Policy at the tool boundary | `crates/agent/src/policy/target.rs:10`, `:260` | Action (Read/Write/Egress/Spawn/Capability), PolicyInput (session, preset, tool, action, target) | `tool` is a bare name; there is no capability ID, no version, no provider |

Two facts from the audit matter more than the rest:

1. **`Action::Capability` already exists** (`crates/agent/src/policy/target.rs:25`) as the
   registration-time question — "may this tool be offered at all". The RFC's capability
   authorization is the same question asked with a richer subject. The adapter does not need a new
   policy engine; it needs to feed the existing one a capability identity.
2. **The host already rebuilds the agent from settings on every turn**
   (`crates/bsh/src/caps.rs:1`). A per-turn workspace view is therefore a natural fit: there is
   already exactly one place where "what this turn may have" is decided.

### 2.1 The reference adapter is ExtensionTool

`crates/bsh/src/extension/agent.rs` already contains the shape the RFC asks for, minus the
descriptor model:

- a qualified surface name (extension and tool, dot-joined), because a package-local id is not
  globally unique;
- a per-call authorization check with the call's own path/host/secret as the scope (`:561`);
- call admission and per-call cancellation tokens, with a revocation lease that closes admission
  atomically (`CallTracker`, `:247`);
- a wall-clock ceiling, a post-hook output ceiling, and bounded progress forwarding;
- policy denial reported as *a tool error the model can read*, not a panic and not a silent no-op.

Any capability provider adapter that does less than this is a regression. The design in §4 is
therefore mostly "generalize this file", not "invent a provider model".

### 2.2 The Zenoh substrate is already in the tree

Zenoh is not a dependency to add for federation; it is the layer the bus already sits on
(`crates/bus/src/lib.rs:1` — "pub/sub, queryable, and caller/callable over Zenoh"):

| Piece | Location | What it gives the federation plane |
|---|---|---|
| Session and config | `crates/bus/src/session.rs:154` | `BusConfig { mode, connect, listen, peer }` rendered into a Zenoh config; endpoints and router/peer modes already exist |
| Pub/sub | `crates/bus/src/publisher.rs`, `subscriber.rs` | events, progress, cancellation notices |
| Query/reply | `crates/bus/src/query.rs`, `queryable.rs` | request/response with `query_with_timeout` and `ConsolidationMode` — the invocation primitive |
| RPC | `crates/bus/src/caller.rs`, `callable.rs` | caller/callable over a URI — the delegation primitive |
| Agent RPC server | `crates/agent/src/bus/server.rs:134` | an existing agent-to-agent surface to learn from, not to reuse (§5.4) |
| Codec | `crates/bus/src/codec.rs` | rkyv wire format, intra-node |
| Node announcement | `crates/resource/src/discovery.rs:22`, `:53` | `bos/discovery/{node_id}` with an HMAC-signed `NodeAnnounce` (node id, QUIC address, capabilities, load) — the address field disappears with the QUIC retirement |
| Resource data plane | `crates/resource/src/transport/quic.rs:184` | quinn + rustls mTLS, `vnode://` resources — today's second data plane, retired for federation by §5.7 |

Three consequences:

1. **Choosing Zenoh adds no new transport to the build.** The federation transport is a thin
   implementation of the `FederationTransport` trait over `bus`, not a new dependency tree.
2. **There are two transports today, and the decision is to converge on one.** Zenoh carries
   messages and, from the federation slice on, the resource bytes too. The QUIC path is an
   implementation detail being retired for federation traffic — it is not a second federation
   plane, and it must not be advertised as one. Bulk payloads are chunked over Zenoh with an
   explicit credit and a bounded chunk size.
3. **Zenoh supplies connectivity, not any of the semantics the RFC requires.** Node identity,
   grants, delegation provenance, durable task state, and replay protection are all missing from
   the substrate — they are the work.

---

## 3. Gap analysis

Severity: **P0** = blocks the phase-1 exit criteria, **P1** = blocks federation phases,
**P2** = quality/consistency.

| RFC requirement | Today | Gap | Sev |
|---|---|---|---|
| Stable capability ID + version (§6.1) | Tool *names* only; versions live only in Ferris package manifests | No capability identity; no version on a built-in or MCP tool | P0 |
| Provider identity and version (§6.1, §8.3) | Implied by registration path; never recorded | A snapshot cannot say who provided a tool | P0 |
| Input/output schema and content types (§6.1) | json_schema() for tools; ToolResult for Ferris only | Output shape is untyped at the Tool seam (plain Value) | P1 |
| Required permissions, side effects, risk (§6.1) | Per-call Rego checks; Risk exists only in Ferris | No static descriptor, so a composition cannot be validated before activation | P0 |
| Invocation constraints: timeout, payload, concurrency, idempotency (§6.1) | Timeout/concurrency only in ExtensionTool; others absent | Not uniform; built-ins have no declared budget | P1 |
| Streaming / cancellation / progress (§6.1) | AsyncTool has all three; Tool has cancel only | Sync tools cannot honor a deadline at all | P1 |
| Three distinct registries (§17.1) | One dispatch ToolRegistry; discovery reads it whole | No metadata authority; no workspace view; dispatch doubles as catalogue | P0 |
| Capability metadata is untrusted (§6.1) | Descriptions go straight into the prompt | No provenance marking on MCP/remote metadata | P1 |
| Per-invocation execution context (§6.5) | CallContext (Ferris) has extension/correlation/workspace/timeout; PolicyInput has session/preset/tool | No single context type: no principal, task, deadline object, trace id, delegation | P0 |
| Error taxonomy: denial is not transport and not provider (§12.2, §19) | ToolError (NotFound, Failed, InvalidInput, SchemaMismatch, Timeout) | Policy denial is `Failed`; remote unknown-outcome cannot be expressed | P0 |
| Remote capability is distinct from local (§12.1, ADR-007) | AgentCallerTool looks local to the engine | No proxy identity, no dual authorization, no idempotency key | P1 |
| Snapshot of resolved composition (§8.3) | Session JSON + policy denial ring | No per-turn record of providers/versions/policy refs | P1 |
| Composition Engine independent of Agent/ReAct (ADR-002) | n/a | No composition crate; must not be created inside `agent` | P0 |
| AI proposal → deterministic validation (§4.4) | n/a | No validated-composition type to validate against | P1 |

### 3.1 The four gaps that actually cost something

1. **Identity.** `read_file`, `mcp_hello_add`, `com.open1s.tools.echo`, and a remote node's
   build tool are all "a tool name" to the engine. That single flat namespace is what makes
   §17.1's three registries impossible and §12.1's remote distinction unrepresentable.
2. **Context.** Nothing flows from the turn into the tool except the arguments. Ferris invented
   CallContext for this, ExtensionTool reads a workspace path, and policy reads session/preset —
   three partial contexts, none of them the RFC's Execution Context.
3. **Error meaning.** ToolError::Failed(String) is where policy denials, provider crashes, and
   transport failures currently converge. §12.2 requires them to be distinguishable *before*
   anything federated exists, because the remote contract is defined in those terms.
4. **Authority.** call_tool deliberately reaches every registered tool
   (`crates/agent/src/agent/agentic/discovery.rs:17`). That is correct today and fatal for a
   workspace-scoped view: withholding the *definition* is not withholding the *ability*.

### 3.2 The missing split: Local Composition and Federation Composition

Today there is one layer: one registry, one process, one authority. The RFC needs two, and the
distinction is *not* the capability model — it is what owns the decision and the state.

| Dimension | Local Composition | Federation Composition |
|---|---|---|
| What a plan covers | one Workspace | peers, grants, tasks, leases |
| Authority | this node's Security Kernel | each node's own kernel; a grant only attenuates |
| State owner | Workspace + session store | each node's durable task store; events are not state (§15.5) |
| Failure unit | a provider | a peer, a task, a lease |
| Lifecycle | create/start/stop one workspace | enroll/revoke a peer, accept/cancel a task |
| Isolation boundary | filesystem, process, credentials, budgets | peer identity, grants, quotas |
| Transport | in-process call | Zenoh for messages and bytes: one endpoint, one identity |
| Shared | everything below this row: capability IDs, descriptors, invocation contract, error taxonomy | same |

The rules that keep this honest in code:

- A remote capability appearing in a local workspace is a **binding**, not a local provider. It
  borrows another node's Security Kernel to execute; it does not borrow its authority, and it does
  not turn the federation into a workspace.
- **No workspace spans nodes.** A federation composition is a plan over autonomous nodes, never
  one Workspace with a distributed lifecycle. A design that needs a global lifecycle owner has
  merged two security boundaries.
- One Capability Model, two authorization decisions: the local admit and the remote admit are
  different decisions by different kernels, and both are recorded.

The five collaboration modes in the product sketch are not five protocols. Underneath there are
**two primitives** — invoke a capability, and delegate a task — and the five modes are objects and
policies composed from them:

| Mode | Modelled as | Lives where | Not |
|---|---|---|---|
| Capability Federation | `ProviderKind::Remote` binding in a local workspace | local composition + federation data plane | a copy of the remote tool |
| Agent Collaboration | `CapabilityKind::Agent` plus delegation | federation coordination plane | a tool call |
| Task Delegation | a Delegation record with durable state | federation coordination plane | fire-and-forget RPC |
| Resource Federation | `CapabilityKind::Resource` plus a lease | federation data plane (Zenoh, chunked) | raw filesystem or network access |
| Knowledge Federation | a query capability with a data-minimization policy | federation data plane | bulk replication of source data |

That is the design constraint: if a mode cannot be expressed as an invocation or a delegation with
a grant, it is a new primitive and needs its own ADR.

---

## 4. The adapter boundary

### 4.1 Shape: a lowering, with three registries and one authority

```text
provider impls            adapter                 authority                 consumer
---------------           ---------               ---------                 --------
react::Tool/AsyncTool ─┐
ferris Runtime ─────────┤                     CapabilityRegistry      WorkspaceView
McpClient ──────────────┼──► CapabilityProvider ──► (descriptors,    ──► (authorized   ──► ToolRegistry
resource:// URI ────────┤      (describe+invoke)    provider refs)       subset)          (dispatch)
federation peer ────────┘                              │
                                                       ▼
                                              Composition Engine (later crate)
```

Rules:

- The adapter **never** owns state the underlying surface owns. NativeToolProvider holds
  `Arc<dyn AsyncTool>`; it does not copy the tool.
- The adapter **never** decides policy. It calls an injected PolicyDecider; the decision and its
  reference are recorded on the invocation.
- Descriptors are derived from the implementation (describe()), not authored in a second file, so a
  schema cannot drift. Where a declaration must exist (Ferris ToolFile, MCP list_tools), the
  declaration is the source and the runtime is checked against it.
- The view is applied **at registration**, not at dispatch, because dispatch already admits
  everything registered (§3.1 gap 4).

### 4.2 The contract

Proposed crate `crates/capability` — domain only. Dependencies: serde, serde_json, semver,
async-trait, thiserror. No react, no agent, no ferris, no gpui; that is what lets the Composition
Engine live above it without violating ADR-002 or §18.1.

```rust
/// A provider-scoped, stable capability identity.
///
/// The provider is part of the identity: two providers may each ship a tool
/// called `search`, and they are different capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CapabilityId {
    /// The provider that offers it, for example `builtin`, `mcp:github`,
    /// `ferris:com.example`.
    pub provider: ProviderId,
    /// The provider-local name. Stable for the life of that provider.
    pub name: String,
}

/// One versioned, described unit of functionality.
pub struct CapabilityDescriptor {
    /// Stable identity.
    pub id: CapabilityId,
    /// Semantic version of the contract (not of the implementation).
    pub version: semver::Version,
    /// Where the version came from.
    pub version_source: VersionSource,
    /// Semantic kind: tool, skill, service, resource, agent, workflow.
    pub kind: CapabilityKind,
    /// Human-readable name and description (untrusted for authorization).
    pub display_name: String,
    pub description: String,
    /// JSON Schema for arguments, and the content types the result may carry.
    pub input_schema: serde_json::Value,
    pub output: OutputContract,
    /// Static permission and side-effect declaration.
    pub permissions: Vec<Permission>,
    pub side_effects: SideEffectSet,
    pub risk: Risk,
    /// Invocation constraints the runtime will enforce.
    pub limits: InvocationLimits,
    pub supports: FeatureSet,
    /// The name the model sees, when it differs from the capability name.
    pub surface: SurfaceName,
}

/// How a provider is reached: this is the only place transport is named.
pub enum ProviderKind {
    Builtin,
    Ferris,
    Mcp,
    Wasm,
    Process,
    Remote { node: NodeId },
    Resource,
}

#[async_trait]
pub trait CapabilityProvider: Send + Sync {
    /// Stable provider identity.
    fn id(&self) -> &ProviderId;
    /// How this provider is reached.
    fn kind(&self) -> ProviderKind;
    /// Everything this provider currently offers.
    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError>;
    /// Invoke one capability.
    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError>;
    /// Stop one in-flight invocation, if it is still running.
    async fn cancel(&self, invocation: &InvocationId) -> Result<(), CapabilityError>;
    /// Availability, without promising that a later call will succeed.
    async fn health(&self) -> ProviderHealth;
}
```

Deliberate choices:

- **invoke is async-only.** `react::Tool::run` is synchronous, and every existing bridge to it
  spawns a thread plus a fresh current_thread runtime per call
  (`crates/agent/src/mcp/adapter.rs:57`, `crates/agent/src/tools/registry.rs:250`). That
  pattern cannot carry a deadline, cannot cancel, and meters nothing. The sync path stays as a
  compatibility shim at the engine edge; the provider contract is the async one.
- **CapabilityOutput mirrors Ferris ToolResult** (text/image/file/json content plus optional
  structured value). That shape already round-trips MCP content, so no adapter is lossy at that
  seam.
- **surface is separate from the capability name** — see §4.3.
- **health() does not mean available.** describe() is a cache-able catalogue; §12.4 says a stale
  record is not proof of availability, so every invoke re-checks at the provider.

### 4.3 Identity and naming: three names, one rule each

The existing names are load-bearing: presets, policy rules, audit rows, skill manifests, and
binding tests all reference tool names as strings. Renaming for the capability model would be a
gratuitous breaking change, and §28 requires existing behavior to stay regression-tested. So v1
keeps three names with pinned roles:

| Name | Example | Role | May change? |
|---|---|---|---|
| CapabilityId | `builtin/read_file`, `mcp:github/create_issue`, `ferris:com.example/echo` | Governance, audit, snapshot, policy subject | No — the stable identity |
| SurfaceName (tool name) | `read_file`, `mcp_github_create_issue`, `com.example.echo` | What the model calls and what the engine dispatches | Only as a deliberate, tested migration |
| Provider-local id | `read_file`, `create_issue`, `echo` | The underlying surface's own key | Owned by that surface |

Invariants, each enforceable by test:

1. Every provider-registered surface name is unique across the view (today's registry already
   enforces this at `crates/agent/src/tools/registry.rs:45`).
2. Every Tool in a built registry has exactly one descriptor, and vice versa. A phantom descriptor
   and an ungoverned tool are both test failures.
3. descriptor.input_schema is equal (after normalization) to the surface's own schema.
4. A capability exposes at most one surface name in v1; two surfaces for one capability is an
   aliasing decision that needs its own grant model.

### 4.4 Invocation context

```rust
/// Everything one invocation is allowed to know about why it is running.
pub struct InvocationContext {
    pub invocation: InvocationId,
    /// Who is acting: a local turn, or a delegated remote node.
    pub principal: Principal,
    pub workspace: WorkspaceId,
    pub task: Option<TaskId>,
    /// Absolute deadline; providers must not start work they cannot finish.
    pub deadline: Option<std::time::Instant>,
    pub cancel: CancelToken,
    pub trace: TraceContext,
    /// Present only when the call arrived through a delegation chain.
    pub delegation: Option<DelegationChain>,
    /// The decision that admitted this call, for the audit record.
    pub policy: Option<PolicyDecisionRef>,
}
```

Notes that are easy to get wrong:

- **Deadline is absolute**, not a duration. A duration re-started at each hop is how a federated
  call outlives its budget.
- **CancelToken is per invocation**, not per provider. CallTracker
  (`crates/bsh/src/extension/agent.rs:247`) exists because one shared flag cancels two
  overlapping calls; that lesson generalizes.
- **DelegationChain is optional, and None is not "no authority"** — it means the call started
  locally. Sub-delegation is denied by default (§16.4), so the field is a record, not a permission.

### 4.5 Error taxonomy and the mapping

```rust
pub enum CapabilityError {
    /// Arguments did not match the declared schema.
    Invalid { message: String },
    /// A policy refused the call. Never retryable, never a provider fault.
    Denied { reason: String, policy: Option<PolicyDecisionRef> },
    /// The capability does not exist (or was revoked) at invocation time.
    NotFound { capability: CapabilityId },
    /// The provider is not currently able to serve it.
    Unavailable { reason: String },
    /// A quota or rate limit refused it.
    RateLimited { retry_after_ms: Option<u64> },
    /// The deadline expired; the partial value carries what was produced before.
    Timeout { elapsed_ms: u64, partial: Option<serde_json::Value> },
    Cancelled,
    /// The provider ran and failed.
    ProviderFailed { message: String },
    /// The transport failed; delivered distinguishes "never sent" from "sent, no answer".
    Transport { message: String, delivered: DeliveryState },
    /// The request was delivered and its outcome is unknown. MUST NOT be retried blindly.
    UnknownOutcome { message: String },
}
```

DeliveryState is the one field that makes the remote contract honest: a transport error before the
request left is safely retryable, and one after it left is the RFC's unknown outcome (§12.2,
§13.5). Collapsing them into "failed" is how duplicate side effects happen.

Mapping today's errors into this taxonomy — done **at the source**, never by parsing message text:

| Today | Maps to | Notes |
|---|---|---|
| `ToolError::NotFound` | `NotFound` | |
| `ToolError::SchemaMismatch` | `Invalid` | |
| `ToolError::InvalidInput` | `Invalid` | |
| `ToolError::Timeout` | `Timeout` | Keep the captured output as the partial value |
| Policy denial (Rego / `McpPolicy` / Ferris `Decision`) | `Denied` | Carries the rule id today; promote to `PolicyDecisionRef` |
| `ToolError::Failed` from an MCP/HTTP call | `Transport` or `ProviderFailed` | **Must be classified in McpClient, not after** |
| `ToolError::Failed` from a Ferris runtime | `ProviderFailed`, or `Cancelled` when the token fired | `ExtensionTool` already knows which |

The current loss happens because MCP and Ferris errors are stringified into ToolError::Failed
(`crates/agent/src/mcp/adapter.rs:63`, `:138`). Classification has to move into the
provider, which is the crate that still knows what happened.

### 4.6 The decider seam

Policy must not be duplicated into every adapter. One trait, implemented once by the existing
engine:

```rust
#[async_trait]
pub trait PolicyDecider: Send + Sync {
    /// Decide one invocation. The decision is recorded on the context whether it
    /// allows or denies.
    async fn admit(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &InvocationContext,
        input: &serde_json::Value,
    ) -> PolicyDecision;
}
```

Implementation: `agent::policy::PolicySet` with a new `Action::Capability` subject of
`capability.id` instead of `tool.name`. That is additive — existing rules that match on
`input.tool` keep working while the subject gains the provider-qualified form.

### 4.7 Rules that keep the two layers apart

1. **One descriptor, two bindings.** A remote capability's descriptor is a *projection* — same ID,
   same schema, but `origin: Remote { node }` — and its provider is a proxy. Presenting it as
   local is the confused-deputy bug ADR-007 exists to prevent.
2. **A grant is not a policy copy.** The caller-side admit answers "may this workspace use this
   binding"; the provider-side admit answers "may this caller invoke this capability". Neither is
   derived from the other, and the proxy records both `PolicyDecisionRef`s.
3. **Two state machines.** Workspace lifecycle is local and never leaves the node; task and
   delegation lifecycle belongs to the node that owns the task. Everything crossing the boundary is
   an event or a status query, never the state itself.
4. **A remote capability is not a fallback.** If the peer is unreachable the binding is
   `Unavailable` (§23); it must not silently resolve to a local capability with the same name.

---

## 5. Adapter implementations

### 5.1 Built-in tools (native provider)

```rust
/// Lowers the engine's tool traits into the capability contract.
pub struct NativeToolProvider {
    provider: ProviderId,
    tools: Vec<NativeTool>,
}

struct NativeTool {
    descriptor: CapabilityDescriptor,
    surface_name: String,
    body: NativeBody,
}

enum NativeBody {
    Sync(Arc<dyn react::tool::Tool>),
    Async(Arc<dyn react::tool::AsyncTool>),
}
```

- describe() returns descriptors captured at registration (schema read once, cached — the registry
  already does this in its schema cache).
- invoke() dispatches to `AsyncTool::run_with_call_id` when async, and for a sync tool runs
  Tool::run on a bounded blocking pool, not a new runtime per call. The deadline is checked before
  dispatch and after return; a sync body that overruns is reported as Timeout with whatever it
  returned, because it cannot be interrupted.
- cancel() forwards to Tool::cancel / AsyncTool::cancel with the invocation id.
- Permissions are declared statically per built-in and *checked against the policy engine's own
  view at startup*: a tool declared with a write side effect whose Rego rules never mention a write
  action is a configuration error, not a runtime surprise.

Where: `crates/agent/src/capability/native.rs` (agent already depends on react; react must not
gain an agent dependency).

### 5.2 Ferris extensions

The Ferris contract is already most of a provider: tools() returns ToolDescriptors,
call_tool(name, args, context) runs one, and the call context already has workspace and timeout.
The adapter is a thin composition of what exists:

- describe() = Runtime::tools() lifted to CapabilityDescriptor, with the version taken from the
  package manifest (via ExtensionId and the manager) and version_source = Declared.
- invoke() = policy admit, then CallContext (extension, correlation_id = invocation, workspace,
  timeout_ms), then Runtime::call_tool, then ToolResult to CapabilityOutput.
- The generic Runtime trait (`crates/ferris-core/src/lib.rs:341`) provides invoke for
  non-tool contributions; tools ride the richer ferris_runtime contract, which is what
  ExtensionTool uses today.

The real work is **extraction, not invention**: ExtensionTool's invariants — admission, per-call
token, wall clock, post-hook output ceiling, event publication, revocation lease — belong in the
provider, with ExtensionTool reduced to an AsyncTool view over it so the engine path does not
change in the same slice.

Where: `crates/ferris-runtime/src/capability.rs` (depends on ferris-core and the capability
crate), and bsh keeps only the wiring. agent must not gain a Ferris dependency — it does not have
one today, and adding it would invert the layering in AGENTS.md.

### 5.3 MCP

MCP is the clearest case for "adapter, not model" (§17.3):

- one provider **per server**, provider id mcp:server;
- describe() = McpClient::list_tools() (`crates/agent/src/mcp/client.rs:281`) mapped to
  descriptors: version_source = Unknown (MCP carries no tool version — do not invent one), risk =
  Medium, side effects assumed external, permissions = network egress to the server plus whatever
  the server declares;
- invoke() = call_tool (`:305`) with the invocation deadline as the client timeout;
- the namespace-prefix suffix fallback (`crates/agent/src/tools/registry.rs:182`) must go once
  capability IDs exist. It exists because a model can name the bare tool; with a view that
  registers the exact surface name, the fallback only creates a second way to reach a tool by a
  name no descriptor owns — a confusable identity.
- McpPolicy is demoted from authority to **input**: it becomes one source the PolicyDecider
  consults, so there is exactly one decision recorded per call. Today a call can be admitted by
  McpPolicy at registration and never re-checked at invocation.

Where: `crates/agent/src/capability/mcp.rs`, replacing the adapter module's role once the shim
is in place. Keep McpToolAdapter implementing Tool for one release as a deprecated path.

### 5.4 Remote capabilities (federation phase)

RemoteCapabilityProvider holds a NodeId, a grant reference, a peer session, and the remote
descriptor set. It differs from every local adapter in exactly four ways, and each is testable
without a network:

1. **It is marked remote in the descriptor** (ProviderKind::Remote { node }), so nothing can
   mistake it for a local implementation (ADR-007).
2. **Dual authorization**: caller-side admit before send, and the provider side decides again. The
   local record carries both decisions (PolicyDecisionRef twice: local and echoed remote).
3. **Idempotency key** is generated per invocation and echoed; a retry after UnknownOutcome
   requires the same key, and a provider that does not support idempotency must refuse the retry
   rather than silently duplicate.
4. **Transport failure classifies by DeliveryState** — the single field that distinguishes a safe
   retry from an unknown-outcome report.

AgentCallerTool (`crates/agent/src/bus/tool.rs:13`) is **not** the basis for this. It is
agent-to-agent RPC: no grant, no provenance, no idempotency. Reusing it as the remote capability
proxy would import a privilege-escalation surface (§16.4) under a new name.

Where: a new `crates/federation` (depends on the capability crate; transport is Zenoh through
`bus`, see §5.7). Nothing in agent or react moves.

### 5.5 Resources

ResourceTool (`crates/resource/src/tool.rs:13`) is already a provider-shaped object — URI,
description, JSON action in/out — it just has no schema and does not implement the engine trait.
Map it as CapabilityKind::Resource with a schema that describes ResourceAction and one operation
(invoke). Do not force resources into react::Tool just to get them into the view; the view consumes
descriptors, and a resource can be dispatched by its own provider.

For federation, the resource path has **no separate transport abstraction**. Its remote half is
written directly against `FederationTransport`: the resource-stream protocol (open, chunk, credit,
close) is a sequence of federation messages, and the receiving side dispatches through the existing
`Dispatcher` with the authenticated `NodeId` as the peer identity. `resource::Transport` stays
for in-process resources; `QuicTransport` leaves the federation path at parity. One trait, one
authorization point: if the remote path needed a second abstraction to reach a peer, that
abstraction would become a second place to make a policy decision.

### 5.6 The workspace view (the part that delivers §10.3)

```rust
/// The authorized subset of the catalogue for one workspace instance.
pub struct WorkspaceView {
    workspace: WorkspaceId,
    admitted: std::collections::BTreeSet<CapabilityId>,
    budget: ViewBudget,
}

impl WorkspaceView {
    /// Whether this view admits a capability at all.
    pub fn admits(&self, id: &CapabilityId) -> bool;
    /// The descriptors this view projects into a prompt.
    pub fn project<'a>(&self, all: &'a [CapabilityDescriptor]) -> Vec<&'a CapabilityDescriptor>;
}
```

The invariant that makes it real: **the host builds the turn's `ToolRegistry` from `project()`**, so a
capability outside the view is not registered, and call_tool cannot reach it. If the view were
applied only to the advertised array, call_tool would still reach everything
(`crates/agent/src/agent/agentic/discovery.rs:17`) and §10.3 would be cosmetics.

Deferral stays what it is — a prompt-bytes optimization *inside* the admitted set — so the two
mechanisms compose: the view decides what exists, deferral decides what is described up front.

### 5.7 Zenoh as the federation transport

Decision: Zenoh carries the federation protocol **and** the resource bytes. ADR-005 still holds —
the protocol is defined above the transport and the transport may not define authorization
semantics (§15.4) — but there is exactly one federation endpoint per node, and it is Zenoh. The
envelope is JSON; a second encoding would be a protocol change, not a configuration option.

| Protocol need | Zenoh primitive | Key expression | Note |
|---|---|---|---|
| session | `zenoh::open(BusConfig)` | — | reuse `crates/bus/src/session.rs:154`; no new config surface |
| discovery | queryable + liveliness token | `bsh/v1/node/{node}/caps` | bounded timeout; a live token is not availability (§12.4) |
| invoke | query + `query_with_timeout` | `bsh/v1/node/{dst}/cap/{provider}/{name}` | reply is the result or a structured `CapabilityError` |
| cancel | publish | `bsh/v1/node/{dst}/task/{task}/cancel` | idempotent, monotonic per task |
| progress | publish | `bsh/v1/node/{dst}/task/{task}/event` | events are not durable state (§15.5) |
| delegate | query + task topic | `bsh/v1/node/{dst}/task` | the provider owns the durable record |
| resource bytes | query/publish, chunked | `bsh/v1/node/{dst}/res/{lease}` | same transport as the protocol; credit-based, bounded chunks |

Design constraints that follow from the substrate:

- **The key space is addressing, not authority.** Node id first
  (`bsh/v1/node/{node}/**`) so a per-peer Zenoh ACL can be written without enumerating
  capabilities. That ACL is defense in depth; the kernel still decides (§15.4). A peer able to
  publish to a key must still be denied by the receiving kernel.
- **The envelope is JSON, not rkyv.** `crates/bus/src/codec.rs` uses rkyv, whose archived layout
  is build-specific; two BSH versions would decode each other's bytes as garbage or reject them
  outright. The federation envelope is a JSON document with an explicit `protocol` and
  `schema_version`, message id, sender and recipient `NodeId`, issued-at and expiry,
  correlation, idempotency key, content type, and signature. rkyv stays intra-node. JSON costs
  bytes, so payload size is bounded by `InvocationLimits`: binary content is base64 in a field or
  streamed as chunks, never a raw blob inside the envelope.
- **One identity covers both planes.** §11.2 requires identity distinct from the transport
  endpoint, and `NodeId` is an ed25519 public key (`ferris-core` already depends on
  `ed25519-dalek`). The Zenoh id is bound to it by a signed enrollment record, re-verified per
  session, and revocable. Because resource bytes now ride Zenoh, the QUIC mTLS identity and the
  resource announce's HMAC secret are retired from the federation path, not carried alongside it.
- **TLS on a Zenoh link is hop-by-hop.** A routed federation may traverse a relay, so link TLS
  authenticates a hop, never the sender. The envelope signature and the receiving kernel's grant
  check are what make authority end-to-end; the transport must not be treated as the
  authenticator of a principal (§15.4).
- **Discovery announces public metadata only.** Anything under `.../caps` is readable by any
  peer that can subscribe. A descriptor carries `visibility: Public | Granted`; the
  announcement publishes only `Public` fields — name, version, kind, risk class — and never the
  schema body, description, cost, or endpoint details. Everything else requires a grant first.
- **Resource streams are messages, not trait methods.** A lease names a stream inside the same
  session, not a second endpoint: `ResOpen { lease, uri, offset }`, `ResChunk { lease, seq, bytes }`,
  `ResCredit { lease, bytes }`, `ResClose { lease, total, checksum }`. Chunks are bounded and the
  receiver grants credit, so a stalled reader applies backpressure instead of growing memory, and a
  resumed stream continues from the acknowledged offset. Keeping the stream in the message set —
  rather than adding `open_stream` to `FederationTransport` — is what stops one transport trait from
  growing a resource-specific surface.
- **Timeouts come from the deadline.** `query_with_timeout` is the deadline's carrier; a timeout
  *after* the query was sent is `UnknownOutcome`, never a silent retry.

Where: `crates/federation` implements `FederationTransport` over `bus` (not over raw Zenoh), so
the key space, envelope, and session lifecycle live in one crate and the capability crate stays
transport-free.

---

## 6. Where the code goes

```text
crates/capability          (new)  domain: IDs, descriptors, provider trait, errors, context
        ^                                  ^                    ^
        |                                  |                    |
crates/react ------------------------------+                    |
  (may implement capability for its tools, must not depend on agent)
        ^                                                       |
crates/agent -- native + mcp adapters, view, decider -----------+
        ^
crates/bsh -- wiring, per-turn view build, snapshot
        |
ferris-sdk / ferris-runtime -- ferris adapter (depends on capability, not on agent)
```

| Decision | Why |
|---|---|
| The capability crate has **no workspace dependencies** | ADR-002 and §18.1: the Composition Engine must not depend on Agent/ReAct, and the capability domain must not depend on GPUI |
| Native and MCP adapters live in `agent` | agent already owns react and mcp; a separate crate would need agent types back |
| Ferris adapter lives in `ferris-runtime` | Keeps agent free of Ferris (it is today); the contract types are already there |
| Federation provider lives in a new crate, over `bus` | Transport and identity must not leak into agent; §18.1 forbids federation code reaching another node's registries; Zenoh already lives below `react` via `bus` |
| `resource` depends on `capability` for `FederationTransport` | The remote path must implement the one trait, and `capability` is a leaf (no react, no agent, no ferris), so the crate stays standalone |
| No binding surface in v1 | `crates/jsbos/test/api-docs.test.js` requires every public binding member to be documented and the parity test pins both languages; a capability API added now would freeze the contract before it is proven |

---

## 7. Migration slices

Each slice is independently shippable, keeps the workspace green, and can be reverted alone.

### Slice 0 — Descriptors as a sidecar (no runtime change)

- Add the capability crate with IDs, descriptors, errors, context.
- Add describe() for built-in tools, derived from the tools themselves.
- Test: every tool the host builds in caps.rs has exactly one descriptor, and the schema is
  identical. Nothing dispatches through it yet.

**Exit:** the capability tests plus the guard test pass; zero behavior change.

**Status (2026-11-06):** partially implemented. `crates/capability` holds the domain and the
composition resolver; `crates/agent/src/capabilities.rs` lowers a live `ToolRegistry` into a
catalogue. The resolver already covers both composition levels — a spec resolves locally, or binds
a peer under a grant — and the rules that keep them apart are pinned by
`crates/capability/tests/composition.rs` and `crates/agent/tests/capabilities.rs`.
Metadata is now declarable on the tool itself (`react::ToolMetadata`), and the host declares it
for the built-ins it composes through `agent::tools::DeclaredTool` / `DeclaredAsyncTool`
(`crates/bsh/src/tool_metadata.rs`): shell, the eight file tools, plan, the goal pair, and the job
tools. The catalogue validates declarations (an unknown risk class or side effect is an error, not
a silent default), and a guard test builds a real agent and asserts the declarations reach the
registry (`crates/bsh/src/caps.rs`, `the_builtin_tools_declare_their_capability_metadata`).
The web-search and subagent tools declare theirs too (network, spend, and a run read), and MCP tools
now declare network egress, medium risk, and `net.egress` — with **no version**, because MCP carries
none, so the catalogue records `VersionSource::Unknown` instead of inventing one. The MCP gateway
declares the same. Extension (Ferris-runtime) tools declare medium risk, an `extension` side
effect, `extension.call`, and Unknown version; because packages register them LIVE, the plan and
snapshot read a merged registry (base + live overlay), and a test pins that a live tool reaches the
composition. The capability registry itself stays opt-in. Model-authored compositions go through `capability::proposal`: approved ids only, a risk ceiling, no remote binding or peer when the host forbids it, and no unseen capability; `resolve_proposal` refuses a spec the host resolver would accept. The gate returns an `ApprovedComposition` token that `resolve_approved` requires; the only constructor is `approve`, pinned by a compile_fail doctest. The public host entry point is named `resolve_host`, so no neutral `resolve` remains for a model path to call by accident. The producer is `agent::composition`: `prompt` renders the catalogue, `parse` rejects unknown fields and unknown ids, and `compose` runs parse -> approve -> resolve_approved with no bypass.

### Slice 1 — Native provider behind a shim

- NativeToolProvider plus a CapabilityTool adapter that implements AsyncTool by calling invoke, so
  the engine path is unchanged.
- Equivalence fixtures: for a fixed call table, the result and the error class are identical
  through the old and new paths.

**Exit:** equivalence suite green; the shim is the only new dispatch path.

**Status (2026-11-06):** the provider half is implemented in
`crates/agent/src/capabilities/native.rs`: describe, invoke, cancel, health; schema validation
before dispatch; synchronous bodies on the blocking pool; the absolute deadline; per-invocation
cancellation; classified failures. The engine-facing shim now exists
(`agent::capabilities::CapabilityTool`: one capability exposed as an `AsyncTool`, with a
deadline, a cancellation token, and the capability-to-tool error mapping). The switch exists:
`general.capability_registry = true` rebuilds the turn's registry as `CapabilityTool` shims over a
`NativeToolProvider` that keeps the raw tools, then swaps it in through `Agent::set_registry`; a
failure keeps the raw registry and logs, so the turn still runs. A test asserts the raw synchronous
tool is gone, the shim is async, and the shim still executes the real tool. A call-table test now
compares the two registries: a successful call returns the same value, and schema rejection, a body
failure, and an unknown name land in the same class on both paths. The shim now also carries the
facts that decide PRESENTATION, not execution: `ToolFacts` (deferred, skill, cancelable) and
`ProxySource` are read from the replaced tool at compose time, so a flag-deferred tool stays out of the
advert and a dispatcher still names its real target (tests in
`crates/agent/tests/capability_tool.rs`). The switch still defaults off for cost, not for semantics:
composing the registry rebuilds descriptors and a runtime on every build, so making it the default
needs the composition cached first. It is: `composition_for` keys a thread-local cache on the tool
surface (names, schemas, declarations), reuses the plan and snapshot, writes the log once per distinct
composition, and caches the shim registry under the surface mixed with the turn principal. The
user-facing record is [bsh-federation.md](bsh-federation.md). General settings reports the node,
discovered peers, remote capabilities, and active delegations from the runtime's cache and its
delegation store (`active_delegations`), all read-only. One two-node acceptance test runs discovery,
composition, a remote call, a delegation submit/status/cancel, a quota-bounded resource stream, and
revocation across the same pair (`two_nodes_share_capabilities_delegations_and_bytes_until_revoked`). Robustness is
pinned too: the canonical signing form is specified in [federation-signing.md](federation-signing.md)
and a fixed canonical/signature pair is pinned in crates/federation/tests/signing_vectors.rs;
forged, expired, and future-schema frames are dropped without a reply while an entitled
peer is still served (`forged_expired_and_wrong_version_frames_are_dropped`), and a remote tool whose
peer leaves fails without a local fallback (`a_trusted_peer_capability_is_offered_as_a_tool`). Every
invocation now leaves a record on each side that decided (`federation::audit`), and the bsh runtime
writes them to a bounded `audit.jsonl` beside the identity; the record type cannot carry arguments
or results. Transport TLS is configurable (`bus::TlsConfig`, emitted as `transport.link.tls`) and the
federation node opens its session through `Bus::try_from`, so a TLS listener without usable material is a
startup error rather than a silent plaintext session. The audit export includes the newest 50 records as a `federation` array. Records cover
`invoke`, `delegate`, `delegation_status`, `delegation_cancel`, and resource opens today, with a
`subject` naming the delegation or lease. A resource stream records its end once on EACH side
(provider: quota, failure, completion; caller: close, error, or a dropped reader) with the bytes
moved, so the two byte counts can be compared. Discovery is recorded on both sides too, and
`bsh federation status` prints the five most recent decisions. The shell reads those five once at launch into
`AppState.federation_recent`, so the General row can show the newest one without touching the log
per frame; a successful write also broadcasts in-process and the shell drains that channel (a
non-blocking receive) so the row follows new decisions while it runs. Under the row, up to five
enrolled peers are listed (short id, trust state, grant count) from a launch-time PeerStore read. The
right panel's Federation tab is the fuller surface: every peer with its grants, plus enroll, grant (with an optional
expiry), revoke-grant, and revoke-peer actions (`ui/right_panel.rs`), applied live when a node runs
and written to the store file otherwise; a peer revoke asks for a second press, and two switches write `enabled` and `include_remote_tools` through the settings file, and the status line can Start or Stop the node (Stop releases the data
directory lock). One writer per data
directory: the runtime holds `node.lock` for its lifetime, the CLI's mutating commands and the pane's
file edits take it too, and the pane distinguishes "off in settings" from "enabled but failed to start".
Nodes also announce themselves on `bsh/v1/announce` (signed presence + public descriptors, every ten
seconds); the pane lists those discovered peers with their advertised capabilities, excludes itself,
and grants only what the reader selects. A bsh node advertises the read-only capabilities configured under `[federation] expose` (read_file,
read_files, glob, grep), served from a workspace confined to `expose_root`; anything else is
refused at start. Without `expose` the list is empty and the discovered row says so. The sink writes a
per-process `audit-<pid>.jsonl` so two instances sharing a data directory never compact each other's
inode; readers merge the per-process files with the legacy log by event time.

### Slice 2 — Workspace view

- Build the turn's registry from WorkspaceView::project() in the host.
- Static guard: a tool outside the view is not merely deferred — it is absent, and a call_tool
  attempt is NotFound.
- Measure prompt bytes and tool count before/after for a preset with extensions and MCP.

**Exit:** §10.3 is observable as a number, not a claim.

**Status (2026-11-06):** the runtime half exists — `capability::WorkspaceRuntime` instantiates a
resolved plan and dispatches local and remote bindings through one call path
(`crates/capability/src/workspace.rs`). The host now projects a real turn build through the model:
`plan_for_registry` resolves the final registry as a local composition, and a test asserts the plan
and the registry name exactly the same set (`crates/bsh/src/caps.rs`). The switch itself —
registering `CapabilityTool` shims instead of raw tools — is not made yet.

### Slice 3 — Snapshot and audit

- Emit a composition snapshot per turn build: resolved capability IDs, versions, provider IDs, view
  allowlist, policy version hash. No secrets (§8.3).
- Reuse the existing audit path rather than adding a second log.

**Exit:** a turn can be explained after the fact from one record.

**Status (2026-11-06):** `capability::CompositionSnapshot` records the composition id and version,
the workspace, every capability's id/version/provider/origin (a peer and its grant for a remote
binding), and a policy reference — never a secret. A bsh test snapshots a real turn build
(`a_turn_build_snapshots_its_composition`). Every real build now takes one: `BuiltAgent` carries it,
the `INSTRUCTION_BUILD` event publishes the capability count, and the build logs the composition at debug
level. Every build also appends it to a bounded workspace log (`compositions.jsonl`: append-only,
512 KiB cap, the newest 200 records survive compaction, and a torn line never hides the records
around it) at `Settings::compositions_path()`, which is scratch-safe in tests. The loader exists but is
read by the audit export: `write_audit` adds a `compositions` array beside the view's own JSON
(shape preserved) containing the last 50 records loaded from `compositions_path()`, so an exported
audit explains which capabilities a turn composed, at which versions, and which ran on a peer. Federation identity and trust persist too:
`NodeIdentity::load_or_create` keeps the Node's seed file (owner-only on Unix) and
`PeerStore::save`/`load` round-trip enrollments, grants, expiry, and revocation atomically.
The headless management surface is real: `bsh federation status|enroll|revoke|grant|revoke-grant`
operates on the same files a running node loads, so a peer can be enrolled before any window
exists (`crates/bsh/src/federation.rs`). The app runs the node itself when `[federation]` enables it
(`crates/bsh/src/federation_node.rs`): it loads the identity and peers, opens the Zenoh session from the
configured endpoints, subscribes to the inbox, and starts the router — once per process, before
the window, and a node that cannot start is logged rather than blocking the app. It also keeps a
discovered catalogue per trusted peer (`FederationNode::peer_catalogue`: discover, verify, insert only the
public descriptors), refreshed at startup and every 30 seconds, so `status` reports
`discovered_peers=N` and the host has each peer's signed capabilities cached for composition.
`federation_node::compose` now builds one `WorkspaceRuntime` over the local tool registry and every
cached peer catalogue: local capabilities bind to `NativeToolProvider`, remote ones to a
`RemoteCapabilityProvider` per peer, and an id a local tool already owns is never shadowed by a
remote one. The status line reports the composed capability count and how many are remote, and a
test invokes a local tool and a remote capability through the same runtime (RFC section 27 MVP:
"a Workspace that composes one local and one remote capability"). With
`[federation] include_remote_tools = true`, the turn build attaches those remote capabilities to
the agent as live `CapabilityTool` shims before the prompt guidance and the context meter are
computed; a name the live registry already holds is skipped, so the local tool always wins. The
switch is off by default, and without a running node the build simply has no remote tools.

### Slice 4 — Ferris extraction

- Move ExtensionTool's admission/cancel/limits/events into the provider; keep ExtensionTool as an
  AsyncTool over it.
- Regression: the existing extension tests (`crates/agent/tests/extensions.rs` and the bsh
  extension tests) must pass unchanged.

### Slice 5 — MCP

- Descriptors from list_tools; error classification inside McpClient; McpPolicy as decider input;
  suffix fallback removed after a deprecation note in CHANGELOG.

### Slice 6 — Zenoh transport, then the remote provider (RFC phase 4)

- Envelope type in `crates/federation`: versioned, signed, JSON; golden fixtures that pin forward
  compatibility (a v1 reader must reject or explicitly ignore a v2 field set, never silently).
- `ZenohTransport` over `bus`; a two-node test opens two in-process sessions in peer mode with
  scouting and no router, extending what the existing `server.rs` test already does with one session.
- Then `RemoteCapabilityProvider`: remote-marked descriptor, dual authorization, idempotency,
  delivery-state error mapping, fault tests per §22.3.
- Resource path: the stream protocol (open, chunk, credit, close) implemented directly on
  `FederationTransport`; `resource::Transport` stays for in-process only, and the QUIC path leaves
  the federation path at parity. Drive it through an in-memory `FederationTransport` in tests, so
  credit and backpressure are provable without Zenoh.

**Exit:** a capability is invoked across two in-process nodes; denial, timeout, peer loss, and
duplicate delivery each produce the specified error; a v1 and a v2 node negotiate the envelope
version.

**Status (2026-11-06):** the foundation is implemented. `capability::transport` defines the
transport seam (connect / send a sealed frame / close, reporting DeliveryState);
`crates/federation` provides Node identity (Ed25519, a NodeId is the public key), the signed
JSON envelope with protocol/schema negotiation and expiry, and an in-process transport used by the
tests (`crates/federation/tests/federation.rs`). The Zenoh transport is implemented
(`crates/federation/src/zenoh.rs`) and verified by a loopback test across two real sessions
(`crates/federation/tests/zenoh_transport.rs`). Peer trust and grants are implemented
(`crates/federation/src/peers.rs`: enroll, revoke, grant with expiry, and an authorize() that
returns a Denied reason rather than a bare false), and discovery is a signed, public-only record
(`crates/federation/src/discovery.rs`). `federation::FederationNode`
(`crates/federation/src/service.rs`) performs the dual-authorized invocation: caller-side admit, sealed
request, provider-side admit, sealed result, correlation routing, and UnknownOutcome when a sent
request gets no reply. The peer-as-provider adapter is implemented (`crates/federation/src/provider.rs`),
and delegation has a validated state machine with attenuation (`crates/federation/src/delegation.rs`).
All four federation planes now have a wire path on the node: dual-authorized invocation, delegation
submit/status/cancel, a chunked resource stream with credit and per-chunk lease charging, and signed
discovery (`crates/federation/tests/discovery_wire.rs`). What remains is host wiring: building the
turn's tool view from a workspace (`caps.rs`) and declaring per-built-in metadata.

---

## 8. Testing strategy

**Conformance suite, one implementation, every adapter.** A trait with the same cases run against
NativeToolProvider, the Ferris adapter, the MCP adapter (against a fake server), and the remote
provider (against an in-process peer):

1. duplicate surface name is refused;
2. descriptor schema equals the surface schema;
3. unknown capability is NotFound, not Denied;
4. missing required argument is Invalid before the body runs;
5. policy denial is Denied and the body never runs;
6. timeout produces Timeout and the body observes cancellation;
7. cancel during work yields Cancelled, and a later call still works;
8. output over the cap is truncated per contract, not dropped;
9. provider crash is ProviderFailed, never Denied;
10. transport failure before send is retry-safe; after send it is UnknownOutcome;
11. describe then invoke round-trip for every declared capability (no phantom descriptors);
12. revoked capability is NotFound at invocation, without a restart;
13. a resource stream respects credit (the sender blocks when credit is exhausted), resumes at the
    acknowledged offset, and rejects a lease id it does not own.

Cases 11 and 12 catch the real-world failures: a catalogue that advertises what the runtime no
longer offers, and a revocation that only applies to the next process start. Case 13 catches the
stream that looks correct until credit runs out.

**Static guards, in the style this repo already uses** (`crates/jsbos/test/parity.test.js` and
the API-docs test): the descriptor set and the registered tool set must be equal as sets, checked
in a test that builds the real host composition rather than a fixture. This is the cheapest defense
against the adapter becoming a second, drifting catalogue.

**Fixtures, not prose.** Version pinning for capability IDs and surface names belongs in a JSON
fixture compared across Rust and (later) the bindings, the same way memory_ranking.json pins
ranking across languages.

---

## 9. Risks

| Risk | Why it happens here | Mitigation |
|---|---|---|
| The adapter becomes a god layer | Every provider needs "just one more" policy/limit/quota branch | Adapter is describe + invoke only; policy via injected decider; limits as data, not per-adapter code |
| Three names for one tool | Compatibility pressure from presets, audit, and tests | §4.3 invariants plus the set-equality guard |
| Version theatre | Semver is required by the RFC, MCP supplies none | VersionSource with Declared, Derived, Unknown; never invent a version |
| Two authorities | McpPolicy and Rego both can deny today | One PolicyDecider, one recorded decision per call |
| Schema duplication | A descriptor file that drifts from the code | Derived descriptors; declared descriptors are the source and the runtime is checked |
| Engine rewrite creep | The view "needs" a new engine loop | Slices 0-3 do not touch the ReAct engine; the shim lives at the tool trait |
| Binding churn | Capability types look like a public API | Rust-only until the contract survives a release |
| rkyv on the wire | The bus codec is rkyv and its layout is build-specific | JSON envelope with explicit `protocol` and `schema_version`; rkyv stays intra-node |
| Key space mistaken for authority | Zenoh ACLs look like policy | ACL is defense in depth; the receiving kernel decides |
| Discovery leaks metadata | The caps key is readable by any subscriber | `visibility` on descriptors; announce name, version, kind and risk only |
| Two data planes outlive the decision | QUIC has mTLS and the resource announce has an HMAC secret | One `NodeId`; federation endpoints are Zenoh only; QUIC is not advertised once chunked transfer reaches parity |
| Bulk bytes over a message bus | Payload limits, head-of-line blocking, memory pressure on large artifacts | Credit-based chunking with a bounded chunk size; a lease names a stream, never a second endpoint |
| The trait grows resource-specific methods | One transport must serve messages and megabytes | Streams are message sequences keyed by lease; the trait keeps connect/send/close |
| JSON costs bytes and CPU | JSON is larger and slower than rkyv | Bound payloads by `InvocationLimits`; base64 only for small binaries; measure before optimizing |

---

## 10. Open questions for the RFC owners

Beyond §26 of the RFC:

1. **Is a capability ID allowed to be provider-qualified**, or must a capability ID outlive its
   provider (so a second provider can satisfy the same ID)? This decides whether substitution
   (§6.3 "conflict and substitution policy") is expressible at all.
2. **Who owns the version of an MCP tool?** Unknown is honest but makes a composition range check
   meaningless. Either ranges are advisory for Unknown, or MCP capabilities are excluded from
   pinned compositions.
3. **Does the workspace view apply to Skills and Resources too**, or only to Tools in v1? The RFC
   says capabilities; the engine has one tool surface today, and find_skill already has its own
   withholding mechanism (`crates/agent/src/agent/agentic/discovery.rs:124`).
4. **Is deferral part of the descriptor or the view?** It is a prompt-budget decision, so it reads
   like the view's; but a provider knows whether a tool is cheap to describe.
5. **May a remote capability be substituted by a local one** under the same ID when a peer is
   offline? §23 says remote-dependent capabilities must degrade explicitly, which implies no
   silent substitution — worth stating in ADR-007's terms.

---

## 11. Checklist: adding a provider

The operational answer to "how to adapter", in order:

1. Decide the provider id and the provider kind. If it needs a transport, say so here and nowhere
   else — a remote provider goes through §5.7 and nothing else may name Zenoh.
2. Implement describe() from the implementation, not from a parallel list. If only a declaration
   exists (package manifest, list_tools), make the declaration the source and verify the runtime
   against it.
3. Map every capability to one surface name that is unique in the view; keep the old name
   byte-identical if one exists.
4. Fill the descriptor: version and version source, kind, schema, output contract, permissions,
   side effects, risk, limits, feature set.
5. Implement invoke() as the shortest path to the existing body. Do not re-implement what the
   underlying runtime already does (timeouts, sandbox, auth).
6. Classify errors **at the point of failure**. Never parse message text.
7. Forward cancellation to the underlying body and honor the absolute deadline before starting.
8. Take the PolicyDecider as a dependency; never decide policy inside the adapter.
9. Add the provider to the conformance suite (§8) before wiring it into a workspace.
10. Add the set-equality guard: descriptors equal registered surfaces, in a test that builds the
    real host, not a fixture.

If a provider cannot do 2, 4, or 6, it is not ready to be composed — it can still run as a legacy
tool, but it cannot claim a capability ID.
