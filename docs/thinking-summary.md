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
step (the last 16,000 characters) and extracts three kinds of item:

| Item | How it is recognised | Confidence |
|---|---|---|
| Decision | judgment markers ("the fix is", "the cause is", "the plan is", "决定", "方案是", "采用") or intent markers ("I will", "let's", "我将", "我会") | `inferred` |
| Rationale | the clause after " because ", " since ", "因为", "由于" | attached to the decision |
| Open question | a sentence ending in `?`/`？`, or an uncertainty marker ("need to check", "unclear", "待确认", "不确定") | unresolved |
| Next action | "next step:", "next I will", "接下来", "下一步", or a leading imperative ("run ", "check ", "运行", "检查") | tentative |

Only an observed tool result can change a decision to `verified` (see below).
A decision parsed from reasoning is never rendered as a fact, even when the
model stated it confidently. The note itself opens with
`[source: thinking-summary revision=N phase=...]` and tells the reader that
inferred items are unverified.

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
- A success marks a related `inferred` decision `verified`, with the tool result
  recorded as evidence. It also moves the phase to `verifying` when the output
  looks like a test or build result.
- A failure retires the judgment that led to the call, adds an open question
  naming the failure, and sets the next action to diagnosis instead of a retry.
  Two consecutive failures move the phase to `blocked`.
- `finish` moves the phase to `complete`; a terminal summary is never injected.
- A new turn keeps only verified decisions and unresolved questions that
  share a word with the new request; inferred judgments from a closed turn are
  stale and are not carried forward.
- An interrupted run (phase not `complete`) resumes exactly where it stopped.

## 4. Persistence and resume

`ThinkingSnapshot` (schema-versioned) is serializable. `ReactSession` gained two
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

## 5. Tests

- `crates/react/src/thinking/*`: extraction, rendering, supersession,
  corroboration, correction, duplicate suppression, completion, new-turn
  filtering, interrupted resume, snapshot round-trip.
- `crates/react/tests/thinking_injection_test.rs`: end-to-end provider requests
  prove the note reaches `wire_system_prompt`, never the message list, and that
  a failed result replaces the retired judgment with an open question.
- `crates/agent/src/agent/context.rs`: session save/restore round-trips the
  snapshot and an older session file without the field still loads.
