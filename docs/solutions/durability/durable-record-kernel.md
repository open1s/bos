---
module: hostkit, agent::memory, federation, bsh
tags: [durability, atomic-commit, memory, federation, scope, provenance, retrieval]
problem_type: bug-fix-and-architecture
issues: [338, 339, 340, 341, 342, 343, 344, 345, 346]
---

# One kernel for every durable transition (issues #338–#346)

## Problem

Nine open issues were one invariant violated in nine places: **a state
transition was acknowledged before (or without) its durable commit, and records
crossed trust boundaries without identity, scope or provenance.**

- Federation rotation (#338) and knowledge publication (#339) mutated live
  state, logged persistence failures, and answered `{"ok":true}`. A restart
  could resurrect a revoked identity or discard acknowledged knowledge; a
  malformed-but-writable knowledge file could be overwritten by an
  empty-derived snapshot.
- FileMemory (#341, #342) kept an instance-local cache and write guard: two
  handles to one path lost updates, and every persistence failure was a log
  line the caller never saw.
- Memory had no scope (#340), no lifecycle/provenance model (#344), no governed
  capture (#343), CJK-blind ranking (#345), and context diagnostics re-derived
  memory inclusion by searching rendered prompt strings (#346).

## Solution: three shared primitives, built once

The full design is `docs/durable-record-kernel.md`. In one sentence: **one
commit kernel, one storage identity, one record model** — reused everywhere.

1. `hostkit::durable` — the staged commit kernel. `commit()` walks
   CreateDir → CreateTemp → Write → Sync → Harden(temp) → Replace; every stage
   returns a typed `DurableError { stage, path, source }`; any failure leaves
   the previous file intact. `CommitGate` intercepts each boundary, so tests
   inject failures at exact stages without filesystem tricks. `load()` is
   fail-closed: `Missing` vs `Present` vs error — never an empty fallback.
   `fspriv::write_private` now delegates to it (behavior kept).
2. `hostkit::coord` — `canonical_identity()` resolves symlink/relative aliases
   to one key; `FileLock` (fs2 advisory lock on `<path>.lock`) serializes
   commits across processes. Lock files are never deleted; the OS releases
   them on crash.
3. `agent::memory::record` — schema-2 `MemoryRecord`: kind, scope
   (User/Workspace/Session/Agent), provenance (source/session/turn/evidence),
   heuristic confidence (documented as NOT a probability), lifecycle
   (candidate/active/superseded/expired/tombstoned), revision, relations
   (supersedes/contradicts/derived-from), validity. Legacy JSONL migrates on
   read; a malformed v2 line is an error, never a silent downgrade.

## The pattern every consumer now follows

```text
lock (one write guard / one async gate + cross-process FileLock)
  → clone the live state
  → apply the transition to the clone
  → durable::commit the clone
  → ONLY ON SUCCESS: publish the clone to live state, acknowledge, audit ok
  → on failure: live == durable == pre-transition; typed error; non-ok audit
```

- `FileMemory`/`FileRepository` (#341/#342): a process-wide registry keyed by
  canonical identity gives every handle one cache + one write gate; each commit
  re-reads the durable snapshot under the cross-process lock (tombstones make
  merges conflict-safe), then publishes. `MemoryStore` stays as the documented
  best-effort compatibility surface; `try_*` and `MemoryRepository` carry the
  guarantee. Barrier tests force stale-snapshot interleavings; stage-injection
  tests prove cache/file agreement after every failure.
- `serve_rotation` (#338): the whole clone→rotate→commit→publish runs under one
  PeerStore write guard; a durable `rotations` log (serde-defaulted) makes a
  committed rotation replay as ok and a failed one change nothing. Retry rule:
  fresh envelope (new message_id), same signed record. Audit rows carry
  `old -> new` and `ok`/`storage_failed`/`denied`.
- `serve_knowledge_publication` + BSH startup (#339): same pattern under one
  KnowledgeService write guard; startup uses fail-closed `load()` — missing →
  empty, malformed/unreadable → knowledge DISABLED (publication answers
  Unavailable, original file preserved) unless the explicit, logged
  `BSH_KNOWLEDGE_RECOVERY=reset` mode is chosen.
- BSH `MemoryScopeResolver` (#340): workspace store
  `<save_root>/memory/<workspace-id>.jsonl` + the legacy user store
  `<save_root>/memory.jsonl` (upgraded in place, never destroyed), composed by
  `ScopedMemory`. The workspace registry ID is the identity, so a rename/move
  keeps the memory. Scope is enforced on reads AND writes, before ranking.
- Governed capture (#343): `agent::memory::capture` (typed candidates,
  deterministic extractor, dedupe by normalized content key, contradiction
  relations, policy caps) + `bsh::memory_capture::CaptureHandle` spawned at the
  completed-turn boundary — Done only, asynchronous, failures observable and
  never turn-failing. Opt-in via `memory_capture`; explicit "remember this"
  works whenever memory is enabled.
- Hybrid recall (#345): `agent::memory::retrieval` — Unicode-normalized tokens
  with CJK unigrams+bigrams, BM25, kind/verified/recency features with per-hit
  explanations, scope/kind/metadata filtering BEFORE ranking, token-budgeted
  top-K with recorded truncation. The legacy `search` ranking stays
  fixture-pinned for the bindings; the agent prompt path uses the new `recall`
  seam. Versioned benchmark corpus:
  `crates/agent/tests/fixtures/memory_recall_benchmark_v1.json` (Recall@3 ≥ 0.95,
  Precision@3 ≥ 0.4, P50/P95 asserted).
- Context assembly (#346): `Agent::assemble_prompt` returns a typed
  `PromptAssembly` (prompt + base + recall account); BSH freezes an immutable
  `AssemblySnapshot` of `ContextBlock`s (source, trust, tokens, provenance) and
  derives every diagnostic from it. No production code searches rendered
  strings; memory tokens are accounted separately and checked against the
  serialized prompt within a documented 2-token estimator tolerance.

## Compatibility policies (non-destructive)

- Legacy `memory.jsonl` keeps its location and becomes the User store; records
  upgrade to schema 2 in place on the next commit.
- `MemoryItem`/`MemoryStore` signatures and the PyO3/Node `Memory` ranking
  fixture are unchanged; fallible APIs are additive.
- Peer/knowledge JSON shapes keep their fields; new fields are serde-defaulted.
- Context Assembly RFC slices 1–2 behavior preserved; request bytes unchanged.

## Verification

- `cargo test -p hostkit` (36), `-p agent` (578 lib + integration),
  `-p federation` (262), `-p bsh` targeted suites — all green.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`.
- Stage-injection tests at CreateTemp/Write/Sync/Replace/Harden for memory,
  rotation and knowledge; barrier interleaving tests; restart recovery tests;
  concurrent-reader snapshot tests; multilingual benchmark with latency bounds.

## Lessons

- A test caught the design flaw immediately: hardening AFTER rename cannot
  preserve the previous file. Harden the temp BEFORE replacement — the rename
  swaps in the already-private file, and every stage keeps the invariant.
- Hard links are not aliases under atomic rename (rename replaces one name's
  inode); symlinks are the alias case canonicalization must resolve.
- Clone→commit→publish must run under ONE lock: cloning under a read lock and
  publishing under a later write lock lets two concurrent transitions interleave
  their commits and lose an acknowledged write.
