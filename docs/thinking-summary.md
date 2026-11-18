# Thinking summaries

Status: implemented in `react::thinking` and persisted by `agent::AgentSession`.

The raw reasoning stream is model-private output. It is shown to the reader and
streamed as `StreamToken::ReasoningContent`, but it is not conversation history:
replaying it as a user or assistant turn would invent a message nobody sent.
This feature keeps the useful part of that stream - the decisions, the questions
it left open, and the next action - and feeds it back as a small, explicitly
labelled note on the NEXT provider request.

## 1. Extraction (REQ-1)

`ThinkingState::observe_reasoning` parses the bounded tail of one reasoning
step (the last 16,000 characters) and extracts the following items:

| Item | How it is recognised | Kind / confidence |
|---|---|---|
| Claim | cause/defect markers ("the cause is", "the bug is", "the issue is", "原因是", "根因") | `claim`, `inferred` |
| Intent | plan/action markers ("the fix is", "I will", "let's", "决定", "方案是", "采用") | `intent`, `inferred` |
| Rationale | the clause after " because ", " since ", "因为", "由于" | attached to the decision |
| Open question | a sentence ending in `?`/`？`, or an uncertainty marker ("need to check", "unclear", "待确认", "不确定") | unresolved |
| Next action | "next step:", "next I will", "接下来", "下一步", or a leading imperative ("run ", "check ", "运行", "检查") | tentative |

Only a `claim` can become `verified`, and only when a tool result actually
supports it: the output must match the claim's **effective** polarity, carry
the claim's predicate or a direct antonym, and share enough significant tokens
to be about the same subject. Effective polarity resolves negation around the
predicate: "does not fail" is positive, "does not pass" is negative, and "is
not in the parser" is an absence claim that positive evidence cannot confirm.
A successful tool call is execution, not verification, so an `intent` is never
promoted by its own success. Output polarity is scored across predicate
occurrences with their negation context: `did not fail` and `no failures` are
positive evidence, `did not pass` and `failed to pass` are negative, `not
present` is an absence, and an absence report (`no test named parser`) never
confirms a claim that a test failed. A decision parsed from reasoning is never
rendered as a fact, even when the model stated it confidently. The note itself
opens with `[source: thinking-summary revision=N phase=...]` and tells the
reader that inferred items are unverified.

The full chain of thought is deliberately not stored: only the bounded set of
items above, at most 12 decisions and 8 questions.

## 2. Injection (REQ-2)

`ThinkingState::prepare_injection` returns a
`HostGuidance { purpose: "thinking-summary", content }`. Host guidance is the
existing seam for exactly this shape of data:

- it is composed into the system prompt of one call by
  `ProviderRequest::wire_system_prompt`;
- it is never appended to `messages`, so no user or assistant turn is forged;
- it is cleared after the call, so it cannot leak into later history;
- it carries a stable purpose and source marker, visible in request diagnostics
  and in the wire prompt.

The note is omitted when:

- the summary has no live items (or the turn is `complete`);
- the step already received the same revision (one note per step);
- an item's text already appears in the last 40 history rows (duplicate guard);
- nothing survives that filtering (the function returns `None`).

## 3. Lifecycle (REQ-3)

`ThinkingState` is the owner. It ingests:

- `observe_reasoning` - new decisions, questions and next action;
- `observe_tool_call` - the call and the action it was meant to carry out;
- `observe_tool_result` - corroboration or correction;
- `finish` - the turn closed.

Rules:

- A new decision that shares the same subject with an older one (high token
  overlap, or opposite negation) retires the older one
  (`active = false`, `confidence = superseded`). The retired item is kept for
  audit but never injected.
- A success promotes only a `claim` whose output is confirming evidence, and
  records the supporting excerpt with the tool and step. It also moves the phase
  to `verifying` when the output looks like a test or build result. A success
  with no confirming evidence leaves the claim `inferred`.
- A failure retires the judgment that led to the call, adds an open question
  naming the failure, and sets the next action to diagnosis instead of a retry.
  If the decision had been `verified` by that same tool, the verification is
  withdrawn and the decision returns to `inferred`; a failed call must never
  leave an unsupported decision marked `verified`. Two consecutive failures move
  the phase to `blocked`.
- `finish` moves the phase to `complete`; a terminal summary is never injected.
- A new turn keeps only verified decisions and unresolved questions that
  share a word with the new request; inferred judgments from a closed turn are
  stale and are not carried forward.
- The turn phase alone does not decide resume-versus-new-task. A completed
  turn is always a new task. An unfinished turn resumes only when the new
  request is a continuation command, still shares enough significant words with
  the stored request signature, or relates to a pending call; a single subject
  word can continue the work, while a shared generic verb (`fix`, `run`) cannot.
  A different request is a new task and clears stale pending state. A call that was in flight on a
  resume is preserved with `interrupted = true`; it cannot be paired with a later
  result and its outcome is recorded as an open question ("outcome is unknown").
- Streamed reasoning is folded into the summary when a tool call starts, when a
  step ends, on provider error, on stop, and in batches once 2,048 characters
  have arrived. A dropped stream therefore keeps the batched part; a drop before
  the first batch has no summary to persist, and neither path marks the turn
  `complete`.

## 4. Persistence and resume

`ThinkingSnapshot` (schema version 3) is serializable. `ReactSession` gained two
default methods, `thinking_snapshot` / `set_thinking_snapshot`; both
`LlmSession` and `AgentSession` implement them.

- The engine loads the snapshot from the session at the start of every run and
  writes it back after every change (`begin_thinking`, `persist_thinking`).
- `AgentSession` stores it beside the messages, not in them, with
  `#[serde(default)]`; session schema generation 6. Older files load with
  `thinking: None`.
- `SessionRestore` (the agent's run-scoped session guard) carries the snapshot
  back on success, cancellation and `Drop`, so a stopped turn keeps the valid
  part of its summary.
- `Agent::save_session` / `restore_session` therefore persist the summary
  without changing the message history.

## 5. Configuration

The switch is part of the Agent loop configuration and is **on by default**:

```rust
let agent = AgentConfig::default().thinking_context(false);
// or, for an engine embedded directly:
let engine = ReActEngineBuilder::<MyApp>::new()
    .llm(client)
    .thinking_context(false)
    .build()?;
```

In TOML the same switch is `thinking_context = false`; an absent key keeps the
default (on), so existing files are unchanged. The bindings expose it as
`AgentBuilder.with_thinking_context(False)` in Python and
`AgentBuilder.thinkingContext(false)` in JavaScript. BSH exposes the same
switch in **Settings → Agent loop → Thinking context**; the workspace value is
`[agent] thinking_context`. Disabling it makes the loop
skip extraction, snapshot persistence and `thinking-summary` host guidance; a
snapshot already stored in the session is left untouched and can be restored if
the switch is re-enabled.

## 6. Tests

- `crates/react/src/thinking/*`: extraction, rendering, supersession,
  corroboration, correction, duplicate suppression, completion, new-turn
  filtering, interrupted resume, snapshot round-trip.
- `crates/react/tests/thinking_injection_test.rs`: end-to-end provider requests
  prove a confirming result verifies only its claim, the note reaches
  `wire_system_prompt` and never the message list, and a failed result replaces
  the retired judgment with an open question.
- `crates/react/tests/thinking_interruption_test.rs`: provider error, stop,
  stream drop (before and after the reasoning batch), and normal completion.
- `crates/react/tests/thinking_tool_interruption_test.rs`: a tool interrupted
  while executing is restored as `interrupted`/unknown and injected as such on
  the next request; a different task after that interruption clears it.
- `crates/react/tests/thinking_injection_test.rs` also runs the same turn with
  `thinking_context(false)` and proves no snapshot, guidance or system-prompt
  note is produced.
- `crates/agent/src/agent/context.rs`: session save/restore round-trips the
  snapshot and an older session file without the field still loads.
