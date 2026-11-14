# BSH Context Assembly Engine — RFC and migration plan (issue #337)

Status: design accepted; first slice landed (see "Current state"). This document
defines the target architecture, maps today's code onto it, and gives an
incremental migration plan. It deliberately does **not** authorize a rewrite.

## 1. Goal

Persona, runtime facts, workspace instructions, retrieved memory, capability/tool
guidance and conversation state are assembled across several BSH and Agent code
paths. The engine makes each an independently managed **context provider** with
explicit lifecycle, scope, provenance, budget and diagnostics, producing an
immutable per-request snapshot plus a diagnostics record.

## 2. Interfaces

```text
ContextSource        persona | host-runtime | time-context
                     | workspace-instructions | retrieved-memory
                     | tool-guidance | user-input
Trust                host-authority | untrusted-data
ContextProvider      id, scope, precedence, refresh policy
                     fn assemble(ctx) -> Result<ContextBlock, ProviderError>
ContextBlock         source, trust, text, token_estimate, provenance metadata
AssemblySnapshot     ordered blocks + composition fingerprint (immutable)
AssemblyReport       per-source inclusion/omission + reason, memory state,
                     retrieval hits, token accounting, fingerprint
```

The concrete first slice of these types lives in
`crates/bsh/src/context_assembly.rs` (`ContextSource`, `Trust`, `MemoryState`,
`SourceStatus`, `ContextAssemblyReport`).

## 3. Design requirements

- **Deterministic composition.** Equivalent inputs + provider versions produce
  the same ordered snapshot and the same fingerprint. Ordering is a function of
  the provider set, not registration order (the tool array already follows this
  rule; the assembly extends it).
- **Precedence.** host-runtime contract > persona > workspace instructions >
  retrieved memory > user input, for *guidance*; but **no untrusted source may
  override host-enforced controls** (sandbox, permission, tool admission) — those
  are code, not prompt, and are never a function of assembled text.
- **Trust/provenance.** Every block carries its source and trust until provider
  serialization. Workspace instructions and retrieved memory are framed as data
  (`<system-reminder>`, "Relevant memory:") and are never host authority.
- **Token budget.** Selection/prioritization/truncation are budget-aware; the
  report records estimated usage and any omission/truncation reason. Estimates
  use the documented estimator (`agent::agent::context::estimate_tokens`).
- **Per-turn refresh.** Dynamic context (runtime snapshot, memory) is reassembled
  each turn; stale snapshots are not accumulated. The system-prompt cache is
  cleared per turn and on memory change.
- **Scope isolation.** workspace / session / subagent / future federated scopes
  resolve their own instructions and memory; a child cannot widen its scope.
- **Failure isolation.** A provider failure (e.g. memory open) is surfaced as a
  state, not a silent disable, and does not crash unrelated execution unless a
  strict mode is configured.
- **Serialization.** Provider-specific wire formatting stays in the LLM adapter;
  the engine hands it typed blocks, never pre-serialized authority.

## 4. Gap analysis (current code → design)

| Concern | Today | Target |
|---|---|---|
| Persona composition | `caps.rs` + `agent::prompts::compose_system_prompt_with` | Persona provider |
| Runtime contract | `prompts::HARNESS_CONTEXT`, appended in both modes (#332) | host-runtime provider |
| Runtime snapshot | `context.rs::render_snapshot` | host-runtime provider (dynamic) |
| Time snapshot | `context.rs::render_time_context` (instant, zone rule, elapsed) | time-context provider (dynamic, per-turn) |
| Workspace instructions | `instructions.rs::render` | workspace provider (untrusted) |
| Memory retrieval | `agent recalled_context` / `system_prompt_for` | memory provider (untrusted) |
| Memory lifecycle | `caps.rs::open_memory` → `MemoryState` (#334) | provider failure state |
| Multimodal retrieval | `Content::retrieval_text` (#335) | memory provider query strategy |
| Tool guidance | `prompts::append_tool_guidance` | capability provider |
| Provenance/trust | `context_assembly::ContextSource/Trust` (#333) | block metadata to serialization |
| Diagnostics | `ContextAssemblyReport` logged per turn (#336) | full per-request report |

## 5. Migration plan (behavior-preserving, no rewrite)

1. **Slice 1 (landed).** Typed sources/trust, memory lifecycle states, multimodal
   retrieval query, per-turn assembly report + fingerprint, runtime contract in
   complete mode. Existing assembly paths unchanged; they now *report* through
   the shared types.
2. **Slice 2.** Extract each existing path behind a `ContextProvider` impl with
   the same output bytes; add architecture tests for determinism and scope
   isolation. No behavior change.
3. **Slice 3.** Move budget-aware selection/truncation into the engine; the
   report gains omission/truncation reasons. Provider adapters consume typed
   blocks.
4. **Slice 4.** Scope isolation for subagent/federated instances; provider
   lifecycle (cancellation, failure isolation, observability).

Each slice keeps the previous behavior byte-identical until its own tests prove
the new guarantee, so the migration never requires a big-bang rewrite.

## 6. Non-goals

- No change to permission/sandbox enforcement semantics (they are host code).
- No broad rewrite of the agent loop or provider adapters.
- Retrieval never injects the whole memory store as a fallback.
