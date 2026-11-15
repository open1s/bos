# The Durable Record Kernel — unified fix for issues #338–#346

Status: accepted design, implemented in one stack. This document is the single
architecture behind the nine open issues; each issue maps onto one or more of
the three shared primitives below. It deliberately adds **fewer concepts, not
more**: one commit kernel, one storage identity, one record model.

## 1. The shared invariant

Every issue is a violation of one rule:

> A state transition is real only after its durable commit succeeds, and every
> record that crosses a trust boundary carries its identity, scope, provenance
> and lifecycle with it.

- #338/#339: federation mutates live state, then *logs* persistence failures and
  acknowledges success.
- #341/#342: FileMemory mutates a cache, then *logs* persistence failures; two
  handles to one path hold two caches.
- #340/#343/#344: memory has no scope, provenance or lifecycle, so capture,
  isolation and governance have nothing to stand on.
- #345: recall cannot rank what the record does not carry (kind, freshness,
  verification), and tokenization drops continuous CJK text.
- #346: diagnostics re-derive inclusion from rendered strings, losing the typed
  trust/provenance the assembly already knows.

## 2. Primitive 1 — `hostkit::durable`: the staged commit kernel

One implementation of "write bytes so a crash or a failure cannot lie":

```text
commit(path, bytes, privacy, gate)
  stages: CreateDir → CreateTemp → Write → Sync → Harden(temp) → Replace
  every stage: typed DurableError { stage, path, source } on failure
  failure at any stage: temp removed, previous file intact, Err returned
load(path) -> Missing | Present(bytes) | Err(DurableError)
  fail-closed: NotFound is Missing; every other read/parse failure is an error,
  never an empty fallback
```

- `CommitGate` is the innovation that makes the acceptance criteria testable:
  a trait with one `pass(stage, path)` hook per boundary. Production passes a
  no-op gate; tests inject failures at temp creation, write, sync or replacement
  without touching the filesystem's own semantics.
- `fspriv::write_private` is re-expressed on top of `commit` (behavior kept).
- `hostkit::coord` adds the storage identity: `canonical_identity(path)`
  resolves symlinks/aliases to one key, and `FileLock` (fs2 advisory lock on
  `<path>.lock`) serializes commits across processes. Lock files are never
  deleted (deletion races with other lockers); the OS releases them on crash.

Consumers: FileMemory (#341/#342), peer rotation (#338), knowledge publication
(#339). All three get the same rule: **under ONE lock, clone → mutate the clone
→ commit → publish the clone to live state only on success**. A failed commit
leaves live and durable state identical, returns a typed error, and records a
non-success audit outcome. The single lock matters: cloning under a read lock
and publishing under a later write lock lets two concurrent transitions
interleave their commits and lose an acknowledged write.

## 3. Primitive 2 — `agent::memory::record`: one versioned record

`MemoryRecord` (schema 2) replaces the bare item as the storage unit:

```text
id, revision, kind(Note|Exchange|Fact|Preference|Decision|Lesson|Procedure|TaskState)
scope(User|Workspace|Session|Agent, with identities), content, metadata
provenance { source: UserStatement|ModelInference|ToolEvidence|Legacy|System,
             session, turn, evidence[] }
confidence { level: Unverified|Low|Medium|High, verified }   // heuristic, NOT a probability
status(Candidate|Active|Superseded|Expired|Tombstoned)
created/updated/expires, relations { supersedes, contradicts, derived_from }
```

- Legacy JSONL lines migrate on read (kind inferred, scope = the store's
  default, provenance = Legacy); nothing is destroyed, and the legacy
  `MemoryItem`/`MemoryStore` surface stays byte-compatible for the bindings.
- Supersession keeps the previous revision as `Superseded` with a relation;
  deletion writes a `Tombstoned` record so cross-process merges cannot
  resurrect it; expiry is deterministic against a supplied clock.
- Untrusted content stays data: no field of a record is ever read as policy.

## 4. Primitive 3 — coordinated repository + scope

- `MemoryRepository` is the fallible API (#342): add/update/supersede/remove/
  clear/search return `Result<_, MemoryError>` with typed serialization, I/O,
  locking, corruption, conflict and unavailable variants. `MemoryStore` remains
  as the compatibility adapter (documented best-effort semantics).
- `FileMemory::open` routes through a process-wide manager keyed by
  `canonical_identity` (#341): one shared cache + one async write gate per
  storage identity; every commit takes the cross-process `FileLock`, re-reads
  the durable snapshot, merges by (id, revision) with tombstone precedence, and
  only then publishes. Barriers in the tests force stale-snapshot interleavings.
- `MemoryScopeResolver` (BSH, #340) resolves storage identity AND access scope:
  workspace store `<save_root>/memory/<workspace-id>.jsonl`, user store = the
  legacy `<save_root>/memory.jsonl` (non-destructive: same file, records upgraded
  in place to User scope). Session/Agent scopes are narrower tags inside the
  workspace store. Recall and write both enforce scope BEFORE ranking/injection.
  Workspace identity is the registry id, so a rename/move keeps the memory.
- Governed capture (#343): a BSH coordinator at the completed-turn boundary
  turns evidence into typed candidates (deterministic extractor baseline;
  model-assisted extraction behind a trait seam), deduplicates by normalized
  content key, flags contradictions as relations, and commits only what the
  policy allows — asynchronously, so capture failure never fails a turn.
- Hybrid recall (#345): deterministic BM25 over Unicode-normalized tokens with
  CJK character n-grams, scope/kind filtering before ranking, kind/recency/
  verification features with per-hit explanations, token-budgeted top-K. The
  legacy `search` ranking stays fixture-pinned for the bindings; the new
  `recall` seam is what the agent prompt path uses.
- Context assembly (#346): the agent returns a typed `PromptAssembly` (prompt +
  recall report); BSH builds an immutable `AssemblySnapshot` of typed blocks
  (source, trust, tokens, provenance) and derives diagnostics from it. No
  production diagnostic searches rendered strings; memory tokens are accounted
  separately and checked against the serialized prompt within the estimator's
  documented rounding limits.

## 5. Compatibility policies (documented, non-destructive)

1. Legacy `memory.jsonl`: read as User scope, upgraded in place on next write;
   never deleted, never merged into a workspace file.
2. `MemoryStore`/`MemoryItem` and the PyO3/Node `Memory` classes keep their
   signatures and the shared ranking fixture; fallible results are additive.
3. Federation peer/knowledge files keep their JSON shape; rotation gains an
   idempotent retry rule (a committed rotation replays as ok; a failed one
   changes nothing, so a fresh signed record always succeeds).
4. Context Assembly RFC slices 1–2 behavior is preserved; the snapshot replaces
   string-sniffing diagnostics without changing request bytes.

## 6. Issue map

| Issue | Primitive(s) | Seams |
|---|---|---|
| #338 | commit kernel | `serve_rotation`: clone→rotate→commit→publish |
| #339 | commit kernel + fail-closed load | BSH assembly + `serve_knowledge_publication` |
| #340 | scope resolver | `bsh::memory_scope`, `open_memory` |
| #341 | coord + manager | `FileMemory::open`, shared store registry |
| #342 | repository + commit | `MemoryRepository`, `MemoryError` |
| #343 | record + capture | `bsh::memory_capture`, `agent::memory::capture` |
| #344 | record | `agent::memory::record` + migration |
| #345 | record + recall | `agent::memory::retrieval` |
| #346 | snapshot | `context_assembly`, `Agent::assemble_prompt` |
