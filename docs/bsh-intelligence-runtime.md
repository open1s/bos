# BSH Intelligence Runtime

One design now backs the 19 open issues in the goal, Skill and tool intelligence
epics. The shared rule is that a catalogue holds **metadata**, a **plan** decides
what belongs to one task, and a **fenced runtime** is the only place that turns a
selection into context.

## Goal runtime

`crates/agent/src/goal.rs` now gives every goal a durable identity and a
monotonic revision. `GoalFence` is captured by `GoalStore` when a round is
seeded, and `AppState::settle_goal` applies a completion or blocker only while
the durable goal still carries that fence. Pause, edit, resume, clear and policy
blocks move the revision (or replace the identity), so a late result from a
stale round is discarded. An in-flight model request is deliberately **not
cancelled**: it is allowed to finish and its settlement is thrown away, which is
the policy issue #289 asks a host to state.

Objectives are validated before durable state is touched. An over-limit
objective is refused with `GoalRefusal::ObjectiveTooLong`; only a value read
from an older file is clamped by `Goal::sanitize`. The round-limit blocker keeps
the stable code `round-limit` and localizes only its message at creation time
(`goal.roundLimit`), so the code stays routable after a locale change.

## Skill intelligence

`crates/agent/src/skills/mod.rs` parses and validates the complete frontmatter:
name, description, category, version, tags, dependencies, conflicts, source,
publisher and a SHA-256 digest. Malformed values are reported, never repaired
into a different version.

`crates/agent/src/skills/index.rs` is the metadata-only catalogue:

- discovery retains no instruction bodies; `load` opens exactly one manifest and
  refuses it when its digest no longer matches the indexed entry;
- ranking weights are exact name 120, name prefix 60, name term 25, tag 18,
  category 12, description 6, with Unicode-aware tokenization and CJK bigrams;
- `SemanticIndex` is provider-neutral; a missing, failing or malformed provider
  falls back to the lexical ranking, and only approved metadata is embedded;
- `resolve` produces a deterministic dependency-first plan and detects missing,
  incompatible, cyclic and conflicting selections;
- `merge_remote` admits federated metadata only while it is unexpired, rejects
  duplicate digests, lets a local Skill win a name collision, and resolves a
  newer revision deterministically;
- `TrustPolicy` gates source, publisher and user-only invocation.

`crates/agent/src/skills/runtime.rs` is the task-scoped runtime: plan, lazy
load, activate, compose, deactivate and finish. State is keyed by task id, the
aggregate and per-Skill context budgets are enforced before anything is added,
identical bodies are emitted once and reported, and telemetry records names,
counts and outcomes without instruction text or prompts.

`find_skill` now ranks through the same metadata scorer and returns version,
category and tags with each result. The production `load_skill` path resolves
through the runtime: it plans dependencies, activates only the selected set under
trust and budget checks, refuses a manifest whose digest changed, and returns one
composed block with dependency-first ordering. The task scope is the turn; each
run clears the previous activation. The composed block is a request-scoped
instruction, not transcript content: `after_tool_result` moves it into the
context, the tool result keeps an acknowledgement, and the block is cleared on
the final answer and again at the next run. `Agent::drain_skill_events` and
`Agent::active_skills` expose the runtime state to a host observer.

## Tool intelligence

`ToolMetadata` gains `category`, `tags` and `intents`. Discovery scores exact
name 120, prefix 60, name term 25, tag 18, intent 18, category 12, description 6,
so a synonym in metadata can find a tool whose description uses other words.

`crates/react/src/engine/schema_budget.rs` bounds the schemas a turn actually
adds. Repeating the same name and revision is idempotent; a changed schema is a
new revision charged at its delta; one oversized schema and cumulative
exhaustion are refused with an explicit `budget` note on the tool result, so the
model is told what was not added and how to continue. The budget resets at the
start of every run.

## Measured baselines

Both retrieval corpora are asserted tests, so a scoring change that loses recall
fails the suite (run with `--nocapture` for the report):

- Skill corpus (6 Skills, 4 synonym-heavy queries): P@3 0.33 (the 1/3 ceiling for
  one relevant Skill), R@3 1.00, MRR 1.00.
- Tool corpus (6 tools, 5 queries): P@3 0.33, R@3 1.00, MRR 1.00.

## Issue coverage

| Issue | State | Notes |
|---|---|---|
| #288 goal identity fencing | implemented | identity + revision, settlement fence, tests |
| #289 in-flight mutation policy | implemented | documented policy; stale settlements discarded |
| #290 overlength objectives | implemented | validation before mutation, boundary tests |
| #291 localized round limit | implemented | stable code, localized message at creation |
| #273 full Skill metadata | implemented | version, tags, deps, conflicts, provenance, digest |
| #274 metadata-aware find_skill | implemented | shared scorer, Unicode, tags/category |
| #275 optional semantic stage | implemented | provider-neutral trait, hybrid merge, fallback |
| #276 metadata-only indexing | implemented | index + lazy digest-checked load used by production load_skill; the cross-turn TTL cache was removed, so a loaded body is always the indexed revision |
| #277 activation lifecycle | implemented | load_skill activates in the task-scoped runtime; idempotent; reset at each run; events exposed |
| #278 dependency/conflict resolution | implemented | deterministic plan, cycles, optional omissions |
| #279 provenance and trust | implemented | provenance/digest, source/publisher/user-only policy, stale and source-change invalidation, local-over-remote collisions, injection-style instructions proven inert |
| #280 effectiveness telemetry | implemented | offline activation benchmark: with/without success rates, activation precision, no-Skill accuracy, retrieval P@3/R@3/MRR and instruction bytes |
| #281 federated metadata | implemented | policy-scoped transport, expiry, duplicate/collision handling, withdrawal on refresh, unreachable peers, explicit trust grant and digest before remote load |
| #282 end-to-end skill runtime | implemented | production load path plans, trusts, loads, activates, composes and clears per turn |
| #283 bounded context composer | implemented | runtime composes dependency-first, deduplicated, budgeted instructions into one request-scoped instruction block; the tool result is an acknowledgement; block cleared on final answer and at the next run |
| #284 hybrid tool retrieval | implemented | metadata contract + ranking; asserted P@3/R@3/MRR corpus |
| #285 capability-view routing | implemented | discovery reads the live composed registry, reports capability id/version/provider, and calls dispatch through the shim's workspace invocation boundary |
| #286 cumulative schema budget | implemented | per-tool/per-turn, idempotent, explicit refusal note |
| #287 tool effectiveness telemetry | implemented | turn-correlated discovery, selection and final events; an accumulator reports discovery-use, rejection and candidate rates; retrieval corpora assert P@3/R@3/MRR |

## Operation

- `run_benchmark` is the repeatable task-outcome harness; it needs no model or
  network.
- `FederatedSkills::refresh` is the only remote metadata ingress; instruction
  loading stays behind `authorize_instructions`.
- Host loops subscribe to the engine's `TelemetryEvent` stream and feed
  `ToolEffectiveness` for a turn-correlated report.
