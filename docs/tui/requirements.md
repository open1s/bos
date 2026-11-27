# BSH TUI — Product Requirements and Delivery Backlog

- **Status:** Proposed
- **Related issue:** [#452](https://github.com/open1s/bos/issues/452)
- **Sizing rule:** Every story is planned at no more than three working days.
- **Implementation gate:** Complete code Gap Analysis and contract review before coding.

## 1. Product requirements

Priority:
- **P0:** required for a safe and useful first release.
- **P1:** required for a complete operational workspace.
- **P2:** future polish or extension.

| ID | Requirement | Priority | Acceptance evidence |
|---|---|---:|---|
| TUI-R001 | TUI is an independent client of standalone Runtime Host | P0 | TUI exit leaves Host-owned work running |
| TUI-R002 | Domain mutations go through shared Application Contract | P0 | No direct storage/Agent mutation from rendering code |
| TUI-R003 | Host owns authorization and approval decisions | P0 | Forged/stale/expired decisions rejected |
| TUI-R004 | Durable events support cursor replay and gap-free live handoff | P0 | Fault-injection proves no committed event is skipped |
| TUI-R005 | Commands are idempotent and expose unknown outcomes | P0 | Retry with same key does not duplicate mutation |
| TUI-R006 | Conversation renders typed semantic timeline blocks | P0 | Tool/delegation/approval are not guessed from Markdown |
| TUI-R007 | Composer supports multiline Unicode and safe paste | P0 | CJK, combining marks and bracketed paste tests pass |
| TUI-R008 | Adaptive layout supports narrow terminal | P0 | Core tasks usable at 80 columns |
| TUI-R009 | Terminal state is restored on exit and panic | P0 | Raw mode/alternate screen restored in fault tests |
| TUI-R010 | Job lifecycle is independent of current conversation view | P0 | Job remains queryable after navigation/disconnect |
| TUI-R011 | Typed errors distinguish conflict, forbidden, offline and unknown outcome | P0 | Contract fixtures map to actionable UI states |
| TUI-R012 | Approval modal binds to immutable action and scope | P0 | Digest/target/scope/expiry mismatch rejected |
| TUI-R013 | Slow clients cannot block Host execution | P0 | Bounded subscriber queues and backpressure tests |
| TUI-R014 | GUI/TUI share operation semantics | P0 | Shared contract conformance suite passes |
| TUI-R015 | Activity and Audit are separate views | P1 | Operational events and security facts are distinguishable |
| TUI-R016 | Jobs view supports filters and detail timeline | P1 | Active/waiting/completed/failed/interrupted states shown |
| TUI-R017 | Agents view represents delegation hierarchy | P1 | Parent/child relationships and status are visible |
| TUI-R018 | Inspector provides permission-filtered input/output/authorization details | P1 | Secret redaction tests pass |
| TUI-R019 | Command palette exposes destinations and actions | P1 | All primary destinations keyboard reachable |
| TUI-R020 | Capability browser shows scope and binding | P1 | Granted/requested/denied/expired states distinct |
| TUI-R021 | Theme and keymap are configurable | P2 | Preferences persist per client |
| TUI-R022 | Shared Application Contract is transport-neutral for Web/Mobile | P1 | Contract tests do not import Ratatui types |

## 2. Epics and user stories

Estimates are elapsed engineering effort, not calendar commitments. Stories should be re-sized after the Gap Analysis.

### E1 — Contract and code Gap Analysis

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-001 | Inventory current BSH AppState, Runner and execution ownership | 2d | Evidence-backed ownership map |
| TUI-002 | Inventory current Session/Turn/Task/Tool identities and states | 2d | State/identity matrix with missing transitions |
| TUI-003 | Audit current event persistence, journal and replay behavior | 3d | Durability and cursor gap report |
| TUI-004 | Audit approval lifecycle and cross-client race behavior | 2d | State machine and security gaps documented |
| TUI-005 | Map current GUI services to transport-neutral operations | 3d | Command/query candidate matrix |
| TUI-006 | Validate Ratatui/backend and platform dependency policy | 1d | Supported backend decision with evidence |

### E2 — Application Contract and Host client

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-007 | Specify typed command/query/control envelopes | 2d | Versioned contract fixtures |
| TUI-008 | Specify idempotency and unknown-outcome semantics | 2d | Duplicate/mismatched key tests defined |
| TUI-009 | Specify projection identity, revision and snapshot semantics | 2d | Consistent read model contract |
| TUI-010 | Define Host client trait and typed error taxonomy | 2d | Ratatui-independent API |
| TUI-011 | Define local IPC authentication and endpoint discovery | 3d | Threat model and protocol contract |

### E3 — Durable event and reconnect

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-012 | Define durable event envelope and stream sequencing | 2d | Schema and compatibility policy |
| TUI-013 | Define snapshot cursor and replay protocol | 2d | Snapshot exact-sequence invariant |
| TUI-014 | Define gap-free replay-to-live handoff | 3d | Race/fault test plan |
| TUI-015 | Define slow-consumer limits and progress coalescing | 2d | Bounded memory/backpressure contract |
| TUI-016 | Define reconnect, expired cursor and resync UX | 2d | Recovery state transitions documented |

### E4 — TUI foundation

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-017 | Define terminal lifecycle and panic-safe restoration | 2d | Lifecycle design and test matrix |
| TUI-018 | Define application state and navigation model | 2d | Screen/focus/back-stack model |
| TUI-019 | Define adaptive layout breakpoints and constraints | 2d | Layout test matrix |
| TUI-020 | Define keymap, input modes and command palette | 2d | Keymap contract and conflict audit |
| TUI-021 | Define theme tokens and 16-color fallback | 1d | Contrast and monochrome rules |
| TUI-022 | Define terminal snapshot and fake-Host test harness | 2d | Deterministic test strategy |

### E5 — Conversation and composer

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-023 | Define typed timeline projection and rendering rules | 3d | Message/tool/delegation/approval fixtures |
| TUI-024 | Define streaming, coalescing and reconnect rendering | 2d | No duplicate committed blocks |
| TUI-025 | Define multiline composer and safe paste behavior | 3d | Unicode/paste/key behavior matrix |
| TUI-026 | Define draft persistence and navigation behavior | 2d | Draft/cursor preservation rules |
| TUI-027 | Define slash command and completion UX | 2d | Discoverable, non-ambiguous submit behavior |

### E6 — Execution, Jobs and Agents

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-028 | Define inline tool execution card | 2d | State and correlation identity visible |
| TUI-029 | Define Inspector detail tabs and redaction | 2d | Permission-filtered detail contract |
| TUI-030 | Define Jobs list/filter/detail UX | 2d | Job independent of Conversation |
| TUI-031 | Define Agent delegation tree and detail UX | 2d | Parent/child/status semantics |
| TUI-032 | Define cancellation affordance and confirmation | 2d | Host state gates cancel action |

### E7 — Security, capability and audit

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-033 | Define approval action digest and modal UX | 3d | Exact action/scope/expiry shown |
| TUI-034 | Define atomic approval decision and stale-state handling | 3d | Exactly one decision commits |
| TUI-035 | Define capability browser and scope presentation | 2d | Available/granted/requested/denied distinct |
| TUI-036 | Define Activity versus Audit projections | 2d | Security facts are durable and separate |
| TUI-037 | Define secret redaction and diagnostic disclosure rules | 2d | No secret in UI fixtures/log snapshots |

### E8 — Cross-client and stabilization

| ID | Story | Estimate | Acceptance |
|---|---|---:|---|
| TUI-038 | Define GUI/TUI shared contract conformance suite | 3d | Same operation fixtures for both clients |
| TUI-039 | Define future Web/SSE compatibility checks | 2d | No terminal-specific types in contract |
| TUI-040 | Define mobile stale/offline projection semantics | 2d | Offline state is visibly read-only |
| TUI-041 | Define platform terminal compatibility matrix | 2d | macOS/Linux/Windows terminal cases listed |
| TUI-042 | Define end-to-end vertical slice and release gate | 2d | Host → TUI → conversation → stream → reconnect |

## 3. Suggested delivery sequence

1. **Discovery:** TUI-001–006. Revalidate the current code and existing multi-interface RFC.
2. **Contracts:** TUI-007–011. Freeze only the minimum Host client semantics.
3. **Durability:** TUI-012–016. Prove replay and reconnect before building a rich timeline.
4. **Foundation:** TUI-017–022. Establish safe terminal lifecycle and deterministic test harness.
5. **Vertical slice:** TUI-023–027. One conversation with streaming, Unicode composer and reconnect.
6. **Operations:** TUI-028–032. Inline execution, Inspector, Jobs and Agents.
7. **Security:** TUI-033–037. Approval and capability UX backed by Host authority.
8. **Cross-client:** TUI-038–042. Conformance and future interface compatibility.

## 4. Release acceptance gates

### Gate A — Safe foundation
- Host execution does not depend on TUI lifetime.
- No privileged action is authorized by client state.
- Terminal restores its state after panic/exit.
- Unknown outcomes are never reported as definite failures.

### Gate B — Useful TUI
- Conversation, composer, inline tool state and reconnect work end-to-end.
- Core navigation works at 80 columns.
- Unicode and paste behavior pass platform tests.

### Gate C — Operational workspace
- Jobs and Agents are independently discoverable.
- Approval modal is bound to exact Host action.
- Inspector and Audit obey authorization and redaction.

### Gate D — Multi-interface readiness
- GUI and TUI pass the same Application Contract suite.
- Web/Mobile can reuse contracts without importing terminal types.
- Slow/disconnected clients do not affect Host execution.

## 5. Explicit non-goals

- Replacing or redesigning BSH GUI in this workstream.
- Moving Agent execution into the TUI process.
- Introducing a second TUI-only persistence system.
- Exposing internal reasoning traces; only policy-approved summaries may be shown.
- Claiming pause/resume/retry support until current Host semantics are verified.
- Implementing Web/Mobile clients as part of the TUI first release.
