# Tasks: durable, tier-bound work the reader and the model share

Status: design approved (grilling session, 2026-11-07). Implemented so far:
the record plus store, the typed host.runs seam and its shell service, task_create
and task_join, the Tasks tab, the turn-tier scheduler, the subagent executor, and
the delegation executor (a delegated task carries a capability contract, which
is what a peer executes on this build), and resume triggers (a timed-out join
arms the task, the tick delivers one harness-generated turn with the settled
results, and the arming clears in the same write), and task_cancel (turn,
subagent and delegation runs are stopped, scheduled tasks are disabled). The
job executor, the peer-side agent turn for objective-only delegations and
peer-owned tasks are still open; the peer-side task queue (record, claim,
settle, cancel, dead-claim recovery) and its wire operations (task_create,
task_status, task_cancel, origin-scoped) and the peer shell drain (the peer
claims due tasks, runs each as a background turn in its own session, and settles
the queue with the result), and the local mirror with the peer-task tier (the
origin stores a Host::Peer record, forwards create/cancel, and refreshes status
and result from the owner on join). The job executor, the peer-side agent turn
for objective-only delegations and the extension-packaging gate remain; the
phase table below is the plan of record.

A task is a durable unit of user intent. It has a lifecycle, a trigger, and at
most one active attempt. Attempts are runs bound to one of the existing execution
tiers: a turn, a subagent, a job, or a delegation to a peer. The tier registries
remain the only things that execute; the task store owns intent, scheduling,
retries, results and the audit trail. Parallelism within a request is expressed
as child tasks, not as several attempts of one task.

Two remote mechanisms exist on purpose:

- Interactive work delegates a run to the peer (a delegation), immediate and
  joinable, with no task record on the peer.
- Cadence work creates a peer-owned task, fired by the peer's clock, mirrored
  locally as a reference plus a cached snapshot.

## Vocabulary

- Task: the durable record. id, intent, owner, trigger, tier, budget, status.
- Attempt: one execution try of a task. New attempt per retry.
- Run: the tier object an attempt binds to: a turn, a subagent id, a job id, or
  a delegation id. Runs are referenced, never duplicated.
- Trigger: now, at, or every. The former schedule cadence becomes a trigger.
- Delivery: the ledger entry that says a settled run has been shipped to its
  origin session as a resume trigger, exactly once.

## Record

One file per task, same doctrine as sessions:

    <workspace>/.bsh/tasks/<id>.json

Unknown workspace falls back to the harness directory, like sessions do.

Fields:

- id: opaque, stable, prefixed for the host when mirrored.
- title and intent: short label plus the full prompt/objective.
- host: local, or peer + node id for a mirrored peer-owned task.
- parent: optional task id. Roots are legal; origin is a session plus turn.
- origin: session id plus turn index (the trace).
- trigger: now | at(unix ms) | every(seconds) plus last_run.
- tier: turn | subagent | job | delegation. Legality: subagent only for tasks
  created inside a turn; scheduled tasks may bind turn, job or delegation.
- target: for delegation, the peer node id; optional capability allowlist.
- command: for job, the command line; cwd is the session's workspace.
- deps: task ids that must succeed first.
- budget: max steps, max tokens, deadline ms.
- retry: manual (default) or auto with max attempts.
- status: scheduled | queued | running | waiting | blocked | succeeded |
  failed | cancelled | interrupted | unknown.
- attempts: ordered list of (id, run kind, run id, started, ended, status,
  result summary, usage, error).
- delivery: per attempt, delivered_at or never.

Status rules:

- scheduled: between firings, for an at or every task.
- queued: waiting for a resource; waiting_on says which.
- running: an attempt is live.
- waiting: blocked on deps.
- blocked: waiting for a human approval (permission gate).
- unknown: a peer-owned task or delegation whose peer is unreachable; NOT a
  failure, and never a reason to fire locally.
- succeeded / failed / cancelled / interrupted: terminal.

## Tiers and dispatch

The scheduler dispatches an attempt according to tier:

- turn: opens a turn in the origin session with the intent as its prompt.
- subagent: only legal inside a turn; the turn's own tool call starts it.
- job: starts the command as a background job owned by the session.
- delegation: posts the objective to the peer with the narrow contract below.

A scheduled task fires as turn, job or delegation. Subagent is refused for a
scheduled task with a clear error rather than silently starting something else.

## Federation: delegation run (interactive)

The peer executes a delegated objective as a real agent turn:

- its own remote-work profile (model, preset, permission tier, workspace),
- a fresh session per delegation, titled from the objective, listed on the peer
  as Federation work,
- the peer's policy applies; the origin cannot widen it.

The contract is narrow-only:

- Origin sends: objective, deadline, max steps, max tokens, optional capability
  allowlist (intersected with the peer's profile), trace id. It cannot name the
  peer's model, preset, session, workspace or permission tier.
- Peer returns: state, bounded final text, artifact ids, usage (tokens, steps,
  wall clock), refusal reason when denied.

Cancel propagates to the remote turn. The full transcript stays on the peer and
is openable there; the origin sees the bounded result.

## Federation: peer-owned task (cadence)

The peer is the only writer. The local node holds a mirrored record plus a
command channel.

- Create: the origin sends a task-create request; the peer validates its own
  policy for external tasks, stores the task, and owns its clock.
- Mirror: local status, next run, last result summary, peer version, last seen.
- Commands: cancel, retry, update schedule. The peer's answer updates the
  mirror; peer wins conflicts. The origin never fires a peer-owned task locally.
- Unreachable peer: the mirror row shows unknown/stale and is re-checked on the
  tick; it is never executed locally as a fallback.

## Orchestration: spawn, continue, join

The model spawns tasks without blocking, continues its own work, then joins:

    task_join(ids, mode = all, timeout)

The join returns one snapshot: per-run state, result text, artifacts, usage, and
still-running for unfinished runs. Joining is by run id; joining a parent task
does not implicitly join descendants.

If the join times out, it returns the partial snapshot and arms a resume for the
unsettled ids only.

## Resume triggers

- Armed only by explicit wait intent: a join timeout, or a turn ending with an
  intent outstanding. A reader stop never arms; a model that never joined does
  not arm.
- Coalesced: runs settling within a short window ship as one trigger turn;
  later settlers ship in a later batch.
- Deliver once: the ledger guarantees a run is shipped at most once.
- Shape: a synthetic user row, visibly marked as generated by the harness,
  listing each settled run's tier, status, clipped result, artifacts and usage,
  plus a prompt section that this turn was triggered by completed work.
- If the origin session is mid-turn, the batch queues behind it; if the session
  is gone, the runs stay on the board and are never delivered.

## Admission

A due or newly created attempt that cannot start stays queued with a reason:

- session_busy (one turn per session),
- subagent_slot (three slots, reserved atomically),
- job_cap (per-session pending jobs),
- peer_inflight (per-peer in-flight delegations).

Caps: one turn per session; three subagent slots; a small per-session job cap;
a small per-peer in-flight delegation cap. Dispatch reacts to slot-release
events, with the tick as a backstop. A queued attempt whose deadline passes
fails as capacity_timeout; nothing waits forever.

## Restart

- job, subagent, turn attempts were running at shutdown: interrupted (terminal;
  the transcript up to the cut stays).
- Delegation attempts: re-queried on boot; adopt the peer's authoritative state
  and result; unknown while unreachable, re-checked on the tick.
- Queued tasks: resume dispatch on boot.
- Armed but undelivered trigger batches: re-armed from the ledger.
- Retry: only if the task says auto and budget remains. Default manual.
- Cadence: at most one catch-up attempt on boot, then the normal schedule.

## Permissions

Runs inherit the origin session's effective policy at fire time. Delegations use
the peer's remote-work profile, narrowed by the origin's allowlist. Peer-owned
tasks use the peer's task profile. No task carries its own escalation. An
approval gate hit while unattended becomes blocked: approval required, visible
on the board, resolved by the reader approving through the session (or failing
at the deadline). Nothing auto-approves.

## Model surface (first party, task-first)

    task_create(intent, tier, trigger, target, capability, command, parent,
                deps, budget, retry) -> task id plus run id
    task_join(ids, mode, timeout)       -> per-run state, result, usage
    task_status(id)                     -> record minus large fields
    task_list(filter)                   -> board rows
    task_cancel(id)                     -> outcome

Legacy tools stay as aliases for one release: subagent and subagent_result,
federation_delegate, job tools, and schedule_create/edit/list/delete map onto
the same records. The Automation pane becomes a filtered Tasks view.

## Board (right pane, Tasks tab, seventh)

State-first sections: Running, Queued, Scheduled, History.

Row: status dot, tier, host badge (local or peer: name), origin session title,
elapsed, budget usage, queued reason, approval reason. Children indent under
their parent. Clicking expands inline: intent, trigger, deps, attempts with
per-attempt status and result, artifacts, usage, and actions (stop, retry, open
session). History is collapsed and has Clear completed. Empty states explain
that work appears when a turn spawns a task or a trigger is due.

## Extension-first

The feature ships as a first-party tasks extension through the same manifest,
capability, lifecycle and effect path as a third-party package.

Host seams (typed services or events, added for any package):

- host.runs: start, observe, cancel and join a turn, subagent, job or
  delegation, with a narrow-only spec; permissioned.
- host.clock: a tick the extension can observe.
- host.trigger: arm a resume for a session (deliver-once ledger enforced by the
  host).
- run_settled events: quiescent run completions with result, usage, status.

The extension declares the tools, the Tasks panel, prompt guidance and the
Automation rewrite; its records live in extension storage rooted at the
workspace store. No privileged in-core shortcut. Acceptance follows
docs/bsh-extension.md conformance; the extension ships enabled in the standard
bundle set.

Capability shape: one automation capability with sub-scopes (turns, subagents,
jobs, delegations, triggers), default-deny for third parties, granted per
package. The first-party tasks extension ships with it granted.

## Migration

1. Import [[schedules]] from settings.toml into task records with trigger every
   (or at for one-shots) and tier turn; disable the old clock path.
2. Keep schedule_* and the tier tools as aliases for one release; route both to
   the task records so there is one source of truth.
3. Rewrite the Automation pane as the Tasks board filtered to scheduled rows.
4. Replace prompt guidance: Delegating with subagents and the jobs paragraph
   move to tasks; add the join-before-finishing rule.
5. Update tool docs, the wire budget measurement, and the 2.0 acceptance docs.

## Phases

P1 Walking skeleton (seam plus one vertical slice)
- host.runs (turn and subagent), host.clock, run_settled; extension skeleton
  with the automation capability; task record plus store; task_create(tier =
  subagent, trigger now) and task_join; a minimal Tasks tab row.
- Done: an extension-started subagent appears as a task, the board draws it,
  join returns its result; an ungranted extension is denied.

P2 Local scheduler
- turn and job tiers; at/every; queue with reasons; dispatcher; restart
  reconciliation; retry policy; submit/cancel; budget enforcement.
- Done: a scheduled job task fires and settles across a restart; capacity
  timeout is observable; interrupted attempts are reported.

P3 Peer executor (delegation)
- remote-work profile; fresh session; narrow contract enforcement; budget;
  cancel; refusal; origin delegation attempt plus result.
- Done: two-node E2E where A delegates an objective and B answers from a fresh
  session; cancel stops it; B refuses when its profile forbids the request.

P4 Resume triggers
- arming on join timeout; coalescing; ledger; synthetic row and prompt section;
  queue behind streaming turns; boot re-arm.
- Done: slow peer, partial join, turn ends, runs settle, one trigger turn
  composes the answer; no duplicate delivery after restart.

P5 Peer-owned cadence tasks
- task create/cancel/retry over federation; peer scheduler; mirror; unknown
  while offline; result adoption on reconnect.
- Done: A creates an every task on B; B fires while A is offline; A reconnects
  and sees the result; cancel from A stops it.

P6 Board completion
- sections, expansion, actions, badges, empty states, history, i18n, click
  tests; Automation pane migration.

P7 Migration and hardening
- aliases, prompt rewrite, docs, wire budget, conformance gates, full E2E
  script extension.

## Acceptance scenario

Node A, session S; the user asks for a review. The model creates t1 and t2
(tier subagent) and t3 (tier delegation, target B). t1 and t2 take slots, t3
posts. B runs a fresh-session agent turn under its remote-work profile. A's
turn keeps working, then joins r1, r2, r3. If all settle in time, one snapshot
and a composed final answer in S. If B is slow past the timeout, the join
returns a partial snapshot and arms a resume; when the last run settles, one
coalesced trigger turn composes the final answer. The board shows three settled
runs under the turn, with t3 badged peer: B.

## Budget defaults

20 steps, 200k tokens, 10 minutes per attempt. Cadence attempts default smaller
(5 steps, 50k tokens, 5 minutes). All overridable per task; the peer enforces the
stricter of its profile and the contract.

## Risks

- host.runs is a real privilege for extensions; the capability grant and the
  refusal path must be built before the feature is enabled by default.
- Two remote mechanisms can drift; shared status vocabulary and one board row
  per run are the guardrail.
- Long turns: a join may hold a turn for minutes; the reader must see it and be
  able to stop it.
- The peer executor spends the peer's model budget; budgets and refusal are not
  optional.
- Extension conformance gates may lag the feature; the tasks extension is the
  forcing function for docs/bsh-extension.md.
