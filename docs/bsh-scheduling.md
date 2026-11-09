# Scheduling: automation tasks the reader and the model share

An automation task is a prompt plus a cadence. The scheduler sends that prompt
to a session when the cadence comes due, and the turn that follows is an
ordinary turn: same preset, same tools, same policy. Federation, MCP, skills
and the shell therefore reach a scheduled task exactly as they reach a typed
message - there is no second kind of turn.

## Where a task lives

Tasks live in the machine settings document (`~/.bos/bsh/settings.toml`,
`[[schedules]]`), which the Automation tasks pane edits and the shell clock
reads. The pane edits a DRAFT and writes it on Save; the model's tools go
through the shell as well (see below), so both end in the same list.

```toml
[[schedules]]
id = "task-1762480000"
name = "Peer audit"
prompt = "Check federation_status and list any peer that is unreachable."
session_id = "ses-3f2a"
every_seconds = 3600      # 0 means one shot
enabled = true
last_run = 0
```

The cadence vocabulary is the "every N" family: `every_seconds = 0` runs once
and turns the task off; a positive value repeats. Daily/weekly/cron rules need
date and time-zone arithmetic this shell does not do yet, so they are not
drawn and not accepted by the model's tools either.

## The model's tools

| Tool | What it does |
|---|---|
| `schedule_create` | Queue a task: `prompt` (required), `name`, `every_seconds`, `session_id`. Returns the minted `id`. |
| `schedule_list` | Every saved task with `enabled`, `last_run`, `next_run`, plus the requests still pending from this turn. |
| `schedule_edit` | Change `name`, `prompt`, `every_seconds`, or `enabled` (false pauses, true resumes/re-arms). |
| `schedule_delete` | Remove a task by id. |

Rules worth knowing:

- A task always belongs to the session that created it. `session_id` is
  accepted only when it names that session; a model cannot park work in a
  conversation the reader is not looking at.
- The tools are attached to a writable turn (Read Only never schedules) and
  each answers to the reader's own per-tool switch, so a switch in the Tools
  pane removes exactly that tool.
- They mount through a `schedule` bundle: the shipped `standard`, `ptc` and
  `creator` presets list it, and a custom preset opts in by adding
  `schedule` to its `plugins`. Turning the bundle off in Settings >
  Extensions removes all four at once.
- The id is minted by the tool (`task-<unix>-<n>`) and is refused if a task
  already carries it, so two tasks created in the same second cannot shadow
  each other.

## Why a queue, and when a task lands

A turn is built from a settings SNAPSHOT, so a tool cannot write the settings
document: it would either fork a second source of truth or race the pane's own
save. The tools therefore validate their fields and push a
`ScheduleRequest` onto `crate::schedule`'s queue; the shell drains it

- at the end of the turn that made the request, and
- on the 30-second scheduler tick (before it decides what is due).

One `save_global()` covers the batch. `schedule_list` reports both the saved
list and the pending requests, so a create followed by a list in the same turn
does not look like a lost write.

## Firing

The shell checks every 30 seconds while the window is open. A task that is due
starts a turn in its own session; if that session is already streaming, the
task is skipped for this tick rather than opening a second stream over the same
transcript. A one-shot task marks itself off after it runs, so a restart does
not run it twice.

Consequences worth stating:

- **The app must be running.** The clock is the open window's, not a daemon; a
  closed app fires nothing and catches up on the next tick after launch.
- **A task is not a job.** Deleting a task while its turn streams removes the
  next firing, not the running turn; stop that turn from the composer.
- **A task can schedule nothing.** The scheduled turn gets the same tools as
  any other turn, so it can create further tasks - but it cannot widen the
  permission matrix or the preset it runs under.

## Federation, scheduled

The shipped `standard`, `ptc` and `creator` presets mount the `federation`
bundle, so their turns carry `federation_status`, `federation_peers`,
`federation_delegate`, `federation_cancel`, `federation_query` and
`federation_cleanup`. Only `federation_status` rides every request: it is
the signpost that tells the model the family exists. The other five are
deferred like the browser, so a turn that needs a peer calls `find_tool` and
then the reported tool directly (the prompt's discovery paragraph says so), and every other
turn pays only the small status schema. A read-only turn keeps the three
queries and loses the three actions.

A task whose prompt asks for a peer check runs those tools in a normal turn,
and its result folds into the session like any other answer.

A literal "peer audit every hour" is therefore one `schedule_create`:

```json
{
  "prompt": "Call federation_status and federation_peers. Report each peer that is unreachable and the last error; do not delegate anything.",
  "name": "Peer audit",
  "every_seconds": 3600
}
```

Set `federation.enabled = false` (Settings > Federation, or the settings file)
to run without a node: the bundle leaves the build, and a scheduled prompt
that names a federation tool runs without it. A custom preset opts in by adding
`federation` to its `plugins` list.
