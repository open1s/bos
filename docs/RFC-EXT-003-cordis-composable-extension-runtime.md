# RFC-EXT-003: Cordis-Grade Composability for the BSH Extension Runtime

- **Status:** proposal for review
- **Scope:** crates/bsh/src/extension, the host composition layer, and a
  first-class Cordis/DSH runtime adapter
- **Depends on:** docs/RFC-EXT-002-composable-extension-runtime-2.0.md
- **Compatibility:** additive. Manifest 1.0 packages keep working; the new
  semantics are opt-in through manifest 1.1 fields and host profiles.
- **Security invariant:** composition is not authorization. Service discovery,
  event delivery, and plugin mounting never grant filesystem, network, process,
  credential, or Computer Use access.

---

## Implementation status (2026-11-02)

The composition core has landed in the BSH extension platform:

- composable/effects.rs -- async and nested EffectScope, Disposer,
  DisposalReport; LIFO, idempotent, panic-isolated, late effects run
  immediately.
- composable/context.rs -- Context (parent, kind, scope path, services,
  effects) with inherited Host/Workspace/Session resolution, exact
  acquire_exact, child effect scopes, and extension-private scopes.
- composable/reactivity.rs -- Inject, Epoch, RefreshAction, DependencyIndex,
  and a coalescing Reconciler; a fiber refreshes against the registry's
  type-independent generation index.
- composable/runtime.rs -- CompositionRuntime: activates a component when its
  hard dependencies are ready, suspends it when they disappear, reloads it on
  provider replacement, and awaits effect cleanup on unload.
- composable/plan.rs -- CompositionPlanner: profiles, groups, disabled rows,
  ordered patches, duplicate-contribution checks, missing-dependency and cycle
  errors, deterministic topological order.
- composable/dispatch.rs -- EventDispatcher: emit, parallel, serial, bail, and
  waterfall; deterministic order; subscriptions are effects.
- ServiceRegistry::generation() -- generation lookup used by coeffect
  snapshots; generation updates happen under the registry lock.
- Fiber owns an EffectScope, supports own_async / dispose_async, and carries
  inject, epoch, and revision.
- The host attaches a Context to each AppliedExtension; unload_async() awaits
  effect cleanup.
- Per-service isolate realms are part of the service key, and ServiceChange now
  carries the realm so isolated dependencies react to provider changes.
- runtime/cordis.rs executes real apply(ctx, config) packages through the
  sandboxed Node bridge, maps their tool registrations to BSH tools, and proxies
  invocation; examples/extensions/cordis-echo is the fixture.
- The host ServicePolicyAuthorizer checks a provider's declared capabilities
  against the consumer's grants before acquisition; provides entries may now
  declare capabilities.
- CompositionPlanner lowers cordis.patch.yml insert rows into profile entries.

Not yet implemented: wiring CompositionRuntime / CompositionPlanner /
EventDispatcher into the production turn builder, live HMR, live mounting of
lowered Cordis rows and non-tool Cordis registrations, and native UI slots. The design below
remains the target; the expressiveness matrix and remaining work are tracked in
docs/bsh-extension-expressiveness.md, and the acceptance standard is
docs/bsh-extension-gold-standard.md.


---

## 1. Executive summary

BSH already has the hard, unglamorous parts of an extension platform: package
identity, hashed installs, SemVer resolution, trust and signing, capability
policy with audit, a lifecycle state machine, six execution runtimes, bounded
events, per-extension storage, declarative UI, and a developer CLI. What it does
not have is a **live composition model**.

Today an extension is a package that the host applies once into one Agent build.
Its dependencies are checked before activation and never checked again. Its
contributions are held by a lease that must be explicitly revoked. Its services
are typed values in a flat registry whose availability changes are published but
not consumed. There is no context that an extension can use to register a tool,
subscribe to an event, provide a service, and later have all of that disappear
because the owning activation was unloaded.

Cordis supplies exactly that missing layer, and it does so with a small,
well-defined set of mechanisms:

1. **Context** -- the only surface a plugin uses; services are looked up through
   it, and every registration is a reversible effect of the context.
2. **Fiber** -- one live activation with a state machine, an epoch derived from
   its injected services, and deterministic reload/unload behavior.
3. **Reactive coeffects** -- when a required service appears, disappears, or is
   replaced, dependent fibers are suspended or reloaded automatically.
4. **Revertible effects** -- every registration returns a disposer; teardown is
   reverse-ordered, idempotent, and dependent-aware.
5. **A reconciling composition tree** -- entries, groups, isolation realms,
   configuration, disabled rows, and patches are planned and reconciled rather
   than hand-started.

**Recommendation:** implement the Cordis *semantics* in Rust inside the BSH
extension platform, and treat the real Cordis JavaScript runtime as one optional
execution backend for DSH bundles. Do not import Cordis as a dependency and do
not weaken the capability boundary to imitate its dynamic property access.

The work is six slices:

| Slice | Deliverable | Outcome |
|---|---|---|
| A | Context + EffectScope | every registration is owned and reversible |
| B | Fiber epochs + reconciler | dependency changes reload/suspend dependents |
| C | Scoped service graph + isolate | host/workspace/session/extension lifetimes compose |
| D | Composition profiles + patches | bundles and DSH patches mount deterministically |
| E | Typed dispatch + interceptors | waterfall policy without ad-hoc host code |
| F | Cordis/DSH runtime adapter | real apply(ctx, config) bundles run as a first-class runtime |

Slices A--C are the foundation and should be treated as RFC-EXT-002's unfinished
2.0-B/2.0-C work, not as a new platform. D--F are the visible capability gain.

---

## 2. Research: what Cordis actually is

Cordis describes itself as *"a meta-framework of spatiotemporal composability"*
([repository](https://github.com/cordiverse/cordis),
[paper](https://arxiv.org/abs/2608.25512)). The paper's abstract states the two
orthogonal properties precisely:

- **Temporal composability:** the ability to completely revert a component's
  side effects when it is removed.
- **Spatial composability:** the ability to declare and reactively manage
  inter-component dependencies.

It formalises these as **revertible effects** and **reactive coeffects**, unified
through a single **context** that mediates every effect and coeffect. A
component is a context transformation with an inverse; a system is a set of
components whose effects interleave without disturbing one another.

That is the right mental model for the gap. BSH has effects, but not a general
revertible effect discipline. BSH has service declarations, but not reactive
coeffects.

### 2.1 The five concepts a plugin author sees

| Concept | Cordis mechanism | Source |
|---|---|---|
| Plugin | function (ctx, config), class, or an object with apply(ctx, config) | packages/core/src/registry.ts |
| Context | proxy over a service container, with extend, isolate, intercept | packages/core/src/context.ts |
| Injection | inject declares services; the fiber waits until they are available | packages/core/src/registry.ts, fiber.ts |
| Events | emit, parallel, serial, bail, waterfall, all context-scoped | packages/core/src/events.ts |
| Effects | ctx.effect() and every ctx.on/provide/plugin registration | packages/core/src/fiber.ts |

The public documentation used by DeepSeek Harness is the
[Cordis primer](https://deepseek-harness.github.io/deepseek-harness/reference/cordis-primer).
It states the operating rule directly: *"Every registration should have a
corresponding disposer: either return one from ctx.effect(), or use a Cordis
helper that handles it automatically."*

### 2.2 Fiber: the unit of live composition

Cordis's Fiber is not a package record. It is one activation instance. Its state
machine is:

~~~
PENDING -> LOADING -> ACTIVE -> UNLOADING -> DISPOSED
                \-> FAILED
~~~

Three mechanisms make it more than a state label:

1. **Epoch.** The fiber computes an epoch from its revision and the generation
   of every injected service:

   ~~~
   epoch = revision + ":" + impl.uid for each injected name
   ~~~

   A different epoch means the activation's dependency set changed. If the new
   epoch is active, the fiber reloads; if not, it unloads. See Fiber._refresh and
   Fiber._setEpoch in
   [fiber.ts](https://github.com/cordiverse/cordis/blob/main/packages/core/src/fiber.ts).
2. **Effects.** Every registration inside the plugin is collected into the
   fiber's effect scope. Reload and unload dispose that scope in reverse order.
3. **Dependents.** A fiber that provides services tracks the fibers that consume
   them. It does not dispose its provisions until every dependent has unloaded.
   This is the ordering rule that makes provider replacement safe. See
   Fiber._unload and Impl.dependents.

The loader adds one more layer: an entry tree of plugin rows with id, name,
config, disabled, group, and isolate. Groups nest rows; the loader reconciles the
tree on every configuration update, and HMR replaces a plugin's implementation
while preserving its fibers where possible.

### 2.3 Services and isolation

Cordis services occupy stable keys (ctx.tools, ctx.llm, ctx.sessions).
ctx.reflect.provide(name, value, check) registers a generation; access without a
declared injection throws a named error, which is why missing dependencies fail
loudly instead of returning undefined.

Two mechanisms are especially relevant to BSH:

- **ctx.isolate(name, label)** creates a named realm. Two parts of the system can
  each have an llm service without seeing each other's. The loader exposes this
  as per-entry isolate maps, including true for a private realm and a string for
  a shared named realm.
- **ctx.intercept(name, config)** merges per-service configuration down the
  context chain. The loader uses it to make an entry's await option decide
  whether a service is ready.

### 2.4 Events

Cordis distinguishes five dispatch modes, and the mode is part of the event's
contract:

| Mode | Awaited | Order | Return |
|---|---|---|---|
| emit | no | registration order | none |
| parallel | yes | all listeners concurrently | none |
| serial | yes | registration order | first bail value |
| bail | no | registration order | first bail value |
| waterfall | no | registration order, next() chain | transformed value |

waterfall is the important one for BSH: it is an around-middleware chain. A
listener receives (...args, next); calling next() runs the downstream listeners,
and the listener may wrap or replace the result. Not calling next()
short-circuits. That is how policy and interception are expressed without making
every subsystem know about every plugin.

Cordis also has internal lifecycle events (internal/plugin, internal/service,
internal/status, internal/update, internal/config, internal/get, internal/set)
that the runtime itself uses. BSH currently has no equivalent in-process event
surface; its extension event bus is a JSON pub/sub bus for observers.

### 2.5 Loader and composition

The loader's tree is the closest thing Cordis has to a composition language:

- Entry -- one plugin row, with config, disabled flag, isolate map, and parent
  group.
- EntryGroup -- a group of rows, itself an entry; groups can nest.
- EntryTree -- a reconciling store with create/update/remove/commit.
- Group -- a plugin whose payload is a list of child entries; updating the group
  updates its children.
- HMR -- watches modules and replaces the exported plugin, preserving the
  running fiber where the new plugin is compatible.

This is what makes DSH bundles work: a bundle patch is a set of rows that the
loader merges into the tree, not a set of files the host must special-case.

### 2.6 What transfers, and what does not

| Cordis idea | Transfer to BSH? | Reason |
|---|---|---|
| Context as the plugin API | **Yes, as a Rust facade** | Gives one ownership surface for tools, events, services, and UI |
| Fiber epochs and reload | **Yes** | Required for reactive dependencies and hot reload |
| Revertible effects | **Yes, including async cleanup** | Required for process/WASM/service teardown |
| Reactive coeffects | **Yes** | The central missing mechanism |
| Typed event modes | **Yes, for in-process host events** | Enables safe interception; JSON bus stays for remote observers |
| Loader tree / groups / patches | **Yes, as host-level profiles** | Needed for bundles and presets |
| Dynamic property access (ctx.anything) | **No, not in Rust** | Type safety and auditability; use explicit service names/contracts |
| Unrestricted service access | **No** | BSH's capability model is stronger and must remain |
| !!js config evaluation | **No in the host** | Code execution in config is a supply-chain hazard; only the JS adapter may evaluate inside its sandbox |

---

## 3. The current BSH extension framework

### 3.1 Layers

~~~
Package store + SemVer resolver + trust/signature
                        |
             Capability policy + audit
                        |
              Runtime dispatcher
        Builtin | Declarative | Process | WASM | Extism | MCP
                        |
        Contribution installation into one Agent build
        (tools, commands, panels, prompts, skills, providers)
                        |
                Lifecycle records + status + CLI
~~~

The platform is implemented in crates/bsh/src/extension/. It is independent of
GPUI, which is correct and should remain true.

### 3.2 Activation path today

install_with_fibers opens or refreshes one process-global ExtensionHost, then
calls apply_with_fibers. For each loaded package it:

1. builds a Contributed descriptor from the package and its declarative files;
2. calls contribution::install_with_lease, which registers tools, prompt
   sections, skills, commands, panels, providers, and owner-scoped plugin
   entries into the Agent and PluginRegistry;
3. creates a Fiber, moves it Loading -> Active, and attaches the lease's
   liveness gate to it;
4. stores the result in an AppliedExtension handle.

AppliedExtension is build-scoped. Its own documentation says the caller must
call unload before reusing the Agent/PluginRegistry pair; dropping the build
drops the registries, but dropping the handle alone does not physically remove
the contributions.

~~~rust
// crates/bsh/src/extension/host.rs (paraphrased)
pub(crate) struct AppliedExtension {
    fiber: Fiber,
    lease: ContributionLease,
    services: Arc<ServiceRegistry>,
    // ...
}
~~~

ContributionLease::revoke is the only path that removes tools, prompts, skills,
commands, and panels. attach_to_fiber only flips an AtomicBool liveness gate.

### 3.3 Services today

crates/bsh/src/extension/composable.rs (2.0-A) provides:

- scoped keys (scope, name, TypeId) with scopes Host, Workspace, Session,
  Extension;
- generation-safe provider tokens;
- call-scoped ServiceLease<T> with in-flight draining;
- availability notifications Available, Replaced, Unavailable;
- a deterministic dependency graph and topological activation plan.

It deliberately does **not** provide:

- a consumer for those availability notifications;
- parent-scope inheritance or named isolation realms;
- a context through which a component can register anything;
- service provisioning from Host, Workspace, or Session scopes by a build-scoped
  activation (the host fails closed for those scopes).

The standard states the boundary plainly: *"Runtime transport/binding of
manifest contracts to typed service values is a subsequent implementation step"*
(docs/bsh-extension-standard.md section 4.8).

### 3.4 Effects today

EffectSet stores Box<dyn FnOnce() + Send> cleanups. It is:

- synchronous only;
- flat (no nested scopes);
- idempotent and reverse-ordered;
- panic-isolated with catch_unwind;
- run automatically on Drop, but async resources cannot be awaited there.

That is a good start, but it cannot own a process kill, a WASM store teardown, an
HTTP subscription, or any cleanup that must await.

### 3.5 Events and hooks today

The extension EventBus is a bounded JSON topic bus with filters and metrics. It
is observer-oriented. The host hook system is explicitly observe-only:

> "A hook is not an event. ... Intercept hooks (which may deny or replace) are a
> separate class and are not implemented by this module yet."
> -- crates/bsh/src/extension/hooks.rs

There is no waterfall chain, no in-process typed event, and no extension-visible
lifecycle event.

### 3.6 Composition today

The SemVer resolver selects a package set. Manifest dependencies and hard
services.requires produce a topological activation order. That is a plan, but it
is flat: no groups, no profiles, no patches, no nested composition. RFC-EXT-002
lists "profile/bundle/patch composition" as a later slice (2.0-E), and the DSH
bundle refusal remains.

### 3.7 The DSH bridge today

crates/agent/src/extensions contains a Node subprocess bridge and a
dsh-host-shim.js that executes real DSH plugins. It is a valuable prototype, but
it is not the platform:

- it lives in the legacy shim, not in crates/bsh/src/extension;
- ctx.effect is tracked locally but not tied to a BSH Fiber;
- ctx.inject is answered once from the services present at load;
- ctx.on listeners are recorded, not dispatched for arbitrary events;
- ctx.llm, ctx.fs, ctx.subprocess, ctx.slots, and ctx.loader are refusing seams;
- dsh.bundle.patch rows are parsed and dropped rather than mounted.

The bridge proves demand. The design below turns its successful parts into a
first-class runtime and replaces the ad-hoc shim with a Cordis-compatible host
protocol.

### 3.8 What should not change

- The package store, trust model, signing, and SemVer resolver.
- Capability policy and audit at every protected operation.
- The six execution runtimes and their sandbox limits.
- The declarative UI contract and host-owned GPUI objects.
- The extension CLI vocabulary and status surfaces.

## 4. Gap analysis

Severity is about user-visible capability loss, not code size.

| # | Dimension | Cordis | BSH today | Gap | Severity |
|---|---|---|---|---|---|
| G1 | Reactive dependencies | Epoch recompute reloads/suspends dependents | Plan once; ServiceChange has no consumer | Dynamic services cannot appear or disappear safely | **Critical** |
| G2 | Effect ownership | Every registration is a disposer; async and nested | Sync flat cleanups; explicit lease revoke | Leaks on forget; cannot await teardown | **Critical** |
| G3 | Context API | One surface for services, events, effects, plugins | AppliedExtension handle plus manual adapters | No nested composition; every contribution needs bespoke code | **High** |
| G4 | Fiber lifecycle | Epoch, reload, suspend, dependents, inertia | Thin state machine around one apply | No live reactivation or reload | **High** |
| G5 | Composition | Entry tree, groups, isolate, patches, HMR | Flat package set | Bundles and profiles cannot be represented | **High** |
| G6 | Events | Five dispatch modes, context-scoped, internal events | JSON pub/sub; observe-only hooks | No waterfall or interception; no lifecycle events | **High** |
| G7 | Service scope | Prototype chain plus isolate realms | Exact scope only; host/workspace/session fail closed | Cannot compose per-workspace or per-session services | **High** |
| G8 | Config | Schema-validated reactive fiber.update | Schema subset at apply; no live update | Config changes need a rebuild | **Medium** |
| G9 | Hot reload | HMR preserves fibers | Package update/rollback restarts the host | Slow dev loop; no live repair | **Medium** |
| G10 | Diagnostics | Effect tree, fiber state, why-inactive | Lifecycle/status/metrics | Operators cannot see the composition graph | **Medium** |
| G11 | Cordis bundles | Native | Legacy bridge, refusing seams, patches refused | Real DSH packages contribute little or nothing | **High** |
| G12 | Cross-runtime services | JS objects | Typed Arc<T> only | Process/WASM extensions cannot provide services | **Medium** |

### 4.1 G1 -- no reactive coeffect

Evidence:

- ServiceRegistry publishes ServiceChange on provide, replace, and revoke
  (composable.rs), but a workspace-wide search finds no consumer outside tests.
- RFC-EXT-002's own status says: *"Provider replacement reconciliation and
  dependent Fiber reload remain future work; the registry's lease/drain
  primitive exists but is not yet wired into extension Fiber replacement or
  unload."*
- AppliedExtension::provide_service explicitly rejects Host, Workspace, and
  Session scopes because a build-scoped fiber cannot own their lifetime.

Consequence: a consumer either captures a stale Arc or fails at activation.
There is no supported way to write "start when the search index is ready, stop
when it goes away, restart when it is replaced."

### 4.2 G2 -- effects are not yet the ownership model

The current model has two independent ownership mechanisms:

- EffectSet for cleanup closures;
- ContributionLease for host registrations.

They are bridged by one closure that flips a liveness AtomicBool. Actual removal
requires AppliedExtension::unload(agent, registry). Dropping a fiber does not
call it. This is documented rather than hidden, but it is not a composable
ownership model:

- a forgotten unload leaks tools, prompts, skills, commands, and panels;
- a failed partial activation relies on the caller to revoke the lease;
- async teardown (process kill, WASM store, HTTP subscription) cannot be
  expressed;
- nested registrations (a tool that subscribes to an event that owns another
  registration) have no scope tree.

### 4.3 G3 -- no context surface

A built-in component today receives host services through Contributed and
install_with_lease, not through a context. It cannot say:

~~~rust
ctx.effect("register git tool", || agent.register_tool(...))?;
ctx.on("tool.before", handler)?;
ctx.provide("com.example.git", service)?;
ctx.plugin(child_component, config)?;
~~~

Instead each capability has a bespoke install function. That is why host.rs is
3,300 lines and why every new contribution type touches several modules.

### 4.4 G4 -- the fiber is a label, not a scheduler

The current Fiber has legal transitions, an effect set, and an id. It does not
have:

- an epoch or a dependency snapshot;
- a reload or suspend operation;
- a parent/child tree;
- a dependents set;
- an await_idle equivalent;
- a failure reason that distinguishes "dependency missing" from "apply failed".

The persisted LifecycleRecord in lifecycle.rs is a package record, not a live
activation. The two are not connected by a reconciler.

### 4.5 G5 -- no composition tree

The resolver picks package versions; the host activates **all** enabled packages
that pass the preset filter. There is no way to express:

- a group of entries that can be enabled as a unit;
- a profile that selects a subset and overlays config;
- a patch that changes one entry's config without editing the package;
- an isolation realm shared by two entries but not a third;
- "mount this DSH bundle patch as rows".

RFC-EXT-002's 2.0-E is the right home for this. The design below gives it a
concrete data model.

### 4.6 G6 -- events are observer-only

The hook module's own doc says interception is not implemented. The extension
event bus is a JSON queue, not an in-process dispatch. Consequences:

- model request and response policy cannot be expressed as a plugin;
- tool-before and tool-after cannot deny or rewrite;
- a plugin cannot observe its own fiber lifecycle;
- there is no waterfall for configuration or capability decisions.

### 4.7 G7 -- scope is exact and flat

ServiceRegistry::get and acquire take an exact Scope. There is no inheritance. A
session-scoped consumer cannot see a workspace-scoped provider unless it names
the workspace scope explicitly. The RFC-EXT-002 decision to defer implicit
fallback was reasonable for safety, but the result is that the common
"host -> workspace -> session -> extension" hierarchy is not usable without every
consumer hard-coding all ancestor scopes.

Cordis's prototype chain solves this, but it also makes ownership hard to audit.
BSH needs a **controlled hierarchy**, not an arbitrary prototype chain.

### 4.8 G8--G10 -- config, hot reload, diagnostics

Config is validated at apply time with a JSON Schema subset and migrations. There
is no fiber.update. Package update and rollback replace files and rebuild the
host. Diagnostics show lifecycle state and counters but not the live graph: which
fiber is waiting on which service, which effect is still registered, and why an
entry is not active.

### 4.9 G11 -- real Cordis packages are second-class

docs/extension-standards.md measures the DSH ecosystem: 301 host entries, all
Cordis plugins, with the minimum surface being ctx.effect, ctx.on, ctx.logger,
ctx.get, ctx.inject, ctx.fiber, ctx.tools, ctx.llm, ctx.commands, and ctx.skills.
The legacy bridge implements a subset and refuses the rest by name. That is
honest, but it means the largest available plugin ecosystem cannot contribute its
actual payload.

### 4.10 G12 -- cross-runtime services

ServiceRegistry stores Arc<T> where T: Any. A process or WASM extension cannot
provide an Arc<T>; it can only expose JSON-RPC methods. There is no
ServiceEndpoint abstraction for native vs remote services, so service
composition is effectively native-only.

---

## 5. Design principles

1. **Semantics, not dependency.** Implement Cordis's context, fiber, effect, and
   coeffect discipline in Rust. Do not link Cordis. The JS runtime, when used, is
   an adapter behind the BSH Runtime trait.
2. **One ownership rule.** Everything a component registers is an effect of its
   fiber. If it is not an effect, it is not owned, and that is a bug.
3. **Plan before activate.** The composition plan is immutable and validated
   before any effect runs. Partial activation is rolled back in reverse order.
4. **Reactive, but bounded.** Dependency changes coalesce and are applied by one
   scheduler. No unbounded reload storms; no polling.
5. **Fail closed and explain.** Missing dependencies suspend with a typed reason;
   unauthorized access is denied and audited; diagnostics say why.
6. **Exact where it matters, inherited where it is safe.** Extension-scoped
   services never leak. Host, workspace, and session services follow an explicit,
   auditable hierarchy.
7. **Compatibility is a feature.** Manifest 1.0 packages, existing CLI commands,
   and current status rows keep working. New fields are additive.
8. **Performance is part of the contract.** Service lookup stays O(1); the
   reconciler is event-driven; no per-render or per-token graph walk.

## 6. Target architecture

~~~
                    +-----------------------------------------+
                    |            ExtensionHost                |
                    |  store / resolver / policy / trust      |
                    +---------------+---------------+---------+
                                    | resolved package set
                    +---------------v-------------------------+
                    |          CompositionPlanner             |
                    |  profiles + bundles + patches + deps    |
                    |  capability union + conflict checks     |
                    +---------------+---------------+---------+
                                    | immutable ActivationPlan
                    +---------------v-------------------------+
                    |           CompositionRuntime            |
                    |                                         |
                    |  Context tree      ServiceRegistry v2   |
                    |  Fiber tree        DependencyIndex      |
                    |  EffectScope       Reconciler           |
                    |  EventDispatcher   ConfigStore          |
                    +-------+---------------------+-----------+
                            |                     |
              +-------------v-------+   +---------v--------------+
              | Native components   |   | Runtime adapters        |
              | builtin/declarative |   | process / wasm / extism |
              |                     |   | mcp / cordis            |
              +---------------------+   +-------------------------+
                            |
              +-------------v-----------------------------------+
              | Existing host seams                             |
              | Agent tools / prompts / skills / commands / UI  |
              | EventBus (remote observers) / storage / audit   |
              +-------------------------------------------------+
~~~

The **CompositionRuntime** is new. It does not replace the package store,
resolver, policy engine, or runtimes. It sits between the resolved package set
and the existing contribution installation, and it owns the live graph.

### 6.1 Module layout

~~~
crates/bsh/src/extension/
|-- composable/
|   |-- mod.rs          # re-exports; the old composable.rs becomes this module
|   |-- context.rs      # Context, ContextKind, ScopePath, isolate
|   |-- effects.rs      # EffectScope, Cleanup, Disposer, DisposalReport
|   |-- fiber.rs        # Fiber v2, FiberState, Epoch, Inject, FiberHandle
|   |-- registry.rs     # ServiceRegistry v2, ServiceKey, ServiceLease
|   |-- graph.rs        # DependencyGraph, DependencyIndex, cycle checks
|   |-- plan.rs         # ActivationPlan, PlanEntry, Profile, Patch
|   |-- scheduler.rs    # Reconciler, coalescing, ordering
|   |-- events.rs       # DispatchMode, EventDispatcher, internal events
|   +-- config.rs       # ConfigHandle, validation, migration
|-- runtime/
|   +-- cordis/         # phase F: process adapter + shim + protocol
+-- host.rs             # orchestration; no longer owns composition semantics
~~~

The current single-file composable.rs is split mechanically. The tests move with
the types.

---

## 7. Core contracts

The sketches below are the intended shapes, not final signatures. They are
deliberately small enough to review before implementation.

### 7.1 Context

~~~rust
pub(crate) struct Context {
    inner: Arc<ContextInner>,
}

struct ContextInner {
    id: ContextId,
    parent: Option<Context>,
    kind: ContextKind,
    scope: ScopePath,
    fiber: FiberRef,
    effects: EffectScope,
    config: ConfigHandle,
}

pub(crate) enum ContextKind {
    Root,
    Extension(ExtensionId),
    Component { parent: FiberId, name: String },
    Session(SessionId),
    Workspace(WorkspaceId),
}

impl Context {
    pub(crate) fn root(host: &ExtensionHost) -> Context;
    pub(crate) fn extend(&self, kind: ContextKind) -> Context;

    /// Create a named isolation realm for one service name. Descendants inherit
    /// the mapping until another isolate() overrides it.
    pub(crate) fn isolate(&self, name: &str, realm: Realm) -> Context;

    /// Register a reversible effect. The callback may return one cleanup or a
    /// list; cleanup runs in reverse order and may be async.
    pub(crate) fn effect(
        &self,
        label: impl Into<String>,
        f: impl FnOnce(&Context) -> EffectResult,
    ) -> Result<Disposer, EffectError>;

    /// Declare and consume services. Access checks the manifest, the contract,
    /// the scope, and the capability policy before returning a lease.
    pub(crate) fn service<T: Service>(&self, name: &str) -> Result<ServiceLease<T>, ServiceError>;
    pub(crate) fn provide<T: Service>(
        &self,
        name: &str,
        value: Arc<T>,
    ) -> Result<ServiceToken, ServiceError>;

    /// In-process typed event registration; the disposer is an effect.
    pub(crate) fn on(&self, event: EventKey, order: Order, handler: Listener) -> Disposer;

    /// Mount a child component as a child fiber.
    pub(crate) fn plugin(
        &self,
        component: Arc<dyn Component>,
        config: Value,
    ) -> Result<FiberHandle, FiberError>;
}
~~~

Context is Clone (an Arc bump). It is the only type a component needs.

### 7.2 Component

~~~rust
pub(crate) trait Component: Send + Sync + 'static {
    fn name(&self) -> &str;

    fn inject(&self) -> Inject {
        Inject::default()
    }

    fn provides(&self) -> &[ServiceContractDecl] {
        &[]
    }

    /// Apply the component. Every registration must go through ctx.effect().
    fn apply(&self, ctx: &Context) -> Result<(), ExtensionError>;

    /// Optional live update. The default refuses, which forces a reload.
    fn update(
        &self,
        _ctx: &Context,
        _old: &Value,
        _new: &Value,
    ) -> Result<(), ExtensionError> {
        Err(ExtensionError::Unsupported("live update"))
    }
}
~~~

Built-in bundles become components. The declarative runtime becomes one generic
component that reads its files and registers contributions. The process, WASM,
Extism, MCP, and Cordis adapters become components whose apply sends protocol
messages and whose effects own the child runtime.

### 7.3 Inject

~~~rust
pub(crate) struct Inject {
    pub(crate) hard: Vec<ServiceRequirement>,
    pub(crate) soft: Vec<ServiceRequirement>,
}

pub(crate) struct ServiceRequirement {
    pub(crate) name: String,
    pub(crate) contract: ContractId,
    pub(crate) version: Requirement,
    pub(crate) scope: ServiceScope,
    /// What happens when a soft dependency changes.
    pub(crate) reaction: Reaction,
}

pub(crate) enum Reaction {
    /// Dispose and re-apply on any change (Cordis default behavior).
    Reload,
    /// Notify the component; it decides whether to reload.
    Notify,
}
~~~

Hard requirements gate activation at plan time and suspend at runtime. Soft
requirements never block activation; they participate in the epoch when present
and can trigger a reload when Reaction::Reload.

### 7.4 EffectScope

~~~rust
pub(crate) enum Cleanup {
    Sync(Box<dyn FnOnce() + Send + 'static>),
    Async(Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send + 'static>),
}

pub(crate) struct EffectScope {
    effects: Vec<EffectSlot>,
    disposed: bool,
}

impl EffectScope {
    pub(crate) fn push_sync(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce() + Send + 'static,
    ) -> Disposer;
    pub(crate) fn push_async(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce() -> BoxFuture<'static, ()> + Send + 'static,
    ) -> Disposer;
    pub(crate) fn child(&self, label: impl Into<String>) -> EffectScope;

    /// LIFO, panic-isolated, per-effect timeout, continues after failure.
    pub(crate) async fn dispose(&mut self) -> DisposalReport;
}

pub(crate) struct DisposalReport {
    pub(crate) attempted: usize,
    pub(crate) failed: Vec<EffectFailure>,
    pub(crate) timed_out: Vec<String>,
}
~~~

Properties:

- Disposer::dispose() sets a fired flag and runs the cleanup through its slot; a
  later parent disposal observes the flag and skips it.
- An effect registered after disposal runs immediately (sync) or is awaited
  (async) and is reported as late.
- EffectScope::child() registers the child's disposal as an effect of the parent,
  so child effects always run before the parent's earlier effects.
- Drop is best-effort: sync cleanups run; async cleanups are spawned with a
  warning and an effect_leaked metric. Correct code calls dispose().await.

### 7.5 Fiber

~~~rust
pub(crate) struct Fiber {
    id: FiberId,
    component: Arc<dyn Component>,
    parent: Option<FiberId>,
    ctx: Context,
    state: FiberState,
    epoch: Epoch,
    revision: u64,
    inject: Inject,
    effects: EffectScope,
    committed: BTreeMap<ServiceKey, Generation>,
    dependents: BTreeSet<FiberId>,
    children: BTreeSet<FiberId>,
    error: Option<FiberError>,
    inertia: Option<JoinHandle<()>>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Epoch {
    pub(crate) revision: u64,
    pub(crate) deps: BTreeMap<ServiceKey, Generation>,
}
~~~

refresh() recomputes the epoch from the inject set and current generations. If it
changed, it schedules one of:

| New dependency state | Action |
|---|---|
| all hard deps present, epoch changed | reload: dispose effects, apply again |
| a hard dep missing or inactive | suspend: dispose effects, state Pending |
| component failed | stay Failed until retry or a config update |

reload is cancel-safe: it checks the epoch before committing any registration; if
a newer epoch arrives, it rolls back and retries with the newer snapshot.

### 7.6 ServiceRegistry v2

~~~rust
pub(crate) struct ServiceKey {
    pub(crate) scope: Scope,
    pub(crate) realm: RealmId,
    pub(crate) name: String,
    pub(crate) contract: ContractId,
}

pub(crate) struct ServiceEntry {
    pub(crate) generation: Generation,
    pub(crate) provider: FiberId,
    pub(crate) value: Arc<dyn Any + Send + Sync>,
    pub(crate) policy: ServicePolicy,
}

pub(crate) struct ServiceRegistry {
    entries: RwLock<HashMap<ServiceKey, ServiceEntry>>,
    dependents: RwLock<HashMap<ServiceKey, BTreeSet<FiberId>>>,
    changes: broadcast::Sender<ServiceChange>,
    next_generation: AtomicU64,
}
~~~

Additions over the current registry:

- realm (from Context::isolate) is part of the key;
- contract is part of the key, so two providers with the same name but different
  contracts do not collide;
- dependents is maintained on acquire and release, or on declaration plus epoch;
- ServiceChange carries provider, contract, realm, and kind;
- provide and revoke publish after the state change, never before;
- revoke returns a retirement handle; the caller awaits the drain outside the
  lock.

Resolution:

~~~
resolve(ctx, name, contract):
  1. realm = ctx.isolate_realm(name)
  2. for scope in ctx.scope.ancestors():    # Extension -> Session -> Workspace -> Host
       if entry = registry.get(scope, realm, name, contract):
           if entry.visible_to(ctx) and policy.allows(ctx, entry):
               return lease(entry)
  3. return Missing or Unauthorized
~~~

Extension scope is never inherited. Host scope is visible to all. Workspace scope
is visible to its sessions and their extensions. Session scope is visible to its
extensions. This is a controlled hierarchy, not an arbitrary prototype chain.

### 7.7 Reconciler

~~~rust
pub(crate) struct Reconciler {
    queue: mpsc::Sender<ReconcileEvent>,
    index: Arc<DependencyIndex>,
    registry: Arc<ServiceRegistry>,
}

pub(crate) enum ReconcileEvent {
    ServiceChanged(ServiceChange),
    FiberFailed(FiberId, FiberError),
    ConfigChanged(FiberId, ConfigRevision),
    Replace(FiberId, Arc<dyn Component>),
    Shutdown,
}
~~~

Rules:

- one worker (or a bounded pool) per host, not per fiber;
- events for the same fiber coalesce to the newest epoch;
- activation order follows the plan's topological order;
- unload order follows reverse topological order: dependents before providers;
- every operation has a timeout and a cancellation token;
- a failed unload is logged and does not block the rest of the queue;
- a queue overflow marks the affected fibers dirty and schedules a full
  reconciliation rather than dropping the event silently.

### 7.8 Events

The remote EventBus remains JSON and bounded. The in-process dispatcher gains
Cordis's five modes for host and extension events:

~~~rust
pub(crate) enum DispatchMode {
    Emit,
    Parallel,
    Serial,
    Bail,
    Waterfall,
}

pub(crate) trait EventSpec {
    const NAME: &'static str;
    const MODE: DispatchMode;
    type Payload: Serialize + DeserializeOwned + Send + 'static;
    type Output;
}
~~~

For extension-declared events, the payload is serde_json::Value and the mode is
declared in the manifest or profile. For host events, the payload is a Rust type.
waterfall handlers receive a Next continuation and may replace the value.
Observers may not mutate; interceptors are a separate registration class that
requires an intercept capability and is audited.

Internal events mirror Cordis:

~~~
internal/fiber-created
internal/fiber-status     (fiber, old, new)
internal/service          (name, contract, scope, generation)
internal/config           (fiber, next)
internal/update           (fiber, config, next)
internal/dispose          (fiber)
~~~

They are delivered in-process only; they are never published on the remote bus by
default.

### 7.9 Config

~~~rust
pub(crate) struct ConfigHandle {
    schema: ConfigSchema,
    revision: u64,
    value: Value,
}

impl Fiber {
    /// Validate, migrate, then either update in place or reload.
    pub(crate) async fn update(&mut self, next: Value) -> Result<(), FiberError>;
}
~~~

Update algorithm:

1. validate next against the schema; reject with path-level issues;
2. run declared migrations if the schema version changed;
3. if the component implements update, call it with (old, new);
4. otherwise dispose and re-apply with next;
5. if the new activation fails, restore the previous config and re-apply it; if
   that also fails, mark Failed with both errors.

Config interpolation is host-owned and limited to declared variables. !!js is
never evaluated in the BSH host.

## 8. Normative semantics

This section is the contract implementation and tests should follow.

### 8.1 Activation

1. The planner validates the entire plan before any effect runs.
2. Entries activate in dependency-first order.
3. A hard dependency missing at plan time fails the plan (fail fast).
4. A hard dependency that disappears at runtime suspends the dependent; it does
   not crash the host.
5. A soft dependency never blocks activation.
6. Apply failure rolls back that fiber's effects and marks it Failed; unrelated
   fibers continue.

### 8.2 Reload

A reload is suspend followed by apply with the same component and the new config
and dependency snapshot. It must not expose a half-applied state:

- registrations are staged in the fiber's effect scope;
- if apply fails, the scope is disposed and the previous state is reported;
- if the epoch changed during apply, the result is discarded and retried.

### 8.3 Suspension

Suspension disposes effects, releases service provisions (after dependents), and
moves to Pending with a typed reason:

~~~rust
pub(crate) enum PendingReason {
    MissingService { name: String, contract: ContractId, scope: Scope },
    ProviderInactive { fiber: FiberId },
    ParentInactive { fiber: FiberId },
    AwaitingRetry { after: Duration },
}
~~~

### 8.4 Provider replacement

1. New generation is published atomically.
2. Old generation stops accepting new leases.
3. Dependents' epochs change; they reload against the new generation.
4. Old generation disposes after its in-flight leases drain and its dependents
   have released.
5. A stale cleanup from the old provider cannot revoke the new provider.

### 8.5 Dependency ordering and cycles

- Plan-time cycles are rejected.
- Runtime soft dependencies may introduce a cycle. The reconciler detects it
  before activating and returns FiberError::DependencyCycle; it does not
  deadlock.
- Provider unload waits for dependents; dependent unload never waits for a
  provider that is already unloading.

### 8.6 Effect disposal

- Reverse registration order.
- Child scopes before parent scopes.
- Each cleanup is panic-isolated.
- Each async cleanup has a timeout (default 5 seconds, component-overridable).
- Disposal is idempotent.
- An effect registered after disposal runs immediately and is reported.

### 8.7 Capability checks

- Declaring a service requirement is not a grant.
- Every acquisition checks the contract's required capabilities against the
  consumer's grants.
- Every protected host call re-checks at operation time.
- Interceptors require a separate capability.
- Service acquisition, denial, and interception are audited with provider,
  consumer, contract, scope, and generation.

### 8.8 Compatibility

- Manifest 1.0: synthesize a plan with one entry per enabled package and hard
  edges from dependencies and services.requires; behavior is unchanged except
  that dynamic service changes now suspend or reload instead of leaving stale
  handles.
- Manifest 1.1: adds reaction, service capabilities, and optional composition
  references.
- AppliedExtension remains as a facade over the new FiberHandle and
  ContributionLease so existing callers compile.

---

## 9. Composition: profiles, bundles, and patches

Composition is host-level. A package manifest describes what a package can
contribute; a profile describes what the host mounts.

### 9.1 Profile format

~~~toml
schema_version = "1.0"

[profile]
name = "coding"
extends = "default"

[[entry]]
id = "tools.git"
source = "builtin:git"
enabled = true

[[entry]]
id = "tools.search"
source = "package:com.example.search"
config = { limit = 20 }
isolate = { index = "workspace" }

[[entry]]
id = "tools"
group = true
entries = ["tools.git", "tools.search"]

[[patch]]
id = "tools.search"
config = { limit = 50 }
~~~

Each entry has:

| Field | Meaning |
|---|---|
| id | stable, unique within the profile; may be a colon-separated path |
| source | builtin:name, package:extension-id, group, or cordis:package#row |
| config | validated against the component or package schema |
| enabled | disabled rows stay in the plan but do not activate |
| isolate | per-service realm names; true means private |
| inject | optional overrides for declared soft dependencies |
| entries | child ids when group = true |

### 9.2 Planning

~~~
1. resolve the selected profile (extends chain)
2. expand groups and bundles depth-first
3. apply patches in declared order
4. resolve package versions for package and cordis sources
5. merge service contracts and compute provider -> consumer edges
6. check duplicate providers, missing hard providers, version mismatch, cycles
7. check contribution conflicts (tool, command, panel ids, route ids)
8. compute the capability union and per-entry grants
9. check resource budgets (tools, panels, memory, concurrency)
10. produce an immutable ActivationPlan with a topological order
~~~

Any failure rejects the whole plan before activation. A partially activated plan
is rolled back in reverse order.

### 9.3 DSH bundle patches

A cordis.patch.yml row maps to an entry:

| Patch field | Entry field |
|---|---|
| id or row key | id |
| name | source = cordis:package#name |
| config | config |
| disabled | enabled = not disabled |
| isolate | isolate |
| group | group = true |

Unknown rows are refused by name, not silently skipped. A patch that cannot be
mounted is a plan error, not a warning.

## 10. Cordis / DSH runtime adapter

This is the bridge from the current legacy shim to a first-class runtime. It is
phase F because it depends on phases A through D.

### 10.1 Why a subprocess

The existing bridge already established the decision: Node is present, adding an
embedded JS engine is a multi-hundred-megabyte dependency, and a subprocess is
the only honest OS boundary. A Cordis plugin is arbitrary JavaScript; running it
in-process in Rust would create a false sandbox.

### 10.2 Runtime kind and manifest

~~~toml
[runtime]
kind = "cordis"
entry = "lib/index.js"
startup = "on-demand"
~~~

The package is still a BSH package: it has an id, version, hash, trust level,
capabilities, and a manifest. The cordis runtime is an adapter, not an escape
from the package model.

### 10.3 Protocol

Newline-delimited JSON-RPC 2.0 over stdio, same framing and limits as the
process runtime. Host-to-guest methods:

~~~
cordis/handshake      { protocol, host, capabilities, config }
cordis/inject         { services: [{name, contract, version, scope, generation}] }
cordis/service        { name, contract, scope, generation, available }
cordis/config         { revision, value }
cordis/reload         { revision }
cordis/dispose        { reason }
cordis/call           { registration, arguments }   # tool invocation
cordis/event          { name, payload }
~~~

Guest-to-host frames:

~~~
effect/push           { id, label, kind: "sync" | "async" }
effect/dispose        { id }
service/provide       { name, contract, version, scope, private }
service/acquire       { name, contract, scope }
event/listen          { name, mode, order }
event/unlisten        { id }
tool/register         { descriptor }
command/register      { descriptor }
skill/register        { descriptor }
provider/register     { descriptor }
log                   { level, message }
refuse                { seam, reason }
~~~

Every registration frame creates an effect on the BSH side. When the guest sends
effect/dispose, the host disposes that effect. When the fiber unloads, the host
sends cordis/dispose and waits for the guest's local disposers to finish, bounded
by a timeout.

### 10.4 Shim behavior

The shim is a small Cordis-compatible context, not a reimplementation of the
whole framework:

- ctx.effect(fn) runs fn, stores the returned disposer in a tree, and sends
  effect/push.
- ctx.on(name, fn, options) records a listener, sends event/listen, and returns a
  disposer.
- ctx.inject(deps, cb) creates a child fiber that runs cb when all deps are
  present. ctx.get(name, false) returns undefined for a missing optional service.
- ctx.provide(name, value) registers a private service for other plugins in the
  same process; it becomes visible to the host only if the manifest declares it
  and the host accepts the contract.
- ctx.plugin(plugin, config) mounts a nested plugin as a child fiber. Its effects
  are tracked under the parent unless it declares isolation.
- ctx.logger forwards to the host log with the extension id and fiber id.
- ctx.config is the merged config from the profile, package, and patch.
- ctx.fiber exposes state, uid, dispose, and update as far as the protocol
  supports them.

Host service adapters:

| Cordis service | BSH behavior |
|---|---|
| ctx.tools | register returns a disposer; invocation calls back over the protocol |
| ctx.commands | register validated by the host; handler is a remote call |
| ctx.skills | register with an inline body is supported by the new skill contribution variant |
| ctx.llm | provider registration through the existing provider descriptor contract |
| ctx.fs | only with workspace.read or workspace.write; path-scoped |
| ctx.subprocess | refused by default; a separate high-risk capability if ever added |
| ctx.settings | read-only view of the extension's own config |
| ctx.credentials | named secrets only, with a per-call purpose |
| ctx.slots | refused: the DSH client plane is browser JS; BSH renders native GPUI |
| ctx.loader | host-owned; read-only introspection, no mutation |

### 10.5 Bundle mounting

A DSH package's cordis.patch.yml becomes a set of profile entries. The host does
not need the DSH loader; it needs the same reconciliation semantics:

1. parse rows into entries;
2. resolve each row to a package or module source;
3. merge config with the package's default;
4. validate the resulting plan;
5. activate entries in dependency order;
6. on patch change, reconcile entries by id (create, update, remove), not
   restart the world.

This is the concrete answer to RFC-EXT-002's "DSH Cordis bundle patches remain
unsupported".

### 10.6 Security

- The subprocess runs with the same OS limits as the process runtime.
- Every host adapter call goes through the policy engine with the extension's
  identity and grants. The shim cannot reach a capability the manifest did not
  request and the user did not grant.
- Service provision from the guest is private by default; cross-extension
  visibility requires a manifest declaration and host acceptance.
- !!js config evaluation is confined to the guest process and is never used to
  compute host paths, capabilities, or grants.
- The adapter does not make Cordis a trusted in-process framework; it makes it a
  sandboxed runtime behind BSH's policy.

---

## 11. Migration and rollout

### Phase A -- Context and effects (foundation)

Deliver:

- Context, ContextKind, ScopePath;
- EffectScope with sync, async, and nested cleanups;
- Disposer and DisposalReport;
- built-in bundles migrated to Component;
- ContributionLease becomes an effect of the fiber.

Exit criteria:

- dropping a FiberHandle without explicit unload produces a measurable leak
  warning, and explicit unload removes every contribution;
- async cleanup is awaited with a timeout;
- LIFO, idempotence, panic isolation, and late-effect tests pass;
- existing extension tests pass unchanged.

### Phase B -- Fiber epochs and reconciler

Deliver:

- Epoch, refresh, reload, suspend, await_idle;
- DependencyIndex wired to ServiceChange;
- Reconciler with coalescing and ordered activation and unload;
- typed PendingReason and FiberError.

Exit criteria:

- required service appears: pending fiber activates;
- required service disappears: active fiber suspends and releases effects;
- provider replacement: dependents reload against the new generation;
- provider unload waits for dependents;
- stale cleanup cannot revoke a replacement;
- no reload storm under 100 rapid changes (bounded by coalescing).

### Phase C -- Scope hierarchy and isolate

Deliver:

- controlled ancestor resolution for Host, Workspace, and Session;
- Context::isolate and RealmId;
- Host, Workspace, and Session provider ownership moved to persistent owners,
  not build-scoped AppliedExtension;
- service policy checks at acquisition.

Exit criteria:

- a session extension sees its session and workspace services without naming
  every scope;
- extension-scoped services never leak;
- two isolated realms with the same service name do not collide;
- unauthorized acquisition is denied and audited.

### Phase D -- Composition profiles and patches

Deliver:

- profiles, bundles, and patches on disk;
- CompositionPlanner and ActivationPlan;
- group expansion, patch ordering, conflict checks, transactional rollback;
- bsh ext graph, bsh ext why, bsh ext reload, bsh ext effects.

Exit criteria:

- a profile selects a subset and overlays config;
- a patch changes one entry without editing the package;
- an invalid plan activates nothing;
- a failed activation rolls back in reverse order;
- graph output matches the live fiber and dependency state.

### Phase E -- Event dispatch and interceptors

Deliver:

- typed EventDispatcher with five modes;
- internal lifecycle events;
- observer and interceptor registration classes;
- capability and audit for interceptors;
- waterfall hooks for model, tool, and config decisions.

Exit criteria:

- an observer cannot mutate a payload;
- a waterfall interceptor can wrap or short-circuit a model request;
- a denied interceptor is audited and does not run;
- listener removal is an effect and survives reload.

### Phase F -- Cordis runtime and bundles

Deliver:

- RuntimeKind::Cordis, runtime/cordis, host shim;
- protocol frames, effect tree, inject and service availability;
- DSH bundle patch to profile entries;
- fixture plugins and conformance suite.

Exit criteria:

- a real DSH plugin's apply(ctx, config) runs and registers a callable tool;
- ctx.effect cleanup runs on fiber unload;
- ctx.inject waits for a service that appears after load;
- a bundle patch mounts its rows, including groups and MCP rows where supported;
- refusing seams still refuse by name (no silent success).

### Compatibility mapping

| Current API | New API | Compatibility |
|---|---|---|
| apply_with_fibers | CompositionRuntime::activate(plan) | keep as wrapper |
| AppliedExtension | FiberHandle and ContributionLease | keep as facade |
| ServiceRegistry::provide | Context::provide and registry v2 | keep old signature |
| EffectSet | EffectScope | type alias or adapter |
| Fiber::unload | Fiber::dispose().await | keep sync wrapper that logs async leaks |
| EventBus | unchanged for remote observers | new dispatcher is additive |
| manifest 1.0 | synthesized plan | no package changes required |

## 12. Testing and conformance

### 12.1 Unit tests

- EffectScope: LIFO, idempotence, panic isolation, async timeout, late effect,
  child scope ordering.
- Epoch: revision and generation changes produce the expected reload or suspend.
- ServiceRegistry: realm and contract keys, generation safety, dependent index,
  stale revoke.
- DependencyGraph: missing, cycle, deterministic order, optional edges.
- Config: schema path errors, migration, rollback.
- EventDispatcher: all five modes, ordering, bail, waterfall short-circuit,
  listener removal.

### 12.2 Integration tests

- required service appears after activation;
- required service disappears and reappears;
- provider replacement reloads dependents;
- provider unload waits for dependents;
- two isolated realms;
- session and workspace inheritance;
- profile patch and rollback;
- group enable and disable;
- failed component isolation;
- intercept denial.

### 12.3 Security tests

- service acquisition without a declaration is denied;
- service acquisition without a capability is denied and audited;
- interceptor without capability is denied;
- ctx.fs path traversal is refused;
- ctx.subprocess is refused by default;
- ctx.slots is refused;
- a guest cannot revoke another extension's service;
- a stale guest effect cannot remove a replacement;
- bundle patch rows cannot introduce an undeclared capability.

### 12.4 Performance tests

- service resolve: p99 under 1 microsecond in-process, no allocation on the hot
  path beyond the lease Arc clones;
- 1,000 fibers with 10 dependencies each: activation plan under 50 ms;
- 100 rapid service changes: coalesced to a bounded number of reloads;
- idle reconciler: no polling, no wakeups;
- extension build with 50 packages: no regression against the current baseline.

### 12.5 Conformance fixtures

- effect-tree: nested effects, async disposer, panic.
- inject-wait: plugin waits for a service provided later.
- provider-replace: two generations, dependent reload, drain.
- isolate-realms: same name, two realms.
- waterfall-policy: interceptor wraps a request.
- cordis-bundle: a DSH patch with a group and a tool row.
- cordis-refuse: a plugin touching ctx.llm without a grant gets a named refusal.

---

## 13. Performance notes

The design must not regress BSH's gold requirement.

- **Service lookup** is a read-lock hash lookup plus Arc clones. The realm and
  contract are part of the key, so no second resolution pass.
- **Dependency index** is updated on provide and revoke, not by scanning fibers.
- **Reconciler** is event-driven and coalesces by fiber; there is no periodic
  graph walk.
- **Effects** store an enum, so the common sync cleanup does not allocate a boxed
  future.
- **Context** is an Arc; extending it clones two pointers and allocates one small
  inner struct per component, not per call.
- **Leases** are per call only where drain safety matters (tools and model
  calls); read-only introspection uses a snapshot.
- **Host lock**: the current process-global Mutex over ExtensionHost is on the
  build path, not the tool hot path. If profiling shows contention, split the
  host into an Arc with interior mutability per subsystem.
- **Cordis adapter**: the subprocess boundary is the cost; tool calls are one
  round trip, same as the existing process runtime. Batch effect and event frames
  during load to avoid per-registration syscalls where possible.

---

## 14. Security analysis

### 14.1 Threats

| Threat | Control |
|---|---|
| Malicious package declares a service to intercept secrets | Contract policy, capability check, audit |
| Guest provides a service with a spoofed contract | Host validates contract id, version, and provider identity |
| Stale cleanup removes a replacement | Generation-bound tokens |
| Reload race leaves half-registered tools | Staged effect scope and epoch check before commit |
| Interceptor exfiltrates model data | Interceptor capability, audit, bounded payloads |
| Config injection reaches host paths | No !!js in host; interpolation restricted to declared values |
| Bundle patch adds an undeclared capability | Plan-time capability union check; patch cannot grant |
| Subprocess escapes resource limits | Same OS limits as process runtime; fail closed |

### 14.2 Non-negotiables

- Service discovery is not authorization.
- Registration is not authorization.
- A manifest declaration is a request, not a grant.
- Interception is a capability-bearing contribution, not an accidental extension
  of observer hooks.
- The host owns GPUI objects; ctx.slots remains refused.

---

## 15. Risks and alternatives

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Cordis semantics are subtle and easy to get wrong | High | Medium | Conformance suite first; small primitives; no big-bang rewrite |
| Async cleanup in Drop causes leaks or hangs | Medium | High | Explicit dispose await; bounded timeout; leak metric |
| Dependent wait deadlocks | Medium | High | Topological order; cycle detection; timeout; no waiting on unloading providers |
| Reload storms | Medium | Medium | Coalescing; dirty flags; bounded queue; backoff |
| Service hierarchy leaks session data | Low | Critical | Exact extension scope; explicit visibility; audit |
| Cordis adapter becomes a second platform | Medium | High | Keep it behind Runtime; one contribution model; no parallel registry |
| Composition profiles duplicate presets | Medium | Low | Profiles subsume preset filters; presets become named profiles |
| Performance regression | Medium | High | Benchmarks in CI; O(1) lookup; no per-token graph walk |

Alternatives considered:

1. **Keep the current model and add ad-hoc dependency callbacks.** Rejected: it
   recreates the same ordering, cleanup, and race problems in every subsystem.
2. **Import Cordis as a Rust or JS dependency.** Rejected: wrong language
   boundary, no capability model, large runtime, and the existing RFC explicitly
   avoids it.
3. **Implement only the Cordis bridge, not the Rust core.** Rejected: leaves
   native extensions and host services outside the composition model; the bridge
   would become a second framework.
4. **Copy Cordis's dynamic proxy API into Rust.** Rejected: loses type safety and
   auditability; explicit contracts are a BSH advantage.

---

## 16. Implementation backlog

Estimates are planning assumptions, not commitments.

| ID | Deliverable | Depends | Estimate |
|---|---|---|---|
| EXT3-001 | Split composable.rs into context, effects, fiber, registry modules | -- | 2d |
| EXT3-002 | EffectScope with sync, async, nested cleanup and DisposalReport | 001 | 3d |
| EXT3-003 | Context and Component trait; migrate built-in bundles | 002 | 4d |
| EXT3-004 | Make ContributionLease an effect; automatic unload on dispose | 003 | 3d |
| EXT3-005 | Epoch, refresh, reload, suspend, await_idle | 002 | 4d |
| EXT3-006 | DependencyIndex and Reconciler with coalescing | 005 | 4d |
| EXT3-007 | Typed PendingReason and FiberError; diagnostics in status | 005 | 2d |
| EXT3-008 | Controlled scope hierarchy and RealmId | 003 | 3d |
| EXT3-009 | Persistent Host, Workspace, Session service owners | 008 | 4d |
| EXT3-010 | Service contract policy and acquisition audit | 008 | 3d |
| EXT3-011 | ConfigHandle, Fiber::update, rollback | 005 | 3d |
| EXT3-012 | ActivationPlan, profiles, groups, patches | 006 | 5d |
| EXT3-013 | Conflict checks, transactional rollback, plan diagnostics | 012 | 3d |
| EXT3-014 | bsh ext graph, why, effects, reload | 007, 012 | 4d |
| EXT3-015 | Typed EventDispatcher with five modes | 003 | 4d |
| EXT3-016 | Internal lifecycle events | 015 | 2d |
| EXT3-017 | Observer and interceptor split, capability, audit | 015 | 3d |
| EXT3-018 | Cordis runtime protocol and shim | 006, 010 | 6d |
| EXT3-019 | Bundle patch to profile entries | 012, 018 | 4d |
| EXT3-020 | Conformance fixtures, security tests, benchmarks | all | 5d |

Total: roughly 71 engineer-days before review and platform-specific work.

---

## 17. Open questions

1. Should Context be the only public surface for built-in components, or should
   AppliedExtension remain the stable facade for host code? (Proposal: context
   for new code, facade for compatibility.)
2. Is Reaction::Reload the right default for soft dependencies, or should the
   default be Notify to avoid surprising reloads?
3. Should config update be full reload by default, or should components opt into
   in-place update?
4. Does service inheritance default to enabled for Host, Workspace, and Session,
   or must a consumer declare inherit = true?
5. Should interceptors be a separate manifest contribution type or an event-mode
   flag? (Proposal: separate type, separate capability.)
6. Is a per-host reconciler enough, or does a large workspace need per-scope
   schedulers?
7. How much of the DSH client plane can ever be represented on native GPUI?
   (Current answer: none; ctx.slots remains refused.)
8. Do we need a WASM Cordis runtime for untrusted bundles, or is the subprocess
   boundary sufficient?
9. Should the Cordis adapter expose ctx.fs at all, or only the declarative
   contribution seams?
10. What is the migration policy for packages that rely on the current
    "activation never re-evaluates dependencies" behavior?

---

## 18. Appendix: Cordis source map

Repository: [cordiverse/cordis](https://github.com/cordiverse/cordis), commit
2bca2278451bc5914a91c6d658658b7905dd87d3 (2026-11-02).

| Concern | File | Key lines |
|---|---|---|
| Context, isolate, intercept | packages/core/src/context.ts | 20--78 |
| Service base, provide, check, intercept merge | packages/core/src/service.ts | 6--83 |
| Plugin types, inject, registry, replace | packages/core/src/registry.ts | 63--123, 185--245 |
| Fiber state, effect, epoch, reload and unload | packages/core/src/fiber.ts | 90--97, 292--357, 418--514 |
| Service resolution, provide, notify, mixin | packages/core/src/reflect.ts | 83--102, 213--270 |
| Event modes and internal events | packages/core/src/events.ts | 14--35, 110--163, 211--224 |
| Loader groups and reconciliation | packages/loader/src/config/group.ts | 7--120 |
| Loader tree and entry changes | packages/loader/src/config/tree.ts | 51--260 |
| Loader isolation realms | packages/loader/src/config/isolate.ts | 16--159 |
| Loader entry, config, commit | packages/loader/src/config/entry.ts | 140--373 |
| HMR | packages/hmr/src/index.ts | 88--760 |
| Paper abstract | https://arxiv.org/abs/2608.25512 | -- |
| DSH primer | https://deepseek-harness.github.io/deepseek-harness/reference/cordis-primer | -- |

---

## 19. Final recommendation

Build the missing **live composition layer** in Rust, not a Cordis clone:

1. **Context and EffectScope** -- one ownership surface, every registration
   reversible.
2. **Fiber epochs and Reconciler** -- reactive coeffects, dependency-aware
   teardown, no stale services.
3. **Scoped service graph** -- controlled inheritance, explicit isolation,
   capability-checked access.
4. **Composition profiles and patches** -- deterministic plans, groups, bundles,
   rollback.
5. **Typed dispatch and interceptors** -- policy as a contribution, not host
   code.
6. **Cordis and DSH runtime adapter** -- real bundles behind the same policy and
   ownership model.

The first three are RFC-EXT-002's unfinished foundation and should land before
any new user-visible composition feature. The last three are what make BSH feel
like Cordis without giving up what makes BSH safe.
