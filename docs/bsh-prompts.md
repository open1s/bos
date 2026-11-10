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

A prompt cannot enumerate every case, and a list of virtues ("infer the goal",
"think briefly", "be careful") does not change a choice. The strategy here is a
decision procedure with five questions, in the order a turn runs:

1. **What is being asked?** Judge the verb, not the punctuation: a question, a
   defect report, an instruction, or a goal with no target.
2. **Is it concrete enough to act on?** Name the object, the change and the
   done condition. If neither the request nor the workspace supplies them, the
   request is too vague; ask, do not guess.
3. **Which instruction wins?** The reader's latest explicit instruction, a
   correction they accepted, workspace instructions and loaded skills, then the
   contract.
4. **Act or ask?** When the request is concrete, act if a wrong guess is cheap
   to undo; ask if it would destroy something or cost substantial rework.
5. **When is the turn done?** A question is answered; a change is made and
   verified. Then stop - no refactor, no commit, no narration.

Everything else in the corpus serves one of those five.

## 2. Assembly order

`compose_system_prompt` joins the first three layers; the BSH build appends
the rest, and `Agent::system_prompt_for` writes the discovery block ahead of
all of them. Order is deterministic, so a request is reproducible and the
KV-cache prefix stays stable.

Discovery is layer 0, not a tail paragraph: a withheld tool or a searched skill
is unreachable until the model knows to look, and a paragraph appended after the
whole contract arrives after the first action has been chosen. The block keeps
the capability gate - it is written only for a search this agent really carries -
and it is the first thing in the system prompt, under
`## Check what you were given first`, whose line then names the searches the
request actually carries (`find_tool`, `find_skill`, or both). The heading is
derived from the request's tool array, not from a second guess at the same
policy, so it can neither promise a search the request dropped nor hide one it
kept.

| # | Layer | Constant | Owner | When |
|---|-------|----------|-------|------|
| 0 | Discovery priority | `DISCOVERY_PRIORITY` + `DISCOVERY_GUIDANCE` / `SKILL_DISCOVERY_GUIDANCE` | `agent` | when a tool is withheld or a skill is searched |
| 1 | Harness identity | `HARNESS_IDENTITY` | `agent` | always |
| 2 | Persona slot | `DEFAULT_PERSONA` or the workspace/preset persona | `agent` | always (unless `complete`) |
| 3 | Decision procedure | `WORKFLOW_GUIDANCE` | `agent` | always (unless `complete`) |
| 4 | Product context | `HARNESS_CONTEXT` | `bsh` | when the persona is not `complete` |
| 5 | Tool sections | `agent::prompts::TOOL_GUIDANCE` then `bsh::prompts::TOOL_GUIDANCE` | both | only for registered tools |
| 6 | Per-turn plan directive | `PLAN_DIRECTIVE` | `bsh` | plan mode only |
| 7 | Workspace instructions | `instructions::render` | `bsh` | every turn, framed as a user-role reminder |
| 8 | Runtime snapshot | `context::render_snapshot` | `bsh` | every turn, first user message |

A subagent child inherits the parent's layers, drops the delegation tail
(`## Delegating with subagents`) because a leaf cannot delegate again, and
gains a prefix that names its reporting contract: the parent cannot see the
child's transcript, so the result must carry the evidence the parent needs.

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
- **Verification is part of the task.** Find the project's real lint and
  typecheck commands and run them before claiming completion.

BSH adopts the proactiveness boundary. It keeps the "ask" instinct but cannot
copy the mechanism: BSH has no `ask_user_question` tool, so a question is a
reply that ends the turn. The contract therefore makes the question the last
resort and requires a default with it. Plan mode is the structured version of
the same path: the reader approves a concrete plan before work starts.

### DeepSeek Harness: intent as capability-gated context

DSH is the smallest static prompt of the three. It sends an identity, a
persona, short per-tool sections and runtime context; it does not carry a long
workflow block. What makes it precise:

- **Sections are capability-gated.** Each `dsh-tool-*` package gates its
  section on `ctx.tools.get(...)`; `SECTION_ORDERS` only fixes placement, so
  the prompt cannot name a tool the request does not carry.
- **Runtime context is a snapshot.** Working directory, sandbox policy,
  approval policy, subagent depth and time arrive as context that "supersedes
  earlier runtime-context snapshots" - facts, not prose the model has to infer.
- **Questions have semantics.** `ask_user_question` asks "brief, direct,
  self-contained questions about missing information, preferences, or
  decisions"; a skipped question means "continue without it, do not ask it
  again, and do not treat the skip as permission".
- **Tool rules live with the tool.** "Use the glob tool - not shell find",
  "check the [exit code: N] marker", "do not busy-poll or sleep" - one sentence
  where the model needs it, not a general essay.

BSH adopts capability-gating (its `append_tool_guidance` is the same idea), the
"one sentence at the point of use" rule, and the question semantics. It cannot
adopt runtime interpolation: BSH sends one static system prompt for the build,
so per-turn facts arrive as user-role context instead. BSH's runtime snapshot
already carries cwd, the workspace layout, sandbox, permission and preset.

### Why the first BSH revision was too long

The first revision of this contract was 3,001 bytes of well-written rules -
"infer the goal", "think briefly", "plan once", "ask only for what you cannot
discover". It read better than the old seven-point list and changed less than
it should have. Three failure modes:

- **Virtues instead of decisions.** "Infer the goal" tells the model what to
  value, not what to do when a request is a question, a defect, or a goal with
  no target. A clause has to resolve a choice.
- **No precedence.** Six intent bullets and seven action bullets left the
  order between the reader's latest instruction, an accepted correction,
  workspace instructions and the contract implicit. Models resolve conflicts
  with whichever rule is most recent in the prompt.
- **No stop condition.** "Work until done" plus "stay in scope" does not say
  when done is. The model kept polishing or added a summary the reader did not
  ask for.
- **No concreteness gate.** "Ask when vague" was a feeling; nothing defined
  vague, so a request like "make it better" had no decision attached to it.

The current contract is 2,485 bytes: 17% shorter than that revision, with the
concreteness gate and stop condition it lacked. Design rules for a clause:

1. It must name a choice or a contrast ("A question ends without an edit"),
   never a virtue ("Be careful", "Think deeply").
2. Precedence beats repetition: state the order once, then stop repeating it
   in every bullet.
3. If the runtime or a tool already enforces the fact, cut the clause. cwd,
   sandbox and the workspace layout are in the snapshot; file-tool behaviour is
   in the tool sections.
4. One concrete example per ambiguous case, not a taxonomy of cases.
5. Vagueness is a missing slot, not a feeling: name the slots (object, change,
   done condition) and the shape of the question that fills them.
6. Every byte rides on every step, so a clause that does not change a decision
   is a per-request cost with no benefit.

## 4. The decisions BSH ships

| Decision | Clause | Why it is there |
|---|---|---|
| What kind of request is this? | "Judge the verb, not the punctuation: 'why does X fail?' asks for an explanation, 'can you fix X?' asks for a change. A defect report is reproduced, fixed and verified; a terse instruction with a file open means change that code; a goal with no target ('make it faster') is measured before it is changed." | The four readings that most often pick the wrong first action. |
| Is it concrete enough? | "Name the object (what file, screen or behavior), the change (what about it) and the done condition (what proves it) before you act. If the request and the workspace cannot supply them, it is too vague to act on: ask one short question, in the reader's terms, that offers the two or three readings you can see, best first, and says what you will do by default; then stop." | Vagueness is missing information, not a tone; the question must be answerable in one line, with options. |
| What shape does the answer take? | "A question ends without an edit; a change request ends with the change made, not instructions for making it." | The two symmetric failures: editing on a question, and explaining instead of doing. |
| Which instruction wins? | "When instructions conflict, obey the reader's latest explicit instruction, then a correction they accepted, then workspace instructions and loaded skills, then this contract." | A general rule never overrides a specific request; a correction outranks the original, and a skill ranks with the workspace. |
| Act or ask? | "If the request is concrete but two readings remain, take the one that acts on the workspace when a wrong guess is cheap to undo; ask when it would delete, publish or spend something the reader owns, or when a wrong guess means substantial rework." | Cheap ambiguity should not cost a turn; destructive or wasteful ambiguity should not be guessed. |
| When is a question allowed? | "Never ask for a fact the workspace can answer, and never ask twice for what the session already authorized." | Removes the two question classes that waste the reader's time. |
| What does a later message mean? | "A later message corrects the current task; it replaces the task only when it cancels it or asks for something incompatible." | Steering, not silent replacement. |
| How does work start? | "Start now once the request is concrete: batch independent calls, and never spend a turn narrating the next step." | The narration failure costs a whole turn; the concreteness gate comes first. |
| How large is a change? | "Read before you edit; make the smallest change that satisfies the request; follow the workspace's existing patterns." | Convention drift and scope creep are the common review rejections. |
| How is done proved? | "Prove it with the command that checks it, or say what you could not check. After two identical failures, stop and report the error and what must change in the environment." | Evidence closes the turn; a retry loop is not progress, and a repeated failure is usually an environment fact. |
| When does the turn end? | "Stop when the request is answered or the change is verified. Do not refactor, commit, push or open a pull request unless asked." | The stop condition the first revision lacked. |
| How is it reported? | "Outcome first, then evidence, then anything unverified. No preamble or process narration." | The final message must stand alone. |
| What does it sound like? | "Plain words, concrete nouns, file paths the reader can open; no filler or praise." | Filler hides the result. |
| How much? | "Match the reader's language; brief by default, thorough when the decision needs it." | Length is a property of the decision, not a fixed cap. |

The BSH product context adds the facts the code cannot reveal:

- the conversation carries the intent, the workspace carries the evidence;
- a screenshot is evidence, not a task, and a UI reference must be classified
  as a bug report or a code change before acting;
- a message during a turn is queued for the next turn, not injected;
- the Stop button is a decision, not a failure to retry;
- `.bsh/` is internal and denied;
- a terse request in an open workspace is work in the code; say which reading
  you take when it is equally likely to be a question;

Plan mode is the one mode that overrides the start-now rule, and it says so in
the directive: inspect and propose first, then wait for the reader's approval.

## 5. Deliberate omissions

- **OpenCode's "fewer than 4 lines" rule.** BSH answers often need a diff
  summary and evidence; a hard line cap would truncate the part the reader
  needs. The contract makes length follow the decision.
- **Codex's "do not add or run tests unless asked".** This workspace requires
  verification; the contract says run the command that proves the change.
- **Codex's absolute "do not ask permission".** BSH has real sandbox approvals
  and a deny list. The contract governs the conversational question, not the
  host's approval flow.
- **DSH runtime interpolation.** The BSH prompt is one static string per build.
  Per-turn facts (directory, layout, sandbox, plan mode, instructions) arrive as
  context or directives, and the prompt never pretends to interpolate them.
- **OpenCode's question tool.** BSH has no `ask_user_question`; the contract
  says how to ask in a reply instead of naming a tool that does not exist.

## 6. Budget

The system prompt rides on every step of every turn, so its size is a
per-request cost. Measured bytes:

| Layer | Bytes |
|---|---|
| `DEFAULT_SYSTEM_PROMPT` (identity + persona + contract) | 2,627 |
| `HARNESS_CONTEXT` | 1,421 |
| Tool sections (bare build) | 5,143 |
| Built prompt | 9,191 |

The first contract revision measured 3,001 bytes and a 9,909-byte built prompt;
this one is 17% shorter in the contract and 7.2% shorter overall, with the
concreteness gate, precedence, question and stop rules it lacked.

`the_system_prompt_stays_within_its_budget` in `crates/bsh/src/caps.rs` caps
the built prompt at 12,000 bytes; `the_framework_prompt_stays_small` caps the
framework default at 6,000. The skill catalogue is deliberately not in the
prompt: it is ranked against the request instead.

The discovery block is extra when a search is carried: the heading base
`DISCOVERY_PRIORITY` (34 bytes) plus the name it appends (15 for one search,
28 for "with find_tool or find_skill"), `DISCOVERY_GUIDANCE` (617) and
`SKILL_DISCOVERY_GUIDANCE` (274) - 957 bytes with both, written first and
gated on the search that makes it true.

## 7. Scenario checks

The contract is only as good as the first move it produces. These are the
scenarios a prompt change is checked against; "first move" means the first tool
call or the reply, before any correction from the reader.

| Reader message | Context | First move | Clause that decides it |
|---|---|---|---|
| "why does login fail?" | repo open | read the auth path and answer; no edit | Judge the verb; a question ends without an edit |
| "login is broken" | repo open | reproduce, fix, verify | A defect report is reproduced, fixed and verified |
| "can you fix the retry?" | repo open | read the call site, change it, test it | "can you fix X?" asks for a change |
| "make it faster" | no metric | measure or profile first, then change | A goal with no target is measured first |
| "make it better" | file open | ask: faster, simpler or more correct, and which screen or file; offer those options, best first | Name the object, change and done condition before acting |
| "fix the app" | no file, no symptom | ask which screen and what happened; do not open a random file | Too vague to act on; ask, do not guess |
| "fix this" + button screenshot | app | ask: bug report or code change? | Product context: a screenshot is evidence, not a task |
| "actually keep the old API" | mid-turn | fold the constraint into the current task | A later message corrects the current task |
| "is this file used?" | repo open | grep, read the callers, answer | A question ends without an edit |
| "drop the users table" | repo open | inspect the target, then ask before executing | Expensive ambiguity asks one question |
| "what is the test command?" | repo open | read the README or justfile; do not ask | Never ask for a fact the workspace can answer |
| "reduce latency" | product tradeoff | measure; ask only if the target belongs to the reader | Concrete readings act; destructive or wasteful ones ask |
| "can you deploy this?" | no deploy tool in the list | call `find_tool` with "deploy", then call what it reports | Discovery is the first priority: check what you were given before concluding it is missing |
| any task a specialist procedure covers | skills are searched | call `find_skill`, then `load_skill` for the match | The skill paragraph rides the same priority block |

A scenario that produces the wrong first move is a prompt bug even when the
final answer is right: the reader paid for the wrong step.

## 8. Changing a prompt clause

1. Change the constant, never a rendered copy. `DEFAULT_SYSTEM_PROMPT` is a
   literal of the three layers; regenerate it when the workflow changes, or
   `default_prompt_is_the_composition` fails.
2. Keep the framework block tool-name-free. A preset can remove a tool, and a
   rule that names it becomes a lie. Tool-specific rules belong in
   `TOOL_GUIDANCE`, appended only when the tool is registered.
3. Add or update the test that pins the clause. A deleted line is a changed
   instruction, not a cosmetic diff.
4. Apply the five design rules in section 3 before adding a clause. A rule
   that does not resolve a choice is prose, and prose is charged every step.
5. Re-measure the budget and update the table above.
6. If a clause was copied from a reference, record the exact source here. If it
   states a BSH fact, say which fact and where it is enforced.

## 9. Sources

- Codex: `openai/codex`, `codex-rs/core/prompt.md` and
  `gpt_5_codex_prompt.md`; measured from the desktop build's base
  instructions (`/Applications/ChatGPT.app/Contents/Resources/codex-cli` and
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
  `crates/bsh/src/prompts.rs`, `crates/bsh/src/instructions.rs`,
  `crates/bsh/src/context.rs`.
