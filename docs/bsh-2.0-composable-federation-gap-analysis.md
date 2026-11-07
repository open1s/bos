# BSH 2.0 — Composable & Federation Source Gap Analysis

- **Status:** Source-level review
- **Review date:** 2026-11-06
- **Audited revision:** [`6db3ae86751646d62b082a6d7c4ec58732b38f83`](https://github.com/open1s/bos/commit/6db3ae86751646d62b082a6d7c4ec58732b38f83)
- **Repository:** [open1s/bos](https://github.com/open1s/bos)
- **Architecture baseline:** [BSH 2.0 Architecture RFC](bsh-2.0-architecture-rfc.md)
- **Related implementation research:** [Provider Adapter Research](bsh-2.0-adapter-research.md)
- **Acceptance baseline:** [Single-Node Workspace Acceptance](bsh-2.0-e2e-acceptance.md)

> This document records what the audited source proves, what it does not prove, and the work required to reach the Composable and Federation goals. A type or module existing is not evidence that the complete product behavior is implemented. No runtime implementation is changed by this review.

---

## 1. Executive assessment

BOS already has meaningful foundations for both goals:

- A standalone `capability` crate with provider-qualified IDs, descriptors, version requirements, provider contracts, local/remote bindings, composition resolution, proposal validation, invocation context, and a workspace dispatch container.
- A standalone `federation` crate with Ed25519 identity, signed envelopes and discovery records, peer trust/grants, remote providers, dual authorization, delegation state transitions, resource leases/streams, audit seams, and Zenoh transport.
- BSH adapters that compose local tools and remote capabilities and expose remote tools to the Agent.
- Ferris remains a separate extension runtime and can be adapted as one provider implementation rather than defining the Capability domain.

**Assessment: foundation is real; the complete composable runtime and durable federation control plane are not yet proven.**

The most important distinction is between *resolving a list of capabilities* and *operating a Workspace as a governed runtime*. Current code is materially stronger at the former than the latter.

### 1.1 Goal status

| Goal | Current status | Assessment |
|---|---|---|
| Independent Capability model | Implemented foundation | IDs, descriptors, providers and errors are separated from Agent/Ferris |
| Hybrid composition | Partial | Runtime `CompositionSpec` resolves local/remote requirements; persistent static definitions and a complete AI assembly pipeline are not evidenced |
| Independent Composition Engine | Implemented foundation | Domain crate is independent of UI/Agent; host orchestration remains in BSH |
| Hybrid provider execution | Implemented foundation | Built-in, Ferris, MCP, Process/WASM and Remote provider kinds are represented; not every provider kind is proven end-to-end |
| Workspace-scoped dispatch | Implemented foundation | Resolved capability set is bound to providers and invoked through a WorkspaceRuntime |
| Full Workspace lifecycle and isolation | Gap | Current WorkspaceRuntime is a dispatch map, not a transactional lifecycle/resource/security manager |
| Federation identity and trust | Implemented foundation | Node identity, explicit peer enrollment, grants and revocation exist |
| Remote capability invocation | Implemented foundation | Signed transport messages, Remote provider and dual authorization exist |
| Durable task federation | Implemented for records | Delegations and leases persist, recover explicitly, and have restart tests; task-graph orchestration remains GAP-FED-005 |
| Agent/task collaboration | Partial | Bounded delegation primitive exists; durable orchestration, scheduling and collaboration graphs are not evidenced |
| Resource federation | Implemented foundation | Lease and chunk-stream primitives exist; production recovery/retention semantics remain incomplete |
| Knowledge federation | Implemented (opt-in) | Signed query/publication kinds, provenance, minimization, revocation, and wire tests; BSH UI remains owed |

---

## 2. What is already reusable

### 2.1 Capability domain

Evidence:
- `crates/capability/src/id.rs`
- `crates/capability/src/descriptor.rs`
- `crates/capability/src/catalogue.rs`
- `crates/capability/src/provider.rs`
- `crates/capability/src/context.rs`
- `crates/capability/src/error.rs`

The current model already captures:
- Stable capability/provider/workspace/node identities.
- Semantic capability version and version provenance.
- Kind, visibility, risk, side effects, permissions and invocation limits.
- Provider-neutral async describe/invoke contracts.
- Principal, workspace, task, deadline, cancellation, trace and delegation context.
- Distinct denial, provider, transport, timeout, cancellation and unknown-outcome errors.

**Decision:** Keep this crate as the canonical domain layer. Do not duplicate Capability IDs, descriptors, invocation context or error types in Ferris or BSH.

### 2.2 Composition resolution

Evidence:
- `crates/capability/src/composition.rs`
- `crates/capability/src/proposal.rs`
- `crates/capability/tests/composition.rs`
- `crates/capability/tests/proposal.rs`

The resolver already supports:
- Local-first resolution.
- Explicit remote-allowed and remote-required policies.
- Peer allowlists and grant references.
- Semantic version matching.
- Required and optional requirements.
- A single Workspace identity even when some bindings are remote.
- A separate approval gate for model-authored proposals.

The local/remote boundary is deliberately explicit: a remote binding is not rewritten as a local provider, and a forbidden local capability is not silently routed to a peer.

**Decision:** Extend this resolver; do not replace it with a second BSH-only resolver.

### 2.3 Provider and Agent integration

Evidence:
- `crates/agent/src/capabilities/`
- `crates/bsh/src/caps.rs`
- `crates/bsh/src/compositions.rs`
- `crates/bsh/src/extension/agent.rs`
- `crates/bsh/src/extension/framework.rs`
- `crates/agent/src/tools/registry.rs`

The implementation has real adapters and a workspace-facing composition path. Ferris extension tools have a stronger lifecycle/call-admission adapter than a plain name-only tool registration. The Agent registry remains a dispatch surface; it must not become the authority or the global catalogue.

**Decision:** Preserve the adapter/lowering approach. Complete the Workspace view so every model-facing tool schema and dispatch handle comes from the same validated set.

### 2.4 Federation foundation

Evidence:
- `crates/federation/src/identity.rs`
- `crates/federation/src/envelope.rs`
- `crates/federation/src/peers.rs`
- `crates/federation/src/discovery.rs`
- `crates/federation/src/provider.rs`
- `crates/federation/src/service.rs`
- `crates/federation/src/delegation.rs`
- `crates/federation/src/resource.rs`
- `crates/federation/src/zenoh.rs`
- `crates/federation/tests/`
- `crates/bsh/src/federation_node.rs`

The protocol already distinguishes authentication from authorization, checks the caller's local peer grant and the provider's local grant, preserves an unknown outcome when a sent request has no response, and records audit events on both sides.

There is also a real loopback Zenoh transport test in `crates/federation/tests/zenoh_transport.rs`. This is stronger evidence than an in-memory-only protocol test, but it is not a substitute for multi-process, restart, partition, and production security tests.

---

## 3. P0 — Gaps that block a trustworthy Composable Workspace

### GAP-COMP-001 — Composition is not yet a reproducible, persisted artifact

**Evidence**
- `CompositionSpec` and `ResolvedComposition` are runtime Rust structs in `crates/capability/src/composition.rs`.
- The resolved result contains ID, version, Workspace ID, resolved capabilities and skipped IDs.
- The current spec has requirements, peers and budgets, but no complete persisted composition manifest, lockfile, registry snapshot revision, dependency graph, policy revision, or deterministic fingerprint.
- The RFC describes snapshots, but a snapshot log is not a lockfile or a resolver input.

**Impact**
- A saved Workspace cannot yet be proven to resolve to the same provider versions and bindings after a registry or peer catalogue changes.
- A dynamic composition cannot be safely diffed, reviewed, promoted or rolled back as a versioned artifact.
- A remote provider's changing descriptor can alter the effective plan unless the exact resolved contract is pinned.

**Required change**
1. Define a versioned, serializable Composition document.
2. Add dependency declarations distinct from a flat list of requested capabilities.
3. Resolve against immutable local and peer catalogue revisions.
4. Produce a lock record with exact capability/provider versions, bindings, grant references, policy revision and source identity.
5. Compute a canonical plan fingerprint and verify it before activation.

**Acceptance**
- Same manifest + same registry snapshot + same policy inputs => identical fingerprint.
- Changed provider version, grant, policy or configuration => fingerprint changes.
- Missing pinned version fails explicitly; no silent upgrade/downgrade.

### GAP-COMP-002 — CompositionSpec budgets are not carried into the resolved runtime

**Evidence**
- `CompositionSpec` includes `Budgets` in `crates/capability/src/composition.rs`.
- `ResolvedComposition` does not include those budgets.
- `WorkspaceRuntime::instantiate` in `crates/capability/src/workspace.rs` consumes the resolved capability bindings and stores a map of dispatch entries.
- `WorkspaceRuntime::invoke_operation` dispatches to the provider; the WorkspaceRuntime itself does not enforce a global concurrent-invocation limit or a default timeout.

**Impact**
- The declared composition budget is not an enforceable runtime contract at this layer.
- A caller can believe a budget was applied when it was not propagated to the object doing dispatch.

**Required change**
- Carry an immutable effective budget into the resolved plan and Workspace runtime.
- Enforce concurrency admission, deadline derivation, payload/output ceilings and cancellation at the Workspace invocation boundary.
- Preserve provider-specific tighter limits; the effective limit is the minimum of Workspace, capability, provider and caller limits.

**Acceptance**
- Concurrency and deadline fault-injection tests prove the Workspace rejects or cancels excess work.
- The effective budget is visible in explain/snapshot output.
- No timeout is restarted at a remote hop.

### GAP-COMP-003 — WorkspaceRuntime is not a complete Workspace lifecycle manager

**Evidence**
- `crates/capability/src/workspace.rs` stores a Workspace ID and a map of capability → provider/binding.
- Its public behavior is instantiate, describe, binding, contains and invoke.
- It does not own a lifecycle state machine, provider activation/deactivation, admission closure, transactional rollback, update revision, suspend/resume, or resource/effect disposal.

**Impact**
- The abstraction currently proves scoped dispatch, not a complete isolated runtime.
- Partial provider startup, hot update, shutdown races and leaked registrations cannot be made transactional at this layer.
- The RFC's Workspace states and rollback guarantees remain target behavior, not demonstrated behavior.

**Required change**
- Keep `WorkspaceRuntime` as the execution/view object or rename it to make its current scope explicit.
- Introduce a Workspace Manager/Activator that owns state transitions and provider lifecycle leases.
- Activation must stage providers, validate all bindings, publish the Workspace view atomically, and roll back in reverse order on failure.
- Shutdown must close admission first, cancel/drain calls, revoke views, dispose providers and release owned resources.
- Updates must be revisioned; old and new revisions must not share mutable authority accidentally.

**Acceptance**
- Inject failure at every activation step; no partial Workspace remains visible.
- Repeated activate/deactivate is idempotent.
- Shutdown racing with invocation has deterministic admission and drain behavior.
- Resource and contribution counts return to baseline after teardown.

### GAP-COMP-004 — Workspace ownership is not checked at the dispatch boundary

**Evidence**
- `InvocationContext` carries a `workspace` identity in `crates/capability/src/context.rs`.
- `WorkspaceRuntime::invoke_operation` receives that context but does not compare `context.workspace` with its own `self.workspace` before provider dispatch.

**Impact**
- The WorkspaceRuntime contract does not independently reject a context created for another Workspace.
- A provider may enforce its own scope, but the central dispatch boundary should not rely on every provider adapter to duplicate this invariant.

**Required change**
- Reject mismatched Workspace IDs before dispatch.
- Add tests for cross-Workspace context reuse, remote binding included.
- Ensure the provider receives a context whose Workspace matches the owning runtime.

**Acceptance**
- A context from Workspace A cannot invoke through Workspace B even if both bind the same provider/capability.
- Denial occurs before provider side effects.

### GAP-COMP-005 — Dependency graph and activation order are not represented

**Evidence**
- `CompositionSpec.requirements` is a flat vector of `CapabilityRequirement`.
- The resolver validates duplicate IDs and version ranges but does not model explicit dependency edges, dependency source, optional feature closure, or activation order.
- `ResolvedComposition` preserves the resolved capabilities but has no dependency graph or ordered activation plan.

**Impact**
- Capability dependency resolution and deterministic startup/shutdown ordering are not represented by the current plan.
- Providers that require another service to be active first cannot rely on a compiler-produced order.

**Required change**
- Model dependency edges, constraints and optional features.
- Detect cycles and report the full dependency path.
- Compile a deterministic topological activation order and reverse disposal order.
- Distinguish dependency edges from mere co-presence in a Workspace.

**Acceptance**
- Tests cover cycles, missing transitive dependencies, conflicting versions and stable order independent of catalogue insertion order.

### GAP-COMP-006 — AI Assembly is a validation gate, not yet an assembly engine

**Evidence**
- `crates/capability/src/proposal.rs` validates a proposed spec against an allowlist, risk ceiling and remote policy.
- It does not itself analyze a natural-language task, search/rank a catalogue, calculate dependency closure, estimate budgets, or produce an approval-ready proposal.
- BSH currently has a host composition path, but the complete Task Understanding → Discovery → Matching → Proposal → Approval → Compile → Activate pipeline is not evidenced.

**Impact**
- The deterministic safety gate exists, but the AI-native product experience remains incomplete.
- Avoid describing proposal validation as automatic AI composition.

**Required change**
- Add a host-neutral Assembly API that emits a proposal only.
- Keep LLM discovery metadata separate from invocation schemas.
- Let the deterministic resolver and policy kernel decide the final plan.
- Require explicit approval for policy-defined sensitive capabilities; AI must never mint grants.

**Acceptance**
- A model proposal cannot exceed a host-approved capability set, risk ceiling, remote policy or budget.
- Rejected proposals produce explainable reasons and never activate providers.
- Model output is treated as untrusted input.

---

## 4. P0/P1 — Federation gaps

### GAP-FED-001 — Durable delegation and lease state

**Evidence**
- `FederationNode` initializes `DelegationStore` and `LeaseStore` behind in-memory locks in `crates/federation/src/service.rs`.
- `DelegationStore` is a `BTreeMap` in `crates/federation/src/delegation.rs`.
- BSH persists Node identity and peer grants, but the audited FederationNode constructor does not restore delegation/lease state from a durable store.
- RFC implementation status itself notes that a restart loses in-memory delegations.

**Impact**
- A process restart loses authoritative task state and active lease bookkeeping.
- A caller cannot reliably query or reconcile long-running delegated work after target restart.
- A lost lease ledger complicates resource stream cleanup and revocation.

**Required change**
1. Define storage traits for delegation and lease records.
2. Implement an embedded durable store with atomic transitions and schema versioning.
3. Persist idempotency/deduplication records with task state.
4. On restart, recover active tasks as recoverable/unknown and reconcile with providers; do not blindly replay side effects.
5. Persist lease expiry and ensure expired leases cannot resume.

**Status: implemented for records and leases, with explicit recovery.** `DelegationStore`
and `LeaseStore` are serializable; `FederationNode::with_delegation_persistence` and
`with_lease_persistence` load them at start, and mutations are written atomically
(0600, one writer) before success is reported. Delegation recovery marks
Created/Accepted/Running records as Failed with an indeterminate-outcome reason
rather than resetting them; terminal records survive. Lease recovery drops expired
records, revocation rewrites the file, and grant/revoke/charge fail closed on a
write error. Restart, expiry, revocation and write-failure tests pin the behavior.
The replay/idempotency cache is also persisted when a path is configured
(`federation/replay.json`): unexpired entries load at startup and request
admission writes the cache; a write failure is logged and the request is still
processed, so replay protection degrades visibly instead of silently dropping
authenticated work. Still owed: a schema version for the replay file and lease
renewal, which the current stream model does not need because a restart ends its
streams.

**Acceptance**
- Restart tests recover terminal state and classify interrupted in-flight work.
- Duplicate message/retry tests do not duplicate non-idempotent side effects.
- Lease expiry/revocation remains effective after restart.

### GAP-FED-002 — Federation transport ingress uses an unbounded in-memory queue

**Evidence**
- `crates/federation/src/zenoh.rs` creates the inbox with `mpsc::unbounded_channel()` and forwards every received frame into it.
- No backpressure or queue-size admission is enforced at this transport boundary.

**Impact**
- A peer or reachable publisher can create unbounded memory pressure by sending frames faster than the node processes them.
- Application-level payload limits do not bound the number of queued frames.

**Required change**
- Replace the unbounded queue with a bounded channel.
- Define queue-full behavior and metrics.

**Status: fixed.** The inbox is a 1024-frame bounded channel and the announce
channel holds 256. The Zenoh subscriber forwards with `try_send`; a full queue
drops the frame and logs the first drop and every 1024th after it, so a slow
router can no longer grow the process without bound. The in-memory transport is
bounded too (1024) and blocks its sender when full, which a test pins. Resource
streams keep the existing credit bound rather than an arbitrary queue cap:
the receiver grants credit, and the producer sends within it.
- Apply frame-size limits before expensive decode/verification.
- Ensure the subscriber task exits cleanly when the consumer is dropped and that overload does not silently discard security-relevant messages.

**Acceptance**
- Flood test proves memory/queue use is bounded.
- Queue saturation is observable and produces a defined transport outcome.
- Shutdown drains or terminates the ingress task deterministically.

### GAP-FED-003 — Production transport security and endpoint lifecycle need proof

**Evidence**
- BSH builds the Zenoh `BusConfig` from mode/connect/listen endpoints in `crates/bsh/src/federation_node.rs`.
- The current settings bridge does not demonstrate a complete production configuration surface for peer authentication, key rotation, endpoint policy, certificate/TLS lifecycle or protocol compatibility negotiation.
- The RFC status table also records that endpoint TLS/routing configuration is not exposed.

**Impact**
- Signed envelopes provide message identity/integrity, but do not alone provide transport confidentiality, availability controls or a hardened deployment configuration.
- A loopback Zenoh test does not establish internet/LAN production hardening.

**Required change**
- Define supported secure transport profiles and their configuration.
- Document the difference between envelope signing, transport encryption and Zenoh access control.
- Add protocol compatibility negotiation and endpoint rotation/revocation behavior.
- Add deployment tests for authenticated peers, untrusted publishers, network partitions and reconnects.

**Status: mTLS verified across processes.** Federation settings expose TLS
CA/certificate/key, client certificate requirement, endpoint and listen
configuration, and verify-name; the Zenoh adapter validates them and a TLS
listener without usable material fails rather than opening plaintext.
`tests/tls_transport.rs` generates a CA, server and client certificate, opens
two real Zenoh sessions over mutual TLS with hostname verification, and sends a
sealed frame. A second test spawns the test executable as a separate OS process,
runs the TLS listener there, and proves the same frame crosses the process
boundary. A third test presents a client certificate signed by another CA and
proves the listener accepts no frame from it. A fourth drops the TLS listener,
reopens it on the same port, and proves the existing connector reconnects and
delivers another frame. TLS deployments disable Zenoh scouting, because scouting
otherwise opens a parallel plaintext link that bypasses the configured TLS
endpoints. Still owed: replacing a certificate without dropping the session.

**Acceptance**
- Production profile refuses insecure configuration unless explicitly enabled for development.
- Key rotation and peer revocation take effect without relying on stale discovery.
- CI or release evidence covers the supported OS/network matrix.

**Key rotation is now operational.** `federation::RotationRecord` is signed by
the old Node key, names the successor, and is verified before `PeerStore::rotate`
moves the old grants to the new identity and revokes the old one. `bsh federation
rotate [reason]` generates the successor, writes `rotation.json`, and replaces the
key under the data-directory lock; `bsh federation accept-rotation <file>` applies
the signed record on a peer. Tests cover the library primitive, forged/unenrolled
records, the two-root CLI flow, and a real wire delivery that applies the record
on the peer. BSH also delivers `rotation.json` to every enrolled peer on the
next launch, idempotently, so an offline peer catches up later.

### GAP-FED-004 — Knowledge Federation contract and provenance

**Status: implemented as an opt-in protocol.** `federation::knowledge` defines
versioned query/publication/response contracts, a hash-bearing provenance record,
classification/purpose/scope authorization through a host `KnowledgeAuthorizer`,
retention/expiry, publisher-scoped revocation, bounded results and audit records.
The signed envelope kinds `knowledge_query` and `knowledge_publication` are routed
by `FederationNode`; a node without a service and authorizer refuses explicitly,
and a denied authorizer fails closed. The wire tests cover query, publication,
revocation, authorization and stale-query refusal. Results are always marked
`KnowledgeOrigin::Remote`; they are never merged into local memory.

**Still owed**
- A BSH UI/CLI surface for knowledge queries and publications.
- A concrete per-source allowlist beyond the host authorization callback.
- Bulk-source replication remains deliberately unsupported: queries return
  bounded items, not a replicated index.

### GAP-FED-005 — Federation delegation is not yet a durable collaboration orchestrator

**Evidence**
- Delegation models a bounded request, parent relationship, capability subset, target Node and deadline.
- The store enforces state transitions and attenuation.
- The audited model does not include a durable task DAG, dependency scheduling, artifact exchange contract, result acceptance workflow, or restart reconciliation.

**Impact**
- Current delegation is a useful primitive, but it is not yet a general multi-Agent collaboration runtime.
- Parallel/sequential execution, independent verification and long-running recovery remain higher-level orchestration work.

**Status: implemented foundation.** `federation::collaboration` adds a
versioned, durable `TaskGraph` with explicit dependencies, cycle and unknown-edge
validation, deterministic topological/ready order, bounded parallel execution
through `TaskExecutor`, failure containment (only dependents are cancelled), and
restart recovery that marks an interrupted Running task Failed with an
indeterminate reason. The graph persists atomically (0600) and every runner
transition is written. Tests cover dependency-first order, concurrency bounds,
partial failure, persistence/recovery, cycles, and deterministic ready order.
Tasks carry max attempts, attempts, an acceptance criterion, artifact
references, and a compensation instruction. A retryable failure returns the task
to Pending until the ceiling; a final failure runs the executor's compensation
hook and records whether it succeeded. The delegation-backed DelegationExecutor maps a task to a Federation delegation,
polls its authoritative status, and composes with the retry/compensation hooks.
Still owed: richer artifact retention policy and automatic distribution of
rotation records.

**Required change**
- Keep delegation as the primitive.
- Add a separate task orchestration layer above it; do not overload the federation transport or CapabilityProvider.
- Model task graph, artifact references, deadlines, retries, compensation and acceptance criteria.
- Keep each Node authoritative for its own execution and state.

**Acceptance**
- Multi-node task graph tests cover partial success, peer loss, cancellation races and recovery.
- Every child task retains immutable provenance and attenuated grants.

---

## 5. Cross-cutting architectural risks

| Risk | Why it matters | Required invariant |
|---|---|---|
| Multiple registries become authorities | Agent registry, Ferris registry, Capability Catalogue and peer discovery have different jobs | One canonical metadata/plan authority; registries are adapters or dispatch indexes |
| Discovery is confused with authorization | Public descriptors and semantic matches are untrusted claims | Discovery never grants; policy is checked at activation and invocation |
| Remote binding is treated as local | Confused-deputy and fallback risk | Remote binding preserves Node + grant and always crosses remote authorization |
| Workspace becomes a distributed transaction | Merges security and lifecycle boundaries | One Workspace belongs to one Node; remote calls remain remote |
| LLM context includes the entire catalogue | Prompt overhead and capability confusion | Discovery → inspection → active Workspace schema are separate context tiers |
| Runtime-kind is confused with capability-kind | Locks logical contract to Ferris/WASM/MCP | Capability identity is stable across provider implementations |
| In-memory state is mistaken for durable state | Restart loses tasks, leases and deduplication | Explicit durability classification for every state owner |
| Federation membership is treated as trust | Untrusted peers gain unintended access | Explicit enrollment, scoped grants, revocation and dual authorization |

---

## 6. Recommended implementation sequence

Keep changes reviewable and independently testable. Target no more than three engineering days per story.

| Priority | Slice | Deliverable | Exit condition |
|---|---|---|---|
| P0 | Workspace boundary hardening | Context/workspace identity check + tests | Cross-workspace invocation is rejected before provider call |
| P0 | Budget propagation | Carry effective budgets into plan/runtime; enforce admission/deadline | Concurrency and deadline tests pass |
| P0 | Workspace lifecycle coordinator | State machine, activation journal, leases, rollback and teardown | Partial activation and shutdown races are tested |
| P0 | Composition lock model | Serializable manifest, registry revision, lockfile, fingerprint | Reproducible resolution is demonstrated |
| P0 | Dependency graph | Dependency edges, cycle detection, stable topological order | Deterministic plan and reverse teardown tests pass |
| P1 | Bounded federation ingress | Bounded channel, frame limits, overload policy | Flood and shutdown tests pass |
| P1 | Durable delegation/lease store | Storage traits + embedded implementation + recovery | Restart and deduplication tests pass |
| P1 | Production transport profile | Security configuration, compatibility and rotation | Secure multi-process integration tests pass |
| P1 | AI Assembly orchestration | Task analysis, discovery, proposal and approval APIs | AI proposal remains non-authoritative |
| P2 | Knowledge Federation | Typed query/provenance/data-minimization contract | Source-side authorization and provenance tests pass |
| P2 | Collaboration orchestration | Task graph, scheduling, artifact refs, acceptance | Multi-node fault/recovery suite passes |

### Suggested first user stories

1. **Workspace invocation ownership** — reject mismatched Workspace context; test same capability/provider reused by two Workspaces.
2. **Budget-preserving resolution** — include budgets in the resolved plan; test concurrency and deadline enforcement.
3. **Workspace activation transaction** — add explicit states and rollback journal around provider activation.
4. **Bounded Zenoh inbox** — bounded channel and overload behavior; add flood test.
5. **Delegation storage contract** — persistence abstraction and restart-recovery semantics before selecting the storage engine.

Do not start with a marketplace, visual composer, global scheduler or knowledge replication. Those features depend on the invariants above.

---

## 7. Verification required before status promotion

A feature may be called *implemented* only when source, tests and integration behavior agree.

- **Model exists:** types and invariants are tested.
- **Runtime exists:** state transitions, cancellation, budgets and resource ownership are tested.
- **Federation exists:** real multi-process transport, identity, dual authorization, restart and fault behavior are tested.
- **Product exists:** BSH UI/CLI/API consumes the same validated plan and exposes explainability.

The current repository has passing unit/integration coverage for many capability and federation primitives, and a real Zenoh loopback test. This review did not execute the Rust test suite or CI; no new build/test pass is claimed here.

## 8. Final recommendation

Do not redesign the Capability or Federation foundations. They are already useful and should be consolidated, not replaced.

The next engineering move should be to close the Workspace correctness gaps first: ownership validation, budget propagation, transactional lifecycle, and reproducible composition plans. In parallel, bound federation ingress and design durable task/lease state before expanding collaboration.

Only after those boundaries are executable and fault-tested should BSH add the full AI Assembly pipeline and Knowledge Federation.

**Target invariant: one Capability Model; one local Workspace authority per Node; remote capabilities remain remote; each Node owns its own policy, state and execution; federation composes cooperation without merging trust.**
