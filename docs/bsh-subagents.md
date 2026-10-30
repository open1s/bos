# Subagents in bsh

## Behaviour

A parent turn can delegate a self-contained task to a child agent with its own
context. The model calls the async tool named subagent with:

    { "description": "short title", "prompt": "complete task" }

The child receives:

- the parent turn's LLM config (model, endpoint, key, effort);
- a copy of the parent's tool registry at build time;
- a focused-subagent system instruction prepended to the parent persona;
- a fresh Agent session and context;
- no step budget at all: a turn - parent or child - ends when its model stops
  asking for tools, and the reader can stop it from the composer. The engine's
  `max_steps` exists only as an optional runaway guard for embedders that ask
  for one; BSH never sets it.

A child at depth 0 or 1 receives the subagent tool again, giving the bounded DSH
lineage tree: session root, children, and grandchildren. Depth 2 is the leaf.
At most three children run at once across the tree; the slot is reserved under
one lock, so a parallel batch cannot overrun it. A child has no wall-clock limit
of its own: it ends on the same rule as any turn - when its model stops asking
for tools - and the reader can stop the run.

The subagent tool does NOT wait for the child. It returns immediately with a
`subagent_id` and `status: "running"`, and the child is detached on the runner's
Tokio runtime, so the parent turn keeps working while it runs. Collect the
answer with the `subagent_result` tool: pass the id (and optionally
`wait_seconds`, up to 60) or omit it to list this session's runs. A collected
result is bounded to 16,000 characters; the full result and the child
transcript stay in the run registry and the subagent menu.

The child's tools still carry the parent session and policy principal, so the
same workspace policy, permission ceiling, path scopes and builtin Rego modules
apply to a child.

## Model guidance

Every parent turn that carries the subagent tool also gets a standing system
prompt section and a split-first tool description, so the model knows the
intended way to handle a large job:

- if the work has two or more independent workstreams, split it;
- state a short plan naming those parts;
- call subagent once per part with a complete prompt (goal, paths/context,
  constraints, expected output format, what to report);
- run at most three children at once, then later waves;
- reconcile the results and do final verification in the parent.

The guidance explicitly excludes trivial single steps and tightly coupled
sequential work. A leaf child does not inherit the delegation instruction,
because it has no subagent tool.

The child behaves like a normal agent to the reader. Selecting it opens its
transcript, the composer stays available, and a submission is handled the way a
session's own submission is:

- while the child still streams, the prompt is queued behind that turn. It is
  drawn after the transcript as the reader's next row (view-only: a saved child
  never carries a prompt no turn answered) and starts a child turn of its own
  when the current one settles, one prompt per turn;
- a prompt sent to a settled child continues the SAME child agent while its live
  process exists, and otherwise rebuilds the child from its saved Session - same
  model, same tools, same focused-child persona, stored history replayed - so a
  restart is not a dead end;
- the prompt is never redirected into the parent conversation, and a selected
  child never offers the parent turn's stop control.

A refusal - the spawn policy, a missing credential - is said once and the
waiting prompt is dropped rather than retried on every tick.

## Registry

crates/bsh/src/subagent.rs holds the shared registry. Each run records:

- id, root session, direct parent, depth, title, prompt;
- Running/Done/Failed;
- start/finish time;
- prompt and completion tokens from the child's last usage report;
- result or error;
- the child's full flattened transcript (user, assistant and tool rows).
  Adjacent identical assistant rows are collapsed defensively; the engine also
  records exactly one assistant row per final answer.

The header reads only the runs belonging to the active session. A run remains in
the registry after its parent switches away.

## Persistence

A child is saved exactly like a parent conversation: a Session JSON document and
the same Parquet transcript log under the workspace's session directory. The
document carries a SubagentMeta block (root, direct parent, depth, status,
prompt, token counts, timestamps, result or error); the log carries the full
transcript and the tool rows the trajectory reads.

Child Sessions are hidden from the left pane; the subagent selector is their
only entry point. On startup the store is read normally, each child Session is
rehydrated, its
transcript is flattened back into the menu registry, and the menu is restored
with the same titles, counts, statuses and transcripts. Run ids are UUIDs, so a
restart cannot collide with an earlier child's file.

A follow-up uses the live child Agent while it exists. After a restart the
registry restores only the transcript, so the first prompt rebuilds the child
from its saved Session (see the rebuild note above). The registry no longer
evicts settled runs, so a child stays continuable for the life of the process.

## UI (DSH mirror)

The header carries the SubagentHeaderLineage trigger:

- 14x14 activity slot (spinner while running, dot when settled);
- the selected child's title when a child is open;
- a count label such as "25 subagents" (or "1 subagent") while the parent is
  showing;
- 12px chevron, min-height 28, radius 8, gap 4, title capped at 244px.

The trigger is a selector over the session's child runs, not a run itself. It
never renders a synthetic "Subagents" run name and never the root conversation.
The selected child also appears in the title breadcrumb and as the 600-weight
menu row.

Its menu is 336px wide (max 400, max height 560) and lists only child runs:
top-level children start flush left, grandchildren indent by 16px. Each child
row is min-height 44 with:

- activity state;
- title (weight 600 for the current run);
- prompt summary at 10px;
- a right-aligned metrics column: tokens on row one, duration on row two.

Clicking a child selects it. The title row's parent/child breadcrumb returns to
the conversation.

The main column is data-driven: selecting a child materializes its transcript as
a transient Session (the same model a normal conversation uses) and that Session
becomes the active view model. Chat and Trajectory are the existing renderers,
unchanged: the child gets the same user bubbles, markdown, reasoning blocks, tool
cards, turn rail and trajectory rows as the parent.

There is no subagent-specific transcript component. SubagentView holds the run
id, the parent title and the Session; active_session returns that Session, and
the title row adds a parent/child breadcrumb. The normal composer is kept, so the
reader can continue the child. Send routes the draft to the selected child, not
to the parent; the child's answer is appended and the view stays on the child.
Switching sessions clears the child view.

The child react call runs on the Runner's Tokio runtime, never on GPUI's
executor: the engine awaits a Tokio timeout for the LLM call, and GPUI's
executor has no Tokio reactor. The UI task only awaits the runner JoinHandle.

The child react call runs on the Runner's Tokio runtime, never on GPUI's
executor: the engine awaits a Tokio timeout for the LLM call, and GPUI's
executor has no Tokio reactor. The UI task only awaits the runner JoinHandle.

## Limits

- The child transcript is streamed live (text, tool calls, tool results and
  usage) and the settled Agent session is the authority; reasoning chunks are
  not part of the persisted ChatMessage model.
- Child sessions are not listed in the left pane, by design. The child runs
  inside the parent turn; a follow-up is a new child react call, not a steer
  into a running parent turn.
- A child turn is TEXT: the child's composer accepts a prompt, while attachments
  and the slash commands (including `/compact`) stay the parent conversation's.
- A child can be continued for the life of the process, and after a restart the
  first prompt rebuilds it from the saved Session. The rebuilt child is a new
  process: its pre-restart tool arguments are replayed as capped history rows,
  so a tool call whose result was truncated stays truncated.
- Token figures update from stream Usage events; while none has arrived the
  header shows a rough estimate (~prompt/4 + 2k).
- A run is painted by a 100ms child clock, not the 1s jobs clock. Settled runs
  are not evicted.
