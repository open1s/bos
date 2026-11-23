# BSH Multi-Interface Architecture

> **One Runtime. Multiple Interfaces. Local and Remote Access. One Extensible Agent Workspace.**

**Product direction:** BSH is a Local-First, Multi-Interface, Multi-Transport Agent Runtime Platform.

This document defines the target architecture for GUI, TUI, Web and Mobile clients. It is an architecture proposal, not a statement that every interface or transport is already implemented.

## 1. Goals and non-goals

### Goals
- One standalone Runtime Host is the sole authority for Agent execution, session state, tools, approvals and recovery.
- GUI, TUI, Web and Mobile are peer clients over a transport-independent Application Contract.
- Support local-first operation today and authenticated remote access later without changing execution semantics.
- Treat mobile as a first-class client, including native iOS/Android clients and browser/PWA access.
- Preserve execution when a client disconnects, a window closes, a browser reloads or a mobile OS suspends an app.
- Make durable state and replay authoritative; transports provide delivery, not truth.
- Keep authentication, authorization, capability checks and audit decisions in the Host trust boundary.

### Non-goals for the first delivery
- Multi-tenant hosted SaaS, distributed execution, multi-host consensus or active-active Runtime.
- A mandatory WebSocket-only protocol.
- Running Agent execution inside a browser or mobile process.
- Treating push notifications as a reliable event log or command channel.
- Replacing the existing Session Store before a measured need is demonstrated.

## 2. System context

```text
                     BSH Runtime Host (single authority)
  +------------------------------------------------------------------+
  | Agent Execution | Sessions | Tasks | Tools | Approval | Recovery |
  |                         Application Layer                        |
  | Commands | Queries | Controls | Event Projection | Authorization |
  |                    Durable Journal + Read Models                 |
  +------------------------------+-----------------------------------+
                                 |
                      Transport Adapter Boundary
             +-------------------+--------------------+
             |                   |                    |
          Local IPC           Web API              Future Gateway
        UDS / Named Pipe     HTTP + SSE            HTTP / SSE
             |                   |                    |
      +------+------+       Browser / PWA       Remote clients
      |      |      |                             |
     GUI    TUI    CLI
                             Native Mobile
                           iOS / Android Client
                                  |
                       Push Notification Service
                       (wake-up hint only; no authority)
```

The diagram shows logical ownership, not process placement. Initially, IPC and Web adapters may live inside the Host process while remaining isolated behind transport interfaces. A separate Gateway is a later deployment option.

## 3. Architectural invariants

1. **Single execution authority:** exactly one Host owns a Runtime's execution state and side effects.
2. **Clients are replaceable:** a client renders projections and issues authorized commands; it does not own Agent execution.
3. **Transport independence:** command, query, event, identity and error semantics do not depend on IPC, HTTP, SSE or WebSocket.
4. **Durability before publication:** a state transition is committed to durable state/journal (or a transactional outbox) before it is advertised as committed.
5. **At-least-once delivery:** consumers deduplicate using event identity and cursor; end-to-end exactly-once is not promised.
6. **Disconnect is not cancellation:** cancellation is an explicit, separately authorized command.
7. **Host-side authorization:** UI visibility and client-side checks are never security boundaries.
8. **Least privilege:** a remote or mobile principal never inherits the Host OS user's unrestricted authority.
9. **Replay is bounded and explicit:** clients recover from a cursor and reconcile against a snapshot when history is unavailable.
10. **Push is advisory:** notification delivery may be delayed, duplicated or dropped; clients fetch authoritative state from the Host.

## 4. Logical layers

### 4.1 Runtime Host
Owns Agent lifecycle, model/provider calls, tool execution, session and task state, approval decisions, recovery, durable journal, authorization and audit. Host lifecycle should distinguish lifecycle phase (Initializing, Recovering, Ready, Draining, Stopped, Failed) from activity (Idle, Active, Degraded).

### 4.2 Shared Application Layer
Expose stable operations independent of UI and transport:
- Commands: mutate state (send message, create task, cancel execution).
- Queries: read current state and projections.
- Controls: privileged operations (approve/reject, permission grant/revoke, host shutdown).
- Events: durable state-change notifications and ephemeral progress.
- Projections: interface-neutral view models, with explicit redaction policy.

Accepted means the Host durably accepted responsibility; it does not mean the operation completed. A timeout is an unknown outcome, not proof that a command did not execute.

### 4.3 Transport adapters
Translate framing, authentication context, flow control and disconnect behavior. They must not contain Agent business logic or become a second authority.

### 4.4 Interface clients
- **GUI:** existing GPUI desktop client.
- **TUI:** terminal client, independently launched and reconnectable.
- **CLI:** host administration and scripted client; must not instantiate another Agent Runtime.
- **Web:** browser application, initially personal/local-first.
- **Mobile:** native iOS/Android and optionally PWA. Native client is preferred where OS integration, secure storage, push, biometric/device authentication and background lifecycle matter.

## 5. Transport strategy

| Client | Preferred transport | First protocol |
|---|---|---|
| Desktop GUI | Local IPC | versioned request/response + event stream |
| TUI | Local IPC | same Application Contract |
| CLI | Local IPC | command/query |
| Web/PWA | HTTPS | HTTP commands/queries + SSE |
| Native Mobile | HTTPS | HTTP commands/queries + SSE; push hints |
| Future remote Gateway | authenticated service channel | same Application Contract |

**Web v1:** HTTP for commands and queries; Server-Sent Events for Host-to-client event delivery; durable cursor replay for recovery. WebSocket is deferred until a measured requirement for bidirectional low-latency streaming justifies its extra flow-control and lifecycle complexity. If introduced, it must use the same envelope, authorization and replay semantics.

**SSE is not the journal.** Last-Event-ID is a transport cursor hint only. Host must validate it, replay from durable journal, and define expired-cursor behavior. Replay-to-live handoff must avoid gaps and duplicates from the client's perspective.

## 6. Mobile-specific architecture

Mobile is not merely responsive Web. It has distinct device, lifecycle and network constraints.

### 6.1 Client modes
- **PWA:** fastest cross-platform delivery; shares Web API and SSE; limited OS integration and background guarantees.
- **Native iOS/Android:** preferred for secure credential storage, device enrollment, push notifications, biometric unlock, deep links, share sheets and platform-native interaction.
- Both use the same Application Contract and authorization model. A native client must not gain broader authority just because it is installed.

### 6.2 Device identity and enrollment
Represent a device separately from a client process and from a human principal:
- Principal: who is acting.
- Device: enrolled endpoint, platform, public-key identity, status and last-seen metadata.
- Client instance: one app installation/process/session connecting to the Host.
- Workspace membership: which resources the principal may access.

Enrollment should use an explicit pairing flow (short-lived, single-use challenge or QR/deep-link bootstrap), then bind a device public key. Store private keys and refresh credentials in Keychain/Keystore-backed secure storage. Support device listing, revocation, key rotation and lost-device recovery. Do not use a device ID, push token or client-supplied role as proof of identity.

### 6.3 Mobile connectivity and offline behavior
- Treat connectivity as intermittent; reconnect with bounded exponential backoff and jitter.
- Persist the last acknowledged durable cursor locally.
- On reconnect, authenticate, fetch a fresh authorization context, resume from cursor, and reconcile against a snapshot when the cursor has expired.
- Offline mode is read-only over cached, explicitly marked stale projections by default.
- Queueing commands offline is opt-in per command class. Never queue approvals, permission changes, destructive operations or execution cancellation without an explicit user action and a fresh online authorization check.
- A queued command carries a stable idempotency key and is displayed as pending until Host acceptance is confirmed.
- Never imply that a locally queued action has been executed.

### 6.4 Push notifications
Push payloads should contain minimal metadata and an opaque resource reference, not prompts, tool output, secrets or sensitive transcript content. Push is a wake-up hint: the app authenticates and fetches the authoritative event/state. Notification taps use validated deep links and re-check authorization. Delivery is best-effort; journal replay remains the recovery mechanism.

### 6.5 Mobile interaction and approvals
- Approval screens show the exact action, target, scope, risk, expiry and content digest.
- High-impact approval requires an explicit confirmation gesture and online Host decision.
- Biometric unlock may unlock a local credential; it does not replace server-side authorization or approval policy.
- Separate dismissing a notification, closing a screen, disconnecting, cancelling a task and cancelling an execution.
- Provide accessible, compact task/agent status and clear pending/unknown/completed states.

### 6.6 Background execution
The Host, not the mobile app, runs Agents. iOS/Android may suspend or terminate clients at any time. Background refresh and push cannot be prerequisites for correctness. Long-running execution continues according to Host policy; clients rehydrate state on foreground/reconnect.

## 7. Identity and security boundaries

Use separate entities and concepts:
- `Principal`: authenticated user or service identity.
- `Device`: enrolled endpoint with revocable key identity.
- `ClientInstance`: GUI/TUI/Web/Mobile process or browser session.
- `WorkspaceMembership`: principal-to-workspace authorization.
- `Session`: Agent conversation/execution context.

Local IPC uses OS peer credentials and endpoint ACLs, then Host authorization. Remote access uses TLS, explicit authentication, short-lived access tokens, refresh-token rotation, origin validation, CSRF protections where cookie credentials are used, rate limits and audit. Origin is not authentication. Loopback binding is not a substitute for browser-origin/CSRF/DNS-rebinding defenses.

All transports call the same Host authorization service for workspace, session, tool capability, approval and control decisions. Remote principals must not inherit local filesystem or process privileges. Device revocation and principal revocation must invalidate future access and refresh paths.

## 8. Durable events, replay and concurrency

A durable event envelope should include: event_id, workspace_id, session_id, sequence, execution_id, task_id, causation_id, correlation_id, occurred_at, schema_version and payload. Sequence is monotonic within its declared stream scope; UUID identity does not replace ordering.

Separate:
- Durable Journal: committed facts and replay.
- Dispatcher: fan-out and replay/live coordination.
- Event Bus: in-process best-effort notifications.
- Client Subscription: per-client cursor, filters and flow control.
- Snapshot/Read Model: bounded state reconstruction.

Token deltas may be coalesced; persist meaningful message/turn boundaries and recovery checkpoints. Slow clients must not block execution or other subscribers. Define bounded buffers, replay windows, maximum event size, pagination and snapshot fallback.

Mutations are serialized/validated by Host command handling. Use expected_version for optimistic concurrency on mutable resources. Approval decisions require an atomic Pending → Approved/Rejected/Expired transition with idempotent command ID and content digest.

## 9. Deployment evolution

**Stage 1 — Personal Local-First:** one user, one local Runtime Host, multiple local clients; Web may bind loopback only. Pairing and strict browser-origin controls are required even locally.

**Stage 2 — Personal Remote Access:** authenticated remote access to the user's Host through a hardened Gateway or secure tunnel. Keep Host authorization authoritative; do not expose arbitrary Host ports.

**Stage 3 — Hosted / Multi-User:** separate Gateway and Runtime placement, explicit Workspace Membership, tenant isolation, quotas, audit and service-to-service identity. This stage requires a separate threat model and operational design.

Do not implement multi-tenant infrastructure in Stage 1, but avoid schemas that equate Client ID with Principal or assume one interface per user.

## 10. Failure behavior

| Failure | Required behavior |
|---|---|
| Client disconnects | Execution continues; client reconnects and replays |
| Mobile OS suspends app | Host continues; app rehydrates on foreground |
| SSE cursor expired | Snapshot + new cursor, no silent partial state |
| Duplicate command retry | Idempotency key returns original outcome |
| Duplicate event | Client deduplicates by event_id/sequence |
| Slow subscriber | Isolate/drop ephemeral updates; recover durable events by replay |
| Push lost or delayed | No correctness impact |
| Device revoked | Reject new requests and refresh; terminate active sessions by policy |
| Host crash | Recover durable state before accepting mutations |
| Authorization changed | Re-check on every command and protected resource read |

## 11. Acceptance gates

- Closing GUI/TUI/browser/mobile never implicitly cancels an active execution.
- A second client can observe and control the same authorized session without starting another Agent.
- Replay after network loss yields a consistent projection with no missing committed events.
- Duplicate command retries do not duplicate side effects.
- An expired cursor triggers explicit snapshot reconciliation.
- Mobile app suspension and process termination do not affect Host execution.
- Revoked devices cannot refresh credentials or issue commands.
- Push payloads contain no sensitive transcript or tool result.
- Approval decisions are atomic and bound to the exact action digest.
- Unauthorized clients cannot infer or access other workspaces/sessions.
- Local and remote transports produce equivalent business outcomes and authorization decisions.

## 12. Implementation roadmap

1. Inventory existing AppState/Runner/Store/EventBus/Task/Approval ownership and lifecycle.
2. Extract the Host lifecycle and Application Contract; keep GUI behavior stable.
3. Add durable journal semantics, sequence/cursor, replay and snapshot reconciliation.
4. Add local IPC and migrate GUI; then TUI and CLI clients.
5. Add HTTP/SSE Web adapter and Personal Web MVP.
6. Add device/principal enrollment and native Mobile MVP; PWA may precede native apps.
7. Harden remote Gateway and only then consider hosted multi-user deployment.

## 13. Implementation evidence and status

The initial static review was performed against repository commit `4acc8126f89eb98c4160a8d1355c817d7eedc47e` (BSH 4.1.26). It identified coupling between GPUI AppState and Runner, a Session-oriented in-flight journal rather than a general Runtime Event Journal, a best-effort bounded Event Bus, and gaps between Task persistence and generic Tool Execution recovery. These observations are a dated baseline, not a claim about later commits. Re-run the code audit before implementation.

