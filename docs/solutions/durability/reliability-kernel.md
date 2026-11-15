---
module: hostkit, federation, bsh
tags: [durability, commit-levels, file-lock, parquet, manifest, journal, quarantine, page-cache, schema-evolution, resource-stream, replay]
problem_type: bug-fix-and-architecture
issues: [347, 348, 349, 350, 351, 352, 353, 354, 355, 356]
---

# One rule for every abandoned transition (issues #347–#356)

## Problem

Ten open issues, one invariant violated in ten places: **a state transition was
real only at the durability level it claimed, and anything that could no longer
make progress was silently abandoned instead of stopped and accounted for.**

- A dropped `RemoteStream` (#347) settled only the caller; the provider kept
  reading, charging the lease and holding a session slot for up to 600 s.
- That 600 s was not even a bound (#348): it was checked at the top of the
  producer loop, never around the `source.read().await`.
- A replay store that failed to LOAD (#349) was replaced in memory by a fresh
  empty cache, so a still-valid signed envelope could be admitted again — and
  the unreadable file was overwritten by the empty-derived snapshot.
- An unreadable in-flight journal (#350) was deleted with a warning, although it
  could be the only copy of the latest turn.
- A failed Parquet page read (#351) came back as a valid-looking empty page, so
  corruption looked like end-of-history.
- Rename was treated as durability (#352) with no file or directory sync.
- Segment names and temp files were process-local (#353), so two writers on one
  history could clobber each other, and the write lock was process-local too.
- The page cache was bounded by page COUNT (#354), so one huge tool output or
  embedded image could still blow the process, and one row could exceed the
  row-group byte target unboundedly.
- A malformed JSON column decoded to its default (#355): corruption became an
  empty list, silently.
- The manifest was trusted as the fast path (#356), so a stale one produced
  wrong row counts, skipped rows and page offsets past the end.

## Solution: four shared primitives, built once

The full design is `docs/reliability-kernel.md`. In one sentence: **one commit
kernel with explicit levels, one coordination order, one typed error surface,
one recoverable journal.**

1. `hostkit::durable` — `CommitLevel::{Accepted, Written, Durable}` and
   `commit_stream()`, so a Parquet writer streams into the kernel's temp file
   while the kernel syncs it, renames it and (Unix) syncs the parent directory.
   `commit()` keeps its signature and now means `Durable`. `IoStage::SyncDir`
   is the new fault-injection boundary; Windows' weaker guarantee is documented.
2. `hostkit::coord` — `FileLock::acquire_bounded()`: every history mutation
   takes `<dir>.lock` FIRST and the in-process write guard second (one order, no
   deadlock), with a 250 ms budget so a busy directory is a typed `Locked` error
   rather than a stalled UI thread. Segment ids carry stamp + pid + sequence and
   every temp is `create_new`.
3. `history` — `HistoryError` (`Read`, `Decode`, `Schema`, `Oversized`,
   `Locked`) plus `IndexState::{Valid, Rebuilt, Unavailable}`. A present but
   malformed column is an error naming segment/row/column; an ABSENT legacy
   column is a documented default. `bsh.schema` in each segment's key/value
   metadata and `schema_version` in the manifest make the supported set explicit
   (1 = legacy column prefixes, 2 = current), and `diagnose()` is the offline
   audit. Byte budgets: `CACHE_BYTES` 32 MiB, `MAX_INLINE_ROW_BYTES` 32 MiB,
   `MAX_WINDOW_ROWS` 1024, with `Reader::for_each_page` for longer walks.
4. `store` — journals are versioned and checksummed (`Inflight { version,
   checksum, ... }`, FNV-1a over the payload); recovery classifies and NEVER
   deletes what it cannot read: corrupt/truncated/empty/unknown-version/stale
   journals move atomically into `corrupt/<ms>-<pid>-<reason>/` with a
   `report.json` that records counts, never contents. `recovery_report()` and
   `recovery_notice()` make the outcome visible at startup.
5. `federation` — `Open.attempt` and an idempotent `Cancel { lease, attempt }`;
   the provider stops the exact session, wakes a producer waiting for credit and
   ignores a stale attempt; every read is selected against ONE absolute deadline
   and the cancellation signal. The replay store is `Missing | Loaded |
   Unavailable { reason }`; `Unavailable` fails closed and is never overwritten,
   and `reset_replay_store()` is the audited operator recovery.

## Why one pass, not ten patches

- The storage issues share three seams (the commit path, the directory lock, the
  error type). Fixing durability without the lock lets two processes commit over
  each other; fixing the lock without `create_new` still clobbers a temp; fixing
  either without typed errors still reports corruption as empty data.
- The federation issues share one lifecycle object. A cancel token without an
  attempt breaks resumed streams; a deadline without the cancel select keeps a
  stalled read alive; a cancel without a checkpoint policy loses the resume point.
- The tests are the contract: fault injection at every commit stage, a second
  `FileLock`/second `History` standing in for a second process, golden Parquet
  fixtures per schema version, a paused clock for the deadline, and a corrupt
  replay file that must still refuse traffic after a restart.

## Non-negotiable outputs

- Nothing durable is deleted on a failure path; quarantine preserves bytes.
- A read that fails is never an empty page; an empty page is never a failure.
- A cancellation is reported as confirmed or unknown, never assumed.
