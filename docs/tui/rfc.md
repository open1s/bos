# RFC: BSH Multi-Interface TUI Architecture

- **Status:** Proposed
- **Related issue:** [open1s/bos#452](https://github.com/open1s/bos/issues/452)
- **Scope:** BSH TUI client, shared Application Contract, standalone Runtime Host integration
- **Decision inputs:** Host=standalone; execution=local; Event Journal=durable; UI toolkit=Ratatui
- **Non-goal:** This RFC does not implement or assert the existence of the proposed contracts.

## 1. Abstract

This RFC defines BSH TUI as an independent client of a standalone BSH Runtime Host. Ratatui is the rendering and interaction layer, not the owner of Agent execution, task lifecycle, authorization, persistence or recovery. The same transport-neutral application semantics must support the existing GUI and future Web and Mobile clients.

The central invariant is:

> One Runtime, multiple independent interfaces, one authoritative state and one consistent security model.

## 2. Context and problem

A terminal interface added directly around GUI-owned application state would create a second execution path and duplicate lifecycle logic. A TUI-specific event bus or storage implementation would drift from GUI behavior and make reconnect/recovery unreliable. Conversely, exposing internal Rust types or channels as a client API would couple every interface to implementation details.

The architecture therefore separates:
- **Runtime Host:** authoritative domain and execution authority.
- **Application Services:** transport-neutral commands, queries, controls and projections.
- **Transport Adapters:** local IPC now; future HTTPS/SSE without domain policy.
- **Interface Clients:** GUI, TUI, CLI, Web and Mobile.
- **Presentation State:** per-client navigation, focus, layout, drafts and transient selection.

## 3. Architectural decisions

### ADR-01 — Adaptive workspace

**Decision:** TUI layout adapts to terminal dimensions and user preferences. It progressively hides secondary information rather than clipping primary content.

**Consequences:** Layout thresholds are hints, not protocol behavior. Resize must preserve focus, draft text and selected entity.

### ADR-02 — Sidebar navigation

**Decision:** Primary navigation uses a persistent sidebar when width permits, with a compact/hidden mode on smaller terminals.

**Consequences:** Every destination also has a command-palette and keyboard route; navigation cannot depend on pointer input.

### ADR-03 — Modal interaction

**Decision:** Confirmations, approvals, destructive actions and focused forms use modal dialogs.

**Consequences:** Modal focus is trapped; Escape behavior is explicit; high-risk approval dialogs show the exact action, target, scope, expiry and identity before a decision.

### ADR-04 — Inline execution presentation

**Decision:** Tool calls, results, delegation and execution status are represented inline in the conversation timeline, with details available in Inspector.

**Consequences:** Inline cards are projections of typed domain events, not markdown heuristics. Collapsing a card never discards its underlying result or audit record.

### ADR-05 — Ratatui

**Decision:** Use Ratatui as the TUI rendering framework.

**Consequences:** Terminal backend, event loop, input handling, rendering and application state remain separated. Crossterm may be evaluated as a backend, but dependency and platform compatibility must be confirmed against the repository before implementation.

## 4. Logical architecture

```text
+------------------+  +------------------+  +------------------+
| GUI Client       |  | TUI Client       |  | Web / Mobile     |
| presentation     |  | Ratatui          |  | future clients   |
+--------+---------+  +--------+---------+  +--------+---------+
         |                     |                     |
         +---------------------+---------------------+
                               |
                   Transport-neutral Application API
             Commands | Queries | Controls | Projections
                               |
                      Transport Adapter(s)
                  Local IPC now / HTTPS-SSE later
                               |
+--------------------------------------------------------------+
|                    Standalone Runtime Host                  |
|  AuthN/AuthZ | Session | Agent | Job | Tool | Approval       |
|  Recovery    | Projection | Command validation | Audit      |
|  Durable Event Journal              Runtime Event Bus       |
+--------------------------------------------------------------+
                               |
                   Durable local storage / OS
```

The diagram is conceptual. It does not prescribe a concrete IPC serialization format or claim that all boxes exist in the current implementation.

## 5. Ownership model

| State | Owner | Examples |
|---|---|---|
| Domain state | Runtime Host | sessions, turns, jobs, agents, tool executions, approvals |
| Durable history | Runtime Host / Journal | committed events, snapshots, audit facts |
| Authorization | Runtime Host | principal, capability checks, approval validity |
| Client application state | TUI client | active screen, selected conversation, inspector tab |
| Presentation state | TUI renderer | focus, scroll offset, dimensions, modal stack |
| Draft state | Client with explicit persistence policy | unsent composer content |

A client may cache projections for responsiveness. Cached state must be versioned, visibly stale when disconnected, and never used to authorize an action.

## 6. Application Contract

The contract must be independent of Ratatui, terminal input events and transport framing.

### 6.1 Operations

- **Command:** requests a domain mutation (send message, create/cancel job, decide approval).
- **Query:** reads a consistent projection or detail view.
- **Control:** privileged lifecycle/security operations (approve, revoke, shutdown).
- **Event:** reports a committed fact or explicitly ephemeral progress update.
- **Subscription:** requests event delivery from a durable cursor.
- **Snapshot:** describes state at an exact stream revision/cursor.

### 6.2 Common metadata

Every mutation carries:
- `command_id` — idempotency key generated by client.
- `client_instance_id` — identifies this client process, not a Principal.
- `target` — typed domain identity.
- `expected_version` — optional optimistic concurrency precondition.
- `payload` — schema-versioned operation data.

The Host derives the authenticated Principal from the transport/session context. It must never trust a client-supplied Principal field.

### 6.3 Outcome semantics

A request timeout means **unknown outcome**, not failure. The client queries operation status or retries with the same idempotency key. Same key plus same canonical payload returns the original outcome; same key plus different payload is rejected. Accepted, running, completed, rejected and unknown are distinct states.

## 7. Durable events and replay

The durable Journal is the source of replayable committed facts. An in-process Event Bus is a delivery mechanism, not a durability guarantee.

Event envelope minimum:
- event_id, workspace_id, stream_id, sequence
- session_id / execution_id / task_id when applicable
- causation_id, correlation_id, occurred_at
- schema_version, event_kind, payload classification

Requirements:
1. Commit domain mutation and durable event atomically, or use a transactional outbox.
2. Publish only after durable commit.
3. Sequence is monotonic within a declared stream.
4. Delivery is at-least-once; clients deduplicate by event identity.
5. Snapshot records the exact sequence it represents.
6. Replay-to-live handoff is gap-free.
7. Expired cursors return a typed response requiring snapshot refresh.
8. Token deltas and high-frequency progress may be coalesced; turn/message boundaries, approvals, execution transitions and audit facts must remain durable.

## 8. Execution and disconnect semantics

- Client disconnect is not Job cancellation.
- Closing a conversation view is not Session deletion.
- Hiding an Inspector is not cancellation.
- Cancelling a Job is an explicit Host command with authorization and a durable outcome.
- Host shutdown is distinct from client exit.
- Unknown external tool side effects must not be blindly retried after recovery.
- The TUI must represent Interrupted, Recoverable and NeedsIntervention distinctly from Failed or Cancelled.

## 9. Security and approval

The Host is authoritative for authorization and approval state. The TUI is an untrusted presentation client.

Approval UI must bind the decision to:
- exact action digest and immutable action description
- target resource and workspace
- requested capability/scope
- requesting agent/tool identity
- expiry and current approval state

Approval is an atomic Host transition. Stale, expired, already-decided or mismatched actions are rejected. The UI must not optimistically display a completed approval before Host confirmation. Security-sensitive actions produce durable audit facts without recording secrets.

## 10. TUI module boundaries

Suggested modules (names are illustrative and must be reconciled with current workspace layout):

- `bootstrap`: terminal setup, panic-safe restore, signal handling
- `client`: transport-neutral Host client traits and typed errors
- `application`: navigation, focus, command dispatch, projection cache
- `screens`: conversations, jobs, agents, capabilities, activity, audit, settings
- `components`: composer, timeline, execution card, inspector, modal, status bar
- `input`: keymap, modes, paste, resize and command palette
- `rendering`: Ratatui widgets and adaptive layout
- `testing`: fake Host, contract fixtures, terminal snapshots and event replay fixtures

Dependency rule: domain and Host client abstractions must not depend on Ratatui. Rendering must not call Runtime internals directly. Screens depend on application projections and commands, not storage.

## 11. Adaptive layout policy

| Terminal width | Layout |
|---:|---|
| < 80 columns | Single active view; navigation via command palette/back stack |
| 80–99 | Compact sidebar + main; Inspector modal |
| 100–119 | Sidebar + main; Inspector hidden or overlay |
| 120–159 | Three-column workspace |
| ≥ 160 | Three columns with wider detail regions |

Height is also adaptive: below 24 rows, hide secondary status details and use focused views. Preserve composer visibility and a clear way to inspect execution/approval state.

## 12. Error and backpressure model

Typed client-facing error classes include: Unauthorized, Forbidden, NotFound, Conflict, StaleVersion, CursorExpired, RateLimited, ResourceExhausted, HostUnavailable, ProtocolMismatch, ValidationFailed and Internal. Do not expose raw secrets or internal stack traces in user-facing text.

Each subscriber has bounded queues and independent flow control. Slow TUI rendering cannot block Host execution or other clients. Durable facts are recovered by cursor replay; ephemeral progress may be dropped/coalesced with an explicit refresh indicator.

## 13. Future Web and Mobile compatibility

The Application Contract must not encode terminal assumptions. Future HTTP/JSON commands and SSE subscriptions should preserve idempotency, authorization, cursor replay, backpressure and typed errors. Mobile clients may suspend or roam; offline projections are visibly stale and read-only by default. Push is a wake-up hint, never an execution result or approval authority.

This RFC complements `docs/rfc/multi-interface-runtime.md`; it narrows the TUI client and UX requirements and does not supersede the broader multi-interface Runtime RFC.

## 14. Open questions before implementation

1. Which existing BSH application services can be extracted without changing their semantics?
2. What durable Journal and cursor primitives already exist, and which are session-local?
3. Which command/query operations can be supported by a Host client today?
4. What is the current approval state machine and does it provide atomic cross-client decisions?
5. Which terminal backend and supported terminal matrix match the project's dependency policy?
6. What is the minimum vertical slice that proves Host independence from TUI lifetime?

## 15. Required next step

Perform a repository-backed Gap Analysis against the target commit. For each requirement, record current implementation path, evidence, gap, severity, dependency and smallest issue. Do not infer implementation from this RFC or the visual concept.
