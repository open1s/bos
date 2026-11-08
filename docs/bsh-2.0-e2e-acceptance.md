# BSH 2.0 — Single-Node Workspace Acceptance

> Status: **acceptance specification / implementation gap record**  
> Scope: first end-to-end product proof for BSH 2.0 composition.  
> This document does not claim that the target Composition Engine is already implemented.

## 1. Why this is the first acceptance target

The first proof should be a single-node workspace that assembles a bounded set of capabilities, exposes only that set to an Agent, enforces the workspace's policy at invocation time, and tears down cleanly. Federation is deliberately excluded from this first gate: remote composition adds peer identity, dual authorization, delegation, leases and uncertain network outcomes. Those concerns should not obscure whether local composition works.

The product demonstration must prove behavior, not merely show a populated UI.

## 2. Code-backed baseline

The following observations are grounded in the current repository implementation and should be rechecked when this specification is executed against a newer revision.

| Area | Existing code path | What it proves | What it does not prove |
|---|---|---|---|
| Workspace configuration | `crates/bsh/src/settings/workspace.rs` | Per-workspace TOML settings; agent configuration; disabled tool, skill and extension lists | A versioned composition manifest or independent composition lifecycle |
| Agent assembly | `crates/bsh/src/caps.rs` | Agent build registers built-in bundles, installs Ferris extensions, then adds MCP tools | A deterministic Composition Engine that resolves a manifest before Agent construction |
| Tool selection | `crates/bsh/src/caps.rs` | Permission tier, preset allowlists and workspace disabled lists influence tool registration | A general typed Workspace Capability View spanning all provider types |
| Extension integration | `crates/bsh/src/extension/framework.rs`, `crates/bsh/src/extension/agent.rs` | Ferris manager/runtime integration and extension tool invocation adapters | Full composition-plan enforcement across every extension service acquisition |
| Ferris composition context | `crates/ferris-core/src/composable/context.rs` | Scoped contexts, service lookup, effect ownership and cleanup primitives | The file explicitly says manifest declaration and capability policy are not yet enforced at acquisition, pending composition-plan wiring |
| Permission enforcement | `crates/bsh/src/caps.rs`, policy modules and tool adapters | Current built-in tool registration and invocation checks | A universal policy decision for every future capability provider |

### Important distinction

The existing implementation is already composable in a limited, concrete sense: workspace settings, presets, disabled lists, built-in tool bundles, Ferris contributions and MCP tools affect the Agent build.

It is **not yet equivalent** to the BSH 2.0 target model. In particular, the current Agent build path directly assembles tools from settings and providers. The independent composition plan, versioned manifest resolution, normalized capability descriptors, a first-class workspace capability view, and pre-activation validation remain explicit implementation work.

Do not describe the existing workspace settings file as the BSH 2.0 composition manifest.

## 3. Golden scenario: Research Workspace

### User story

As a researcher, I want a dedicated workspace that can inspect project documents and synthesize evidence, but cannot modify source files or execute arbitrary shell commands, so that the Agent's available powers match the task.

### Composition intent

| Element | Desired configuration |
|---|---|
| Workspace | `research-demo`, rooted at a disposable fixture directory |
| Agent | Research assistant with an evidence-first system prompt |
| Model | Explicitly selected provider/model; no implicit credential fallback |
| Capabilities | File listing, bounded file reads, search within the workspace, approved research extensions |
| Excluded capabilities | Shell execution, file writes/edits, unrelated extensions, unapproved MCP servers |
| Policy | Read-only; workspace path boundary; bounded tool output; explicit network policy |
| Context | Only the fixture documents and explicitly attached research context |
| Lifecycle | Create → validate → activate → invoke → inspect audit → deactivate |

The exact capability IDs must come from the canonical registry once the Composition Engine exists. Until then, tests should assert the concrete current tool names at the Agent boundary rather than inventing a parallel capability naming scheme.

### Fixture

Use a temporary project with:
- `README.md`: project purpose and scope.
- `design.md`: architecture decisions and constraints.
- `evidence/` containing two short source documents with intentionally conflicting claims.
- `private/` containing a sentinel file that is outside the granted workspace or explicitly denied by policy.
- `expected-findings.json`: machine-checkable expected facts and source references.

No real credentials, private user documents or external network dependency should be required.

## 4. End-to-end acceptance flow

### A. Compose

1. Load a workspace composition request.
2. Resolve the requested local capabilities and approved extension contributions.
3. Reject unknown capability IDs, incompatible versions, missing dependencies and duplicate/conflicting provider bindings.
4. Resolve the workspace's effective model, context, policy and resource limits.
5. Produce an immutable, versioned composition result and a workspace-scoped capability view.

**Acceptance:** identical inputs and registry snapshots produce the same resolved composition; invalid requests fail before activation and do not partially register tools or start providers.

### B. Validate before activation

Validate all of the following before any provider is started:
- Capability exists and is enabled.
- Provider/runtime is available and compatible.
- Manifest-declared requirements are satisfied.
- Workspace and extension grants cover requested scopes.
- Permission mode and preset ceilings do not conflict.
- Resource limits are finite and internally consistent.
- No capability is exposed solely because it was discovered or installed.

**Acceptance:** a denied or invalid composition leaves no active provider, service registration, tool schema or leaked resource.

### C. Activate and expose

1. Activate the resolved provider set under a workspace-owned lifecycle scope.
2. Build the Workspace Capability View from successfully activated and authorized capabilities only.
3. Build the Agent using that view; do not let the Agent enumerate the node-global registry.
4. Keep provider identity and invocation routing stable for the lifetime of the composition revision.

**Acceptance:** the model-facing tool list equals the authorized workspace view, not the global registry or the raw extension inventory.

### D. Invoke and enforce

Run a deterministic task against the fixture:

> Read the design and evidence documents, identify the two claims that conflict, cite the exact source files, and produce a concise findings report. Do not modify any file.

Verify:
- Allowed reads/searches succeed.
- A shell invocation is absent from the model-facing tool list and cannot be reached through an alternate dispatch path.
- A write/edit invocation is absent and is denied if invoked through a lower-level test seam.
- A path outside the workspace is denied before file contents are read.
- Output and runtime budgets are enforced.
- Every decision is attributable to workspace, session, capability, provider and policy rule.

**Acceptance:** the report matches expected facts and references; forbidden operations fail closed; audit records explain both allow and deny decisions.

### E. Deactivate and recover

1. Deactivate the workspace composition.
2. Dispose effects and provider resources in reverse dependency order.
3. Ensure no workspace-scoped service, tool or task remains reachable.
4. Re-activate the same manifest and confirm deterministic behavior.
5. Simulate provider activation failure and verify rollback.

**Acceptance:** teardown is idempotent; no stale capability remains visible; failed activation leaves the previous active revision intact or leaves the workspace inactive according to the declared update strategy—never half-active.

## 5. Required negative tests

| ID | Fault injection | Required result |
|---|---|---|
| COMP-001 | Unknown capability | Reject before activation |
| COMP-002 | Missing dependency | Reject with dependency path |
| COMP-003 | Version incompatibility | Reject; no silent downgrade |
| COMP-004 | Duplicate capability/provider binding | Deterministic conflict error |
| COMP-005 | Capability not granted to workspace | Not visible and invocation denied |
| COMP-006 | Provider activation fails midway | Roll back all newly activated resources |
| COMP-007 | Workspace is deactivated during invocation | Cancellation/closed-state behavior is explicit; no new calls accepted |
| COMP-008 | Path escapes workspace | Deny before read/write side effects |
| COMP-009 | Read-only composition attempts write | Deny at both registration and invocation boundaries |
| COMP-010 | Extension attempts undeclared service acquisition | Deny at acquisition boundary |
| COMP-011 | Stale composition revision | Reject invocation or route only to the still-valid pinned revision |
| COMP-012 | Resource budget exhausted | Stop/deny predictably and record the reason |
| COMP-013 | Repeated activation/deactivation | No leaked registrations, tasks or provider processes |
| COMP-014 | Registry changes during resolution | Resolve against a stable snapshot or detect revision change and retry/fail |

## 6. Product demo acceptance

A demo is ready only when all of these are true:

- [ ] The workspace is created from a versioned composition request/manifest.
- [ ] The UI shows the resolved capability set and effective permissions.
- [ ] The user can distinguish installed, available, selected, active and denied capabilities.
- [ ] The Agent receives only the workspace-scoped capability view.
- [ ] A real task completes against a disposable fixture.
- [ ] A forbidden operation is visibly denied and recorded.
- [ ] Deactivation removes workspace-scoped capabilities.
- [ ] Re-activation is repeatable.
- [ ] A real screenshot is captured from the running BSH build; conceptual SVGs are not used as UI evidence.

## 7. Implementation gap and recommended order

| Priority | Gap | Deliverable | Exit condition |
|---|---|---|---|
| P0 | No independent Composition Engine wired into Agent build | Composition request, resolver, immutable plan and activation coordinator | Agent construction consumes a validated composition result |
| P0 | No canonical workspace capability view across provider types | Typed capability descriptor + scoped view + invocation handle | Model schema and dispatcher derive from the same authorized view |
| P0 | Ferris acquisition is not yet manifest-plan enforced | Host-side service acquisition authorizer tied to composition/grants | Undeclared acquisition is denied at the boundary |
| P0 | Activation/rollback lifecycle is not a workspace-level transaction | Activation journal and reverse-order disposal | Partial activation cannot leak resources or capabilities |
| P1 | No stable composition revision pinning | Registry snapshot/revision and manifest digest | A running session cannot silently drift to a different provider |
| P1 | Insufficient composition observability | Plan explain, activation diagnostics and policy decision audit | Every included/excluded capability has a reason |
| P1 | No full golden-path test | Disposable fixture + deterministic fake providers + policy fault injection | E2E and negative matrix pass in CI |
| P2 | UI does not yet prove the whole lifecycle | Workspace composition inspector | User can inspect plan, grants, active providers and teardown state |

### Suggested implementation slices

Keep each slice small enough for review (target ≤3 engineering days):

1. **Composition domain types** — IDs, descriptors, requirements, manifest, resolved plan and revision digest.
2. **Resolver** — deterministic dependency/version/conflict resolution over a stable registry snapshot.
3. **Authorization bridge** — workspace grants and Ferris service-acquisition authorization.
4. **Activation coordinator** — staged activation, rollback, idempotent teardown and revision pinning.
5. **Agent integration** — build model-facing tools exclusively from the workspace capability view.
6. **Golden-path tests** — Research Workspace happy path and all P0 negative tests.
7. **Inspector UI** — explain selected/active/denied capabilities and why.

Do not begin with a visual composer. First make the domain model, resolver and authorization boundaries executable and testable.

## 8. Evidence required before calling this shipped

A release note or README may say “workspace composition is implemented” only when:
- the end-to-end acceptance flow runs in CI;
- the P0 negative tests pass;
- the Agent tool list is derived from the authorized view;
- Ferris acquisition checks enforce manifest/grant boundaries;
- activation rollback and teardown are tested;
- the implementation status in `ARCHITECTURE.md` is updated with links to the code and tests.

Until then, use “workspace composition foundation” or “composition in development”, not “complete composable workspace runtime”.

## 9. Federation three-node acceptance

The single-node gate above deliberately excludes Federation. Federation has its
own acceptance gate with **three distinct Nodes**: a caller, a coordinator, and
a worker.

### Scenario

1. Caller A delegates an objective to coordinator B.
2. B claims the objective, selects worker C, and delegates the concrete
   capability to C.
3. C executes the capability and returns the structured result.
4. B settles A's parent objective with C's result.
5. A reads the terminal parent record and sees the worker's result.

### Required evidence

- Three real Zenoh TCP sessions, not one shared in-memory transport.
- Trust and grants are explicit in both directions at each hop.
- The parent objective is claimed before settlement.
- The worker record and the parent record both reach `Succeeded`.
- The result crosses two hops without being rewritten.

Automated test: `crates/federation/tests/three_node_acceptance.rs`.

This gate does not claim transitive sub-delegation to a different target; the
current sub-delegation contract attenuates to the same target. A coordinator
uses a fresh root delegation to a worker, which is the supported pattern.
