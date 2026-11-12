# Ferris Evolution RFC: Cordis-Inspired Composable Extension Runtime

- **Status:** Proposed
- **Date:** 2026-11-12
- **Scope:** BOS Ferris framework (`ferris-core`, `ferris-runtime`, `ferris-sdk`, BSH host adapter)
- **Decision class:** Architecture and migration direction; no implementation authorization is implied
- **Related:** [BSH Extension Platform](bsh-extension.md) · [Ferris Framework](ferris-framework.md) · [Ferris SDK](ferris-sdk.md) · [BOS Architecture](../ARCHITECTURE.md)

## 1. Executive summary

Ferris should evolve from a package-oriented extension loader into the **composable extension and capability runtime of BOS**.

It must remain an extension framework. It must not become a second Agent Runtime, task orchestrator, workspace manager, or UI host. BOS owns Agent, Task, Workflow and delegation semantics. BSH owns the product workspace, GPUI and user interaction. Ferris owns extension identity, package admission, extension lifecycle, capability/service publication, runtime-provider lifecycle and host contribution ownership.

The design takes the strongest ideas from `cordis-rs`—Context, Plugin preparation, Fiber lifecycle handles, typed Services, explicit Event routing, dependency convergence and generation-owned Effects—and maps them onto Ferris's existing package, manifest, provider and host contracts.

The intended result is:

> **Ferris: the composable, lifecycle-safe and policy-governed extension runtime for BOS.**

The architecture has five properties:

1. **Composable:** capabilities and services are independently described, resolved and assembled; package installation is not the same thing as activation.
2. **Temporally safe:** every activation generation owns all registrations, tasks, listeners, service publications and host contributions it creates.
3. **Runtime-neutral:** native, declarative, process, MCP, WASM Component, Extism, workflow and Cordis-compatible providers implement one Ferris lifecycle contract without pretending their isolation and cancellation semantics are identical.
4. **Governed:** discovery, dependency satisfaction, capability authorization and invocation are separate decisions. No declaration or registration grants authority.
5. **Federation-ready:** local capabilities have stable contracts that can later be proxied across BOS nodes, while remote identity, authorization, execution and audit remain owned by the provider node.

This RFC is an evolution plan, not a claim that all target semantics already exist.

## 2. Context and current baseline

### 2.1 Existing Ferris must be preserved

The repository already has a substantial Ferris implementation:

- `ferris-core` owns host-neutral manifest, package, manager, capability, lifecycle, resolver, service, event, effects and contribution contracts.
- `ferris-runtime` supplies runtime contracts, adapters, a runtime catalog and optional provider implementations.
- `ferris-sdk` is both the extension-author API and the host-facing facade.
- BSH consumes Ferris through `ferris-sdk` and host adapters; it should not depend directly on `ferris-core` or `ferris-runtime`.
- Manifest 2.0 is the package contract. The generic contribution envelope is `{ kind, id, payload }`.
- The manager owns install/plan/apply/reload, and activation owns runtime and contribution leases. Failed activation is expected to roll back already-started extensions.
- The runtime contract currently centers on `start`, `invoke` and `shutdown`; a lower-level provider contract also has activate/deactivate/cancel semantics.

These are valuable foundations. The new design must extend and normalize them, not replace them with a greenfield framework.

### 2.2 Current design pressure

The package-centric model is necessary but insufficient for a long-lived composable platform:

- A package is a distribution and trust unit; it is not necessarily the smallest unit that should be activated or exposed.
- A single extension lifecycle state does not fully describe multiple independent activation scopes or replacement generations.
- Dependency resolution must converge on a valid global solution, not merely find a locally plausible traversal order.
- Services need exact contract/version/scope/realm identity. A name match or parent-scope fallback is not a safe substitute.
- Events need explicit routing semantics and lifecycle-owned subscriptions.
- Contributions, prompts, skills, listeners, service publications, background tasks and runtime processes must be revoked as one coherent ownership set.
- Runtime cancellation, drain and shutdown must be observable and provider-specific. Dropping a Rust future is not proof that an external process or remote request stopped.
- Host integration must not leak BSH/GPUI types into Ferris core.

### 2.3 Important naming distinction

The existing `cordis` runtime kind in Ferris is a **JavaScript Cordis package adapter** that launches the package through the Node bridge. It is not the same thing as adopting the Rust `cordis-rs` semantic core described by this RFC.

The two must not be conflated:

- **Cordis-compatible runtime adapter:** executes an existing Cordis-style JavaScript extension.
- **cordis-rs semantic foundation:** provides Rust-side composition and lifecycle concepts that Ferris may reuse or adapt.

The existing adapter remains a supported runtime. This RFC does not replace it.

## 3. Goals and non-goals

### 3.1 Goals

- Define a stable, host-neutral composition and lifecycle model for Ferris.
- Support multiple activation instances/scopes of an extension where the host policy permits it.
- Make each activation generation an explicit owner of effects and contributions.
- Define typed service identity, exact realm resolution and reactive dependency convergence.
- Define explicit event roles, routing and interceptor authorization.
- Make update, restart, replacement, rollback, drain and disposal semantics deterministic.
- Preserve the current SDK and Manifest 2.0 compatibility boundary.
- Provide one provider-neutral invocation surface with explicit provider capability declarations.
- Keep the framework independently testable without BSH, Agent or GPUI dependencies.
- Establish conformance gates that make the framework suitable for future federation.

### 3.2 Non-goals

Ferris will not own:

- Agent reasoning loops, model selection or conversation/session semantics.
- Agent Task, Workflow, delegation or multi-agent orchestration.
- BSH Workspace or GPUI view lifecycle.
- Global federation membership, remote node identity or cross-node task ownership.
- A universal scheduler that erases the execution differences among native, WASM, process and remote providers.
- An automatic trust mechanism based on package signatures, publisher reputation or successful discovery.

These belong to BOS, BSH or the provider node's security boundary.

## 4. Ownership and system boundaries

```text
                         BSH Host
              GPUI / Workspace / User Interaction
                           |
                  Host Adapter + Contributions
                           |
                    BOS Agent Runtime
              Agent / Task / Workflow / Delegation
                           |
                    Capability Interface
                           |
                    Ferris Framework
  +------------------------------------------------------+
  | Package & Manifest | Composition | Lifecycle/Fibers |
  | Capability & Service Registry | Events | Effects    |
  | Policy Integration | Audit Hooks | Runtime Manager  |
  +------------------------------------------------------+
                           |
                    Runtime Providers
  +------------------------------------------------------+
  | Builtin | Declarative | Process | MCP | WASM | Extism |
  | Workflow | Cordis-compatible JS adapter | Native      |
  +------------------------------------------------------+
                           |
                 OS / Process / WASM / Network
                           |
              Optional remote BOS/Ferris provider
```

### 4.1 Authoritative ownership

| Resource or lifecycle | Owner | Ferris responsibility |
|---|---|---|
| Extension package | Ferris | Install, validate, version, trust metadata, store and uninstall |
| Extension activation | Ferris | Prepare, admit, supervise, update and dispose activation generations |
| Capability contract | BOS capability layer, integrated by Ferris | Bind extension-provided implementation to a stable contract |
| Runtime instance | Ferris runtime manager/provider | Construct, invoke, cancel, drain and stop provider instance |
| Host contribution | Host adapter, leased by Ferris | Apply and revoke through an explicit lease |
| Agent / Task / Workflow | BOS Agent Runtime | Ferris exposes capabilities; it does not own these lifecycles |
| Workspace / GPUI surface | BSH | Ferris contributes through host-neutral contribution contracts |
| Remote node / remote task | Provider BOS node and federation protocol | Ferris exposes local extension capabilities to the local capability boundary only |

There must be one lifecycle authority for each resource. Do not create a second Fiber manager in BSH, or a second Agent/Task manager in Ferris.

## 5. Cordis concepts mapped to Ferris

The design adopts semantic ideas, not a blind type-for-type copy.

| cordis-rs concept | Ferris target concept | Adaptation |
|---|---|---|
| `Context` | `ExtensionContext` / `ActivationContext` | Immutable, cheap view of the current activation and authorized host services |
| `Plugin` | Ferris extension implementation | Reusable behavior with typed config and a prepare phase |
| `prepare()` | `prepare()` | Validate/configure/compile without publishing live lifecycle state |
| `PreparedPlugin` | `PreparedExtension` | Immutable prepared artifact admitted by the manager |
| `FiberHandle` | `ActivationHandle` | Explicit owner handle for one admitted activation generation |
| `Fiber` | `Activation` | Runtime lifecycle instance, distinct from installed package identity |
| `Service` | Ferris typed service publication | Exact contract + version + realm identity |
| `Event` | Ferris typed event | Explicit Observer/Responder/Mapper/Around role and routing |
| `Effect` | `EffectScope` | Revertible, generation-owned resource registration |
| Dependency convergence | Ferris resolver/convergence engine | Recompute activation readiness as providers appear/disappear |
| `update` / `era_swap` | Activation update / generation replacement | In-place compatible change versus new-generation replacement |

### 5.1 Keep Ferris vocabulary stable

The public SDK should expose Ferris terms such as `Extension`, `ActivationHandle`, `Capability`, `ServiceContract`, `ContributionLease` and `EffectScope`.

Do not require extension authors to understand internal Cordis Fiber IDs, internal scheduler types or Cordis-specific error types. If `cordis-core` is used internally, Ferris must own the compatibility adapter and public error model.

### 5.2 Dependency strategy

Adopt a **semantic-core-first, adapter-contained** strategy:

1. Audit the exact `cordis-core` release/API and its MSRV, feature graph, synchronization model and runtime assumptions against BOS's workspace toolchain.
2. Build a small compatibility prototype in a non-production branch. Verify Context, prepared plugin admission, FiberHandle ownership, typed service realms, event routing and effect disposal.
3. If the semantics fit without introducing duplicate managers or unacceptable dependency weight, depend on one exact `cordis-core` version behind a private Ferris adapter.
4. Keep Cordis types out of `ferris-sdk` public signatures and serialized manifests.
5. If the upstream API cannot be safely embedded, implement the required semantics in Ferris with a written compatibility matrix. Do not vendor or fork the entire framework merely to preserve names.

Do not add a second async runtime or a second independent lifecycle authority. The dependency decision requires an ADR after the prototype; this RFC does not silently pin a version.

## 6. Target architecture

### 6.1 Composition Kernel

The Composition Kernel turns installed packages and registered providers into a validated activation plan.

Responsibilities:

- Discover candidate extensions and capability providers.
- Validate package identity, manifest, host/API compatibility and trust metadata.
- Resolve versioned package dependencies and typed service requirements.
- Detect conflicts, cycles, ambiguous providers and unsatisfied required dependencies.
- Validate capability requirements and host contribution schemas.
- Produce a deterministic, explainable plan with a stable plan digest.
- Separate plan construction from side effects.

Required resolver properties:

- Global constraint solving or an equivalent deterministic algorithm; greedy DFS without backtracking is not sufficient when constraints interact.
- Stable tie-breaking independent of filesystem iteration or hash-map order.
- Explicit conflict and ambiguity diagnostics.
- Optional dependency semantics that never silently weaken required dependencies.
- Reproducible plan output for identical inputs and policy versions.
- No package activation or external process launch during planning.

### 6.2 Lifecycle Kernel

Ferris must distinguish three identities:

1. **Package identity:** stable extension ID + package version + content digest.
2. **Activation identity:** one live activation in a particular host/scope.
3. **Generation identity:** one immutable activation generation, incremented on replacement.

An extension may be installed once and admitted into multiple activation scopes when its manifest, host policy and runtime provider allow it. The manager must not assume package ID uniquely identifies a live runtime process.

Target lifecycle:

```text
Discovered
   |
Validated
   |
Prepared                 # no live host effects
   |
Admitted                  # manager owns ActivationHandle
   |
Starting
   |
Active <------> Pending   # dependency availability may converge
   |
Updating / Replacing
   |
Draining
   |
Disposing
   |
Disposed

Any non-terminal phase may transition to Failed with a structured cause.
Retry creates a new attempt; it must not resurrect a disposed generation.
```

The exact enum names may differ from the existing lifecycle state machine. The required semantics are normative.

**Preparation is not activation.** Manifest validation, config decoding, artifact verification and WASM compilation may happen before admission. Publishing services, subscribing to events, spawning persistent tasks or registering host contributions happens only after the manager admits an activation.

### 6.3 ActivationHandle and ownership

The manager returns an explicit handle after successful admission:

```rust
// Illustrative target API; not a current compilable SDK contract.
pub struct ActivationHandle {
    activation_id: ActivationId,
    generation: Generation,
    control: ActivationControl,
}

impl ActivationHandle {
    async fn ready(&self) -> Result<ActivationStatus, FerrisError>;
    async fn update(&self, change: PreparedChange) -> Result<(), FerrisError>;
    async fn replace(&self, change: PreparedChange) -> Result<ActivationHandle, FerrisError>;
    async fn drain(&self, deadline: Deadline) -> Result<DrainReport, FerrisError>;
    async fn dispose(&self) -> Result<DisposalReport, FerrisError>;
}
```

Rules:

- Dropping a handle must not implicitly perform asynchronous teardown.
- The manager/host root must retain handles and explicitly dispose them.
- Disposing a handle is idempotent.
- A stale generation cannot publish contributions or invoke new work.
- Every asynchronous callback carries activation and generation identity.
- A generation replacement revokes old-generation effects and leases even if old callers still hold cached references.
- The handle reports state and disposal outcome; it does not expose mutable internals.

### 6.4 EffectScope and temporal composability

Every activation generation owns one root `EffectScope`. Child scopes may be created for a service, event subscription, task group, contribution group or invocation class.

An effect is any resource whose lifetime must be bounded by an owner:

- Service publication.
- Event listener or interceptor.
- Host contribution lease.
- Background task or timer.
- Process/WASM runtime resource.
- Temporary file, socket or other provider resource.
- Policy/audit subscription.

Required behavior:

- Reverse registration order on disposal.
- Child scopes dispose before parents.
- Cleanup is idempotent.
- Late registration after disposal is rejected or immediately cleaned up; it must never leak.
- One cleanup failure does not prevent remaining cleanups.
- Disposal returns a structured report with failures and timeouts.
- Async cleanup has a bounded deadline and can be retried/reconciled where safe.
- Cleanup ownership is explicit; no reliance on object drop for asynchronous resources.

The existing `EffectScope` is a strong foundation and already documents reverse cleanup, late registration handling and disposal reporting. The RFC requires the manager, service/event registries and host contribution sink to use the same ownership model end-to-end.

### 6.5 Typed Service Registry

A service slot must be identified by an exact key:

```text
(ServiceContractId, ContractVersion, ServiceRealm)
```

A `ServiceRealm` identifies the isolation domain in which the service is published (for example node, workspace, extension activation or invocation). It is not an implicit parent chain.

Rules:

- Exact realm lookup by default; no implicit parent fallback.
- Explicitly declared realm projection only when the contract allows it.
- Contract identity is stable and independent of provider display name.
- Version compatibility is checked during resolution and revalidated before invocation.
- A publication is owned by the publishing activation generation.
- Replacing or disposing the provider invalidates the publication and emits a service availability transition.
- Consumers declare required/optional dependencies and receive an explicit Pending/Ready/Unavailable state.
- Service values are typed in-process where possible; serialized transport is an adapter, not the type system.

Avoid a global `Any` map as the semantic service API. Type erasure may exist internally at the host boundary, but contract identity and checked downcast errors must remain explicit.

### 6.6 Dependency convergence

The resolver produces a dependency graph; the runtime converges that graph as service availability changes.

- A required missing service leaves the activation Pending, not partially Active.
- An optional missing service does not block activation but remains observable.
- When a provider becomes available, eligible consumers can transition to Ready under the same policy and version checks.
- When a provider disappears, consumers are notified before their next invocation and must enter a defined degraded, pending or stopped state.
- Cycles are rejected unless a future explicit cycle contract is designed.
- Provider replacement must not expose a window in which consumers use an unvalidated provider.
- Convergence operations are serialized per affected graph or use generation-checked optimistic commits.

The resolver must distinguish package dependency edges, service provider/consumer edges and runtime host-service edges. They are related but not interchangeable.

### 6.7 Event System

Events are typed contracts with explicit dispatch roles:

- **Observer:** receives an event and cannot replace the result.
- **Responder:** supplies a response for a request-style event.
- **Mapper:** transforms a value under a declared ordering and schema.
- **Around:** wraps an operation and must explicitly invoke or refuse the continuation.

Every subscription is owned by an EffectScope and has a stable subscription identity.

Routing must be explicit:

- Unscoped events are visible only to listeners registered for that event's unscoped route.
- Scoped events carry an explicit scope key.
- No implicit broadcast from a narrow scope to all broader scopes.
- Cross-scope routing requires a declared policy and contract.

Interception is privileged. The framework checks the relevant capability at registration and at dispatch. A listener's existence does not grant it permission to observe arbitrary payloads.

### 6.8 Runtime Provider Kernel

Ferris keeps the existing runtime catalog and provider model, but strengthens the contract around preparation, invocation and termination.

Provider families remain:

- Builtin / native in-process.
- Declarative.
- Process.
- MCP.
- WebAssembly Component / WASI.
- Extism.
- Workflow.
- Cordis-compatible JavaScript bridge.

The common semantic lifecycle is:

```text
describe -> prepare -> start -> invoke* -> cancel* -> drain -> stop -> dispose
```

Not every provider supports every operation in the same way. Each provider must declare its guarantees:

| Property | Required provider declaration |
|---|---|
| Isolation | In-process, process, WASM sandbox, remote boundary, or other explicit class |
| Cancellation | Cooperative, protocol-level, process termination, epoch invalidation, or unsupported |
| Drain | Whether accepted calls can finish and how the deadline is enforced |
| Resource accounting | Memory, CPU/time, concurrency, output and I/O limits supported |
| Persistence | Whether state survives deactivate, restart or generation replacement |
| Streaming | Progress/event support and backpressure behavior |
| Network/filesystem | Exact host interfaces and capability checks |
| Re-activation | Whether the same provider handle can reactivate or must be reconstructed |

A cancelled Rust future is not evidence that the underlying process, WASM guest, MCP request or remote operation stopped. Cancellation must reach the actual provider boundary and return a verifiable outcome.

### 6.9 Invocation pipeline

Every invocation follows the same logical sequence:

1. Resolve the capability contract and provider binding.
2. Verify activation and generation are current.
3. Authenticate the caller principal and resolve the requested scope.
4. Authorize the exact capability/resource/action at invocation time.
5. Reserve concurrency and resource budget.
6. Validate input schema and enforce size/depth limits.
7. Dispatch through the provider adapter with deadline and cancellation token.
8. Enforce output size and validate result envelope.
9. Record outcome, provider identity, generation, policy decision and correlation ID.
10. Release reservations and invocation-owned effects.

Discovery and planning may narrow the candidate set; neither grants permission. Authorization is always enforced at the execution boundary.

## 7. Capability and security model

Ferris integrates with BOS's capability and policy model; it must not create a parallel, weaker authorization system.

A grant is conceptually:

```text
Principal
  + CapabilityContract
  + ResourceSelector
  + Scope
  + Constraints
  + Expiry / RevocationEpoch
```

Requirements:

- Manifest capability declarations are requirements, not grants.
- Provider trust level and caller authorization are separate facts.
- Authorization is checked on every invocation, including service calls made by an extension.
- No ambient privilege inheritance from the host, parent extension, caller or runtime process.
- Revocation blocks new calls immediately and drains/cancels in-flight calls according to policy.
- Hot update re-evaluates grants; permissions are not silently inherited by a new artifact digest.
- A package signature proves integrity/identity under a trust root; it does not prove the package is safe.
- Native in-process extensions are a higher-trust execution class; they are not sandboxed merely because they implement the Ferris trait.
- Process, WASM, Extism and MCP adapters must declare and enforce their own isolation/cancellation guarantees.
- Fail closed when a policy engine, required host service, sandbox or trust root is unavailable.
- Audit records include allow/deny, principal, capability, resource, scope, extension, activation, generation, provider, correlation and policy version.

## 8. Update, replacement and rollback

Two update modes are required:

### 8.1 In-place update

Use only when the provider and extension explicitly support a compatible update that preserves activation identity. The manager prepares the change first, validates its dependencies and grants, then commits it under a generation-safe transition.

Failure must leave either the old generation active or the activation stopped in a known state. Partial mixed state is forbidden.

### 8.2 Generation replacement

Use when the runtime, artifact, permissions, service contract or host contributions require a new activation identity.

Target sequence:

1. Prepare and validate the new artifact without live effects.
2. Resolve the complete dependency and policy plan.
3. Admit a new generation in a non-serving state.
4. Start and verify the new provider.
5. Atomically switch the capability binding to the new generation.
6. Stop accepting new calls on the old generation.
7. Drain old in-flight calls to a deadline.
8. Revoke old contributions, services, events and effects.
9. Dispose the old provider and emit a replacement report.

If step 3–5 fails, the old generation remains authoritative. If old-generation disposal fails after the switch, the new generation remains authoritative while the manager reports and tracks the leaked old resources for reconciliation.

## 9. Federation boundary

Ferris should be federation-ready without becoming the federation subsystem.

Local Ferris capabilities may be projected by the BOS capability/federation layer as remote capability descriptors. The projection must be an explicit export contract, not a direct exposure of the local registry.

Remote invocation must preserve:

- Remote node identity and authenticated channel.
- Provider-side policy and resource authority.
- Caller-side policy and outbound grant.
- Contract/version and schema compatibility.
- Bounded request, response, time and concurrency budgets.
- Correlation and provenance across the request chain.
- Provider-side cancellation and revocation semantics.
- Independent audit on both nodes.

A remote service is not a local Ferris ServiceRealm publication. It is a remote provider adapter with a network trust boundary. A local activation handle must never own or dispose a remote node's agent/task lifecycle.

## 10. Workspace and module evolution

Do not create a large number of crates before the contracts are proven. Keep the current three-crate product boundary initially.

Target logical modules:

```text
crates/
  ferris-core/
    identity/          package, activation, generation identities
    composition/       plan, graph, conflict, deterministic resolver
    lifecycle/         activation state machine and manager
    effects/           scopes, disposers, disposal reports
    capability/        contracts, grants, authorization integration
    service/           typed contracts, realms, publications
    events/            typed event specs, roles and routing
    contribution/      host-neutral contribution and lease contracts
    audit/             lifecycle and security decision records
  ferris-runtime/
    contract/          provider lifecycle and invocation contract
    manager/           provider construction, supervision, drain
    adapters/          builtin, declarative, process, MCP, WASM, Extism, Cordis
  ferris-sdk/
    author/            Extension, config, manifest and contribution API
    host/              host facade and stable host adapter contracts
    prelude/            stable author imports
  bsh/
    extension/         BSH-specific HostSpec, contribution sink and GPUI adapter
```

These are logical module boundaries, not a mandate to split every module into a crate. The dependency direction remains:

```text
ferris-core <- ferris-runtime <- ferris-sdk facade
                              ^
                              |
                         BSH host adapter

BOS Agent Runtime -> capability contracts <- Ferris provider adapter
```

The core must not depend on BSH, GPUI, Agent, a concrete runtime provider or a specific transport. Avoid circular dependencies and avoid exporting Cordis implementation types from the SDK.

## 11. Public API direction

The following is illustrative, not a frozen API:

```rust
#[async_trait]
pub trait Extension: Send + Sync + 'static {
    type Config: DeserializeOwned + Send + Sync;

    fn manifest(&self) -> ExtensionManifest;

    async fn prepare(
        &self,
        config: Self::Config,
        context: PrepareContext,
    ) -> Result<PreparedExtension, FerrisError>;
}

pub trait PreparedExtension: Send + Sync {
    fn descriptor(&self) -> &PreparedDescriptor;

    async fn activate(
        &self,
        context: ActivationContext,
        effects: EffectScope,
    ) -> Result<ActiveExtension, FerrisError>;
}

pub trait ActiveExtension: Send + Sync {
    async fn invoke(
        &self,
        capability: CapabilityRef,
        input: Value,
        context: InvocationContext,
    ) -> Result<CapabilityOutput, FerrisError>;

    async fn drain(&self, deadline: Deadline) -> Result<DrainReport, FerrisError>;

    async fn stop(&self) -> Result<(), FerrisError>;
}
```

Implementation must resolve object safety, typed configuration erasure, async trait dispatch, provider reconstruction and handle ownership before this API is committed. Do not copy this snippet directly into production.

## 12. Migration plan

Migration is additive and must preserve current Manifest 2.0 packages and the `ferris-sdk` host facade.

### Phase 0 — Evidence and compatibility audit

- Inventory current lifecycle, effects, resolver, services, events, manager, runtime and contribution code.
- Map each Cordis semantic to an existing implementation, partial implementation or missing contract.
- Prototype `cordis-core` compatibility without changing production dependencies.
- Record the decision in an ADR.
- Establish baseline tests and current behavior.

**Exit gate:** approved mapping, no duplicate lifecycle owner, documented dependency decision and gap register.

### Phase 1 — Identity and activation ownership

- Introduce explicit ActivationId and Generation identity.
- Make manager admission return a retained ActivationHandle.
- Ensure all effects and ContributionLease instances are owned by the activation generation.
- Reject stale-generation registrations and invocations.
- Make explicit disposal and drain reports part of manager operations.

**Exit gate:** activation, replacement and disposal tests prove no contribution or effect survives its owner.

### Phase 2 — Services and dependency convergence

- Define exact ServiceRealm identity and typed contract/version key.
- Implement exact lookup with no implicit fallback.
- Make required dependencies Pending until satisfied.
- Add provider-change notifications and deterministic convergence.
- Replace traversal-order-dependent resolution with deterministic global constraint handling.

**Exit gate:** conflict, missing provider, provider replacement, disappearance, cycle and concurrent convergence tests pass.

### Phase 3 — Events and effect integration

- Normalize event roles and routing.
- Own every subscription/interceptor through EffectScope.
- Enforce interceptor permissions at registration and dispatch.
- Connect service, event, task and contribution resources to activation generation disposal.

**Exit gate:** disposing one activation removes only its resources; sibling activations remain unaffected.

### Phase 4 — Provider lifecycle and invocation guarantees

- Extend provider declarations with isolation, cancellation, drain, resource and reactivation guarantees.
- Make cancellation reach actual provider operations.
- Add concurrency/resource reservations and invocation budgets.
- Produce structured stop/drain/disposal reports.
- Add provider conformance tests across all supported runtimes.

**Exit gate:** timeout, cancellation, crash, restart, partial startup and shutdown tests demonstrate bounded cleanup and no false-success cancellation.

### Phase 5 — BOS/BSH integration and federation projection

- Keep BSH as a thin host adapter using `ferris-sdk`.
- Ensure tools, commands, panels, prompts, contexts, skills, workflows and hooks all use generation-owned leases.
- Expose stable local capability descriptors to BOS capability/federation through an adapter.
- Add dual-authorization and end-to-end audit tests for remote invocation.

**Exit gate:** host rebuild, extension replacement, workspace close and node disconnect leave no stale local registrations or remote authority leaks.

## 13. Work breakdown

Each story is scoped to no more than three working days. These are proposed implementation slices, not a commitment that all work is already scheduled.

| ID | User story / deliverable | Acceptance criteria | Estimate |
|---|---|---|---:|
| FER-001 | As a maintainer, I can see a Cordis-to-Ferris semantic map | Existing code classified as present/partial/missing; dependency decision recorded | 2d |
| FER-002 | As a maintainer, I can identify package, activation and generation separately | Stable ID types and serialization rules documented/tested | 2d |
| FER-003 | As a host, I can retain an explicit activation owner | Manager returns handle; dropping handle does not silently leak or falsely dispose | 3d |
| FER-004 | As a runtime, I cannot publish stale-generation resources | Generation checks guard registration and invocation | 3d |
| FER-005 | As a maintainer, I can inspect one activation's cleanup result | Structured disposal report; all cleanups attempted after failures | 2d |
| FER-006 | As a consumer, I resolve a service only in its declared realm | Exact contract/version/realm lookup; no implicit parent fallback | 3d |
| FER-007 | As a consumer, I become Pending when a required provider disappears | Provider change stream and deterministic state convergence | 3d |
| FER-008 | As a maintainer, I get a globally valid deterministic plan | Conflicts, ambiguity, cycles and version constraints have stable diagnostics | 3d |
| FER-009 | As an extension, I can subscribe to explicitly routed events | Typed role and route validated; subscription owned by EffectScope | 3d |
| FER-010 | As a host, I can revoke every contribution from one generation | Tools, prompts, skills, commands, panels and hooks are all lease-owned | 3d |
| FER-011 | As an operator, I can cancel work at the actual provider | Provider-specific cancellation outcome is observable and tested | 3d |
| FER-012 | As a host, I can replace an extension without mixed generations | New generation commits atomically; old generation drains and disposes | 3d |
| FER-013 | As a maintainer, I can compare provider guarantees | Conformance matrix covers isolation, cancellation, drain and budgets | 2d |
| FER-014 | As a BOS node, I can export a local capability safely | Export descriptor is separate from local registry; dual authorization is tested | 3d |

## 14. Conformance and release gates

Ferris must not be called composable-runtime complete until the following are demonstrated:

### Composition

- Deterministic plans for identical package, service and policy inputs.
- Correct global dependency/conflict resolution.
- Explicit diagnostics for cycles, ambiguity and missing dependencies.
- No side effects during plan construction.

### Lifecycle and ownership

- Multiple activation scopes do not share mutable state accidentally.
- Handle disposal is idempotent and explicit.
- Every listener, task, service, contribution and provider resource has an owner.
- Stale generations cannot publish or invoke.
- Partial startup rolls back all already-acquired resources.

### Services and events

- Exact service realm lookup.
- Version mismatch and provider disappearance are handled.
- Event route and role are explicit.
- Interceptor authorization is enforced.
- Disposal removes only the disposing activation's registrations.

### Runtime and security

- Provider-specific cancellation and drain are tested.
- Memory, time, concurrency and output limits are enforced or explicitly declared unsupported.
- Authorization is checked at invocation time.
- Revocation blocks new calls.
- Process/WASM/MCP/Extism/native trust differences are visible to policy.
- Failure of policy or isolation infrastructure fails closed.

### Host and federation

- BSH depends on Ferris through `ferris-sdk` only.
- Host contribution leases cover every supported contribution kind.
- Federation export does not bypass provider-side authority.
- Caller and provider authorization decisions are independently audited.

## 15. Risks and mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| Directly adopting Cordis types across the public API | Ferris SDK becomes coupled to upstream internals and version churn | Keep an internal adapter and Ferris-owned public vocabulary |
| Two lifecycle managers coexist | Split-brain activation, duplicate cleanup and leaks | One manager owns activation handles and generations |
| Package identity is reused as activation identity | Workspace/scope collisions and stale handles | Separate package, activation and generation IDs |
| Service fallback is implicit | Cross-scope data leakage | Exact realm lookup; explicit projection only |
| Resolver remains greedy | Invalid or nondeterministic composition | Global constraints, deterministic ordering and adversarial tests |
| Cancellation is only future cancellation | External work continues after timeout/revocation | Provider-specific cancellation and termination evidence |
| Native extensions are treated as sandboxed | Host compromise | Explicit trust classes; process/WASM isolation for untrusted code |
| Contribution kinds bypass leases | Stale tools/prompts/skills/UI after unload | One generation-owned lease contract for every contribution |
| Federation is embedded inside Ferris | Duplicated node/task authority | Ferris exports local capabilities; BOS federation owns remote semantics |
| Too many crates too early | Dependency complexity and API churn | Evolve existing three-crate boundary first |

## 16. Decisions requested

This RFC asks maintainers to approve the following direction before implementation:

1. **Ferris remains the BOS extension/capability runtime**, not a separate Agent Runtime.
2. **Cordis semantics are adopted selectively**: Context, prepare/admission, activation handles, typed services, explicit events, dependency convergence and effect ownership.
3. **Cordis integration is adapter-contained**; no Cordis types leak into the stable Ferris SDK or manifest.
4. **Package, activation and generation identities are distinct.**
5. **Every runtime and host contribution is generation-owned and explicitly disposable.**
6. **Service lookup is exact by contract/version/realm**, with no implicit parent fallback.
7. **Provider-specific cancellation, drain and isolation guarantees are mandatory metadata and conformance obligations.**
8. **BOS owns Agent/Task/Workflow and Federation semantics; BSH owns GPUI and user interaction.**
9. **The existing Manifest 2.0 and `ferris-sdk` boundary remain compatible throughout migration.**
10. **No production dependency or code migration is made until Phase 0 and the Cordis compatibility ADR are complete.**

---

## References

- [Ferris framework implementation and phase status](ferris-framework.md)
- [BSH Extension Platform contract](bsh-extension.md)
- [Ferris author SDK](ferris-sdk.md)
- [BOS / BSH Architecture](../ARCHITECTURE.md)
- [cordis-rs upstream repository](https://github.com/dshbox/cordis-rs)
- [cordis-core documentation](https://docs.rs/cordis-core)
