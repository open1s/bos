# BSH prompt strategy

What the model is told, why each clause exists, where it came from, and what it
costs. The code is the source of truth; this document records the decisions
behind it so a later edit changes behaviour on purpose instead of by accident.

Measured 2026-11-09 against the installed DeepSeek Harness, Codex and OpenCode
builds. The prompt constants are pinned by tests in
`crates/agent/src/agent/prompts.rs` and `crates/bsh/src/prompts.rs`; the
build-level budget and composition are pinned in `crates/bsh/src/caps.rs`.

## 1. The problem the prompt solves

A reader rarely states a complete specification. "Fix this", "why is it slow",
a screenshot of a button, a terse sentence in a workspace with an open file -
each is an intent with missing pieces. The agent has to recover the missing
pieces from the conversation and the workspace, decide how much to ask, and
still finish the job in one turn when it can.

A prompt cannot enumerate every case. It can make the right reading the
default. The strategy here is four defaults, in the order a turn runs:

1. **Read the intent before choosing an action.** Infer the goal, not the
   literal words; inspect before asking; ask only for a decision that belongs
   to the reader.
2. **Act on that reading.** Batch independent tool calls, keep changes small,
   and never spend a turn narrating a step that could have been taken.
3. **Verify with evidence.** Run the command that proves the change, or say
   plainly what could not be checked.
4. **Report the outcome, not the process.** Lead with the result, name the
   files, state the evidence and the gaps.

Everything else in the corpus serves one of those four.

## 2. Assembly order

`compose_system_prompt` joins the first three layers; the BSH build appends
the rest. Order is deterministic, so a request is reproducible and the KV-cache
prefix stays stable.

| # | Layer | Constant | Owner | When |
|---|-------|----------|-------|------|
| 1 | Harness identity | `HARNESS_IDENTITY` | `agent` | always |
| 2 | Persona slot | `DEFAULT_PERSONA` or the workspace/preset persona | `agent` | always (unless `complete`) |
| 3 | Operating contract | `WORKFLOW_GUIDANCE` | `agent` | always (unless `complete`) |
| 4 | Product context | `HARNESS_CONTEXT` | `bsh` | when the persona is not `complete` |
| 5 | Tool sections | `agent::prompts::TOOL_GUIDANCE` then `bsh::prompts::TOOL_GUIDANCE` | both | only for registered tools |
| 6 | Per-turn plan directive | `PLAN_DIRECTIVE` | `bsh` | plan mode only |
| 7 | Workspace instructions | `instructions::render` | `bsh` | every turn, framed as a user-role reminder |

A `complete: true` persona replaces layers 1-5 with exactly the persona text
(the DeepSeek Harness arm). It is deliberate, off by default, and still scanned
for stale tool names: the escape hatch changes what is sent, not what is true.

## 3. What the reference harnesses do

### Codex: intent as a completion contract

Codex's prompt is the strongest of the three on intent, and it buys that with
length (about 18 KB in the measured desktop build). Its clauses:

- **Completion, not a plausible reply.** "You and the user share one workspace,
  and your job is to collaborate with them until their intended goal is
  completely handled." It tells the model to "infer the user's intent and task
  scope from the instructions and prior conversation context" and to "bias
  towards action and carry the user's intended task to completion", and it
  refuses a "partial or 'helpful enough' solution".
- **Ambiguity does not stop work.** "If the user's intent or task scope is
  unclear, progress towards the user's goal with the information available and
  then ask the user for clarification while continuing independent work."
- **Permission is economical.** "User authorization and preferences persist
  across turns. Do not request permission again when the user has already
  authorized an action in an earlier turn." Work is completed "so that user
  approval is the final step", approving "a concrete, reviewable result" - and
  the prompt says outright that stopping to ask frustrates the user.
- **Steering is explicit.** A message during a turn is "steering the active
  task rather than replacing it"; status questions are answered briefly and the
  task resumes; replacement needs an explicit cancel or an incompatible goal.
- **Compaction is not an ending.** Work spanning compactions is "one logical
  chain of events"; do not restart or redo completed work.

BSH adopts the completion contract, the ambiguity rule, the permission
economics and the steering rule. It does not copy Codex's length or its
"Do not add or run tests unless the user asks you to test or verify
implementation" rule, which conflicts with this repo's verification
requirement.

### OpenCode: intent as a conversational stance

OpenCode's prompt is mid-sized (under 10 KB) and spends its words on stance:

- **Proactiveness is bounded by the request.** "You are allowed to be proactive,
  but only when the user asks you to do something"; when the user asks how to
  approach something, "answer their question first, and not immediately jump
  into taking actions."
- **Vagueness is a reason to ask.** "When the user's request is vague, use the
  question tool to clarify before reading files or making changes."
- **Tool discipline is explicit.** Run independent calls in parallel, dependent
  calls in sequence, and do not repeat a call with the same arguments once it
  produced a useful result.
- **Verification is part of the task.** Find the project's real lint and
  typecheck commands and run them before claiming completion.

BSH adopts the proactiveness boundary and the parallel/sequential rule. It
keeps the "ask" instinct but cannot copy the mechanism: BSH has no
`ask_user_question` tool, so a question is a reply that ends the turn. The
prompt therefore says to finish independent work first and ask one
self-contained question with a default, rather than asking before reading.

### DeepSeek Harness: intent as capability-gated context

DSH is the smallest static prompt of the three. It sends an identity, a
persona, short per-tool sections and runtime context; it does not carry a long
workflow block. What makes it precise:

- **Sections are capability-gated.** A tool section is registered only when the
  tool is mounted (the `SECTION_ORDERS` table in `dsh-system-prompt`), so the
  prompt cannot name a tool the request does not carry.
- **Runtime context is a snapshot.** Working directory, sandbox policy,
  approval policy, subagent depth and time arrive as context that "supersedes
  earlier runtime-context snapshots" - facts, not prose the model has to infer.
- **Questions have semantics.** `ask_user_question` asks "brief, direct,
  self-contained questions about missing information, preferences, or
  decisions"; a skipped question means "continue without it, do not ask it
  again, and do not treat the skip as permission"; a pending result means the
  user can still answer.
- **Tool rules live with the tool.** "Use the glob tool - not shell find",
  "check the [exit code: N] marker", "do not busy-poll or sleep" - one sentence
  where the model needs it, not a general essay.

BSH adopts capability-gating (its `append_tool_guidance` is the same idea), the
"one sentence at the point of use" rule, and the question semantics. It cannot
adopt runtime interpolation: BSH sends one static system prompt for the build,
so per-turn facts arrive as user-role context instead.

### The comparison that matters

| Behaviour | Codex | OpenCode | DSH | BSH |
|---|---|---|---|---|
| Infer the goal, not the words | yes | partial | via context | yes |
| Ask only when the reader owns the decision | yes (progress first) | yes (before reading) | yes (tool semantics) | yes (work first, then one question) |
| Authorization persists | explicit | implicit | host policy | explicit |
| Mid-turn message steers | explicit | not covered | host/UI | explicit in BSH context |
| Verify with evidence | yes | yes (lint/typecheck) | tool-owned | yes |
| Capability-gated guidance | yes | per-agent | yes | yes |
| Static prompt | no | no | no | yes |

## 4. The clauses BSH ships

Each clause exists because one of the behaviours above fails without it.

| Clause | Why it is there | Pinned by |
|---|---|---|
| "You and the reader share one workspace... carry it to a finished, verified result." | Sets completion, not conversation, as the success condition. | `the_default_prompt_pins_every_workflow_rule` |
| "Infer the goal ... not from the words alone. A terse instruction in a code workspace usually means 'change the code'; a question usually wants an answer, not an edit." | The two most common misreads: answering an instruction, or editing in reply to a question. | same |
| "Act on the obvious reading. Inspect before asking." | Prevents a clarification turn for a fact the workspace can answer. | `the_contract_reads_intent_before_work_and_reporting` |
| "Ask only when ... the reader's and a wrong guess would be expensive ... one short, self-contained question ... which default you would pick." | Keeps questions rare, answerable and actionable; BSH has no question tool, so the reply must carry everything. | same |
| "Do not ask the reader to confirm what the session already authorized ... ask once, when the next step is irreversible and the result is concrete enough to review." | Stops repeated permission turns; makes the approval a review of a result. | same |
| "A message that arrives while you work steers this task ... replace only when the reader cancels it or asks for something incompatible." | Prevents a correction from silently replacing the objective. | same |
| "Workspace instructions and the reader's explicit choices outrank these defaults. A skill applies only when its stated use matches the request." | Sets the precedence order and stops keyword-triggered skill loads. | same |
| "Start now ... batching every independent call ... never send a message that only describes the next step." | The narration failure is the most expensive one: a whole turn with no progress. | `the_default_prompt_pins_every_workflow_rule` |
| "Plan once ... exactly one active ... skip ... for a single trivial step." | Plans are useful for multi-step work and pure overhead for one edit. | same |
| "Read before you change ... follow the workspace's existing functions and patterns ... smallest that solves the task." | Convention drift is the most common review rejection. | same |
| "Verify before you claim done ... after two identical failures, stop and report the exact error." | Evidence closes the turn; a retry loop is not progress. | same |
| "Stay in scope ... do not commit, push or open a pull request unless the reader asks." | Intent is also a boundary: fixing one bug is not licence to refactor, and history is the reader's to write. | same |
| "Lead with the outcome, then the evidence ... no preamble ... plain words and concrete nouns ... name files as paths they can open." | The reader sees the final message; it must stand alone, and filler hides the result. | same |

The BSH product context adds the facts the code cannot reveal:

- the conversation carries the intent, the workspace carries the evidence;
- a screenshot is evidence, not a task, and a UI reference must be classified
  as a bug report or a code change before acting;
- a message during a turn is queued for the next turn, not injected;
- the Stop button is a decision, not a failure to retry;
- `.bsh/` is internal and denied;
- two identical failures end the attempt.

## 5. Deliberate omissions

- **OpenCode's "fewer than 4 lines" rule.** BSH answers often need a diff
  summary and evidence; a hard line cap would truncate the part the reader
  needs. BSH asks for "brief by default, thorough when the decision needs it".
- **Codex's "do not add or run tests unless asked".** This workspace requires
  verification; the contract says run the command that proves the change.
- **Codex's absolute "do not ask permission".** BSH has real sandbox approvals
  and a deny list. The prompt governs the conversational question, not the
  host's approval flow.
- **DSH runtime interpolation.** The BSH prompt is one static string per build.
  Per-turn facts (directory, plan mode, instructions) arrive as context or
  directives, and the prompt never pretends to interpolate them.
- **OpenCode's question tool.** BSH has no `ask_user_question`; the workflow
  says how to ask in a reply instead of naming a tool that does not exist.

## 6. Budget

The system prompt rides on every step of every turn, so its size is a
per-request cost. Measured bytes:

| Layer | Bytes |
|---|---|
| `DEFAULT_SYSTEM_PROMPT` (identity + persona + contract) | 3,143 |
| `HARNESS_CONTEXT` | 1,623 |
| Tool sections (bare build) | 5,143 |
| Built prompt | 9,909 |

`the_system_prompt_stays_within_its_budget` in `crates/bsh/src/caps.rs` caps
the built prompt at 12,000 bytes; `the_framework_prompt_stays_small` caps the
framework default at 6,000. The skill catalogue is deliberately not in the
prompt: it is ranked against the request instead.

## 7. Changing a prompt clause

1. Change the constant, never a rendered copy. `DEFAULT_SYSTEM_PROMPT` is a
   literal of the three layers; regenerate it when the workflow changes, or
   `default_prompt_is_the_composition` fails.
2. Keep the framework block tool-name-free. A preset can remove a tool, and a
   rule that names it becomes a lie. Tool-specific rules belong in
   `TOOL_GUIDANCE`, appended only when the tool is registered.
3. Add or update the test that pins the clause. A deleted line is a changed
   instruction, not a cosmetic diff.
4. Re-measure the budget and update the table above.
5. If a clause was copied from a reference, record the exact source here. If it
   states a BSH fact, say which fact and where it is enforced.

## 8. Sources

- Codex: `openai/codex`, `codex-rs/core/prompt.md` and
  `gpt_5_codex_prompt.md`; measured from the desktop build's base
  instructions (`~/Library/.../ChatGPT.app/Contents/Resources/codex-cli` and
  the rollout records under `~/.codex/sessions`).
- OpenCode: `sst/opencode`, `packages/opencode/src/session/prompt/*.txt`;
  measured from the installed binary at `~/.opencode/bin/opencode` (the
  string begins "You are opencode, an interactive CLI tool that helps users
  with software engineering tasks.").
- DeepSeek Harness: `@deepseek-ai/dsh-system-prompt`,
  `@deepseek-ai/dsh-persona`, and the `dsh-tool-*` packages, extracted from
  `/Applications/DeepSeek Harness.app/Contents/Resources/app.asar` with the
  header parser used by the UI-mirror rounds (`data base = 8 + pickle size`).
- BSH: `crates/agent/src/agent/prompts.rs`,
  `crates/bsh/src/prompts.rs`, `crates/bsh/src/instructions.rs`.
