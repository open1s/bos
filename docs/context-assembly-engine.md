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

## Request identities (issue #412)

Two identities are reported per assembly:

- **Composition** (`ContextFingerprint::composition`, SHA-256 hex): ordered
  block shapes (source, trust, provenance, inclusion) plus source versions: the
  persona's keyed digest or, without a key, a coarse length/token label; the
  workspace instruction digest and file names; each recalled memory
  `<record-id>@<revision>`; the tool-guidance version; the provider/model
  identity; and the turn number. It never contains block text.
- **Content** (`ContextFingerprint::content`, HMAC-SHA256): canonicalized block
  bytes under `BSH_CONTEXT_CONTENT_KEY`, present only when that key is set.
  Without a key no content identity is produced, so logs cannot be dictionary-
  attacked.

Intentional exclusions: provider registration order (BSH models no provider
list at this layer), the session id (not known to the assembly call), and the
content of persona, workspace and memory blocks from the composition identity.
Logs distinguish the two values; the content one is `-` unless keyed.

### Typed provider request (issue #421)

Every provider call carries an immutable `ProviderRequest` built from the same
snapshot as the assembly report: composition/content/execution identities, the
content key id, the block manifest, each block's source/trust/provenance and
token estimate, plus the exact model, system prompt, serialized message history
and advertised tool names of the call. It is attached to the request but never
serialized to the provider; adapters flatten it into roles and text, and a
recording provider can prove the wire request came from that snapshot. A content edit cannot change the
execution identity, and a key rotation changes only the content identity while
recording the new key id.

### Adapter flattening (documented)

A provider protocol here carries only roles and text, so the host uses an
explicit encoding: runtime, time and workspace instruction blocks are injected
as typed messages with a `source`/`trust` footer, and every block — including
persona, retrieved memory and tool guidance — is listed in the
`BSH-CONTEXT-MANIFEST-V1` manifest carried by the first request block. Persona,
memory and tool guidance still have their text in the system prompt; the
manifest is what makes their identity, trust and provenance distinguishable at
the adapter boundary. The typed snapshot remains authoritative for diagnostics
and fingerprints.

## 6. Non-goals

- No change to permission/sandbox enforcement semantics (they are host code).
- No broad rewrite of the agent loop or provider adapters.
- Retrieval never injects the whole memory store as a fallback.
