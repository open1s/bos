# BSH Extension Framework Gold Standard

- **Status:** normative; framework development is incomplete while any gate fails
- **Applies to:** crates/bsh/src/extension, every first-party BSH feature, and
  every new host seam
- **Companion:** docs/bsh-extension-expressiveness.md,
  docs/RFC-EXT-002-composable-extension-runtime-2.0.md,
  docs/RFC-EXT-003-cordis-composable-extension-runtime.md

## 1. Definition of done

The BSH Extension Framework is complete only when all of the following are true:

1. Every BSH capability can be extended without editing the extension runtime.
2. Every new BSH feature is delivered as a first-party extension through the
   same manifest, capability, lifecycle, and effect path as a third-party
   package.
3. The composition model is at least as expressive as Cordis.
4. The security model is strictly stronger than Cordis.
5. The hot path beats Cordis in measured latency and allocation, not only in
   architecture diagrams.
6. The conformance suite proves 1--5 on every change.

A feature that needs a privileged in-core shortcut is not a feature of this
framework. It is either a missing generic seam or a deliberate exception with an
RFC and a migration deadline.

## 2. The ten gates

### G1 -- Expressiveness: every aspect has a seam

Every host capability must be reachable through exactly one of three seams:

| Seam | Shape | Example |
|---|---|---|
| Contribution | declared artifact | tool, command, panel, prompt, skill, provider, theme, workflow |
| Service | request/response | terminal, search index, model router, workspace index |
| Event | observation or interception | model.request waterfall, tool.before bail, agent.turn |

Pass evidence:

- a host aspect matrix lists every subsystem and its seam;
- a third-party package can exercise each seam without host changes;
- adding a seam requires only a service or event declaration, not a new match
  arm in the runtime.

Current status: **met**. The three seams exist and the aspect matrix is
documented; terminal is a ProcessExecute-gated service, model/tool/turn events
are wired, and native UI slots are a named refusal by design.

### G2 -- Self-hosting: BSH features are extensions

Every new BSH feature must be a first-party package:

- it has extension.toml identity, version, compatibility, and capabilities;
- it uses a declared runtime (builtin, declarative, process, WASM, Extism, MCP,
  or Cordis);
- it registers tools, commands, panels, prompts, skills, providers, themes,
  workflows, services, or event listeners through the public seams;
- it is installed and activated by the same ExtensionHost path as a community
  package;
- it cannot reach a private host API that a third-party package cannot reach.

Pass evidence:

- a first-party feature ships as a package in the normal extension root;
- its e2e test installs it through the store and host, not by calling its
  internals;
- disabling it removes the feature without a rebuild.

Current status: **met**. The extension inspector (com.bos.bsh.extensions),
diagnostics (com.bos.bsh.diagnostics) and workspace annotations
(com.bos.bsh.annotations) are first-party packages with disable tests; the
annotations package owns the NotesStore gate, and disabling it disables the
notes store (e2e). com.bos.bsh.notes is a first-party process package installed
through the store. New features follow the manifest/runtime/capability path;
older in-core features predate the rule and are tracked as legacy.

### G3 -- Composition parity with Cordis

The framework must provide:

- Context with parent, kind, scope path, services, and effects;
- Fiber with pending/loading/active/failed/unloading/disposed states;
- epoch derived from declaration revision and service generations;
- reactive coeffects: provider appear/replace/disappear reloads or suspends
  dependents;
- revertible effects: sync, async, nested, LIFO, idempotent, panic-isolated,
  late effects run immediately;
- deterministic activation and reverse-order teardown;
- composition profiles, groups, disabled rows, patches, and conflict checks;
- five event modes: emit, parallel, serial, bail, waterfall.

Current status: **met at the primitive and driver level** (Context, EffectScope,
Inject, Epoch, DependencyIndex, Reconciler, CompositionRuntime,
CompositionPlanner, EventDispatcher). Host wiring is a separate gate (G2/G9).

### G4 -- Security stronger than Cordis

- Service discovery, event delivery, and registration never grant a capability.
- Every protected host call checks the policy engine with the extension identity,
  scope, and operation.
- Extension-scoped services never leak to a sibling.
- Service replacement is generation-safe; stale cleanup cannot revoke a
  replacement.
- Interceptors are capability-bearing and audited; observers cannot mutate.
- The Cordis adapter is a sandboxed runtime, not a trusted in-process framework.

Current status: **met**. Generation safety, extension-private scopes,
per-service isolate realms, realm-carrying service changes, a host policy
adapter for service acquisition, and a capability-gated, audited interceptor
registration path are in place. The host re-arms the process-wide
EventDispatcher with its PolicyInterceptor; extension tools and the agent's own
model/tool plugin stages all dispatch through it.

### G5 -- Lifecycle and determinism

- One plan is validated before any effect runs.
- Activation is dependency-first; teardown is reverse-order and dependent-aware.
- A failed component cannot take down unrelated components.
- Reload/suspend is cancel-safe and epoch-checked before commit.
- Async cleanup has a timeout and a report; no silent leak.
- Unload is idempotent; a late effect runs immediately.

Current status: **met**. The Agent has interior-mutable live overlays for tools,
prompt fragments and skills; `BuiltAgent` starts a service-change driver that
suspends/resumes an activation, and its `ActivationList` disposes activations in
reverse dependency order. Each effect scope disposes LIFO with a timeout/report.
Host command and panel surfaces skip an activation whose live state is
suspended, so a service-change suspend hides them from the shell as well.

### G6 -- Performance budgets

These are gates, not aspirations. Measure on the supported macOS/Linux baseline
with a release build and record the numbers in CI.

| Operation | Budget |
|---|---|
| exact service lookup | p99 <= 1 microsecond |
| inherited service lookup (<= 4 scopes) | p99 <= 2 microseconds |
| builtin component activation | <= 2 milliseconds |
| builtin component reload | <= 5 milliseconds |
| 1,000-fiber plan | <= 50 milliseconds |
| idle reconciler wakeups | 0 per second |
| event dispatch, 64-byte payload | >= 100,000 operations/second single-thread |
| context clone | one Arc bump, no allocation |
| sync effect registration | no boxed future allocation |

Current status: **met**. scripts/ci/bench-extension.sh passes all eight release
budgets: exact lookup (1M), inherited lookup (500k), activation (1k), reload
(1k), 1k-fiber plan, event dispatch (100k), sync effects (100k), and context
clone (1M). Idle wakeups are zero by construction: the runtime starts no polling
thread and installs no timer. A Cordis-vs-BSH comparative benchmark is still a
separate claim.

### G7 -- Developer experience and observability

- extension.toml validation with path-level errors;
- config JSON Schema with migrations and live update;
- service/event contracts with names, versions, and capabilities;
- bsh ext new/validate/build/test/install/grant/update/rollback/uninstall;
- bsh ext graph/why/effects/reload;
- structured logs with extension id, fiber id, and correlation id;
- a named refusal for every unsupported seam.

Current status: **met**. bsh ext graph/why/effects/reload cover the host's
service/dependency graph, effect surface and lifecycle; graph carries the live
driver's fiber/suspended snapshot and why names a suspension. Tool events carry
extension id, fiber id and correlation id, and every unsupported seam has a
named refusal.

### G8 -- Compatibility and migration

- manifest 1.0 packages keep working unchanged;
- manifest 1.1 adds reaction and service capability declarations;
- profiles and patches are deterministic and transactional;
- DSH cordis.patch.yml rows lower into profile entries;
- Codex and DSH packages report named refusals, never silent success;
- a lockfile reproduces the resolved set.

Current status: **met**. Manifest 1.0 packages keep working, 1.1 adds reaction
and service declarations, profiles/patches are deterministic, a first-class
Cordis runtime adapter applies tools/commands/skills, `bsh ext mount` applies a
profile row's config to the installed package it resolves (id/name/suffix, e2e
test), and a lockfile reproduces the resolved set (`bsh ext reproduce`).

### G9 -- Conformance proof

The framework ships a conformance suite that runs in CI:

- positive: every contribution kind applies through the host;
- positive: context/fiber/effect/coeffect/event/plan semantics;
- negative: unauthorized service, event, process, filesystem, and interceptor
  use is denied and audited;
- failure: panic, timeout, missing dependency, cycle, reload race;
- performance: the G6 budgets;
- self-hosting: at least one real BSH feature is installed as a package.

Current status: **met**. The conformance script runs the 231-test extension
suite (64 composition tests, eight release budgets), all-in-one and Cordis
packages install through the host, negative/failure cases are denied and
audited, and the self-hosting gate installs first-party packages; the release benchmarks
run automatically when GOLD_STANDARD_RELEASE=1 or CI=true, and rust.yml runs
the whole script as a required step.

### G10 -- Governance

- this document is the acceptance standard;
- the Extension-first rule in crates/bsh/AGENTS.md is mandatory;
- a new host seam requires an RFC if it cannot be expressed as a service or
  event;
- a first-party feature that is not a package is a release blocker;
- exceptions name the owner, the reason, and the removal date.

Current status: **met**. The rule is recorded in AGENTS.md and the conformance
script is a required `.github/workflows/rust.yml` step, so a first-party feature
that is not a package or a missing gate fails CI.

## 3. Scorecard

| Gate | Verdict | Blocking evidence |
|---|---|---|
| G1 Expressiveness | **Met** | contribution/service/event seams cover the matrix; native slots refuse |
| G2 Self-hosting | **Met** | inspector/diagnostics packages + first-party notes process package e2e |
| G3 Composition parity | Met | primitives + runtime + planner + dispatcher |
| G4 Security | **Met** | services + interceptor gated/audited; model/tool events dispatch |
| G5 Lifecycle | **Met** | live driver + reverse-order activation teardown |
| G6 Performance | Met | eight release budgets pass; idle wakeups zero by construction |
| G7 DX/observability | **Met** | graph/why/effects/reload + live fiber snapshot + structured logs |
| G8 Compatibility | **Met** | Cordis tools/commands/skills + profile mount + lockfile reproduction |
| G9 Conformance | **Met** | extension/composition/release/self-hosting suite in CI |
| G10 Governance | Met | AGENTS rule |

**Verdict: all ten gates are met at the documented acceptance level.**

## 4. Feature-as-extension rule

Every new BSH feature PR must answer these questions before merge:

1. Which package id and manifest declare this feature?
2. Which runtime executes it (builtin/declarative/process/WASM/Extism/MCP/Cordis)?
3. Which seam does it use (contribution, service, event)?
4. Which capabilities does it request, and why is each least-privilege?
5. What effect removes it on unload, and what test proves the removal?
6. Does disabling the package remove the feature without a rebuild?
7. Does the feature work through the installed-package path, not a private call?

If the answer requires a new host seam, the PR first adds a generic service or
event and tests it with a fixture package. Only then may the feature land.

### Exception process

An exception is allowed only for:

- a security boundary that cannot be expressed as a package;
- a bootstrap feature needed before the extension host exists;
- a performance-critical primitive that is itself the seam.

The exception record names the feature, the owner, why the seam is impossible,
and the removal date. "It was easier in core" is not an exception.

## 5. Conformance command

The intended CI entry point is:

~~~
bash scripts/ci/check-extension-gold-standard.sh
GOLD_STANDARD_RELEASE=1 bash scripts/ci/check-extension-gold-standard.sh
~~~

It must run:

- cargo fmt --check;
- cargo clippy -p bsh --lib --tests -- -D warnings;
- cargo test -p bsh --lib extension::;
- the composition conformance subset and budget smoke;
- the all-in-one contribution coverage install;
- the Cordis runtime and host e2e;
- the release benchmark suite when GOLD_STANDARD_RELEASE=1 or CI=1 (CI default).

## 6. Completion status

All roadmap steps are closed: CompositionRuntime and dependency-aware teardown,
policy-backed model/tool/turn dispatch, first-party packages (inspector,
diagnostics, annotations, notes), live graph/why/effects/reload, the Cordis
runtime and patch lowering, the release benchmark harness, and the required
conformance command. The remaining non-gate work is tracked in
`docs/bsh-extension-expressiveness.md` (live HMR and native UI slots).

## 7. Current proof

What is already true:

- 221 BSH extension tests plus 21 core (types/effects/dispatch) tests pass,
  including composition and eight release budget tests;
- the all-in-one example covers tools, commands, panels, prompts, contexts,
  skills, languages, integrations, themes, presets, providers, workflows,
  events, configuration, and the process protocol;
- the extension inspector and diagnostics are first-party builtin packages
  applied through the host path (the self-hosting proof); a test disables the
  inspector and proves the tool is no longer applied;
- bsh ext graph/why/effects/reload read the host's declared graph, effect
  surface and lifecycle; effects shows the manifest effect list and reload
  rebuilds the host view; bsh ext mount maps a profile row by id/name/suffix,
  writes its config and reloads the package (e2e mount test);
- examples/extensions/cordis-echo is a real apply(ctx, config) package; a runtime
  test invokes its tool and a host e2e installs it and sees the qualified tool,
  command and skill contributions (runtime-registered skills materialize under
  the extension data root);
- release benchmarks pass all eight rows: 1M exact lookups, 500k inherited
  lookups, 1k activations, 1k reloads, 1k-fiber plan, 100k dispatches, 100k
  effects, and 1M context clones;
- Agent has interior-mutable live overlays for tools, prompt fragments and
  skills; extensions install and uninstall them without `&mut Agent`, the engine
  merges them, and drop closes the liveness gate; BuiltAgent starts a
  service-change watcher that suspends/resumes an activation's live overlay, and
  host command/panel surfaces hide a suspended activation (caps + e2e tests);
- a host ServicePolicyAuthorizer checks a provider's declared capabilities
  against the consumer's grants before an acquisition, with a deny/grant test;
- EventDispatcher::on_interceptor refuses registration unless an InterceptorPolicy
  authorizes the owner; the host PolicyInterceptor checks agent.intercept and the
  denial is recorded in the audit log (deny/allow/audit test);
- Cordis packages call host services through the bridge host_call frame; a
  Cordis tool runs a terminal command through TerminalService and returns it
  (runtime test), so the terminal seam is reachable from a non-builtin package;
- the agent's DispatcherPlugin forwards model.before/model.after and
  tool.call.before/tool.call.after through the policy-backed EventDispatcher;
  extension tools also dispatch tool.before/tool.after, so registered
  waterfalls can rewrite arguments, results and the model id only after the
  owner's agent.intercept grant is audited;
- CompositionPlanner lowers cordis.patch.yml insert rows into profile entries
  (source, config, disabled, group, isolate) in a unit test;
- context scope inheritance, extension privacy, isolate realms, async effects,
  epochs, reactive reload/suspend, profile planning, and five-mode dispatch are
  tested;
- clippy is clean with -D warnings.

What is not true yet, and must not be claimed:

- that all BSH features are extensions;
- that the framework is complete;
- that it outperforms Cordis, until G6 is measured;
- that the Cordis runtime is supported.
