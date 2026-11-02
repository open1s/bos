# BOS RFC-EXT-002: Composable Extension Runtime 2.0

- Status: implementation in progress
- Scope: BSH extension runtime composition; BOS-wide adapters follow after the core contract stabilises
- Compatibility: internal runtime primitives first; no extension manifest/API version bump in this foundation change
- Security invariant: service discovery is not authorization

## 1. Problem

BSH 1.x already provides package management, SemVer dependency resolution, capability policy, runtime dispatch, package trust, lifecycle records, events and contribution registration. Replacing those systems would duplicate mature behaviour and create unnecessary migration risk.

The missing abstraction is the *live composition graph*: a running extension needs to consume host/extension services, react to service availability changes, own every registration it creates, and release those registrations deterministically when it unloads.

Cordis is useful as a semantic reference, not as a runtime dependency or an API to copy wholesale.

## 2. 2.0 architecture

The existing package and execution path remains authoritative:

```text
Manifest / Store / SemVer Resolver
             |
       Trust + Capability Policy
             |
        Runtime Dispatcher
             |
  Composable Runtime (new)
   - scoped Service Registry
   - dependency graph / activation plan
   - Fiber state + effect ownership
   - typed event and pipeline adapters (next phase)
   - profile / bundle composition (next phase)
             |
   Existing Host Contributions
   Agent / Tools / Providers / GPUI
```

The composable runtime does not replace `ExtensionStore`, `resolver`, `PolicyEngine`, `Runtime`, or the persisted `LifecycleManager`.

## 3. Core contracts

### 3.1 Service Registry

A service is addressed by the tuple:

`(scope, stable name, Rust type)`

Supported scopes are Host, Workspace, Session and Extension. Resolution is exact-scope in this foundation; implicit parent-scope fallback is intentionally deferred because it can obscure ownership and leak session/workspace services.

A provider receives a generation-specific token. Revocation succeeds only when the token still owns the current generation. This prevents a late cleanup from an old provider from deleting a replacement service.

The registry is an in-process composition mechanism, **not a sandbox or permission system**. Every operation that reaches files, network, credentials, tools, subprocesses or other protected resources must still be checked by the existing capability/policy boundary.

### 3.2 Dependency Graph

The graph validates that every declared dependency exists and returns a deterministic dependency-first activation order. Missing nodes and cycles are explicit errors. The graph is separate from SemVer package resolution: SemVer chooses package versions; the runtime graph orders live activation and service dependencies.

### 3.3 Fiber

A Fiber is one live activation instance, not the installed package record. Its initial state machine is:

`Pending -> Loading -> Active -> Unloading -> Disposed`

Loading and Active may fail; Failed may unload. Invalid transitions are rejected. Retry creates a new activation generation rather than reviving a disposed Fiber.

### 3.4 Reversible Effects

Every Fiber owns the registrations and subscriptions it creates. Cleanup executes once, in reverse registration order. One panicking cleanup must not prevent remaining cleanup actions from running. A cleanup registered after disposal executes immediately.

The first implementation uses explicit cleanup closures. Host adapters will wrap registrations in these effects, so unloading a Fiber removes its tools, commands, event subscriptions, service registrations and UI contributions.

## 4. Service replacement semantics

A replacement receives a new generation. The old token can no longer revoke it. The next integration phase adds availability/change notifications and dependent-Fiber reconciliation.

Replacement must not silently switch an in-flight request to a different provider. Host adapters should use a generation snapshot per invocation; new calls resolve the current generation, while in-flight calls finish or are cancelled under an explicit drain policy.

## 5. Event and pipeline semantics (next phase)

The event engine will distinguish:
- broadcast/parallel observers;
- ordered serial handlers;
- waterfall transforms where each handler receives the previous result;
- interceptors with explicit before/after ordering and a host-defined timeout.

Observer APIs must not gain authority to mutate a request. Interception is a separate capability-bearing contribution, not an accidental extension of the current observe-only hook API.

## 6. Profiles, bundles and patches (next phase)

Composition will be declarative and deterministic:
1. resolve the selected profile;
2. expand bundles;
3. apply patches in declared order;
4. validate contribution ownership and conflicts;
5. construct an immutable activation plan;
6. activate in dependency order.

A failed plan is rejected before activation. Patch application must be transactional; partial registration is not visible.

## 7. Compatibility strategy

- Keep the existing BSH 1.x manifest contract during the internal foundation phase.
- Add 2.0 manifest fields only after their semantics and migration rules are tested.
- Preserve current DSH and Codex refusal reporting. Parsing a feature is not equivalent to implementing it.
- DSH Cordis bundle patches remain unsupported until the composition engine is wired and covered by fixtures.
- Native GPUI panels remain the BSH UI surface; DSH browser client modules are not implicitly treated as native GPUI UI.
- Keep runtime adapters (Builtin, Declarative, Process, WASM Component, Extism, MCP) behind the existing dispatcher.

## 8. Implementation sequence

| Slice | Deliverable | Exit criteria |
|---|---|---|
| 2.0-A | Scoped service registry, dependency graph, Fiber, EffectSet | Unit tests for scope isolation, replacement safety, missing/cyclic dependencies, legal transitions, reverse/idempotent cleanup |
| 2.0-B | Host adapter and effect ownership | Existing host registrations are owned by a Fiber and fully removed on unload |
| 2.0-C | Service availability and dependency reconciliation | Provider replacement/unavailability safely suspends or reloads dependents; in-flight generation is stable |
| 2.0-D | Event engine | Parallel/serial/waterfall semantics, ordering, timeout and cancellation tests |
| 2.0-E | Profile/bundle/patch composition | Deterministic, transactional composition with conflict diagnostics |
| 2.0-F | DSH/Codex compatibility | Fixtures prove each advertised feature is actually mounted and callable |
| 2.0-G | Security and conformance | Capability checks remain on every protected call; lifecycle, race, failure and resource tests pass |

## 9. Non-goals

- Replacing the package store or SemVer resolver.
- Embedding the DeepSeek Harness loader or importing Cordis as a dependency.
- Treating a trusted JavaScript process as an OS sandbox.
- Making service registration grant filesystem/network/credential access.
- Promising DSH browser-client compatibility on a native GPUI host.

## 10. Risks and controls

| Risk | Control |
|---|---|
| Stale cleanup removes a replacement | Generation-bound service tokens |
| Dependency cycle or missing provider | Validate before activation; deterministic graph errors |
| Cleanup panic leaks later registrations | Catch each cleanup panic and continue |
| Replacement disrupts in-flight calls | Per-call generation snapshot and explicit drain policy |
| Service lookup bypasses policy | Keep authorization at each host operation |
| Partial bundle activation | Build and validate an immutable plan before committing effects |
| False compatibility claims | Conformance fixtures must invoke the contribution, not merely parse it |

## 11. Current implementation boundary

### Implemented

- The in-process scoped service registry, generation-safe provider tokens, deterministic dependency planner, Fiber state machine and reverse-order effect cleanup are implemented with unit tests. `ServiceRegistry::provide_owned()` binds a provider token to a Fiber through a weak registry reference, so stale Fiber cleanup cannot remove a newer provider generation.
- The host now owns one host-scoped `ServiceRegistry`. Registry changes publish bounded `Available`, `Replaced` and `Unavailable` notifications; consumers must re-query the registry after receiving a notification.
- The registry now offers call-scoped `ServiceLease<T>` acquisition and generation-specific `ServiceRetirement` handles. Replacement publishes the new generation atomically, rejects new leases on the old generation, and allows the host to await old in-flight leases with a timeout. Legacy `get()` remains an untracked snapshot and must not be used for drain-sensitive calls.
- The host exposes `apply_with_fibers()`, returning one `AppliedExtension` handle per activation.
- A contribution lease tracks installed async tools, exact prompt sections and newly registered skill manifest paths. Fiber cleanup first closes the shared tool-liveness gate; explicit unload then removes tools, prompt sections, skills and owner-scoped PluginRegistry entries.
- Agent cache invalidation is part of tool, prompt and skill removal.
- The production host rebuild now runs `plan_service_dependencies()` on the concrete package set selected by the SemVer resolver, before capability authorization and activation. Required service contracts add provider-before-consumer edges; missing providers, duplicate shared-scope providers, incompatible required versions and cycles prevent the resolved activation set from starting.
- Extension-scoped service declarations are private to their declaring extension in the contract planner. Optional requirements tolerate both missing and version-incompatible providers; they do not create activation edges.
- `AppliedExtension` now exposes manifest-gated typed `provide_service()` and `acquire_service()` adapters. Providers register through `provide_owned()` on their Fiber; consumers receive a `ServiceLease<T>` only for a declared name/scope requirement. Service-only extensions are retained as activations so their service contracts are not silently skipped.

### Not yet complete / not yet verified

- The production turn builder now calls `install_with_fibers()` and carries each `AppliedExtension` in `BuiltAgent`. The live shell stores those handles by session alongside the turn Agent, and drops them on terminal completion, stop or archive. The temporary compaction build retains its handles until the compaction task exits.
- Production teardown disposes the whole build rather than mutating a live Agent in place. Dropping the Fiber's effect set closes the shared tool-liveness gate; the Agent and its tool registry are then released with the build. The explicit `AppliedExtension::unload(agent, registry)` path remains available for future in-place replacement where the same registries survive.
- Calls admitted before revocation are not drained or forcibly cancelled. The liveness gate prevents new invocations; already-running work may finish.
- The production integration and new lifecycle test still require targeted Cargo tests, formatting and CI validation. A merged commit is not evidence that these checks passed.
- Manifest service contracts now declare provider/consumer name, interface id, SemVer and explicit host/workspace/session scope. Planning rejects missing required providers, incompatible versions, duplicate providers and cycles; optional missing services do not block activation. Service requirements become provider-before-consumer graph edges, and the host now preserves the planner's topological order during activation.
- Manifest declarations are not yet bound to concrete typed service values in the registry. Provider replacement reconciliation and dependent Fiber reload remain future work; the registry's lease/drain primitive exists but is not yet wired into extension Fiber replacement or unload.
- Service contract planning currently fails the configured root set conservatively when the selected set contains an invalid service graph; finer-grained failure isolation between independent roots remains future work.
- DSH bundle patches remain unsupported until the composition engine is wired and covered by fixtures.

Do not mark 2.0-B complete until tests prove that production turn ownership retains Fiber handles through the live turn, closes tool liveness on every teardown path, and cleans prompt/skill/tool/command/panel contributions on explicit unload and partial activation failure.
