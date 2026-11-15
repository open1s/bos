# The Reliability Kernel — unified fix for issues #347–#356

Status: accepted design, implemented in one stack. Ten issues, four shared
primitives, one rule each. Follows `docs/durable-record-kernel.md` (#338–#346):
fewer concepts, not more.

## 1. The shared invariant

> A state transition is real only at the durability level it promises, and a
> stream or a journal that can no longer make progress must be stopped and
> accounted for — never silently abandoned.

- #347/#348: a provider-side producer outlives its consumer and its deadline.
- #349: a replay store that cannot be loaded is replaced by an empty one, so a
  replay window reopens after a startup failure.
- #350: an unreadable journal is deleted, and it may be the only copy of a turn.
- #351: a failed page read is reported as an empty page.
- #352: rename is treated as durability, but no file or directory is synced.
- #353: segment names and temp files are process-local, and the write lock is
  process-local too.
- #354: the page cache is bounded by pages, not bytes; a row can be unbounded.
- #355: a malformed JSON column silently decodes as its default value.
- #356: the manifest is trusted without being validated against the files.

## 2. Primitive 1 — commit levels (`hostkit::durable`, #352)

`commit` gains an explicit level and a streaming entry point:

```text
CommitLevel::Accepted   bytes accepted by the storage layer (in memory / temp)
CommitLevel::Written    file written, rename completed
CommitLevel::Durable    file contents synced, and the parent directory synced so
                        the entry survives a power loss (Unix; documented weaker
                        Windows guarantee, where a directory cannot be synced)

commit_stream(path, privacy, level, gate, write: FnOnce(&mut File) -> io::Result<()>)
```

Stages: `CreateDir → CreateTemp → Write → Sync → Harden → Replace → SyncDir`
(the last only for `Durable`). `IoStage::SyncDir` is a new typed stage. A
failure at any stage removes the temp file and leaves the previous target
intact. `commit()` keeps its signature and means `Durable`; existing consumers
(memory, rotation, knowledge) get the directory sync for free.

BSH routes segment, manifest and session-document writes through
`commit_stream`/`commit`, and documents per write path which level it promises:

| write | level |
|---|---|
| Parquet segment (`msg-*`, `traj-*`) | Durable |
| `manifest.json` | Durable |
| session document | Durable |
| journal (`inflight.json`) | Written (a journal is a retry aid, not the log) |

Ordering: segment committed → manifest committed → old segments removed; the
journal is released only after the history append returned at its level.

### 2.1 Platform guarantees (documented, per issue #352)

A file written and renamed is atomically *visible*, but only a synced file plus
a synced parent directory is *durable against power loss*. What each platform
actually provides:

| platform | file sync | parent-directory sync | Durable means |
|---|---|---|---|
| Linux | `fsync` | `fsync` on the directory fd | contents and the directory entry survive a power cut |
| macOS | `fsync` (delivers to the drive; `F_FULLFSYNC` would also flush the drive cache, which this kernel does not require) | `fsync` on the directory fd | same, bounded by the driver's write cache |
| Windows | `FlushFileBuffers` | not available: a directory handle cannot be flushed | contents are flushed; the rename is visible after a normal restart, and a power cut may lose the directory entry, which a crash detector reports as a missing tail segment rather than as a corrupt one |

The weaker Windows directory guarantee is the reason a missing tail segment is
a *repairable* manifest inconsistency (Primitive 3) while an unreadable present
segment is a diagnostic: losing an entry is survivable, reading garbage is not.

## 3. Primitive 2 — one coordinated storage owner (`hostkit::coord`, #353)

- **Cross-process lock**: every mutation of a history directory (append, seal,
  compaction swap, manifest publication, recovery) holds `coord::FileLock` on
  `<history dir>.lock` before touching files. The in-process `RwLock` stays as
  the cheap first level; the file lock is taken only by writers, and always
  before the write guard (one order, so no deadlock).
- **Collision-safe names**: `create_new(true)` for every temp file, and segment
  ids carry the pid and a per-process sequence beside the millisecond stamp
  (`msg-<stamp>-<pid>-<seq>.parquet`), so two processes never emit the same
  name and a clock anomaly cannot collide. `stamp_of` keeps reading legacy
  `msg-<stamp>.parquet` names, so old histories still sort correctly.
- **Reader contract**: a reader holds a read guard; during a compaction swap the
  directory is renamed aside and back, so a read that finds its segment missing
  reopens the directory once (the existing retry) and otherwise reports the typed
  error from Primitive 4. A compaction marker is still honoured.

## 4. Primitive 3 — typed history errors (`history`, #351, #355, #356)

`HistoryError` (thiserror) with the context each variant needs:

```text
Read { segment, group, detail, source }        page/segment read failed
Decode { segment, row, column, detail }        present column failed to decode
Schema { segment, found, supported }           unsupported schema/column shape
Manifest { path, problem, detail }             manifest invalid/unavailable
Oversized { row, bytes, limit }                a row exceeds the inline limit
Locked { path }                                another process holds the lock
```

- `Reader::messages`/`trajectory_page` and the store's page readers return
  `Result<_, HistoryError>`; a genuinely empty page is still `Ok(vec![])`, a
  failed read is never defaulted. The one bounded retry (reopen after a
  compaction swap) is explicit, and its final failure is propagated.
- Decoding distinguishes **absent legacy column** (documented default) from
  **present but malformed** (typed `Decode` error naming segment, row, column).
- Every segment written carries a schema version in its Parquet key/value
  metadata (`bsh.schema`), and the manifest carries `schema_version`. An
  unknown version is rejected with `Schema`, never guessed.
- `history::diagnose(dir)` is the offline validation path: it walks segments
  and reports per-file/per-row problems without materializing the history.

## 5. Primitive 4 — recoverable in-flight journals (#350)

`Inflight` gains `version` and a `checksum` over its payload:

```text
Inflight { version, checksum, base, rows }
```

Recovery classifies a journal and never deletes what it cannot read:

| state | action |
|---|---|
| missing | nothing to recover |
| valid, replays | append/rewrite, then remove the journal |
| valid, already covered by the log | remove the journal (idempotent second start) |
| valid, replay fails | keep the journal for the next launch (retryable) |
| truncated / empty / checksum mismatch / unknown version | **quarantine** |
| base beyond the log (stale directory) | quarantine, not delete |

Quarantine is `<history>/corrupt/inflight-<stamp>-<reason>.json` moved with one
atomic rename, plus a ledger line in `<history>/corrupt/quarantine.json`
(session, source, reason, version, timestamp — never message contents).
`Store::recovery_report()` exposes the outcome so a caller can show it instead
of claiming a clean start.

## 6. Bounded memory (#354)

- `PageCache` carries a byte budget beside the page ceiling and evicts until
  both hold; the weight is the decoded row estimate, not the encoded size.
- A single row above `MAX_INLINE_ROW_BYTES` is rejected at the append boundary
  with `HistoryError::Oversized` (documented policy: large text/binary belongs
  in a reference, not inline). Payload limits are named constants, not magic.
- Window materialization is bounded by `MAX_WINDOW_ROWS`; whole-prefix reads go
  through a page-walking stream instead of one allocation.
- `History::cache_stats()` exposes bytes, pages, evictions and rejections.

## 7. Resource-stream lifecycle (federation, #347, #348)

- `Open` carries a stream `attempt` (generation). `Cancel { lease, attempt }`
  is the new, idempotent, peer-authenticated message.
- `Drop for RemoteStream` sends `Cancel` (bounded wait), then settles locally as
  `cancelled` with `confirmed` true/false; an unconfirmed cancellation is
  recorded as an unknown outcome, never as "the producer stopped".
- The provider keeps a per-session cancellation handle: a cancel with the
  current attempt wakes a producer waiting for credit, stops reads/charges/sends
  and releases the session slot; an older attempt is ignored.
- The producer computes ONE absolute deadline and selects every source read
  against it and the cancellation signal, so a stalled read cannot outlive it.

## 8. Fail-closed replay store (federation, #349)

`with_replay_persistence` records a typed replay-store state: `Missing`
(initialise empty), `Loaded`, `Unavailable { reason }`. An `Unavailable` store
refuses side-effectful admissions (fail closed) and never overwrites the
unreadable file. `FederationNode::reset_replay_store()` is the explicit operator
recovery: it moves the unreadable file to `<name>.corrupt-<stamp>` (bytes
preserved), starts empty, and audits that prior history was discarded.

## 9. Compatibility policies

1. Existing histories keep reading: legacy segment names, missing columns and
   pre-version manifests stay supported; nothing is rewritten in place except
   the manifest, and only from facts the Parquet files already carry.
2. Wire format: `ResourceStreamMessage` gains `Cancel`; peers that do not know
   it simply never see it (the caller's drop path is best-effort by nature).
3. Nothing durable is deleted on a failure path. Quarantine preserves bytes;
   segment removal still happens only after the manifest that stops referencing
   them is committed.
