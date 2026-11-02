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
| 2.0-B | Host adapter and effect ownership | Async tools, commands and panels have owner-scoped revocation; Fiber disposal disables retained tool handles |
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

The first 2.0 foundation commit adds the in-process primitives and their unit tests. It does not yet claim that the existing host uses Fiber-owned effects, that service changes trigger reloads, or that DSH bundle patches mount. Those are subsequent integration slices and remain explicit gaps until verified.

## 12. 2.0-B implementation note

The host adapter now has an opt-in Fiber-managed activation path:
- `PluginRegistry::remove_owner` removes only the contributions whose authoritative owner entry matches the requested extension.
- `ToolRegistry::remove_async` removes the async tool, schema cache entry and MCP marker together.
- `Agent::remove_async_tools` invalidates the cached engine/context after revocation.
- `ContributionLease` records the tool names and extension owner for one Agent build.
- A lease can attach its liveness guard to a Fiber. Disposing the Fiber blocks new calls even if an older tool handle was retained by a cached caller.
- `ExtensionHost::apply_with_fibers` returns explicit activation handles; callers must retain them and call `AppliedExtension::unload`.

Prompt/context text and skills are currently immutable Agent-build snapshot contributions. They are not independently removed by a lease; the owning Agent build must be discarded after unload if those contribution types were installed. The legacy `apply` method remains for compatibility and does not yet expose activation handles. Wiring every production Agent construction path to `apply_with_fibers` is a separate integration step and must be verified against the current call sites before the legacy path is retired.
