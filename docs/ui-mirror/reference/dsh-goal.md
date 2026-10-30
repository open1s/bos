# The reference goal feature, measured (round 126; model completion tools added round 127)

Source: the shipped asar at
/Applications/DeepSeek Harness.app/Contents/Resources/app.asar. The goal
packages were extracted with the recipe in crates/bsh/AGENTS.md (header JSON at
offset 16, data base 8 + pickle size). The extraction is reproducible with
python3 and lives only under /tmp during a round; the paths below are the
package paths inside the asar.

| Package | Role |
|---|---|
| @deepseek-ai/dsh-command-goal | the human /goal slash command |
| @deepseek-ai/dsh-goal | durable goal domain, phases, activation, CAS |
| @deepseek-ai/dsh-goal-round-driver | the automatic continuation prompt and gates |
| @deepseek-ai/dsh-tool-goal | model-facing get_goal / create_goal / update_goal |
| @deepseek-ai/dsh-client-ui-goal | the GoalBar strip, its dictionary, edit form |

## Command grammar (dsh-command-goal/lib/index.js)

The parser trims its input, then:

- empty -> show
- lowercased exactly clear / pause / resume -> that verb
- lowercased exactly edit -> invalid-edit
- /^edit(?=\s)/iu -> edit, objective = input.slice(4).trim()
- anything else -> create, objective = the trimmed input

So control words count only when they fill the whole argument; every other
non-empty suffix is an objective. Unknown words are objectives, not errors.

USAGE = "Usage: /goal [<objective>|clear|edit <objective>|pause|resume]".

## Direct output (verbatim template)

renderGoal(title, goal):

    <title>
    Status: <phase>
    [Blocker: <code>: <message>]        (blocked only)
    Objective: <objective>
    Rounds: <roundsStarted>/<maxGoalRounds>
    Activation: <armed|disarmed>
    <blank>
    Commands: <hint>

Titles: show "Goal", create "Goal created", edit "Goal updated", pause
"Goal paused", resume "Goal resumed". clear returns only "Goal cleared.". A
bare /goal with no goal returns "No goal is currently set.\n" + USAGE. clear
with no goal returns "No goal to clear.".

Command hints by live state:

| phase / activation | hint |
|---|---|
| active, armed | /goal edit <objective>, /goal pause, /goal clear |
| active, disarmed | /goal edit <objective>, /goal resume, /goal clear |
| paused | /goal edit <objective>, /goal resume, /goal clear |
| blocked | /goal edit <objective>, /goal resume, /goal clear |
| complete | /goal <objective>, /goal clear |

## State rules (dsh-goal/lib/index.js)

- Four durable phases: active, paused, blocked, complete. One current goal per
  session. defaultMaxGoalRounds = 256.
- create: only when no goal or the current phase is complete. New identity,
  rounds 0, armed.
- edit: objective and/or cap in place, phase/rounds/activation unchanged. A
  COMPLETED goal is replaced with a fresh identity (the command reports
  "Goal created"). No current goal -> GOAL_NOT_FOUND.
- pause: active only -> paused + disarmed. Every other phase is
  GOAL_INVALID_TRANSITION.
- resume: active, paused or blocked; refuses an active ARMED goal; refuses an
  exhausted cap (roundsStarted >= maxGoalRounds); sets active + armed and
  clears the blocker reason.
- complete: any phase but complete -> complete + disarmed + reason cleared.
- block: active only, records a lower-kebab-case code + non-empty message,
  disarms.
- clear: any current goal -> removed; a revisioned tombstone stays in the log.
- Activation is process-local and never persisted. Every agent/created edge
  disarms; a fresh cache starts disarmed. resolveObjective trims; empty is
  GOAL_INVALID_OBJECTIVE.

## Refusals used by the command

- create over a non-complete goal:
  "A goal is already <phase>. Use /goal edit <objective> to change it or /goal clear before replacing it."
- bare edit:
  "Goal editing requires a replacement objective.\n" + USAGE.
- missing goal for edit/pause/resume:
  "No goal is currently set; /goal <verb> requires one. " + USAGE.
- any GoalError (invalid transition, already armed, exhausted cap, empty edit):
  "The goal command is not valid for the current state. Run /goal to view available commands."
- attachments on a non-objective verb:
  "Attachments only accompany a goal objective: /goal <objective> or /goal edit <objective>."
- attachments on a successful create/edit: one user followup with the admitted
  blocks plus the fixed text "Reference attachments for the goal objective."

## GoalBar (dsh-client-ui-goal/lib/client.js)

GoalBar.module.css, measured:

- bar: height 36, radius md, padding 4 5 4 12, gap 10, no border, panel
  elevation, specific-menu fill; label 13/24 weight 500 in label-primary;
  objective 13/20 ellipsised in label-primary-dimmed; glyph 14 in
  label-tertiary; actions gap 10; icon button 28x28, radius 999, 14 glyph,
  label-tertiary, hover interactive-bg-hover + label-secondary, disabled 0.4.
- loading (undefined), no goal (null), and complete render NOTHING.
- phase labels (en / zh): Ongoing Goal / 进行中的目标, Inactive Goal /
  已暂停的目标, Paused Goal / 已暂停的目标, Blocked Goal / 受阻的目标.
- actions: pause only if active AND armed; resume if paused OR (active AND
  disarmed); edit always; clear always. A BLOCKED bar therefore offers edit and
  clear only, although the /goal command can resume it.
- blocked reason is the bar's title attribute.
- edit swaps the bar for an inline editor (save disabled on an empty draft);
  action errors render inline as "<message> (<code>)".

## Round driver (dsh-goal-round-driver/lib/index.js)

Exact retained prompt, one text block:

    <goal_round>
    Objective: <JSON.stringify(objective)>
    Round: <n>/<maxGoalRounds>

    Continue working toward the objective in this same session. Treat the current workspace, tool results, and durable session state as authoritative; inspect them instead of assuming earlier narration is still current. Make concrete progress and verify the result. Before claiming completion, gather evidence that the whole objective is achieved, read the current goal, and mark it complete. If work remains, leave the goal active for the next round. Follow the configured goal-tool policy before reporting a blocker.
    </goal_round>

At the cap it records a blocker with code round-limit and message
"Goal reached its configured limit of <max> rounds." A failed/cancelled round
disarms continuation; it does not change the durable phase. A round starts only
at idle, armed and with room.

## bsh parity

| Reference behaviour | bsh now | Status |
|---|---|---|
| grammar: empty/clear/pause/resume/edit/objective, case-insensitive whole-word controls | same parser in state.rs::parse_goal_request | mirrored |
| exact report lines, order and blank line | goal_report() + locale keys | mirrored (en byte-exact) |
| titles show/created/updated/paused/resumed, clear "Goal cleared." | same | mirrored |
| no goal show, no goal to clear, already-exists, missing verb, invalid edit, invalid transition, attachments-only | same wording | mirrored (en) |
| phases active/paused/blocked/complete, cap 256, rounds | Goal::GoalState + max_rounds u32 + DEFAULT_MAX_ROUNDS | mirrored |
| create refuses unfinished, replaces complete, arms | AppState::create_goal | mirrored |
| edit keeps phase/rounds/activation, replaced when complete | AppState::edit_goal | mirrored |
| pause active only, disarms | AppState::pause_goal / Goal::pause | mirrored |
| resume active-disarmed/paused/blocked, refuses armed and exhausted, clears reason | AppState::resume_goal / Goal::resume | mirrored |
| complete/block transitions, blocker code + message | Goal::complete / Goal::block / AppState::block_goal | mirrored in the model; no command verb exists in the reference either |
| activation process-local, reload disarmed | Goal.activation #[serde(skip)] | mirrored: store load is bsh's session-start edge |
| attachments only on create/edit; one user message with fixed text | run_goal_line + append_goal_attachments + message_content | mirrored for image attachments; bsh has no file-block attachment type |
| command output never enters model requests | UI notice only | mirrored |
| GoalBar labels/actions per phase+activation; complete hidden; blocked shown | header::goal_row + goal_bar_label_key | mirrored (icons, 36/12/4-5-4-12/gap 10) |
| blocked reason as bar title | tooltip with reason.message | mirrored |
| inline edit disabled on empty, never clears | commit_note | mirrored |
| round prompt exact, cap blocker round-limit, idle-only, failed round disarms | goal_round_input / continue_goal / disarm_goal_on_error | mirrored |
| round retained as a goal-sourced user message in history ("Continuing goal" trigger) | bsh passes the identical prompt as the turn input and stores no user row | deliberately different: bsh has no goal-source transcript row; storing the block would render raw XML. Later rounds do not resend earlier round prompts. |
| get_goal/create_goal/update_goal model tools | get_goal + update_goal (actions complete/blocked) mounted on any turn whose session has a goal; a recorded transition settles onto the durable goal when the turn ends | mirrored for the two settlings an automatic round may make, which is what the round prompt asks for; create_goal, edit, pause and resume stay operator-side (the /goal command and the goal bar own them), and there is no CAS because the shell is the only writer |
| model completion ends the continuation loop | the pump settles the recorded transition BEFORE continue_goal decides, so a completed or blocked goal starts no round n+1; a failed round still disarms | mirrored: the dormant-completion gap this table recorded is closed |
| CAS id/revision, tombstones, goal/change events | none | deliberately different: one UI thread mutates through AppState; the command never exposes ids/revisions, so no CAS is needed |
| specific-menu fill, exact hover colour shift, per-field help aria | page fill + hover background; a11y not exposed by GPUI | stylesheet approximation |
