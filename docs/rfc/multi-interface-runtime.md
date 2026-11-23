# RFC: Multi-Interface Runtime, Web and Mobile Access

- **Status:** Proposed
- **Scope:** BSH Runtime Host, Application Contract, local IPC, Web and Mobile clients
- **Decision class:** Architecture / Security / Protocol
- **Target delivery:** Personal Local-First BSH first; remote and hosted stages later
- **Baseline reviewed:** `open1s/bos` commit `4acc8126f89eb98c4160a8d1355c817d7eedc47e`

## Abstract

This RFC proposes one standalone BSH Runtime Host as the single execution authority, with GUI, TUI, CLI, Web and Mobile as independent clients. It defines transport-independent application semantics, durable event replay, identity separation, mobile device enrollment, offline behavior, push notification constraints, security boundaries and staged deployment.

## Motivation

The current desktop-oriented ownership model couples execution lifecycle and event delivery to the BSH application process. Adding TUI, browser and mobile clients by exposing internal channels or sharing storage would multiply execution authorities, weaken authorization and make recovery dependent on UI lifetime. The platform needs a stable Application Contract and Host-owned lifecycle before adding more interfaces.

## RFC-011 — Standalone Runtime Host

**Requirement:** The Host exclusively owns Agent execution, sessions, tasks, tool invocation, approvals, recovery and durable state.

- Lifecycle phases: Initializing, Recovering, Ready, Draining, Stopped, Failed.
- Activity is modeled separately (Idle, Active, Degraded).
- No business mutation is accepted before recovery reaches Ready.
- Single-instance ownership uses an OS lock plus storage-level protection; PID alone is not a liveness proof.
- Graceful shutdown stops accepting new work, applies configured in-flight policy, flushes durable state and closes transports.
- Immediate shutdown is explicit and recovery-aware.

**Acceptance:** launching two Host processes against one data root cannot create concurrent execution authorities; crash recovery completes before mutations are accepted.

## RFC-012 — Shared Application Contract

Define transport-neutral Command, Query, Control, Event and Projection contracts.

Every mutating command includes command_id, client_instance_id, principal context (derived by Host, never trusted from payload), target, optional expected_version and payload. The Host returns Accepted/Rejected with an operation reference; completion is separately queryable and evented.

- Same command_id + same canonical payload returns the prior outcome.
- Same command_id + different payload is rejected.
- Timeout is an unknown outcome; clients query/retry with the same idempotency key.
- Controls (approval, permission changes, shutdown) have stronger authorization and audit.
- Projection schemas are versioned and redact secrets by policy.

**Acceptance:** GUI, TUI, Web and Mobile invoke the same application operation and receive equivalent domain outcomes.

## RFC-013 — Durable Runtime Event Journal

The Journal is the source of replayable committed facts; the in-process Event Bus is not.

Event envelope fields: event_id, workspace_id, stream_id/session_id, sequence, execution_id, task_id, causation_id, correlation_id, occurred_at, schema_version, payload.

- Sequence is monotonic per explicitly declared stream.
- State mutation and journal append are atomic or use a transactional outbox.
- Publish only after durable commit.
- Delivery is at-least-once; consumers deduplicate.
- Token deltas may be coalesced; durable turn/message boundaries and recovery checkpoints are persisted.
- Snapshot records the exact sequence it represents.
- Replay/live handoff must not lose events between replay and subscription.
- Expired cursors return a typed CursorExpired response with snapshot instructions.

**Acceptance:** fault-injection around commit, publish, reconnect and snapshot proves no committed event is silently skipped.

## RFC-014 — Execution and Host Recovery

Separate state recovery from external side-effect recovery.

Execution states include Created, Queued, Running, WaitingApproval, Completed, Failed, Cancelled, Interrupted, Recoverable and NeedsIntervention.

Persist ToolExecutionRecord with tool_call_id, execution_id, status, timestamps, idempotency_key, side_effect_classification and result_reference. Missing completion record does not prove a tool did not execute.

Recovery classes: SafeRetry, IdempotentRetry, ReconcileRequired, ManualIntervention and NonRecoverable. Unknown external side effects must not be blindly retried.

**Acceptance:** crash at every boundary around tool invocation and result persistence never causes an unclassified automatic duplicate side effect.

## RFC-015 — Local IPC and Client Architecture

- macOS/Linux: Unix Domain Socket; Windows: Named Pipe.
- Host endpoint is user-scoped and protected by OS ACL/peer identity.
- Client registration is distinct from OS identity and Principal identity.
- Bound frame size, connection count, request rate and subscription buffers.
- GUI/TUI/CLI are clients, not Host containers.
- Endpoint discovery must validate Host identity and protocol version.

**Acceptance:** unauthorized local process cannot invoke privileged operations; client exit does not terminate Host execution.

## RFC-016 — Multi-Interface Concurrency

- Read queries may run concurrently under consistent snapshots.
- Mutations pass through Host command validation/serialization.
- Resource versions support optimistic concurrency.
- Approval is an atomic state transition with idempotency and action digest.
- Client presence is advisory, never execution ownership.
- Window close, client disconnect, task cancel, execution cancel and Host shutdown are distinct operations.

**Acceptance:** two clients racing to approve one action yield exactly one committed decision.

## RFC-017 — Multi-Transport Architecture

Application Contract must not depend on transport framing or connection lifetime. Define adapters for local IPC and HTTPS/SSE. A future Gateway may be split into a separate process without changing application semantics.

Transport adapter responsibilities: authentication context establishment, framing, protocol negotiation, size/rate limits, flow control, disconnect detection and error mapping. It must not implement domain policy or execute tools.

**Acceptance:** contract conformance tests run against IPC and HTTP adapters with the same domain test vectors.

## RFC-018 — Web API and Event Protocol

Web v1 uses HTTPS JSON commands/queries and SSE for server-to-client events. Mutations use POST with stable idempotency keys. Responses distinguish accepted, completed, rejected and unknown outcomes.

SSE reconnect uses a validated durable cursor. The Host replays from the Journal and performs a gap-free replay-to-live handoff. Last-Event-ID is not itself a durability guarantee. WebSocket is deferred; if added, it must preserve the same envelope, authorization, cursor and backpressure semantics.

**Acceptance:** browser refresh, tab suspension, proxy disconnect and network change recover a consistent projection.

## RFC-019 — Authentication and Authorization

Separate Principal, Device, ClientInstance, WorkspaceMembership and Session. Authentication proves an identity; authorization evaluates each requested action against current policy.

- TLS for remote connections.
- Short-lived access credentials and refresh rotation.
- Cookie-based auth requires CSRF defense; validate Origin/Host and protect loopback endpoints from hostile browser origins and DNS rebinding.
- Origin is not identity.
- Every command and protected resource read is authorized by Host.
- Remote principals never inherit local OS privileges.
- Device/principal revocation blocks refresh and new commands; active connection termination is policy-defined.
- Audit security-sensitive decisions without logging secrets.

**Acceptance:** negative authorization tests cover cross-workspace reads, session enumeration, tool capability escalation, approval forgery, replayed tokens and revoked devices.

## RFC-020 — Remote Client Lifecycle and Device Enrollment

Native mobile and remote browser clients are expected to disconnect, roam between networks and be suspended.

Device enrollment uses a short-lived, single-use pairing challenge and binds a device public key to an authenticated Principal. Support device inventory, revocation, rotation and lost-device recovery. Never treat push token or client ID as identity.

Client lifecycle: Created → Pairing/Authenticating → Connected → Reconnecting → Revoked/Expired. Connection state does not imply execution state.

**Acceptance:** pairing token replay, expired pairing, key rotation and device revocation are tested; reconnect restores authorized subscriptions.

## RFC-021 — Event Streaming, Flow Control and Push

- Per-client bounded queues and isolation for slow consumers.
- Durable events recover via cursor replay; ephemeral progress may be coalesced/dropped.
- Maximum event size, replay window, rate and subscription quotas are explicit.
- Push notifications are best-effort wake-up hints only.
- Push payloads contain minimal opaque references; sensitive content is fetched after authentication.
- Deep links are validated and authorization is rechecked on open.

**Acceptance:** lost/duplicated push has no correctness impact; slow mobile client does not block Host execution or other subscribers.

## RFC-022 — Workspace Isolation and Hosted Evolution

Stage 1 supports one personal principal and local Workspace while preserving distinct identity entities. Stage 2 adds authenticated remote access to that user's Host. Stage 3 may introduce multi-user hosted deployment with explicit WorkspaceMembership, tenant isolation, quotas, audit, service identities and operational controls.

Do not build multi-tenant infrastructure prematurely. Do not encode a schema assumption that ClientInstance equals Principal or that one user has only one device.

**Acceptance:** isolation tests prove a principal cannot access a workspace/session without membership; hosted mode requires a separate threat model and deployment review.

## Mobile client requirements

1. Native iOS/Android and PWA share the Application Contract; native clients are first-class, not privileged.
2. Store private keys/refresh credentials in platform secure storage (Keychain/Keystore).
3. Network loss triggers bounded exponential backoff with jitter.
4. Persist acknowledged cursor; replay and snapshot reconcile after reconnect.
5. Offline cached projections are visibly stale and read-only by default.
6. Offline command queue is opt-in and excludes approval, permission changes and destructive controls.
7. Never report local queueing as Host acceptance or completion.
8. OS suspension/termination cannot stop Host execution.
9. Push payload excludes prompts, transcripts, tool output and secrets.
10. Approval UI binds decision to exact action digest, target, scope and expiry; online Host decision is authoritative.
11. Deep links re-authenticate/re-authorize; dismissing a notification is not cancellation.
12. Device enrollment, inventory, revocation, rotation and lost-device recovery are mandatory before remote mobile access.

## Proposed ADR set

These identifiers are **proposals** and must be reconciled with the repository's ADR index before being treated as accepted decisions.

| ADR | Proposed decision | Status |
|---|---|---|
| ADR-004 | Multi-Interface Architecture | Proposed |
| ADR-005 | Standalone Runtime Host | Proposed |
| ADR-006 | Shared Application Services | Proposed |
| ADR-007 | Transport-independent Protocol | Proposed |
| ADR-008 | Durable Event Journal | Proposed |
| ADR-009 | Multi-Interface Concurrency | Proposed |
| ADR-010 | Local IPC | Proposed |
| ADR-011 | Web HTTP + SSE v1 | Proposed |
| ADR-012 | Mobile as First-Class Client | Proposed |
| ADR-013 | Principal / Device / Client Identity Separation | Proposed |
| ADR-014 | Push as Advisory Notification | Proposed |

## Gap baseline from static review

At commit `4acc8126f89eb98c4160a8d1355c817d7eedc47e`, the initial review recorded:
- **P0:** Runner/Agent execution lifecycle is coupled to BSH AppState/GUI process.
- **P0:** existing Session in-flight journal is not a general Runtime Event Journal with cursor/replay semantics.
- **P0:** bounded Event Bus uses best-effort publication and may drop events.
- **P1:** Agent SessionManager is an in-memory cache, not Host persistence.
- **P1:** Task recovery foundations do not prove safe recovery for every generic Tool Execution.
- **P1:** Approval data structures do not by themselves prove atomic cross-client Host decisions.
- **P1:** CLI is not yet a full Runtime Host management/client interface.

These findings must be revalidated against the target branch before implementation; this RFC does not claim a current code audit.

## Delivery plan and user stories

Each story is intentionally sized at no more than three working days.

| ID | Story | Estimate |
|---|---|---:|
| MI-016 | Inventory AppState/Runner ownership and lifecycle | 2d |
| MI-017 | Audit Store, Parquet history, in-flight journal durability | 2d |
| MI-018 | Define Session/Turn/Task/Tool Execution identity model | 2d |
| MI-019 | Trace Runtime Event and Event Bus lifecycle | 2d |
| MI-020 | Define Session Command/Query contract | 2d |
| MI-021 | Define Execution Command/Query contract | 2d |
| MI-022 | Specify atomic Approval decision protocol | 2d |
| MI-023 | Specify durable Event Envelope and Cursor | 2d |
| MI-024 | Specify Snapshot/Replay/Live subscription | 3d |
| MI-025 | Define Host lifecycle and single-instance ownership | 2d |
| MI-026 | Define transport-neutral Runtime Client SDK | 2d |
| MI-027 | Implement/specify UDS and Named Pipe IPC contract | 3d |
| MI-028 | Migrate GUI to Runtime Client | 3d |
| MI-029 | Define TUI client architecture and conformance | 2d |
| MI-030 | Define transport-independent protocol conformance suite | 2d |
| MI-031 | Define Principal/Device/Client/Workspace identity schema | 2d |
| MI-032 | Define HTTP API command/query contract | 2d |
| MI-033 | Define SSE cursor replay and recovery contract | 2d |
| MI-034 | Define Web authentication and authorization threat model | 3d |
| MI-035 | Specify Origin/CSRF/DNS-rebinding/rate-limit defenses | 2d |
| MI-036 | Define disconnect/reconnect and unknown command outcome | 2d |
| MI-037 | Define multi-tab/multi-client concurrency and approval races | 2d |
| MI-038 | Define Gateway extraction and deployment boundary | 2d |
| MI-039 | Define Web MVP end-to-end acceptance suite | 3d |
| MI-040 | Define mobile device enrollment, key binding and revocation | 3d |
| MI-041 | Define mobile offline cache, cursor and reconciliation | 3d |
| MI-042 | Define push payload, deep-link and notification security | 2d |
| MI-043 | Define mobile approval UX and action-digest binding | 2d |
| MI-044 | Define native iOS/Android lifecycle and platform integration | 3d |
| MI-045 | Define PWA/native client capability and parity matrix | 2d |
| MI-046 | Build mobile network-loss/suspension/revocation test plan | 3d |

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| GUI remains execution owner | other clients duplicate runtime | Host extraction is a P0 gate |
| Journal and publish diverge | missing or phantom events | atomic append/outbox and fault injection |
| Client ID confused with identity | privilege escalation | separate Principal/Device/Client models |
| Browser reaches loopback Host | CSRF/DNS rebinding | strict Origin/Host validation, auth, CSRF defense |
| Mobile offline action appears complete | unsafe user expectation | explicit queued/accepted/completed states |
| Push contains sensitive content | privacy leak via OS/vendor | opaque minimal payload; fetch after auth |
| OS suspends mobile app | false assumption of execution continuity | Host-owned execution |
| WebSocket added too early | backpressure/lifecycle complexity | HTTP/SSE first; protocol-neutral adapter |
| Hosted multi-tenancy starts too early | large security/ops scope | staged deployment and separate threat model |

## Open decisions

1. Exact Rust crate boundary for Host and Application Layer after code inventory.
2. Whether embedded Web adapter binds loopback only or supports a separately configured listener in Stage 1.
3. Pairing mechanism and whether remote access requires an external Gateway/tunnel.
4. Mobile app delivery order: PWA first, native first, or parallel.
5. Journal storage implementation and transactional guarantees based on existing Store capabilities.
6. Whether user-level Principal is local-only in Stage 1 or supports external identity provider later.

## References

- Repository architecture baseline: `docs/bsh-2.0-architecture-rfc.md`
- Current architecture overview: `ARCHITECTURE.md`
- Initial reviewed source baseline: commit `4acc8126f89eb98c4160a8d1355c817d7eedc47e`
