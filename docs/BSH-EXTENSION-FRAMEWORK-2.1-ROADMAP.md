# BSH Extension Framework 2.1 Roadmap

> Status: proposed execution baseline  
> Baseline: `main` at `15b0ae9b2ee9106f9c80e044ce9b283754e373d4` (2026-11-02)  
> Scope: extension security, runtime lifecycle, composable runtime integration, compatibility, and developer experience.

## 1. Executive assessment

BSH already has a broad extension platform: strict manifests and dependency validation, package staging and rollback, publisher signatures and trust, multiple runtime adapters, a wide contribution model, and lifecycle leases. The current implementation is a strong foundation, but it should not yet be described as a fully isolated, uniformly composable, cross-platform production extension platform.

The engineering assessment at this baseline is approximately **6.5/10**. This is a prioritisation estimate, not a formal quality metric.

| Area | Assessment | Main reason |
|---|---:|---|
| Package, manifest, dependency management | 8.0 | Broad validation, staging, lockfile and rollback support |
| Runtime abstraction and adapters | 7.5 | Seven runtime kinds, but inconsistent process and lifecycle guarantees |
| Contribution surface | 8.0 | Tools, commands, panels, prompts, workflows and other contribution types |
| Capability enforcement | 7.0 | Host-mediated calls are gated; process-level authority is not consistently constrained |
| Lifecycle | 6.5 | Leases and several runtime fixes exist; terminal shutdown is not uniform |
| Composable Runtime 2.0 | 5.5 | Core primitives exist but are not yet the single production activation/reconciliation path |
| OS isolation and resource controls | 4.5 | Current process restrictions do not provide a complete cross-platform security boundary |
| SDK and developer experience | 6.5 | CLI and runtime-specific protocols exist; no single stable runtime-neutral SDK |
| Automated conformance and governance | 5.5 | CI gaps and inconsistent claims across standards/RFC documents |

## 2. Baseline capabilities to preserve

- Strict manifest parsing, extension identity and semantic-version validation, dependency conflict and cycle detection.
- Package staging, install rollback, quarantine, lockfiles and reproducibility support.
- Ed25519 package signatures and publisher trust store.
- Local, archive, Git, HTTP registry and market package sources.
- Seven runtime kinds: Builtin, Declarative, Process, MCP, WASM Component, Extism and Cordis.
- Broad contribution types, including tools, commands, panels, prompts, contexts, skills, workflows, providers, themes, languages, integrations and events/hooks.
- Contribution leases and Fiber-scoped lifecycle cleanup introduced in the recent runtime work.
- MCP lifecycle and concurrency fixes from the recent PR series. These should remain covered by regression tests.

## 3. Priority model

- **P0 — release integrity:** known CI failures, unsafe fail-open behavior, or a correctness defect that invalidates a documented contract.
- **P1 — security/runtime correctness:** isolation, process containment, lifecycle, authorization and composition semantics.
- **P2 — ecosystem maturity:** stable SDK, conformance tooling, documentation and compatibility improvements.

Every implementation story must be independently reviewable and sized at no more than three engineering days. Larger initiatives are epics and must be split into smaller stories.

## 4. Execution backlog

### Phase 0 — Restore a trustworthy baseline (P0)

| ID | Story | Acceptance criteria | Estimate |
|---|---|---|---:|
| BASE-01 | Restore formatting and Core Workspace CI | `cargo fmt --all -- --check` passes on the current base; no unrelated formatting churn | 1 day |
| BASE-02 | Make Cordis Windows E2E deterministic | CI either provisions/pins the required Node interpreter or explicitly gates the test with a documented platform prerequisite; no unexplained environment-dependent failure | 2 days |
| BASE-03 | Reconcile extension status documentation | Gold Standard, Conformance and RFC status agree with actual code; unsupported/partial features are labelled honestly | 2 days |
| BASE-04 | Establish a repeatable extension conformance matrix | Each runtime and contribution contract maps to an executable test or a clearly marked manual check | 2 days |

### Phase 1 — Security hardening (P1)

| ID | Story | Acceptance criteria | Estimate |
|---|---|---|---:|
| SEC-01 | Make resource-limit setup fail closed | Required `setrlimit` and process-group setup errors are surfaced; tests inject failures and prove the child is not launched as if limits were applied | 2 days |
| SEC-02 | Terminate and reap process trees | Unix process-group termination and platform equivalents are used; descendants cannot keep a runtime alive indefinitely through inherited pipes; bounded reap behavior is tested | 3 days |
| SEC-03 | Route Cordis through the common process launcher | Cordis no longer owns a weaker ad-hoc spawn path; sandbox and resource policy are explicit in launch options; unsupported isolation is reported rather than implied | 3 days |
| SEC-04 | Correct Linux filesystem and network boundaries | Bubblewrap does not expose the host root as a broadly readable tree; required package/runtime mounts are explicit; network access is denied by default unless granted | 3 days |
| SEC-05 | Add Windows process containment | Job Object enforces process-tree lifetime and resource ceilings; AppContainer or a documented stronger isolation profile is used where supported; unsupported guarantees are explicit | 3 days |
| SEC-06 | Align capability policy with process authority | Documentation distinguishes host API capability checks from arbitrary process filesystem/network authority; process-level network and filesystem controls are enforced or explicitly denied | 3 days |

**Security design constraints**

1. A sandbox profile must be described by the guarantees it actually enforces, not by its name.
2. Read access is part of the threat model. A read-only bind of the host root is not a least-privilege filesystem boundary.
3. Host-function network grants do not constrain sockets opened directly by a child process.
4. RLIMIT_AS is not an RSS ceiling. Memory enforcement must be platform-aware and tested against real child processes.
5. If the requested isolation mechanism is unavailable, the secure default is refusal or an explicit degraded mode requiring a deliberate policy decision—not silent fallback.

### Phase 2 — Runtime lifecycle and dispatch correctness (P1)

| ID | Story | Acceptance criteria | Estimate |
|---|---|---|---:|
| RT-01 | Define one terminal runtime shutdown contract | Active, Suspended, Stopping and Stopped states are explicit; terminal shutdown cannot be undone by lazy start; deactivate/reactivate remains distinct from terminal stop | 2 days |
| RT-02 | Enforce EventDispatcher mode isolation | Each dispatch API invokes only listeners registered for its mode; cross-mode regression tests cover all five modes | 2 days |
| RT-03 | Bind interceptor authorization to the requesting owner | The owner passed at registration is the principal authorized and audited; activation-bound owner mismatch is denied; tests cover global and activation-scoped policies | 1 day |
| RT-04 | Enforce `max_concurrent_calls` | Manifest limits are applied at the actual invocation boundary; concurrency tests prove the configured ceiling and permit release on success, error, cancellation and timeout | 2 days |
| RT-05 | Bound MCP timeout and worker cleanup | Timeout, child termination, pipe closure and worker completion have bounded behavior; descendant-held pipes are covered by a deterministic regression test | 3 days |

### Phase 3 — Make Composable Runtime the production composition path (P1)

| ID | Story | Acceptance criteria | Estimate |
|---|---|---|---:|
| CMP-01 | Integrate production Host activation with CompositionRuntime | Install, activate, deactivate and unload use one reconciliation path; rollback leaves no partially registered contributions | 3 days |
| CMP-02 | Implement cross-extension typed service binding | Provider/consumer declarations resolve across permitted scopes; capability authorization occurs before acquisition; provider removal suspends or invalidates dependents deterministically | 3 days |
| CMP-03 | Make profile mounting transactional | Group, disabled, dependency and missing-package semantics are explicit; failed mount rolls back config and runtime contributions as one operation | 3 days |
| CMP-04 | Define in-flight contribution drain/cancel semantics | Revocation blocks new calls and has a documented policy for admitted calls: drain, deadline-bound cancellation, or forced termination | 3 days |
| CMP-05 | Add reconciliation/property tests | Dependency changes, provider replacement, cycles, failed activation and rollback are exercised against the production adapter | 3 days |

### Phase 4 — Runtime-neutral SDK and ecosystem (P2)

| ID | Story | Acceptance criteria | Estimate |
|---|---|---|---:|
| SDK-01 | Specify a runtime-neutral extension SDK contract | Metadata, capability requests, logging, diagnostics, deadlines, cancellation, lifecycle, tools, commands, events and version negotiation are specified | 3 days |
| SDK-02 | Publish a first SDK binding and examples | One supported external runtime can build, validate, test and run a minimal extension using the documented contract | 3 days |
| SDK-03 | Add compatibility and migration checks | Protocol and manifest compatibility are tested across supported versions; breaking changes have migration guidance | 2 days |
| SDK-04 | Clarify Cordis compatibility level | Supported, partial and refused Cordis surfaces are versioned and tested; no claim of full DSH compatibility while hooks, service injection or HMR remain unsupported | 2 days |

## 5. Specific implementation findings to verify while fixing

### 5.1 Process isolation

The current Linux Bubblewrap profile uses a read-only bind of the host root and shares the network namespace. Read-only does not mean confidential: an extension may still read files exposed by that mount. The macOS profile primarily denies file writes and does not establish a complete read/network boundary. Windows lacks the equivalent Job Object/AppContainer containment path.

These are security-boundary gaps, not cosmetic sandbox differences. The implementation must document the effective profile on each OS and test the actual child process behavior.

### 5.2 Cordis bridge

The Node bridge currently documents that it does not construct the OS sandbox itself. It must use the same trusted process-launch abstraction as Process/MCP, or clearly refuse to run under a policy that requires isolation. Package-root entry validation is useful but is not a substitute for process sandboxing.

### 5.3 Resource-limit error handling

The process pre-exec resource hook currently ignores errors from resource-limit and process-group setup. This creates a fail-open condition: launch can proceed even though the requested restrictions were not applied. Required setup must return an error and prevent launch.

### 5.4 Event dispatch

The dispatcher stores a mode per listener, but dispatch snapshots must filter by that mode. The five modes need independent, testable semantics. In particular, `parallel` must not be advertised as concurrent if the implementation is synchronous; `serial` must not silently share `bail` behavior; `waterfall` must include only middleware listeners.

### 5.5 Interceptor identity

The host-level policy interceptor currently accepts an owner argument but authorizes using its configured owner field. The global host registration path configures that field as an empty string. Authorization and audit attribution must use the actual requesting owner, while activation-scoped policies must continue to reject owner mismatch.

### 5.6 Composition integration

Composition primitives exist, but production activation still uses host-specific contribution installation and leases. Service declarations do not yet provide a complete cross-extension typed binding mechanism. Profile mounting should become a single transaction across configuration and runtime state.

### 5.7 Runtime concurrency

The manifest exposes `max_concurrent_calls`; verify that the invocation path enforces it rather than merely parsing it. The same invocation boundary should own cancellation, deadlines and permit release to avoid inconsistent enforcement across runtimes.

## 6. CI and release gates

A change is not complete until the relevant gates have evidence:

- Formatting: `cargo fmt --all -- --check`.
- Rust static analysis and workspace build.
- Unit and integration tests for the touched runtime.
- Cross-platform extension tests on macOS, Linux and Windows where the feature is supported.
- Security regression tests that exercise real child processes, not only mocked policy objects.
- Documentation status updated in the same PR as the behavior change.
- No claim of sandbox support on a platform unless the corresponding isolation test passes.

The baseline review observed a Core Workspace formatting failure and a Windows Cordis E2E failure caused by the missing Node interpreter. These must be rechecked against the current branch before treating the baseline as green.

## 7. Delivery strategy

Use separate, reviewable PRs:

1. **Documentation and baseline:** this roadmap plus status reconciliation.
2. **Dispatch and authorization correctness:** RT-02 and RT-03, with focused unit tests.
3. **Process launch hardening:** SEC-01 and SEC-02.
4. **Cordis sandbox integration:** SEC-03, after the shared launcher contract is stable.
5. **OS-specific isolation:** SEC-04 and SEC-05, each with platform-specific tests.
6. **Runtime lifecycle and concurrency:** RT-01, RT-04 and RT-05.
7. **Composition integration:** CMP stories in dependency order.
8. **SDK and compatibility:** Phase 4 after runtime contracts stabilize.

Do not combine broad documentation edits, security-boundary changes and architectural refactors in one PR. Do not merge until CI is green or an explicitly accepted exception is recorded.

## 8. Definition of done

A story is done only when:

- The implementation matches the documented contract.
- Relevant positive, negative, failure-injection and lifecycle tests exist.
- Errors fail safely; no silent downgrade of security guarantees occurs.
- Resource ownership and cleanup are deterministic.
- Cross-platform limitations are explicit.
- CI evidence is attached to the PR.
- The roadmap and conformance status are updated if the supported capability surface changed.

## 9. Immediate next actions

1. Land this roadmap as the planning baseline.
2. Implement RT-02 and RT-03 in a focused code PR.
3. Fix SEC-01 and SEC-02 before expanding Cordis or other process-backed runtimes.
4. Re-run the full CI baseline and resolve the existing formatting and Windows interpreter failures.
5. Keep each story within the three-day size limit and report progress by story ID.
