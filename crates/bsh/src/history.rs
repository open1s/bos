//! Append-only Parquet history: a session's messages and its trajectory.
//!
//! A transcript grows by one turn at a time and is read back in pages, so the
//! store is shaped by those two facts rather than by convenience:
//!
//! * Append-only segments. Each save appends ONE segment file holding only the
//!   rows written since the last one - msg-000.parquet, traj-000.parquet. A save
//!   therefore costs O(new rows), never O(history), which is what makes a long
//!   conversation cheap to keep.
//! * Sealed segments. A small unsealed tail is merged once it reaches a row or
//!   byte target, so the file count grows with history/(segment size), not with
//!   the number of saves. The manifest is installed before old files are removed.
//! * A manifest index. `manifest.json` holds every segment's start row, byte
//!   size and row-group row counts. Opening a history reads that index instead
//!   of every Parquet footer, and a page locates its segment by binary search.
//! * Row groups are pages. A segment is written with at most PAGE rows and a
//!   byte budget per row group, so one huge tool output does not make every
//!   neighbouring page enormous. A page read decodes only the groups it needs.
//! * A page cache with a fixed ceiling. Decoded pages live in an LRU capped at
//!   CACHE_PAGES; the Store keeps one History per session, so scrolling reuses
//!   it rather than reopening the directory.
//!
//! The session document keeps what a session IS - title, workspace, flags, the
//! plan - and this module keeps what was SAID.
//!
//! # Durability and coordination (issues #352, #353)
//!
//! Every segment and the manifest are committed at [`CommitLevel::Durable`]
//! (docs/reliability-kernel.md section 2): the Parquet writer streams into the
//! temp file the commit kernel created, and the kernel syncs it, renames it and
//! syncs the parent directory. The in-flight journal and the session document
//! are written elsewhere and document their own level there.
//!
//! Every mutation takes the directory's cross-process [`FileLock`] first and
//! the in-process write guard second, which is the one order this module uses,
//! so no two writers can deadlock. Segment names carry the pid and a
//! per-process sequence beside the millisecond stamp, and every temp file is
//! created with `create_new(true)`, so two processes never share a name.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hostkit::coord::FileLock;
use hostkit::durable::{self, CommitGate, CommitLevel, DurableError, IoStage, Privacy};

use anyhow::{Context, Result};
use arrow::array::{
    Array, ArrayRef, BooleanArray, BooleanBuilder, Int64Builder, StringArray, StringBuilder,
    UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};

use crate::model::{ChatMessage, MessageMeta, Role};

/// Rows per page (and the row-count ceiling of one row group).
pub(crate) const PAGE: usize = 128;
/// Decoded pages kept at once.
pub(crate) const CACHE_PAGES: usize = 8;
/// Row groups target this many bytes, so one huge tool output cannot make a
/// multi-megabyte page group on its own.
const ROW_GROUP_BYTES: usize = 4 * 1024 * 1024;
/// A sealed segment targets this many rows...
#[cfg(not(test))]
const SEGMENT_ROWS: usize = 8_192;
#[cfg(test)]
const SEGMENT_ROWS: usize = 4 * PAGE;
/// ...or this many bytes, whichever comes first.
#[cfg(not(test))]
const SEGMENT_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(test)]
const SEGMENT_BYTES: u64 = 1024 * 1024;
/// The metadata index written beside the Parquet segments.
const MANIFEST: &str = "manifest.json";
/// Bumped when the manifest shape changes.
///
/// Version 2 records the history schema version and validates every entry
/// against its Parquet footer (#356). A version 1 manifest is still read (its
/// missing fields default), so an older history needs no rewrite.
const MANIFEST_VERSION: u32 = 2;

/// The history schema version: it names the COLUMN SET a segment carries.
///
/// 2 is the current message schema (index, role, text, reasoning, error, meta,
/// tools, images, steering, parts) and the five-column trajectory schema. Every
/// segment records it in its Parquet key/value metadata under [`SCHEMA_KEY`],
/// and the manifest records it beside the segment lists, so a reader can tell
/// a file it understands from one it does not (#355 extends the check to
/// individual columns).
pub(crate) const SCHEMA_VERSION: u32 = 2;

/// The Parquet key/value metadata key carrying [`SCHEMA_VERSION`].
const SCHEMA_KEY: &str = "bsh.schema";

/// The schema version of a file written before the metadata existed.
///
/// Those files have the original seven columns; the reader decodes them with
/// documented defaults, which is why the version is supported rather than
/// unknown.
const LEGACY_SCHEMA_VERSION: u32 = 1;

/// How long a mutation waits for the cross-process history lock (#353).
///
/// Long enough to let a peer finish a commit, short enough that a UI thread
/// never hangs on a process that has stopped responding; the wait then becomes
/// the typed lock error below.
const LOCK_BUDGET: Duration = Duration::from_millis(250);
/// Unsealed save segments allowed before a history is rewritten whole.
///
/// A legacy history - one save per segment, written before the manifest
/// existed - can carry thousands of three-row files, and a page over that
/// region opens one file per segment. Rewriting the log into sealed segments
/// costs O(history) once and makes every later page a single open.
#[cfg(not(test))]
const COMPACT_AFTER_SEGMENTS: usize = 8;
#[cfg(test)]
const COMPACT_AFTER_SEGMENTS: usize = 2;
/// A compaction marker older than this is debris from a killed process.
const COMPACTION_STALE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// A typed history failure a caller can act on instead of parsing a string.
///
/// The reliability kernel's Primitive 3 (#351/#355) adds the read, decode,
/// schema, manifest and oversized variants; the lock variant lives here because
/// the bounded cross-process acquire (#353) has to be distinguishable from a
/// generic I/O error, and a caller may retry it while another process works.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HistoryError {
    /// Another process holds the history directory's cross-process lock.
    #[error("history directory {path} is locked by another process")]
    Locked {
        /// The lock file that is held.
        path: String,
    },
}

/// What validating a history's index against its Parquet files concluded (#356).
///
/// The state is part of the history, so a caller can show "the log was rebuilt"
/// or "rows are unavailable" instead of silently serving a shorter transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexState {
    /// The manifest agrees with every segment file on disk.
    Valid,
    /// The index was rebuilt from the Parquet footers; no data file was changed.
    Rebuilt {
        /// Why the manifest could not be trusted.
        reason: String,
    },
    /// Rows the manifest promised cannot be read; the manifest was kept and
    /// reads of the missing rows fail explicitly instead of being shortened.
    Unavailable {
        /// What is missing or unreadable.
        reason: String,
    },
}

/// One segment's entry in a [`HistoryDiagnosis`] (#356).
#[allow(dead_code)] // the diagnostics surface a caller shows; tests prove the fields
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentDiagnosis {
    /// The file name.
    pub(crate) file: String,
    /// The segment list the file belongs to: "msg" for messages, "traj" for
    /// the trajectory.
    pub(crate) kind: &'static str,
    /// Whether the manifest lists this file.
    pub(crate) listed: bool,
    /// Whether the Parquet footer could be read.
    pub(crate) readable: bool,
    /// Rows the footer reports, when it could be read.
    pub(crate) rows: Option<usize>,
    /// The `bsh.schema` value in the footer; `None` for a legacy file.
    pub(crate) schema: Option<u32>,
    /// Whether this reader can decode the file's column set.
    pub(crate) schema_supported: bool,
    /// Whether the manifest's numbers agree with the footer. An unlisted file
    /// has nothing to disagree with and is reported as consistent.
    pub(crate) consistent: bool,
}

/// The offline validation report for one history directory (#356).
///
/// Built from the manifest and the Parquet footers only: no payload is
/// decoded, so the cost is bounded by the segment count. `findings` is the
/// seam #355 extends with malformed-column findings.
#[allow(dead_code)] // the diagnostics surface a caller shows; tests prove the fields
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryDiagnosis {
    /// What the walk concluded about the index.
    pub(crate) state: IndexState,
    /// One entry per segment file found on disk, messages first.
    pub(crate) segments: Vec<SegmentDiagnosis>,
    /// Human-readable problems, in discovery order.
    pub(crate) findings: Vec<String>,
}

/// A [`CommitGate`] that forwards to the test gate registered for one directory.
///
/// Production registers nothing, so every stage passes. A test registers one
/// gate for one history directory, which is what lets a specific stage fail for
/// a specific history without racing the other tests in the process.
struct DirectoryGate<'a> {
    dir: &'a Path,
}

impl CommitGate for DirectoryGate<'_> {
    fn pass(&self, stage: IoStage, path: &Path) -> io::Result<()> {
        match fault_gate(self.dir) {
            Some(gate) => gate.pass(stage, path),
            None => Ok(()),
        }
    }
}

/// The fault gate registered for a directory, if any (test builds only).
#[cfg(test)]
fn fault_gate(dir: &Path) -> Option<Arc<dyn CommitGate>> {
    fault::registry().lock().ok()?.get(dir).cloned()
}

/// Production builds inject no faults.
#[cfg(not(test))]
fn fault_gate(_dir: &Path) -> Option<Arc<dyn CommitGate>> {
    None
}

/// The test-only fault-injection seam for history writes (#352).
///
/// It is keyed by the history directory, so one test can fail, say, the
/// manifest replacement for its own scratch directory while every other test
/// (and every other directory) commits normally.
#[cfg(test)]
mod fault {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    use hostkit::durable::CommitGate;

    type Registry = Mutex<HashMap<PathBuf, Arc<dyn CommitGate>>>;

    /// The one process-wide registry.
    pub(super) fn registry() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn lock() -> MutexGuard<'static, HashMap<PathBuf, Arc<dyn CommitGate>>> {
        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Register `gate` for `dir`; the guard clears it again on drop.
    pub(super) fn install(dir: &Path, gate: Arc<dyn CommitGate>) -> Installed {
        lock().insert(dir.to_path_buf(), gate);
        Installed {
            dir: dir.to_path_buf(),
        }
    }

    /// Clears the registration it created, even when the test panics.
    pub(super) struct Installed {
        dir: PathBuf,
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            lock().remove(&self.dir);
        }
    }
}

/// One written segment: its file and how many rows of the log it holds.
#[derive(Debug, Clone)]
pub(crate) struct Segment {
    pub(crate) path: PathBuf,
    /// Global row index this segment begins at.
    pub(crate) start: usize,
    pub(crate) rows: usize,
    pub(crate) bytes: u64,
    /// Whether this segment is large enough not to be merged with its
    /// neighbours. Small tail segments stay unsealed until they reach a target.
    pub(crate) sealed: bool,
    /// Rows in each row group, in read order.
    ///
    /// A segment's last group is usually short, and a save appends a segment of
    /// its own, so a global page can span a short segment and the next one. The
    /// reader uses these counts to locate a global row across those boundaries
    /// instead of assuming every group holds PAGE rows.
    pub(crate) group_rows: Vec<usize>,
}

/// One segment as the manifest stores it: the per-segment integrity record.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestSegment {
    /// The file name, relative to the history directory.
    file: String,
    /// Rows the segment holds.
    rows: usize,
    /// Bytes the file occupied when the manifest was written.
    bytes: u64,
    /// Whether the segment may still be merged into a sealed one.
    sealed: bool,
    /// Rows in each row group, in read order.
    group_rows: Vec<usize>,
    /// The schema version the writer recorded; 0 when an older manifest did
    /// not record one per segment.
    #[serde(default)]
    schema_version: u32,
}

/// The per-directory metadata index.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    /// The manifest shape; see [`MANIFEST_VERSION`].
    version: u32,
    /// The history schema the listed segments were written with.
    #[serde(default)]
    schema_version: u32,
    /// The message segments, in write order.
    messages: Vec<ManifestSegment>,
    /// The trajectory segments, in write order.
    trajectory: Vec<ManifestSegment>,
    /// This index was created by scanning a directory that had none, so the
    /// files beside it are the old one-save-one-segment shape. A manifest
    /// written by a compaction clears it.
    #[serde(default)]
    legacy: bool,
}

/// A session's history on disk, read in pages.
pub(crate) struct History {
    dir: PathBuf,
    /// The segment index, shared with background readers: a background page
    /// read clones this in O(1) instead of copying every path.
    messages: Arc<Vec<Segment>>,
    trajectory: Arc<Vec<Segment>>,
    rows: usize,
    trajectory_rows: usize,
    /// The unsealed tail's totals, so an append does not walk every small
    /// segment to decide whether it is time to seal.
    unsealed_rows: usize,
    unsealed_bytes: u64,
    unsealed_trajectory_rows: usize,
    unsealed_trajectory_bytes: u64,
    /// This history was first opened without a manifest, i.e. it was written by
    /// the code from before the index existed.
    legacy: bool,
    /// Whether the live index agrees with the manifest's files (#356).
    state: IndexState,
    /// The per-directory lock shared by every History opened on this path in
    /// the process: a save and a compaction's swap cannot interleave.
    lock: Arc<RwLock<()>>,
    cache: RefCell<PageCache>,
}

/// An immutable snapshot of a history's segment index.
///
/// Reading a page from it touches only the Parquet files, so the snapshot is
/// Send and can travel to the background executor while the UI thread keeps
/// the live History. Cloning is O(1) because the index itself is shared.
#[derive(Clone)]
pub(crate) struct Reader {
    messages: Arc<Vec<Segment>>,
    trajectory: Arc<Vec<Segment>>,
    trajectory_rows: usize,
}

impl Reader {
    /// Pages the trajectory holds.
    pub(crate) fn trajectory_pages(&self) -> usize {
        self.trajectory_rows.div_ceil(PAGE)
    }

    /// A global row range of messages.
    pub(crate) fn messages(&self, from: usize, len: usize) -> Result<Vec<ChatMessage>> {
        read_range(&self.messages, from, len)
    }

    /// The trajectory page counted from the newest end.
    pub(crate) fn trajectory_page(&self, page: usize) -> Result<Vec<ToolRow>> {
        if page >= self.trajectory_pages() {
            return Ok(Vec::new());
        }
        let first = (self.trajectory_pages() - 1 - page) * PAGE;
        read_tools_range(&self.trajectory, first, PAGE)
    }
}

/// What a compaction did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compaction {
    /// The history was already sealed; nothing was written.
    Noop,
    /// Another process is compacting this history, so this call stood down.
    Busy,
    /// The log was rewritten, with this many rows on each side.
    Rewritten {
        /// Message rows in the new log.
        messages: usize,
        /// Trajectory rows in the new log.
        trajectory: usize,
    },
}

impl History {
    /// The directory a session's history lives in, beside its document.
    pub(crate) fn dir_for(session_dir: &Path, id: &str) -> PathBuf {
        session_dir.join("history").join(id)
    }

    /// Open a history over its directory. Missing files are an empty history.
    ///
    /// The manifest answers the common case, but every entry is validated
    /// against its Parquet footer (#356). A stale or missing manifest is
    /// rebuilt from the footers without touching a data file; rows that cannot
    /// be accounted for leave the manifest in place and the history in
    /// [`IndexState::Unavailable`].
    ///
    /// Repair is a mutation, so it needs the directory's cross-process lock. A
    /// lock that is busy means another live process owns the directory: this
    /// call keeps reading (from a scan) and lets the owner publish the repair.
    /// The crash-recovery rename of an interrupted swap is part of the repair,
    /// so it too is left to the live owner when the lock is busy.
    pub(crate) fn open(dir: PathBuf) -> Self {
        let repair_lock = FileLock::try_acquire(&dir).ok();
        let lock_held = repair_lock.is_some();
        if lock_held {
            recover_swap(&dir);
        }
        let history = match read_manifest(&dir) {
            ManifestRead::Valid(index) => Self::with_tail(dir, index),
            ManifestRead::Unavailable { index, reason } => {
                let mut history = Self::with_tail(dir, index);
                history.state = match &history.state {
                    IndexState::Unavailable { reason: tail } => IndexState::Unavailable {
                        reason: format!("{reason}; {tail}"),
                    },
                    _ => IndexState::Unavailable { reason },
                };
                history
            }
            ManifestRead::Repair(repair) => Self::rebuild(dir, repair, lock_held),
        };
        drop(repair_lock);
        history
    }

    /// Build the index from a manifest verdict plus the unlisted tail files.
    ///
    /// Files the manifest does not list are the unsealed tail written since the
    /// last commit. They are few (the seal target bounds them), so only their
    /// footers are opened. An unreadable tail file is not skipped silently: the
    /// history reports it as unavailable.
    fn with_tail(dir: PathBuf, mut index: Index) -> Self {
        let messages = scan_missing(&dir, "msg", &index.listed);
        let trajectory = scan_missing(&dir, "traj", &index.listed);
        let mut skipped = messages.skipped;
        skipped.extend(trajectory.skipped);
        index.messages.extend(messages.segments);
        index.trajectory.extend(trajectory.segments);
        let mut history = Self::from_segments(dir, index.messages, index.trajectory);
        history.legacy = index.legacy;
        if !skipped.is_empty() {
            history.state = IndexState::Unavailable {
                reason: format!(
                    "unreadable segment files in the tail: {}",
                    skipped.join(", ")
                ),
            };
        }
        history
    }

    /// Rebuild the index from Parquet footers after a repairable manifest.
    ///
    /// Footers are the only thing read and no data file is modified. When the
    /// rebuild accounts for every row the manifest promised, a fresh manifest
    /// is published atomically (when this call holds the cross-process lock).
    /// When it cannot, the manifest is kept and the history reports
    /// [`IndexState::Unavailable`] rather than silently shortening the log.
    fn rebuild(dir: PathBuf, repair: Repair, lock_held: bool) -> Self {
        let messages = scan_detailed(&dir, "msg", None, true);
        let trajectory = scan_detailed(&dir, "traj", None, true);
        let mut skipped = messages.skipped;
        skipped.extend(trajectory.skipped);
        let readable_rows = messages
            .segments
            .iter()
            .chain(trajectory.segments.iter())
            .map(|segment| segment.rows)
            .sum::<usize>();
        let promised = repair.promised_rows();
        let lost = repair.lost && promised.is_some_and(|promised| readable_rows < promised);
        if lost || !skipped.is_empty() {
            let mut reason = repair.reason;
            if lost {
                if let Some(promised) = promised {
                    reason.push_str(&format!(
                        "; the manifest promised {promised} rows and only {readable_rows} are readable"
                    ));
                }
            }
            if !skipped.is_empty() {
                reason.push_str(&format!(
                    "; unreadable segment files: {}",
                    skipped.join(", ")
                ));
            }
            let index = repair.index.unwrap_or_else(|| Index {
                messages: messages.segments,
                trajectory: trajectory.segments,
                listed: HashSet::new(),
                legacy: repair.legacy,
            });
            let mut history = Self::from_segments(dir, index.messages, index.trajectory);
            history.legacy = index.legacy;
            history.state = IndexState::Unavailable { reason };
            return history;
        }
        let mut history = Self::from_segments(dir, messages.segments, trajectory.segments);
        history.legacy = repair.legacy;
        if lock_held {
            let _ = history.write_manifest();
        }
        history.state = IndexState::Rebuilt {
            reason: repair.reason,
        };
        history
    }

    /// The live index verdict from opening this history (#356).
    #[cfg(test)]
    pub(crate) fn index_state(&self) -> &IndexState {
        &self.state
    }

    /// The offline validation report for this history's directory (#356).
    ///
    /// The walk reads the manifest and each segment's Parquet footer, never a
    /// payload, so its cost is bounded by the segment count. It runs under the
    /// read guard, so it cannot observe a half-finished swap.
    #[allow(dead_code)] // the diagnostics surface a caller shows; tests prove it
    pub(crate) fn diagnosis(&self) -> HistoryDiagnosis {
        let _guard = self.read_guard();
        let mut report = diagnose(&self.dir);
        // Keep the live verdict when it is the stronger one: a rebuild that
        // could not account for the rows is not undone by a later walk.
        if matches!(self.state, IndexState::Unavailable { .. }) {
            report.state = self.state.clone();
        }
        report
    }

    /// Take the directory's cross-process lock, waiting at most [`LOCK_BUDGET`].
    ///
    /// Every mutation calls this BEFORE it takes the in-process write guard.
    /// That is the module's one lock order (file lock, then write guard), so two
    /// writers can never deadlock; a thread that will not get the file lock
    /// returns the typed [`HistoryError::Locked`] instead of holding the guard.
    fn lock_directory(&self) -> Result<FileLock> {
        FileLock::acquire_bounded(&self.dir, LOCK_BUDGET).map_err(|error| match error {
            DurableError::Locked { path } => anyhow::Error::new(HistoryError::Locked { path }),
            other => anyhow::Error::new(other),
        })
    }

    /// A snapshot a background thread can read pages from.
    pub(crate) fn reader(&self) -> Reader {
        let _guard = self.read_guard();
        Reader {
            messages: Arc::clone(&self.messages),
            trajectory: Arc::clone(&self.trajectory),
            trajectory_rows: self.trajectory_rows,
        }
    }

    /// Whether this history should be rewritten into sealed segments once.
    ///
    /// A legacy history is one whose index had to be created by scanning: its
    /// segments are the old one-save-one-file shape. A history that gained a
    /// scattered tail later is caught by the segment threshold.
    pub(crate) fn needs_compaction(&self) -> bool {
        if self.rows == 0 && self.trajectory_rows == 0 {
            return false;
        }
        // A scanned legacy directory was marked sealed by the scanner, so its
        // shape - one small file per save - is the evidence, not the flag.
        let scattered = self.messages.len() > 1 || self.trajectory.len() > 1;
        self.legacy && scattered || self.unsealed_count() > COMPACT_AFTER_SEGMENTS
    }

    /// Unsealed segments across both logs.
    fn unsealed_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|segment| !segment.sealed)
            .count()
            + self
                .trajectory
                .iter()
                .filter(|segment| !segment.sealed)
                .count()
    }

    /// Assemble the in-memory index from two segment lists.
    fn from_segments(
        dir: PathBuf,
        mut messages: Vec<Segment>,
        mut trajectory: Vec<Segment>,
    ) -> Self {
        let rows = reindex(&mut messages);
        let trajectory_rows = reindex(&mut trajectory);
        let tail = |segments: &[Segment]| {
            let start = segments
                .iter()
                .rposition(|segment| segment.sealed)
                .map_or(0, |index| index + 1);
            let rows = segments[start..].iter().map(|segment| segment.rows).sum();
            let bytes = segments[start..].iter().map(|segment| segment.bytes).sum();
            (rows, bytes)
        };
        let (unsealed_rows, unsealed_bytes) = tail(&messages);
        let (unsealed_trajectory_rows, unsealed_trajectory_bytes) = tail(&trajectory);
        let lock = history_lock(&dir);
        Self {
            dir,
            lock,
            messages: Arc::new(messages),
            trajectory: Arc::new(trajectory),
            rows,
            trajectory_rows,
            unsealed_rows,
            unsealed_bytes,
            unsealed_trajectory_rows,
            unsealed_trajectory_bytes,
            legacy: false,
            state: IndexState::Valid,
            cache: RefCell::new(PageCache::new(CACHE_PAGES)),
        }
    }

    /// The per-directory read guard: a page read must not overlap a swap.
    fn read_guard(&self) -> RwLockReadGuard<'_, ()> {
        self.lock
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Write the metadata manifest atomically and durably (level Durable).
    fn write_manifest(&self) -> Result<()> {
        self.write_manifest_with(&self.messages, &self.trajectory)
    }

    /// Write the manifest for a candidate list before it is committed.
    ///
    /// Level: Durable. Publishing is a mutation, so every caller already holds
    /// the directory's cross-process lock (append/seal, compaction, rewrite) or
    /// took it for the repair (open).
    fn write_manifest_with(&self, messages: &[Segment], trajectory: &[Segment]) -> Result<()> {
        write_manifest_at(&self.dir, messages, trajectory, self.legacy)
    }

    /// Messages written so far.
    pub(crate) fn len(&self) -> usize {
        self.rows
    }

    /// Pages this history holds.
    #[cfg(test)]
    pub(crate) fn pages(&self) -> usize {
        self.rows.div_ceil(PAGE)
    }

    /// Pages the trajectory holds.
    pub(crate) fn trajectory_pages(&self) -> usize {
        self.trajectory_rows.div_ceil(PAGE)
    }

    /// Whether every segment is sealed, for the tests.
    #[cfg(test)]
    pub(crate) fn all_sealed(&self) -> bool {
        self.messages
            .iter()
            .chain(self.trajectory.iter())
            .all(|segment| segment.sealed)
    }

    /// Rows decoded from disk so far: a page read must decode a page.
    #[cfg(test)]
    pub(crate) fn decoded(&self) -> usize {
        self.cache.borrow().decoded
    }

    /// One page of messages, decoded from its row group or from the cache.
    #[cfg(test)]
    pub(crate) fn page(&self, page: usize) -> Result<Vec<ChatMessage>> {
        let _guard = self.read_guard();
        self.page_at(page)
    }

    /// A page read with the caller already holding the read guard.
    fn page_at(&self, page: usize) -> Result<Vec<ChatMessage>> {
        if let Some(hit) = self.cache.borrow_mut().get(page) {
            return Ok(hit);
        }
        let first = page * PAGE;
        if first >= self.rows {
            return Ok(Vec::new());
        }
        let rows = read_range(&self.messages, first, PAGE)?;
        self.cache.borrow_mut().put(page, rows.clone(), rows.len());
        Ok(rows)
    }

    /// A window of messages by global index, page by page.
    pub(crate) fn window(&self, from: usize, len: usize) -> Result<Vec<ChatMessage>> {
        let _guard = self.read_guard();
        self.window_at(from, len)
    }

    /// A window read with the caller already holding the read guard.
    fn window_at(&self, from: usize, len: usize) -> Result<Vec<ChatMessage>> {
        let mut rows = Vec::with_capacity(len.min(PAGE));
        let mut index = from;
        while rows.len() < len && index < self.rows {
            let page = self.page_at(index / PAGE)?;
            let offset = index % PAGE;
            for row in page.into_iter().skip(offset) {
                if rows.len() == len {
                    break;
                }
                rows.push(row);
            }
            index = (index / PAGE + 1) * PAGE;
        }
        Ok(rows)
    }

    /// One page of trajectory rows counted from the newest.
    ///
    /// The trace opens at the live end and pages upward, so its first page is
    /// the last one. The older page it asks for next is prepended, which keeps
    /// the rendered order oldest-to-newest.
    #[cfg(test)]
    pub(crate) fn trajectory_page_from_end(&self, page: usize) -> Result<Vec<ToolRow>> {
        let _guard = self.read_guard();
        let pages = self.trajectory_rows.div_ceil(PAGE);
        if page >= pages {
            return Ok(Vec::new());
        }
        let first = (pages - 1 - page) * PAGE;
        read_tools_range(&self.trajectory, first, PAGE)
    }

    /// The rows written BEFORE the loaded window, read page by page.
    ///
    /// A rewrite has to keep them: the in-memory window starts part way into the
    /// log, and rebuilding the log from the window alone would silently drop
    /// every page above it.
    pub(crate) fn prefix(&self, rows: usize) -> Result<Vec<ChatMessage>> {
        let _guard = self.read_guard();
        self.window_at(0, rows.min(self.rows))
    }

    /// Replace the whole log with these rows: a truncation, a compaction, or a
    /// session migrated from a document that still embedded its transcript.
    ///
    /// The replacement is built in a sibling directory and swapped in by
    /// renaming that directory over this one, exactly as [`Self::compact`]
    /// does. Deleting the live directory first - what this used to do - left a
    /// window in which a kill lost the ENTIRE conversation, not only the rows
    /// being written. A crash now leaves either the original log or the
    /// finished one, never neither.
    pub(crate) fn rewrite(&mut self, messages: &[ChatMessage]) -> Result<()> {
        let _file_lock = self.lock_directory()?;
        let lock = Arc::clone(&self.lock);
        let _guard = lock
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A killed rebuild leaves a sibling nothing lists. Only an old one is
        // debris: a live second window may be building its own right now.
        sweep_stale_siblings(&self.dir, "rewrite-");
        let temporary = sibling(&self.dir, &format!("rewrite-{}", std::process::id()));
        if temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        fs::create_dir_all(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        let wrote = (|| -> Result<()> {
            // A fresh history over the sibling writes the replacement's
            // segments and index; the live directory is untouched while it
            // builds.
            let mut fresh = History::open(temporary.clone());
            fresh.append(messages)?;
            // The replacement is private to this build, but publishing its
            // manifest is still a mutation and takes its own directory lock.
            let _fresh_lock = fresh.lock_directory()?;
            fresh.write_manifest()?;
            Ok(())
        })();
        if let Err(error) = wrote {
            let _ = fs::remove_dir_all(&temporary);
            drop_build_lock(&temporary);
            return Err(error);
        }
        let had_live = self.dir.exists();
        let backup = sibling(&self.dir, &format!("compact-backup-{}", std::process::id()));
        if had_live && backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }
        if had_live {
            fs::rename(&self.dir, &backup)
                .with_context(|| format!("setting {} aside", self.dir.display()))?;
        }
        if let Err(error) = fs::rename(&temporary, &self.dir) {
            if had_live {
                let _ = fs::rename(&backup, &self.dir);
            }
            let _ = fs::remove_dir_all(&temporary);
            drop_build_lock(&temporary);
            return Err(error).with_context(|| format!("installing {}", self.dir.display()));
        }
        if had_live {
            let _ = fs::remove_dir_all(&backup);
        }
        // The build directory is gone; its private lock file must not outlive
        // it, because it sits beside the live history directory.
        drop_build_lock(&temporary);
        *self = History::open(self.dir.clone());
        Ok(())
    }

    /// Append the messages written since the last save, in one segment.
    ///
    /// Durability: the message segment and the trajectory segment are committed
    /// at [`CommitLevel::Durable`] before this returns (see
    /// docs/reliability-kernel.md section 2), so their contents and the
    /// directory entries that name them survive a power loss. The manifest is
    /// only rewritten when the unsealed tail reaches a seal target, and that
    /// publication is durable too.
    ///
    /// Locking: the directory's cross-process file lock is taken first and the
    /// in-process write guard second, which is the one order every mutation
    /// uses (#353). Another process holding the lock yields the typed
    /// [`HistoryError::Locked`] after a bounded wait instead of blocking.
    pub(crate) fn append(&mut self, messages: &[ChatMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let _file_lock = self.lock_directory()?;
        let lock = Arc::clone(&self.lock);
        let _guard = lock
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.append_locked(messages)
    }

    /// An append with the caller already holding the write guard.
    fn append_locked(&mut self, messages: &[ChatMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let path = segment_path(&self.dir, "msg");
        let group_rows = write_messages(&path, self.rows, messages)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Arc::make_mut(&mut self.messages).push(Segment {
            path,
            start: self.rows,
            rows: messages.len(),
            bytes,
            sealed: false,
            group_rows,
        });
        self.rows += messages.len();
        self.unsealed_rows += messages.len();
        self.unsealed_bytes += bytes;
        let tools: Vec<ToolRow> = messages
            .iter()
            .flat_map(|message| message.tools.iter())
            .map(|tool| ToolRow {
                name: tool.name.clone(),
                args: tool.args.clone(),
                output: tool.output.clone(),
                ms: tool.ms,
            })
            .collect();
        if !tools.is_empty() {
            let path = segment_path(&self.dir, "traj");
            let group_rows = write_tools(&path, &tools)?;
            let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            Arc::make_mut(&mut self.trajectory).push(Segment {
                path,
                start: self.trajectory_rows,
                rows: tools.len(),
                bytes,
                sealed: false,
                group_rows,
            });
            self.trajectory_rows += tools.len();
            self.unsealed_trajectory_rows += tools.len();
            self.unsealed_trajectory_bytes += bytes;
        }
        // A save does not rewrite the whole manifest: the new segment is an
        // unlisted tail file that the next open discovers. Sealing is the only
        // operation that commits a manifest, so a save stays O(new rows).
        //
        // A history whose index is unavailable does not seal: publishing a
        // manifest from the readable tail would replace the manifest that still
        // names the missing rows (#356).
        if !matches!(self.state, IndexState::Unavailable { .. }) {
            let _ = self.seal_tail();
        }
        self.cache.borrow_mut().clear();
        Ok(())
    }

    /// Merge a small unsealed tail into one large sealed segment.
    ///
    /// Saves stay O(new rows), but a history does not accumulate one file per
    /// turn forever: once the unsealed tail reaches a row or byte target it is
    /// rewritten once as a sealed segment. The manifest is installed before the
    /// old files are removed, so an interrupted seal leaves the previous
    /// manifest pointing at files that still exist.
    fn seal_tail(&mut self) -> Result<()> {
        self.seal_messages()?;
        self.seal_trajectory()?;
        Ok(())
    }

    fn seal_messages(&mut self) -> Result<()> {
        let first = self
            .messages
            .iter()
            .rposition(|segment| segment.sealed)
            .map_or(0, |index| index + 1);
        let tail = &self.messages[first..];
        if tail.is_empty() {
            return Ok(());
        }
        let rows = self.unsealed_rows;
        let bytes = self.unsealed_bytes;
        if tail.len() == 1 {
            if rows >= SEGMENT_ROWS || bytes >= SEGMENT_BYTES {
                let mut candidate = self.messages.as_ref().clone();
                candidate[first].sealed = true;
                self.write_manifest_with(&candidate, &self.trajectory)?;
                self.messages = Arc::new(candidate);
                self.unsealed_rows = 0;
                self.unsealed_bytes = 0;
            }
            return Ok(());
        }
        if rows < SEGMENT_ROWS && bytes < SEGMENT_BYTES {
            return Ok(());
        }
        let start = tail[0].start;
        let messages = read_range(tail, start, rows)?;
        let old: Vec<PathBuf> = tail.iter().map(|segment| segment.path.clone()).collect();
        let path = segment_path(&self.dir, "msg");
        let group_rows = write_messages(&path, start, &messages)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let mut candidate = self.messages[..first].to_vec();
        candidate.push(Segment {
            path: path.clone(),
            start,
            rows,
            bytes,
            sealed: true,
            group_rows,
        });
        let archived = archive_files(&old)?;
        // Install the candidate index before replacing the live list. If the
        // manifest cannot be written, the new file is removed and the old tail
        // is put back, so the previous manifest stays the truth.
        if let Err(error) = self.write_manifest_with(&candidate, &self.trajectory) {
            restore_files(&old, &archived);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        self.messages = Arc::new(candidate);
        self.unsealed_rows = 0;
        self.unsealed_bytes = 0;
        for path in archived {
            let _ = fs::remove_file(path);
        }
        self.cache.borrow_mut().clear();
        Ok(())
    }

    fn seal_trajectory(&mut self) -> Result<()> {
        let first = self
            .trajectory
            .iter()
            .rposition(|segment| segment.sealed)
            .map_or(0, |index| index + 1);
        let tail = &self.trajectory[first..];
        if tail.is_empty() {
            return Ok(());
        }
        let rows = self.unsealed_trajectory_rows;
        let bytes = self.unsealed_trajectory_bytes;
        if tail.len() == 1 {
            if rows >= SEGMENT_ROWS || bytes >= SEGMENT_BYTES {
                let mut candidate = self.trajectory.as_ref().clone();
                candidate[first].sealed = true;
                self.write_manifest_with(&self.messages, &candidate)?;
                self.trajectory = Arc::new(candidate);
                self.unsealed_trajectory_rows = 0;
                self.unsealed_trajectory_bytes = 0;
            }
            return Ok(());
        }
        if rows < SEGMENT_ROWS && bytes < SEGMENT_BYTES {
            return Ok(());
        }
        let start = tail[0].start;
        let tools = read_tools_range(tail, start, rows)?;
        let old: Vec<PathBuf> = tail.iter().map(|segment| segment.path.clone()).collect();
        let path = segment_path(&self.dir, "traj");
        let group_rows = write_tools(&path, &tools)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let mut candidate = self.trajectory[..first].to_vec();
        candidate.push(Segment {
            path: path.clone(),
            start,
            rows,
            bytes,
            sealed: true,
            group_rows,
        });
        let archived = archive_files(&old)?;
        if let Err(error) = self.write_manifest_with(&self.messages, &candidate) {
            restore_files(&old, &archived);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        self.trajectory = Arc::new(candidate);
        self.unsealed_trajectory_rows = 0;
        self.unsealed_trajectory_bytes = 0;
        for path in archived {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }

    /// Rewrite the whole log into sealed segments, once, for a history that
    /// predates the manifest or carries a scattered tail.
    ///
    /// The rewrite is built in a sibling directory and swapped in by renaming
    /// that directory over this one, so a crash leaves either the original log
    /// or the finished one - never a half-written mix. A marker file makes a
    /// concurrent compaction from a second window stand down instead of racing.
    ///
    /// Row order, every message field (images included) and the trajectory are
    /// preserved by reading both logs back page by page while the new segments
    /// are written. A compacted history is therefore already sealed: calling
    /// this again is a no-op.
    pub(crate) fn compact(&mut self) -> Result<Compaction> {
        // The whole compaction - build, swap and marker - runs under the
        // directory's cross-process lock. A peer that is already rewriting this
        // history is not an error: compaction is an optimization, so this call
        // stands down (Busy) instead of waiting on a possibly long rewrite.
        let _file_lock = match self.lock_directory() {
            Ok(lock) => lock,
            Err(error) if is_locked(&error) => return Ok(Compaction::Busy),
            Err(error) => return Err(error),
        };
        // A history whose rows cannot be accounted for is not rewritten: the
        // rebuild would drop exactly the rows that are missing (#356).
        if matches!(self.state, IndexState::Unavailable { .. }) {
            return Ok(Compaction::Noop);
        }
        // A seal that died between archiving and committing leaves .old files
        // nothing lists; they are debris either way. The sweep removes temp and
        // archive debris too, so it must run under both locks: every temp name
        // carries a pid and a sequence, and a live writer holds the locks, so
        // anything it finds here belongs to a process that is gone.
        {
            let lock = Arc::clone(&self.lock);
            let _guard = lock
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            sweep_archived(&self.dir);
        }
        if !self.needs_compaction() {
            return Ok(Compaction::Noop);
        }
        let Some(guard) = CompactionGuard::acquire(&self.dir)? else {
            return Ok(Compaction::Busy);
        };
        // Another process may have won the race between our check and the
        // marker; re-read the index under the marker.
        let fresh = History::open(self.dir.clone());
        if !fresh.needs_compaction() {
            *self = fresh;
            return Ok(Compaction::Noop);
        }
        // Stale building and backup directories can only be debris now that the
        // marker is ours and the live directory is open.
        sweep_siblings(&self.dir, "compact-");
        sweep_siblings(&self.dir, "compact-backup-");
        let temporary = sibling(&self.dir, &format!("compact-{}", std::process::id()));
        if temporary.exists() {
            let _ = fs::remove_dir_all(&temporary);
        }
        fs::create_dir_all(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        let wrote = (|| -> Result<(Vec<Segment>, Vec<Segment>)> {
            let messages = compact_messages(&fresh, &temporary)?;
            let trajectory = compact_trajectory(&fresh, &temporary)?;
            write_manifest_at(&temporary, &messages, &trajectory, false)?;
            Ok((messages, trajectory))
        })();
        let (messages, trajectory) = match wrote {
            Ok(written) => written,
            Err(error) => {
                let _ = fs::remove_dir_all(&temporary);
                return Err(error);
            }
        };
        // A save may have appended a turn while the rewrite ran. The swap would
        // take the live directory away with that row still unread, so the final
        // check, the swap and a save serialize on the directory's lock: a save
        // that lands first is seen here and stands the compaction down, one
        // that lands second waits for the new directory.
        let lock = Arc::clone(&self.lock);
        let swap_guard = lock
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let latest = History::open(self.dir.clone());
        if latest.rows != fresh.rows || latest.trajectory_rows != fresh.trajectory_rows {
            let _ = fs::remove_dir_all(&temporary);
            return Ok(Compaction::Busy);
        }
        let backup = sibling(&self.dir, &format!("compact-backup-{}", std::process::id()));
        if backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }
        // The swap: the original moves aside whole, the finished directory takes
        // its place. A failure puts the original back before returning.
        fs::rename(&self.dir, &backup)
            .with_context(|| format!("setting {} aside", self.dir.display()))?;
        if let Err(error) = fs::rename(&temporary, &self.dir) {
            let _ = fs::rename(&backup, &self.dir);
            let _ = fs::remove_dir_all(&temporary);
            return Err(error).with_context(|| format!("installing {}", self.dir.display()));
        }
        let _ = fs::remove_dir_all(&backup);
        drop(swap_guard);
        drop(guard);
        *self = History::open(self.dir.clone());
        Ok(Compaction::Rewritten {
            messages: messages.iter().map(|segment| segment.rows).sum(),
            trajectory: trajectory.iter().map(|segment| segment.rows).sum(),
        })
    }
}

/// Rewrite one history directory on its own: the seam the background task
/// calls, because it owns no UI state and opens its own index.
pub(crate) fn compact_directory(dir: &Path) -> Result<Compaction> {
    let mut history = History::open(dir.to_path_buf());
    history.compact()
}

/// Rows of decoded pages, least recently used first.
struct PageCache {
    entries: VecDeque<(usize, Vec<ChatMessage>)>,
    capacity: usize,
    decoded: usize,
}

impl PageCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
            decoded: 0,
        }
    }

    fn get(&mut self, page: usize) -> Option<Vec<ChatMessage>> {
        let at = self.entries.iter().position(|(key, _)| *key == page)?;
        let entry = self.entries.remove(at)?;
        let rows = entry.1.clone();
        self.entries.push_back(entry);
        Some(rows)
    }

    fn put(&mut self, page: usize, rows: Vec<ChatMessage>, decoded: usize) {
        self.decoded += decoded;
        self.entries.retain(|(key, _)| *key != page);
        self.entries.push_back((page, rows));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// One trajectory row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolRow {
    pub(crate) name: String,
    pub(crate) args: String,
    pub(crate) output: Option<String>,
    pub(crate) ms: Option<u64>,
}

/// One process-wide sequence for the names this module creates.
///
/// Segment ids, archive names and debris names carry it beside the pid, so two
/// processes (and a clock that jumps backwards) cannot choose the same name.
static NAME_SEQ: AtomicU64 = AtomicU64::new(0);

/// A millisecond stamp that never repeats within this process.
///
/// Wall clock alone is not enough: two saves in one millisecond, or a clock
/// that steps backwards, would otherwise pick the same segment id.
fn next_stamp() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => prev = current,
        }
    }
}

/// A segment path no existing file occupies.
///
/// The name is `prefix-<stamp:013>-<pid:05>-<seq:03>.parquet`. The pid and the
/// per-process sequence are what keep two processes apart; if the chosen path
/// already exists (a copied directory, a clock anomaly) the sequence is bumped
/// rather than letting the commit rename over an existing segment.
fn segment_path(dir: &Path, prefix: &str) -> PathBuf {
    loop {
        let sequence = NAME_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{prefix}-{:013}-{:05}-{:03}.parquet",
            next_stamp(),
            std::process::id(),
            sequence
        ));
        if !path.exists() {
            return path;
        }
    }
}

/// A pid and a per-process sequence for one debris name.
fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        NAME_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// The numeric first field of a segment name, legacy or current shape.
///
/// `msg-999.parquet` gives 999 and `msg-0000000000123-1234-000.parquet` gives
/// 123, so a migrated history and the segments written after it still sort in
/// write order. u64::MAX marks a name with no numeric first field, which sorts
/// last and makes the manifest validator reject the entry instead of guessing.
fn stamp_of(name: &str) -> u64 {
    let stem = name.strip_suffix(".parquet").unwrap_or(name);
    let rest = stem.split_once('-').map_or(stem, |(_, rest)| rest);
    rest.split('-')
        .next()
        .and_then(|stamp| stamp.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Whether an error is the typed "another process holds the lock" failure.
fn is_locked(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<HistoryError>(),
        Some(HistoryError::Locked { .. })
    )
}

/// Move a seal's old files aside before its manifest is installed.
///
/// A crash between the manifest write and the delete would otherwise leave the
/// old `.parquet` files as unlisted tail files, which the next open would read
/// as duplicate rows. `.parquet.old` is ignored by the scanners.
fn archive_files(old: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let archived: Vec<PathBuf> = old.iter().map(|path| archive_path(path)).collect();
    for (index, (from, to)) in old.iter().zip(archived.iter()).enumerate() {
        if let Err(error) = fs::rename(from, to) {
            for (from, to) in old.iter().zip(archived.iter()).take(index) {
                let _ = fs::rename(to, from);
            }
            return Err(error).with_context(|| format!("archiving {}", from.display()));
        }
    }
    Ok(archived)
}

/// The name one old segment is moved aside to.
///
/// `.msg-000001.parquet.old-<pid>-<seq>` is ignored by the scanners and unique
/// per process, so a second process's debris never shares it.
fn archive_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.old-{}", unique_suffix()))
}

/// Undo `archive_files` after a failed manifest commit.
fn restore_files(old: &[PathBuf], archived: &[PathBuf]) {
    for (from, to) in old.iter().zip(archived.iter()) {
        let _ = fs::rename(to, from);
    }
}

/// The write lock for one history directory.
///
/// Every History opened on the same path in this process gets the same lock,
/// which is what lets a save and a compaction's check-and-swap serialize while
/// the long rewrite still runs outside it. A weak registry keeps the map from
/// holding a directory's lock after its last History is dropped.
fn history_lock(dir: &Path) -> Arc<RwLock<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<RwLock<()>>>>> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = registry
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = locks.get(dir).and_then(Weak::upgrade) {
        return existing;
    }
    let lock = Arc::new(RwLock::new(()));
    locks.insert(dir.to_path_buf(), Arc::downgrade(&lock));
    lock
}

/// Remove the lock file of a private build directory.
///
/// A build sibling is named with the pid, so only this process can have locked
/// it; the lock file is not shared with a live writer and must not outlive the
/// directory, or it becomes debris beside the live history directory.
fn drop_build_lock(build: &Path) {
    let _ = fs::remove_file(sibling(build, "lock"));
}

/// A path beside one history directory: the sibling the swap builds in.
fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    dir.with_file_name(format!("{name}.{suffix}"))
}

/// Remove the files a crashed write or seal left beside the live segments.
fn sweep_archived(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let debris = name.ends_with(".parquet.old")
            || name.ends_with(".parquet.tmp")
            || (name.starts_with('.') && (name.contains(".tmp-") || name.contains(".old-")))
            || (name.starts_with(".manifest-") && name.ends_with(".tmp"));
        if debris {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Remove sibling directories a killed compaction left behind.
fn sweep_siblings(dir: &Path, suffix: &str) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let prefix = format!("{}.{suffix}", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            // A killed build can leave only its lock file behind.
            let _ = fs::remove_file(&path);
        }
    }
}

/// Sweep only rebuild siblings old enough to be a killed process's debris.
///
/// Unlike [`sweep_siblings`], a second window may be building its own
/// replacement right now, so a young sibling is left alone.
fn sweep_stale_siblings(dir: &Path, suffix: &str) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let prefix = format!("{}.{suffix}", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let stale = fs::metadata(entry.path())
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > COMPACTION_STALE);
        if stale {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Put the original back if a compaction died between the two renames.
///
/// The swap renames the live directory aside, then renames the finished one
/// into place. A crash in that window leaves the original as a backup sibling
/// and no live directory; the next open restores it rather than reading an
/// empty history.
fn recover_swap(dir: &Path) {
    if dir.exists() {
        return;
    }
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let prefix = format!("{}.compact-backup-", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    backups.sort();
    if let Some(backup) = backups.pop() {
        let _ = fs::rename(&backup, dir);
    }
}

/// The one-time rewrite's marker: one compactor per history directory.
struct CompactionGuard {
    path: PathBuf,
}

impl CompactionGuard {
    fn acquire(dir: &Path) -> Result<Option<Self>> {
        let path = sibling(dir, "compacting");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                let _ = file.write_all(std::process::id().to_string().as_bytes());
                Ok(Some(Self { path }))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // A marker older than the stale window is debris from a killed
                // process, not a live compaction.
                let stale = fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > COMPACTION_STALE);
                if !stale {
                    return Ok(None);
                }
                let _ = fs::remove_file(&path);
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(mut file) => {
                        let _ = file.write_all(std::process::id().to_string().as_bytes());
                        Ok(Some(Self { path }))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
                    Err(error) => Err(error).with_context(|| format!("marking {}", path.display())),
                }
            }
            Err(error) => Err(error).with_context(|| format!("marking {}", path.display())),
        }
    }
}

impl Drop for CompactionGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Write a history's messages as sealed segments into a fresh directory.
fn compact_messages(source: &History, dir: &Path) -> Result<Vec<Segment>> {
    let mut segments = Vec::new();
    let mut from = 0usize;
    while from < source.rows {
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        while rows.len() < SEGMENT_ROWS
            && bytes < SEGMENT_BYTES as usize
            && from + rows.len() < source.rows
        {
            let len = PAGE.min(source.rows - from - rows.len());
            let page = read_range(&source.messages, from + rows.len(), len)?;
            if page.is_empty() {
                break;
            }
            bytes += page.iter().map(message_size).sum::<usize>();
            rows.extend(page);
        }
        if rows.is_empty() {
            break;
        }
        let path = segment_path(dir, "msg");
        let group_rows = write_messages(&path, from, &rows)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        segments.push(Segment {
            path,
            start: from,
            rows: rows.len(),
            bytes,
            sealed: true,
            group_rows,
        });
        from += rows.len();
    }
    Ok(segments)
}

/// Write a history's trajectory as sealed segments into a fresh directory.
fn compact_trajectory(source: &History, dir: &Path) -> Result<Vec<Segment>> {
    let total = source.trajectory_rows;
    let mut segments = Vec::new();
    let mut from = 0usize;
    while from < total {
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        while rows.len() < SEGMENT_ROWS
            && bytes < SEGMENT_BYTES as usize
            && from + rows.len() < total
        {
            let len = PAGE.min(total - from - rows.len());
            let page = read_tools_range(&source.trajectory, from + rows.len(), len)?;
            if page.is_empty() {
                break;
            }
            bytes += page
                .iter()
                .map(|tool| {
                    64 + tool.name.len()
                        + tool.args.len()
                        + tool.output.as_ref().map_or(0, String::len)
                })
                .sum::<usize>();
            rows.extend(page);
        }
        if rows.is_empty() {
            break;
        }
        let path = segment_path(dir, "traj");
        let group_rows = write_tools(&path, &rows)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        segments.push(Segment {
            path,
            start: from,
            rows: rows.len(),
            bytes,
            sealed: true,
            group_rows,
        });
        from += rows.len();
    }
    Ok(segments)
}

/// Prefix-sum the segment starts and return the total row count.
fn reindex(segments: &mut [Segment]) -> usize {
    let mut start = 0usize;
    for segment in segments.iter_mut() {
        segment.start = start;
        start += segment.rows;
    }
    start
}

/// Write the metadata index for a directory atomically and durably.
///
/// Level: Durable (docs/reliability-kernel.md section 2). The bytes go through
/// [durable::commit], so the manifest is synced, renamed over the previous one
/// and its directory entry is synced; a failure at any stage leaves the
/// previous manifest intact. Callers hold the directory's cross-process lock
/// because publishing an index is a mutation (#353).
fn write_manifest_at(
    dir: &Path,
    messages: &[Segment],
    trajectory: &[Segment],
    legacy: bool,
) -> Result<()> {
    if messages.is_empty() && trajectory.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        schema_version: SCHEMA_VERSION,
        messages: messages.iter().map(manifest_segment).collect(),
        trajectory: trajectory.iter().map(manifest_segment).collect(),
        legacy,
    };
    let bytes = serde_json::to_vec(&manifest)?;
    let path = dir.join(MANIFEST);
    durable::commit(&path, &bytes, Privacy::Inherit, &DirectoryGate { dir })
        .map_err(|error| anyhow::anyhow!("installing {}: {error}", path.display()))
}

/// What a directory's index says: both segment lists, the files it names and
/// whether that index was created by a legacy scan.
struct Index {
    messages: Vec<Segment>,
    trajectory: Vec<Segment>,
    listed: HashSet<String>,
    legacy: bool,
}

/// The manifest record for one live segment.
fn manifest_segment(segment: &Segment) -> ManifestSegment {
    ManifestSegment {
        file: segment
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        rows: segment.rows,
        bytes: segment.bytes,
        sealed: segment.sealed,
        group_rows: segment.group_rows.clone(),
        schema_version: SCHEMA_VERSION,
    }
}

/// The verdict of reading the manifest against the segment files on disk.
enum ManifestRead {
    /// Every listed segment agrees with its footer.
    Valid(Index),
    /// The manifest cannot be trusted; the index must be rebuilt from footers.
    Repair(Repair),
    /// A listed segment exists but carries a column set this reader does not
    /// know; the manifest is kept and reads of those rows fail explicitly.
    Unavailable {
        /// The manifest's own index, kept as the promise.
        index: Index,
        /// What makes the rows unreadable.
        reason: String,
    },
}

/// A repairable manifest problem: the files are the truth, the index is stale.
struct Repair {
    /// Why the manifest could not be trusted.
    reason: String,
    /// The legacy flag the manifest carried (or that the directory is old).
    legacy: bool,
    /// The manifest's own index, used to keep its promise when the rebuild
    /// cannot account for the rows it listed.
    index: Option<Index>,
    /// A listed segment is missing or unreadable, so the rebuild has to prove
    /// that its rows still exist somewhere before shortening the log.
    lost: bool,
}

impl Repair {
    /// Rows the manifest promised, when it could be parsed.
    fn promised_rows(&self) -> Option<usize> {
        self.index.as_ref().map(|index| {
            index
                .messages
                .iter()
                .chain(index.trajectory.iter())
                .map(|segment| segment.rows)
                .sum()
        })
    }
}

/// The verdict for one manifest list.
enum ListCheck {
    /// The list agrees with the files.
    Agree,
    /// The manifest's metadata is stale; a rebuild from the footers fixes it.
    Rebuild(String),
    /// A listed segment is missing or unreadable.
    Lost(String),
    /// A listed segment carries a schema version this reader does not know.
    Unsupported(String),
}

/// Read the manifest and validate it against the segment files (#356).
///
/// Bounded cost: the manifest plus one Parquet footer per listed segment, never
/// a payload decode. The manifest still saves decoding, not the footer reads.
fn read_manifest(dir: &Path) -> ManifestRead {
    let Some(manifest) = load_manifest(dir) else {
        if !has_segments(dir) {
            return ManifestRead::Valid(empty_index());
        }
        return ManifestRead::Repair(Repair {
            reason: "the manifest is missing or unreadable".to_string(),
            legacy: true,
            index: None,
            lost: false,
        });
    };
    if manifest.version == 0 || manifest.version > MANIFEST_VERSION {
        return ManifestRead::Repair(Repair {
            reason: format!(
                "manifest version {} is newer than the {} this build writes",
                manifest.version, MANIFEST_VERSION
            ),
            legacy: manifest.legacy,
            index: None,
            lost: false,
        });
    }
    if manifest.schema_version > SCHEMA_VERSION {
        return ManifestRead::Repair(Repair {
            reason: format!(
                "manifest schema version {} is newer than the {} this build reads",
                manifest.schema_version, SCHEMA_VERSION
            ),
            legacy: manifest.legacy,
            index: None,
            lost: false,
        });
    }
    let mut listed = HashSet::new();
    let mut rebuild = None;
    let mut lost = None;
    let mut unsupported = None;
    for (prefix, entries) in [
        ("msg", manifest.messages.as_slice()),
        ("traj", manifest.trajectory.as_slice()),
    ] {
        match check_list(dir, prefix, entries, &mut listed) {
            ListCheck::Agree => {}
            ListCheck::Rebuild(reason) => {
                rebuild.get_or_insert(reason);
            }
            ListCheck::Lost(reason) => {
                lost.get_or_insert(reason);
            }
            ListCheck::Unsupported(reason) => {
                unsupported.get_or_insert(reason);
            }
        }
    }
    let legacy = manifest.legacy;
    let index = index_from_manifest(dir, &manifest, listed);
    if let Some(reason) = unsupported {
        return ManifestRead::Unavailable { index, reason };
    }
    if let Some(reason) = lost {
        return ManifestRead::Repair(Repair {
            reason,
            legacy,
            index: Some(index),
            lost: true,
        });
    }
    if let Some(reason) = rebuild {
        return ManifestRead::Repair(Repair {
            reason,
            legacy,
            index: Some(index),
            lost: false,
        });
    }
    ManifestRead::Valid(index)
}

/// Validate one manifest list against the segment files.
fn check_list(
    dir: &Path,
    prefix: &str,
    entries: &[ManifestSegment],
    listed: &mut HashSet<String>,
) -> ListCheck {
    let mut previous: Option<(u64, &str)> = None;
    for entry in entries {
        let name = entry.file.as_str();
        let foreign = name.contains('/')
            || name.contains('\\')
            || !name.starts_with(prefix)
            || !name.ends_with(".parquet");
        let stamp = stamp_of(name);
        if foreign || stamp == u64::MAX {
            return ListCheck::Rebuild(format!(
                "the manifest lists {name:?}, which is not a {prefix} segment name"
            ));
        }
        if !listed.insert(entry.file.clone()) {
            return ListCheck::Rebuild(format!("the manifest lists {name} twice"));
        }
        let key = (stamp, name);
        if let Some(previous) = previous {
            if key <= previous {
                return ListCheck::Rebuild(format!("the manifest lists {name} out of order"));
            }
        }
        previous = Some(key);
        if entry.rows == 0
            || entry.group_rows.is_empty()
            || entry.group_rows.iter().sum::<usize>() != entry.rows
        {
            return ListCheck::Rebuild(format!("the manifest's row counts are stale for {name}"));
        }
        let path = dir.join(name);
        let Some(footer) = footer_facts(&path) else {
            return if path.exists() {
                ListCheck::Lost(format!("segment {name} cannot be read"))
            } else {
                ListCheck::Lost(format!("segment {name} is missing"))
            };
        };
        if !footer.schema_supported() {
            return ListCheck::Unsupported(format!(
                "segment {name} carries unsupported schema version {}",
                footer.schema.unwrap_or(0)
            ));
        }
        if footer.rows == 0 {
            // The file exists but holds no rows while the manifest promises
            // some: those rows cannot be accounted for, so this is not a
            // repairable stale count.
            return ListCheck::Lost(format!("segment {name} reports no rows"));
        }
        if footer.rows != entry.rows
            || footer.group_rows != entry.group_rows
            || footer.bytes != entry.bytes
        {
            return ListCheck::Rebuild(format!("the manifest's numbers are stale for {name}"));
        }
        let recorded = entry.schema_version;
        let actual = footer.schema.unwrap_or(LEGACY_SCHEMA_VERSION);
        if recorded != 0 && recorded != actual {
            return ListCheck::Rebuild(format!(
                "the manifest records schema {recorded} for {name} but the file carries {actual}"
            ));
        }
    }
    ListCheck::Agree
}

/// Parse the manifest without validating it against the files.
fn load_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = fs::read(dir.join(MANIFEST)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether the directory holds any file that names a segment.
fn has_segments(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        (name.starts_with("msg-") || name.starts_with("traj-")) && name.ends_with(".parquet")
    })
}

/// An index with no segments.
fn empty_index() -> Index {
    Index {
        messages: Vec::new(),
        trajectory: Vec::new(),
        listed: HashSet::new(),
        legacy: false,
    }
}

/// The manifest's own index, used as the promise even when its numbers are stale.
fn index_from_manifest(dir: &Path, manifest: &Manifest, listed: HashSet<String>) -> Index {
    let segments = |entries: &[ManifestSegment]| {
        entries
            .iter()
            .map(|entry| Segment {
                path: dir.join(&entry.file),
                start: 0,
                rows: entry.rows,
                bytes: entry.bytes,
                sealed: entry.sealed,
                group_rows: entry.group_rows.clone(),
            })
            .collect::<Vec<_>>()
    };
    Index {
        messages: segments(&manifest.messages),
        trajectory: segments(&manifest.trajectory),
        listed,
        legacy: manifest.legacy,
    }
}

/// The footer facts one Parquet segment carries, read without decoding a payload.
struct FooterFacts {
    /// Rows the footer reports.
    rows: usize,
    /// Rows per row group, in read order.
    group_rows: Vec<usize>,
    /// The file's length in bytes.
    bytes: u64,
    /// The recorded `bsh.schema`; `None` for a file written before it existed.
    schema: Option<u32>,
}

impl FooterFacts {
    /// Whether this reader can decode the file's column set.
    fn schema_supported(&self) -> bool {
        match self.schema {
            None => true,
            Some(version) => version == SCHEMA_VERSION,
        }
    }
}

/// Read a segment's Parquet footer: row-group counts, byte length and the
/// recorded schema version. No column data is read.
fn footer_facts(path: &Path) -> Option<FooterFacts> {
    let file = fs::File::open(path).ok()?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).ok()?;
    let metadata = builder.metadata();
    let groups = metadata.num_row_groups();
    let group_rows: Vec<usize> = (0..groups)
        .map(|group| metadata.row_group(group).num_rows() as usize)
        .collect();
    let schema = metadata
        .file_metadata()
        .key_value_metadata()
        .and_then(|values| {
            values
                .iter()
                .find(|value| value.key == SCHEMA_KEY)
                .and_then(|value| value.value.as_deref())
                .and_then(|value| value.parse().ok())
        });
    let bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    Some(FooterFacts {
        rows: group_rows.iter().sum(),
        group_rows,
        bytes,
        schema,
    })
}

/// What a directory scan found.
struct Scanned {
    /// The readable segments, in write order.
    segments: Vec<Segment>,
    /// Names of files that look like segments but cannot be read or decoded.
    skipped: Vec<String>,
}

/// Every segment of one kind in a session directory, in write order.
#[cfg(test)]
fn scan(dir: &Path, prefix: &str) -> Vec<Segment> {
    scan_detailed(dir, prefix, None, true).segments
}

/// Scan only the files a manifest does not list, as the unsealed tail.
fn scan_missing(dir: &Path, prefix: &str, listed: &HashSet<String>) -> Scanned {
    scan_detailed(dir, prefix, Some(listed), false)
}

/// Scan one segment list, sorting by the numeric stamp and keeping every file
/// that cannot be read as a skip instead of dropping it silently (#356).
fn scan_detailed(
    dir: &Path,
    prefix: &str,
    listed: Option<&HashSet<String>>,
    sealed: bool,
) -> Scanned {
    let mut scanned = Scanned {
        segments: Vec::new(),
        skipped: Vec::new(),
    };
    for (name, path) in segment_files(dir, prefix) {
        if listed.is_some_and(|listed| listed.contains(&name)) {
            continue;
        }
        let Some(footer) = footer_facts(&path) else {
            scanned.skipped.push(name);
            continue;
        };
        if !footer.schema_supported() {
            scanned.skipped.push(name);
            continue;
        }
        if footer.rows == 0 {
            continue;
        }
        scanned.segments.push(Segment {
            path,
            start: 0,
            rows: footer.rows,
            bytes: footer.bytes,
            sealed,
            group_rows: footer.group_rows,
        });
    }
    scanned
}

/// The segment files of one kind in a directory, in write order.
///
/// The key is the numeric stamp with the name as the tie-break, so a legacy
/// three-digit name and a millisecond name sort correctly: text order alone put
/// migrated history after the pages that followed it.
fn segment_files(dir: &Path, prefix: &str) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(prefix) && name.ends_with(".parquet") {
                Some((name, entry.path()))
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| (stamp_of(&a.0), &a.0).cmp(&(stamp_of(&b.0), &b.0)));
    files
}

/// Walk a history directory and report per-segment validity (#356).
///
/// This is the offline validation path: it reads the manifest and each
/// segment's Parquet footer, never a payload, so its cost is bounded by the
/// segment count. The `findings` list is where #355 adds malformed-column
/// findings; today it reports the manifest, file, row-count and schema
/// problems this module can prove from metadata alone.
#[allow(dead_code)] // the diagnostics surface a caller shows; tests prove it
pub(crate) fn diagnose(dir: &Path) -> HistoryDiagnosis {
    let mut findings = Vec::new();
    let mut unavailable = false;
    let manifest = load_manifest(dir);
    let mut expected: HashMap<String, ManifestSegment> = HashMap::new();
    if let Some(manifest) = &manifest {
        if manifest.version == 0 || manifest.version > MANIFEST_VERSION {
            findings.push(format!(
                "manifest version {} is not supported",
                manifest.version
            ));
        }
        if manifest.schema_version > SCHEMA_VERSION {
            findings.push(format!(
                "manifest schema version {} is not supported",
                manifest.schema_version
            ));
        }
        for (prefix, entries) in [("msg", &manifest.messages), ("traj", &manifest.trajectory)] {
            let mut previous: Option<(u64, &str)> = None;
            for entry in entries {
                if !entry.file.starts_with(prefix) || !entry.file.ends_with(".parquet") {
                    findings.push(format!("{prefix} lists a foreign name {:?}", entry.file));
                }
                let key = (stamp_of(&entry.file), entry.file.as_str());
                if let Some(previous) = previous {
                    if key <= previous {
                        findings.push(format!("{prefix} lists {} out of order", entry.file));
                    }
                }
                previous = Some(key);
                let name = entry.file.clone();
                if expected.insert(name.clone(), entry.clone()).is_some() {
                    findings.push(format!("the manifest lists {name} twice"));
                }
            }
        }
    } else if has_segments(dir) {
        findings.push("the manifest is missing or unreadable".to_string());
    }
    let mut segments = Vec::new();
    for (kind, prefix) in [("msg", "msg"), ("traj", "traj")] {
        for (name, path) in segment_files(dir, prefix) {
            let listed = expected.get(&name);
            let mut diagnosis = SegmentDiagnosis {
                file: name.clone(),
                kind,
                listed: listed.is_some(),
                readable: false,
                rows: None,
                schema: None,
                schema_supported: false,
                consistent: true,
            };
            match footer_facts(&path) {
                Some(footer) => {
                    diagnosis.readable = true;
                    diagnosis.rows = Some(footer.rows);
                    diagnosis.schema = footer.schema;
                    diagnosis.schema_supported = footer.schema_supported();
                    if !diagnosis.schema_supported {
                        unavailable = true;
                        findings.push(format!(
                            "{name} carries unsupported schema version {}",
                            footer.schema.unwrap_or(0)
                        ));
                    }
                    if let Some(listed) = listed {
                        diagnosis.consistent = listed.rows == footer.rows
                            && listed.group_rows == footer.group_rows
                            && listed.bytes == footer.bytes;
                        if !diagnosis.consistent {
                            findings.push(format!("the manifest's numbers are stale for {name}"));
                        }
                        if footer.rows == 0 && listed.rows > 0 {
                            unavailable = true;
                            findings.push(format!(
                                "{name} reports no rows but the manifest lists {}",
                                listed.rows
                            ));
                        }
                    }
                }
                None => {
                    unavailable = true;
                    diagnosis.consistent = false;
                    findings.push(format!("{name} cannot be read"));
                }
            }
            segments.push(diagnosis);
        }
    }
    for name in expected.keys() {
        if !segments.iter().any(|segment| &segment.file == name) {
            unavailable = true;
            findings.push(format!("the manifest lists {name}, which is missing"));
        }
    }
    let reason = findings.join("; ");
    let state = if unavailable {
        IndexState::Unavailable { reason }
    } else if findings.is_empty() {
        IndexState::Valid
    } else {
        IndexState::Rebuilt { reason }
    };
    HistoryDiagnosis {
        state,
        segments,
        findings,
    }
}

/// Read a global row range across segments and short row groups.
///
/// A save appends a segment of its own, so most segments are shorter than PAGE
/// and a page can begin in one segment and continue in the next. Locating by
/// `first / PAGE` alone would read the wrong rows; this walks the actual group
/// row counts instead.
fn read_range(segments: &[Segment], first: usize, len: usize) -> Result<Vec<ChatMessage>> {
    let mut rows = Vec::with_capacity(len.min(PAGE));
    if len == 0 {
        return Ok(rows);
    }
    let begin = segments.partition_point(|segment| segment.start + segment.rows <= first);
    for (position, segment) in segments[begin..].iter().enumerate() {
        let local = if position == 0 {
            first.saturating_sub(segment.start)
        } else {
            0
        };
        let mut offset = local;
        for (group, group_rows) in segment.group_rows.iter().enumerate() {
            if offset >= *group_rows {
                offset -= *group_rows;
                continue;
            }
            let batch = read_group_at(&segment.path, group, *group_rows)?;
            for row in decode_messages(&batch)?.into_iter().skip(offset) {
                rows.push(row);
                if rows.len() == len {
                    return Ok(rows);
                }
            }
            offset = 0;
        }
    }
    Ok(rows)
}

/// The trajectory counterpart of `read_range`.
fn read_tools_range(segments: &[Segment], first: usize, len: usize) -> Result<Vec<ToolRow>> {
    let mut rows = Vec::with_capacity(len.min(PAGE));
    if len == 0 {
        return Ok(rows);
    }
    let begin = segments.partition_point(|segment| segment.start + segment.rows <= first);
    for (position, segment) in segments[begin..].iter().enumerate() {
        let local = if position == 0 {
            first.saturating_sub(segment.start)
        } else {
            0
        };
        let mut offset = local;
        for (group, group_rows) in segment.group_rows.iter().enumerate() {
            if offset >= *group_rows {
                offset -= *group_rows;
                continue;
            }
            let batch = read_group_at(&segment.path, group, *group_rows)?;
            for row in decode_tools(&batch)?.into_iter().skip(offset) {
                rows.push(row);
                if rows.len() == len {
                    return Ok(rows);
                }
            }
            offset = 0;
        }
    }
    Ok(rows)
}

/// Decode one row group, using its exact row count as the batch size.
fn read_group_at(path: &Path, group: usize, rows: usize) -> Result<RecordBatch> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?
        .with_row_groups(vec![group])
        .with_batch_size(rows.max(1));
    let mut reader = builder.build()?;
    let batch = reader
        .next()
        .transpose()?
        .context("a row group that decodes to nothing")?;
    if batch.num_rows() != rows {
        anyhow::bail!(
            "row group {group} of {} holds {} rows, expected {rows}",
            path.display(),
            batch.num_rows()
        );
    }
    Ok(batch)
}

fn message_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("index", DataType::Int64, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("reasoning", DataType::Utf8, false),
        Field::new("error", DataType::Utf8, true),
        Field::new("meta", DataType::Utf8, true),
        Field::new("tools", DataType::Utf8, false),
        // Appended after the original schema; old files simply have fewer
        // columns and decode with no images.
        Field::new("images", DataType::Utf8, false),
        // Appended with steering; a file written before it decodes every row
        // as a turn prompt, which is what those rows were.
        Field::new("steering", DataType::Boolean, false),
        // Appended with the turn timeline; an older file decodes every row with
        // no segments, which draws in the older order.
        Field::new("parts", DataType::Utf8, false),
    ]))
}

fn tool_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("index", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("args", DataType::Utf8, false),
        Field::new("output", DataType::Utf8, true),
        Field::new("ms", DataType::UInt64, true),
    ]))
}

fn writer_props() -> WriterProperties {
    // Compression is one purpose; the row group boundaries come from flush(),
    // because a row group is the page this store reads by. The key/value
    // metadata names the column set, so a reader can reject a schema it does
    // not know instead of guessing (#356).
    WriterProperties::builder()
        .set_compression(parquet::basic::Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![parquet::file::metadata::KeyValue::new(
            SCHEMA_KEY.to_string(),
            SCHEMA_VERSION.to_string(),
        )]))
        .build()
}

/// The fault-injection gate for the directory that holds `path` (#352).
///
/// Production registers no faults. Tests register one gate per history
/// directory, so a specific stage fails for one history without racing the
/// rest of the suite.
fn directory_gate(path: &Path) -> DirectoryGate<'_> {
    DirectoryGate {
        dir: path.parent().unwrap_or_else(|| Path::new("")),
    }
}

/// Write one message segment, in row groups of at most PAGE rows.
///
/// Level: Durable (docs/reliability-kernel.md section 2). The Arrow writer
/// streams straight into the temp file the commit kernel created with
/// create_new(true), so a 64 MB segment is never buffered here; the kernel
/// then syncs the file, hardens it, renames it into place and syncs the parent
/// directory. A failure at any stage removes the temp and leaves the previous
/// segments untouched.
///
/// Returns the rows in each group so the reader can locate a global row across
/// short segments instead of assuming every group is PAGE rows.
fn write_messages(path: &Path, base: usize, messages: &[ChatMessage]) -> Result<Vec<usize>> {
    let mut group_rows = Vec::new();
    durable::commit_stream(
        path,
        Privacy::Inherit,
        CommitLevel::Durable,
        &directory_gate(path),
        |file| {
            let mut writer = ArrowWriter::try_new(file, message_schema(), Some(writer_props()))
                .map_err(io::Error::other)?;
            for chunk in message_chunks(messages) {
                writer
                    .write(&message_batch(chunk, base).map_err(io::Error::other)?)
                    .map_err(io::Error::other)?;
                // A flush closes the row group: one page per group, which is
                // what the reader addresses.
                writer.flush().map_err(io::Error::other)?;
                group_rows.push(chunk.len());
            }
            writer.close().map_err(io::Error::other)?;
            Ok(())
        },
    )?;
    Ok(group_rows)
}

/// Split messages into row groups bounded by both rows and approximate bytes.
fn message_chunks(messages: &[ChatMessage]) -> Vec<&[ChatMessage]> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        let size = message_size(message);
        if index > start && (index - start >= PAGE || bytes + size > ROW_GROUP_BYTES) {
            chunks.push(&messages[start..index]);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
    }
    if start < messages.len() {
        chunks.push(&messages[start..]);
    }
    chunks
}

/// A cheap upper-bound estimate for one message's row-group weight.
fn message_size(message: &ChatMessage) -> usize {
    128 + message.text.len()
        + message.reasoning.len()
        + message.images.iter().map(String::len).sum::<usize>()
        + message.error.as_ref().map_or(0, String::len)
        + message
            .tools
            .iter()
            .map(|tool| {
                tool.name.len() + tool.args.len() + tool.output.as_ref().map_or(0, String::len) + 48
            })
            .sum::<usize>()
}

/// Split trajectory rows into row groups bounded by rows and approximate bytes.
fn tool_chunks(tools: &[ToolRow]) -> Vec<&[ToolRow]> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0usize;
    for (index, tool) in tools.iter().enumerate() {
        let size =
            64 + tool.name.len() + tool.args.len() + tool.output.as_ref().map_or(0, String::len);
        if index > start && (index - start >= PAGE || bytes + size > ROW_GROUP_BYTES) {
            chunks.push(&tools[start..index]);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
    }
    if start < tools.len() {
        chunks.push(&tools[start..]);
    }
    chunks
}

fn message_batch(messages: &[ChatMessage], base: usize) -> Result<RecordBatch> {
    let mut index = Int64Builder::with_capacity(messages.len());
    let mut role = StringBuilder::new();
    let mut text = StringBuilder::new();
    let mut reasoning = StringBuilder::new();
    let mut error = StringBuilder::new();
    let mut meta = StringBuilder::new();
    let mut tools = StringBuilder::new();
    let mut images = StringBuilder::new();
    let mut steering = BooleanBuilder::new();
    let mut parts = StringBuilder::new();
    for (offset, message) in messages.iter().enumerate() {
        index.append_value((base + offset) as i64);
        role.append_value(match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        });
        text.append_value(&message.text);
        reasoning.append_value(&message.reasoning);
        match &message.error {
            Some(value) => error.append_value(value),
            None => error.append_null(),
        }
        match &message.meta {
            Some(value) => meta.append_value(serde_json::to_string(value)?),
            None => meta.append_null(),
        }
        tools.append_value(serde_json::to_string(&message.tools)?);
        images.append_value(serde_json::to_string(&message.images)?);
        steering.append_value(message.steering);
        parts.append_value(serde_json::to_string(&message.parts)?);
    }
    Ok(RecordBatch::try_new(
        message_schema(),
        vec![
            Arc::new(index.finish()),
            Arc::new(role.finish()),
            Arc::new(text.finish()),
            Arc::new(reasoning.finish()),
            Arc::new(error.finish()),
            Arc::new(meta.finish()),
            Arc::new(tools.finish()),
            Arc::new(images.finish()),
            Arc::new(steering.finish()),
            Arc::new(parts.finish()),
        ],
    )?)
}

/// Write one trajectory segment at [CommitLevel::Durable].
///
/// Same pipeline and promise as [write_messages]: the Arrow writer streams into
/// the kernel's temp file, and the kernel syncs, renames and syncs the parent
/// directory before this returns (docs/reliability-kernel.md section 2).
fn write_tools(path: &Path, tools: &[ToolRow]) -> Result<Vec<usize>> {
    let mut group_rows = Vec::new();
    durable::commit_stream(
        path,
        Privacy::Inherit,
        CommitLevel::Durable,
        &directory_gate(path),
        |file| {
            let mut writer = ArrowWriter::try_new(file, tool_schema(), Some(writer_props()))
                .map_err(io::Error::other)?;
            for (chunk_index, chunk) in tool_chunks(tools).into_iter().enumerate() {
                let mut index = Int64Builder::with_capacity(chunk.len());
                let mut name = StringBuilder::new();
                let mut args = StringBuilder::new();
                let mut output = StringBuilder::new();
                let mut ms = UInt64Builder::new();
                for (offset, tool) in chunk.iter().enumerate() {
                    index.append_value((chunk_index * PAGE + offset) as i64);
                    name.append_value(&tool.name);
                    args.append_value(&tool.args);
                    match &tool.output {
                        Some(value) => output.append_value(value),
                        None => output.append_null(),
                    }
                    match tool.ms {
                        Some(value) => ms.append_value(value),
                        None => ms.append_null(),
                    }
                }
                let batch = RecordBatch::try_new(
                    tool_schema(),
                    vec![
                        Arc::new(index.finish()) as ArrayRef,
                        Arc::new(name.finish()),
                        Arc::new(args.finish()),
                        Arc::new(output.finish()),
                        Arc::new(ms.finish()),
                    ],
                )
                .map_err(io::Error::other)?;
                writer.write(&batch).map_err(io::Error::other)?;
                writer.flush().map_err(io::Error::other)?;
                group_rows.push(chunk.len());
            }
            writer.close().map_err(io::Error::other)?;
            Ok(())
        },
    )?;
    Ok(group_rows)
}

fn decode_messages(batch: &RecordBatch) -> Result<Vec<ChatMessage>> {
    let role = column::<StringArray>(batch, 1)?;
    let text = column::<StringArray>(batch, 2)?;
    let reasoning = column::<StringArray>(batch, 3)?;
    let error = column::<StringArray>(batch, 4)?;
    let meta = column::<StringArray>(batch, 5)?;
    let tools = column::<StringArray>(batch, 6)?;
    let images = (batch.num_columns() > 7)
        .then(|| column::<StringArray>(batch, 7))
        .transpose()?;
    let steering = (batch.num_columns() > 8)
        .then(|| column::<BooleanArray>(batch, 8))
        .transpose()?;
    let parts = (batch.num_columns() > 9)
        .then(|| column::<StringArray>(batch, 9))
        .transpose()?;
    let mut rows = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        rows.push(ChatMessage {
            role: if role.value(index) == "user" {
                Role::User
            } else {
                Role::Assistant
            },
            text: text.value(index).to_string(),
            reasoning: reasoning.value(index).to_string(),
            tools: serde_json::from_str(tools.value(index)).unwrap_or_default(),
            images: images
                .and_then(|images| serde_json::from_str(images.value(index)).ok())
                .unwrap_or_default(),
            error: error
                .is_valid(index)
                .then(|| error.value(index).to_string()),
            meta: meta
                .is_valid(index)
                .then(|| serde_json::from_str::<MessageMeta>(meta.value(index)).ok())
                .flatten(),
            // The invoked-skill blocks are display-only and never ride the
            // request; the log does not carry them.
            skills: Vec::new(),
            injections: Vec::new(),
            // The question card is display-only, like the skill blocks: the
            // log does not carry it.
            question: None,
            steering: steering.as_ref().is_some_and(|column| column.value(index)),
            // A file written before the timeline had no column: its rows draw
            // in the older reasoning-then-text-then-steps order.
            parts: parts
                .and_then(|column| serde_json::from_str(column.value(index)).ok())
                .unwrap_or_default(),
        });
    }
    Ok(rows)
}

fn decode_tools(batch: &RecordBatch) -> Result<Vec<ToolRow>> {
    use arrow::array::UInt64Array;
    let name = column::<StringArray>(batch, 1)?;
    let args = column::<StringArray>(batch, 2)?;
    let output = column::<StringArray>(batch, 3)?;
    let ms = column::<UInt64Array>(batch, 4)?;
    let mut rows = Vec::with_capacity(batch.num_rows());
    for index in 0..batch.num_rows() {
        rows.push(ToolRow {
            name: name.value(index).to_string(),
            args: args.value(index).to_string(),
            output: output
                .is_valid(index)
                .then(|| output.value(index).to_string()),
            ms: ms.is_valid(index).then(|| ms.value(index)),
        });
    }
    Ok(rows)
}

fn column<T: 'static>(batch: &RecordBatch, index: usize) -> Result<&T> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<T>()
        .with_context(|| format!("column {index} is not the type the schema promised"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ToolEvent, TurnPart};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-history-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn message(index: usize) -> ChatMessage {
        ChatMessage {
            role: if index.is_multiple_of(2) {
                Role::User
            } else {
                Role::Assistant
            },
            text: format!("turn {index} text"),
            reasoning: format!("why {index}"),
            tools: if index.is_multiple_of(3) {
                vec![ToolEvent {
                    name: "bash".to_string(),
                    args: format!("{{\"command\": \"run {index}\"}}"),
                    output: Some(format!("out {index}")),
                    ms: Some(index as u64),
                    at: None,
                }]
            } else {
                Vec::new()
            },
            images: if index.is_multiple_of(11) {
                vec![format!("data:image/png;base64,{index}")]
            } else {
                Vec::new()
            },
            error: (index.is_multiple_of(7)).then(|| format!("boom {index}")),
            meta: (index.is_multiple_of(5)).then(MessageMeta::default),
            skills: Vec::new(),
            injections: Vec::new(),
            question: None,
            // An even index is a user row; make one of them a steering row so
            // the round trip proves the flag survives the log.
            steering: index == 2,
            // An assistant row carries a timeline so the round trip proves the
            // ORDER travels with the log, not only the text.
            parts: if index.is_multiple_of(2) {
                Vec::new()
            } else {
                let mut parts = vec![
                    TurnPart::Thinking(format!("why {index}").len()),
                    TurnPart::Content(format!("turn {index} text").len()),
                ];
                if index.is_multiple_of(3) {
                    parts.push(TurnPart::Tool(0));
                }
                parts
            },
        }
    }

    /// The manifest is the fast path; deleting it falls back to a scan without
    /// losing a row.
    #[test]
    fn the_manifest_avoids_a_rescan_and_a_missing_one_falls_back() {
        let dir = scratch("manifest");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..300).map(message).collect();
        history.append(&rows).expect("append");
        drop(history);
        // Opening the history is the first commit point; a save alone leaves
        // the new segment as an unlisted tail file.
        let reopened = History::open(dir.clone());
        assert!(dir.join(MANIFEST).is_file(), "the manifest is written");
        assert_eq!(reopened.len(), 300);
        assert_eq!(reopened.page(0).expect("page")[0].text, "turn 0 text");

        fs::remove_file(dir.join(MANIFEST)).expect("remove manifest");
        let rescanned = History::open(dir.clone());
        assert_eq!(rescanned.len(), 300);
        assert_eq!(
            rescanned.window(299, 1).expect("last row")[0].text,
            "turn 299 text"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A tail of small save segments is merged once it crosses the segment
    /// target, so the file count does not grow with every turn.
    #[test]
    fn a_small_tail_is_sealed() {
        let dir = scratch("seal");
        let mut history = History::open(dir.clone());
        let turns = 400;
        for turn in 0..turns {
            let rows: Vec<ChatMessage> = (turn * 2..turn * 2 + 2).map(message).collect();
            history.append(&rows).expect("append");
        }
        assert!(
            history.messages.iter().any(|segment| segment.sealed),
            "the tail reached a target and was sealed"
        );
        assert!(
            history.messages.len() < turns,
            "sealing keeps the segment count below the save count"
        );
        assert_eq!(history.len(), turns * 2);
        let first = history.window(0, 2).expect("first rows");
        assert_eq!(first[0].text, "turn 0 text");
        let tail = history.window(turns * 2 - 2, 2).expect("last rows");
        assert_eq!(tail[1].text, format!("turn {} text", turns * 2 - 1));

        // The manifest describes both the sealed and the unsealed tail, so a
        // fresh open reads every row without rescanning the Parquet footers.
        drop(history);
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), turns * 2);
        assert_eq!(
            reopened.window(0, 2).expect("first rows")[0].text,
            "turn 0 text"
        );
        assert_eq!(
            reopened.window(turns * 2 - 2, 2).expect("last rows")[1].text,
            format!("turn {} text", turns * 2 - 1)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A global page crosses a short append segment instead of assuming every
    /// row group is PAGE rows. This is the shape every real save produces: each
    /// turn appends a segment of its own, usually only two rows.
    #[test]
    fn a_window_spans_short_segments() {
        let dir = scratch("short-segments");
        let mut history = History::open(dir.clone());
        let first: Vec<ChatMessage> = (0..100).map(message).collect();
        let second: Vec<ChatMessage> = (100..200).map(message).collect();
        history.append(&first).expect("first segment");
        history.append(&second).expect("second segment");

        assert_eq!(history.page(0).expect("page zero").len(), PAGE);
        assert_eq!(
            history.page(0).expect("page zero")[PAGE - 1].text,
            format!("turn {} text", PAGE - 1)
        );
        assert_eq!(
            history.page(1).expect("page one").len(),
            200 - PAGE,
            "the second page starts at global row PAGE, not at segment two"
        );
        assert_eq!(
            history.page(1).expect("page one")[0].text,
            format!("turn {PAGE} text")
        );

        let window = history.window(120, 5).expect("window");
        for (offset, row) in window.iter().enumerate() {
            assert_eq!(row.text, format!("turn {} text", 120 + offset));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The trajectory pages use the same global row counts, so a page can span
    /// two tool segments.
    #[test]
    fn a_trajectory_page_spans_short_segments() {
        let dir = scratch("short-trajectory");
        let mut history = History::open(dir.clone());
        let rows = |range: std::ops::Range<usize>| -> Vec<ChatMessage> {
            range
                .map(|index| {
                    let mut row = message(index);
                    row.tools = vec![crate::model::ToolEvent {
                        name: format!("tool {index}"),
                        args: String::new(),
                        output: None,
                        ms: Some(index as u64),
                        at: None,
                    }];
                    row
                })
                .collect()
        };
        history.append(&rows(0..100)).expect("first segment");
        history.append(&rows(100..200)).expect("second segment");

        let first = history
            .trajectory_page_from_end(1)
            .expect("oldest trajectory page");
        assert_eq!(first.len(), PAGE);
        assert_eq!(first[PAGE - 1].name, format!("tool {}", PAGE - 1));
        let second = history
            .trajectory_page_from_end(0)
            .expect("newest trajectory page");
        assert_eq!(second.len(), 200 - PAGE);
        assert_eq!(second[0].name, format!("tool {PAGE}"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Rows survive the round trip: text, reasoning, error and the tools.
    /// Segments are stamped by wall clock, so a restored session appends AFTER its
    /// own history instead of restarting the counter at zero.
    #[test]
    fn a_segment_stamp_is_strictly_increasing() {
        let first = next_stamp();
        let second = next_stamp();
        assert!(second > first, "{second} follows {first}");
        // Three digits of milliseconds sort before the new names, so legacy
        // history still reads first rather than interleaving with it.
        assert!(stamp_of("msg-999.parquet") < stamp_of(&format!("msg-{first:013}.parquet")));
    }
    #[test]
    fn a_history_round_trips_through_parquet() {
        let dir = scratch("round-trip");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..300).map(message).collect();
        history.append(&rows).expect("append");
        assert_eq!(history.len(), 300);
        assert_eq!(history.pages(), 3, "300 rows of 128 = 3 pages");
        for page in 0..history.pages() {
            let decoded = history.page(page).expect("page");
            let start = page * PAGE;
            for (offset, row) in decoded.iter().enumerate() {
                let expected = &rows[start + offset];
                assert_eq!(row.text, expected.text);
                assert_eq!(row.reasoning, expected.reasoning);
                assert_eq!(row.error, expected.error);
                assert_eq!(row.tools.len(), expected.tools.len());
                assert_eq!(row.images, expected.images, "images survive the log");
                assert_eq!(
                    row.steering, expected.steering,
                    "the steering flag survives the log"
                );
                assert_eq!(
                    row.parts, expected.parts,
                    "the turn timeline survives the log"
                );
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A page read decodes a page, not the history - and the cache answers a
    /// second read without touching the file.
    #[test]
    fn a_page_read_decodes_one_page() {
        let dir = scratch("paging");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..1000).map(message).collect();
        history.append(&rows).expect("append");
        assert_eq!(history.decoded(), 0, "opening decodes nothing");

        let page = history.page(4).expect("page four");
        assert_eq!(page.len(), PAGE);
        assert_eq!(
            history.decoded(),
            PAGE,
            "reading one page decodes one page, not 1000 rows"
        );

        let _again = history.page(4).expect("cached page");
        assert_eq!(history.decoded(), PAGE, "the cache answers the second read");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The cache has a ceiling, so a long scroll cannot grow it without bound.
    #[test]
    fn the_page_cache_keeps_its_ceiling() {
        let dir = scratch("cache");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..(PAGE * 40)).map(message).collect();
        history.append(&rows).expect("append");
        for page in 0..40 {
            let _ = history.page(page).expect("page");
        }
        assert!(
            history.cache.borrow().entries.len() <= CACHE_PAGES,
            "the cache holds at most {CACHE_PAGES} pages"
        );
        assert_eq!(history.decoded(), PAGE * 40, "and a miss decodes one page");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A second save appends a segment; the old rows are still there.
    #[test]
    fn appending_costs_one_segment_not_the_history() {
        let dir = scratch("append");
        let mut history = History::open(dir.clone());
        let first: Vec<ChatMessage> = (0..PAGE).map(message).collect();
        history.append(&first).expect("first");
        let second: Vec<ChatMessage> = (PAGE..PAGE + 3).map(message).collect();
        history.append(&second).expect("second");
        assert_eq!(history.len(), PAGE + 3);
        assert_eq!(history.pages(), 2);
        let tail = history.window(PAGE, 3).expect("tail");
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].text, format!("turn {PAGE} text"));
        assert_eq!(scan(&dir, "msg").len(), 2, "two saves, two segments");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A window that spans a page boundary reads both pages.
    #[test]
    fn a_window_crosses_a_page_boundary() {
        let dir = scratch("window");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..(PAGE * 3)).map(message).collect();
        history.append(&rows).expect("append");
        let from = PAGE - 4;
        let window = history.window(from, 10).expect("window");
        assert_eq!(window.len(), 10);
        for (offset, row) in window.iter().enumerate() {
            assert_eq!(row.text, format!("turn {} text", from + offset));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The trace opens at the newest end: page 0 is the last page, and the next
    /// page is the one before it, so prepending keeps chronological order.
    #[test]
    fn the_trajectory_pages_from_the_newest_end() {
        let dir = scratch("trajectory-from-end");
        let mut history = History::open(dir.clone());
        let rows = |range: std::ops::Range<usize>| -> Vec<ChatMessage> {
            range
                .map(|index| {
                    let mut row = message(index);
                    row.tools = vec![ToolEvent {
                        name: format!("tool {index}"),
                        args: String::new(),
                        output: None,
                        ms: Some(index as u64),
                        at: None,
                    }];
                    row
                })
                .collect()
        };
        history.append(&rows(0..100)).expect("first");
        history.append(&rows(100..200)).expect("second");

        let newest = history.trajectory_page_from_end(0).expect("newest");
        assert_eq!(newest.len(), 200 - PAGE);
        assert_eq!(newest[0].name, format!("tool {PAGE}"));
        assert_eq!(newest.last().expect("last").name, "tool 199");
        let older = history.trajectory_page_from_end(1).expect("older");
        assert_eq!(older.len(), PAGE);
        assert_eq!(older[0].name, "tool 0");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The old writer's shape: a handful of tiny segments and no manifest.
    fn legacy(dir: &Path, segments: usize) -> Vec<ChatMessage> {
        fs::create_dir_all(dir).expect("the history directory");
        let mut all = Vec::new();
        for segment in 0..segments {
            let rows: Vec<ChatMessage> = (segment * 2..segment * 2 + 2).map(message).collect();
            let path = dir.join(format!("msg-{segment:03}.parquet"));
            write_messages(&path, segment * 2, &rows).expect("write a segment");
            let tools: Vec<ToolRow> = rows
                .iter()
                .flat_map(|row| row.tools.iter())
                .map(|tool| ToolRow {
                    name: tool.name.clone(),
                    args: tool.args.clone(),
                    output: tool.output.clone(),
                    ms: tool.ms,
                })
                .collect();
            if !tools.is_empty() {
                let path = dir.join(format!("traj-{segment:03}.parquet"));
                write_tools(&path, &tools).expect("write a trajectory segment");
            }
            all.extend(rows);
        }
        all
    }

    fn same_row(actual: &ChatMessage, expected: &ChatMessage) {
        assert_eq!(actual.role, expected.role);
        assert_eq!(actual.text, expected.text);
        assert_eq!(actual.reasoning, expected.reasoning);
        assert_eq!(actual.images, expected.images, "images survive");
        assert_eq!(actual.error, expected.error);
        assert_eq!(actual.tools.len(), expected.tools.len());
        for (actual, expected) in actual.tools.iter().zip(expected.tools.iter()) {
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.args, expected.args);
            assert_eq!(actual.output, expected.output);
            assert_eq!(actual.ms, expected.ms);
            assert_eq!(actual.at, expected.at);
        }
        assert_eq!(actual.meta.is_some(), expected.meta.is_some());
        if let (Some(actual), Some(expected)) = (&actual.meta, &expected.meta) {
            assert_eq!(actual.feedback, expected.feedback);
            assert_eq!(actual.usage.is_some(), expected.usage.is_some());
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the history directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    /// The rewrite keeps every row in order, with every field and the log's
    /// trajectory.
    #[test]
    fn compaction_preserves_row_order_content_and_images() {
        let dir = scratch("compact-content");
        let expected = legacy(&dir, 5);
        let mut history = History::open(dir.clone());
        assert!(
            history.needs_compaction(),
            "five tiny segments are scattered"
        );
        let expected_tools: Vec<ToolRow> = expected
            .iter()
            .flat_map(|row| row.tools.iter())
            .map(|tool| ToolRow {
                name: tool.name.clone(),
                args: tool.args.clone(),
                output: tool.output.clone(),
                ms: tool.ms,
            })
            .collect();

        let outcome = history.compact().expect("compaction");
        assert_eq!(
            outcome,
            Compaction::Rewritten {
                messages: expected.len(),
                trajectory: expected_tools.len(),
            }
        );
        let rows = history.window(0, expected.len()).expect("rows");
        assert_eq!(rows.len(), expected.len());
        for (actual, expected) in rows.iter().zip(expected.iter()) {
            same_row(actual, expected);
        }
        assert!(
            rows.iter().any(|row| !row.images.is_empty()),
            "the fixture carries an image to prove it survives"
        );
        assert_eq!(
            history.trajectory_page_from_end(0).expect("trajectory"),
            expected_tools
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A compacted directory is one sealed message segment and a manifest, with
    /// no temp or archived files and no leftover sibling.
    #[test]
    fn a_compacted_directory_has_one_sealed_segment_and_a_manifest() {
        let dir = scratch("compact-shape");
        let expected = legacy(&dir, 4);
        let mut history = History::open(dir.clone());
        history.compact().expect("compaction");

        assert!(dir.join(MANIFEST).is_file(), "the index is installed");
        let leftovers = names(&dir);
        assert!(
            leftovers.iter().all(|name| !name.ends_with(".old")
                && !name.ends_with(".tmp")
                && !name.starts_with(".manifest-")),
            "no temp or archived file survives: {leftovers:?}"
        );
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.messages.len(), 1, "one sealed segment");
        assert!(reopened.messages[0].sealed);
        assert_eq!(reopened.trajectory.len(), 1, "the trajectory is sealed too");
        assert!(reopened.trajectory[0].sealed);
        assert_eq!(
            reopened.messages[0].group_rows.iter().sum::<usize>(),
            expected.len(),
            "the group rows describe the segment"
        );
        let parent = dir.parent().expect("the sessions directory");
        let prefix = format!("{}.compact", dir.file_name().unwrap().to_string_lossy());
        let siblings: Vec<String> = fs::read_dir(parent)
            .expect("the parent")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        assert!(
            siblings.is_empty(),
            "the build is swapped in, not left: {siblings:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Compacting an already-compacted history is a no-op: no file changes.
    #[test]
    fn a_second_compaction_is_a_noop() {
        let dir = scratch("compact-twice");
        legacy(&dir, 4);
        let mut history = History::open(dir.clone());
        assert!(matches!(
            history.compact().expect("the first compaction"),
            Compaction::Rewritten { .. }
        ));
        let before = names(&dir);
        assert_eq!(
            history.compact().expect("the second compaction"),
            Compaction::Noop
        );
        assert_eq!(names(&dir), before, "a no-op writes nothing");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A compaction killed before its swap leaves the original readable, and a
    /// second start stands down while a live marker exists.
    #[test]
    fn an_aborted_compaction_leaves_the_original_openable() {
        let dir = scratch("compact-abort");
        let expected = legacy(&dir, 5);
        // What a crash mid-build leaves: a sibling with a partial write.
        let partial = sibling(&dir, "compact-99999");
        fs::create_dir_all(&partial).expect("the partial build");
        fs::write(partial.join("msg-000.parquet"), b"not parquet").expect("partial file");
        fs::write(partial.join(MANIFEST), b"{").expect("partial manifest");

        let readable = History::open(dir.clone());
        assert_eq!(readable.len(), expected.len(), "the original is whole");
        assert_eq!(
            readable.window(0, 1).expect("first row")[0].text,
            expected[0].text
        );

        // A live marker is a concurrent compaction, not debris.
        let marker = sibling(&dir, "compacting");
        fs::write(&marker, b"1").expect("the marker");
        let mut history = History::open(dir.clone());
        assert_eq!(
            history.compact().expect("the guarded compaction"),
            Compaction::Busy
        );
        drop(history);
        assert_eq!(History::open(dir.clone()).len(), expected.len());
        fs::remove_file(&marker).expect("drop the marker");

        // With the guard gone the retry cleans the partial build and succeeds.
        let mut history = History::open(dir.clone());
        assert!(matches!(
            history.compact().expect("the retry"),
            Compaction::Rewritten { .. }
        ));
        assert!(!partial.exists(), "the aborted build was swept");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An image attached to a message is part of the row, so a rewrite that
    /// dropped it would lose the attachment rather than only its order.
    #[test]
    fn images_survive_compaction() {
        let dir = scratch("compact-images");
        fs::create_dir_all(&dir).expect("the history directory");
        let mut rows = Vec::new();
        for index in 0..3 {
            let mut row = message(index);
            row.images = vec![
                format!("data:image/png;base64,first-{index}"),
                format!("data:image/jpeg;base64,second-{index}"),
            ];
            rows.push(row);
            let path = dir.join(format!("msg-{index:03}.parquet"));
            write_messages(&path, index, &rows[index..]).expect("write a segment");
        }
        let mut history = History::open(dir.clone());
        history.compact().expect("compaction");
        let read = history.window(0, rows.len()).expect("rows");
        for (actual, expected) in read.iter().zip(rows.iter()) {
            assert_eq!(
                actual.images, expected.images,
                "every image survives, in order"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A crash between the swap's two renames leaves the original under its
    /// backup name; the next open puts it back before reading anything.
    #[test]
    fn an_interrupted_swap_is_put_back_on_open() {
        let dir = scratch("compact-swap");
        let expected = legacy(&dir, 3);
        let backup = sibling(&dir, "compact-backup-12345");
        fs::rename(&dir, &backup).expect("the crash window");
        assert!(!dir.exists());

        let reopened = History::open(dir.clone());
        assert!(dir.exists(), "the original is back");
        assert!(!backup.exists(), "and its backup name is gone");
        assert_eq!(reopened.len(), expected.len());
        assert_eq!(
            reopened.page(0).expect("first page")[0].text,
            expected[0].text
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A rewrite builds its replacement beside the live log.
    ///
    /// The old implementation deleted the live directory first, so a kill
    /// between the delete and the new write lost the whole conversation. A
    /// build that cannot run must leave the original readable.
    #[test]
    fn a_failed_rewrite_keeps_the_live_log() {
        let dir = scratch("rewrite-failed");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..6).map(message).collect();
        history.append(&rows).expect("the original log");
        // A file at the rebuild's sibling name makes create_dir_all fail.
        let temporary = sibling(&dir, &format!("rewrite-{}", std::process::id()));
        fs::write(&temporary, b"not a directory").expect("the blocker");
        let replacement: Vec<ChatMessage> = (0..3).map(|index| message(index + 100)).collect();
        assert!(
            history.rewrite(&replacement).is_err(),
            "the build cannot start over a file"
        );
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), rows.len(), "the live log is still there");
        assert_eq!(
            reopened.window(0, rows.len()).expect("the original rows")[0].text,
            rows[0].text
        );
        let _ = fs::remove_file(&temporary);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A finished rewrite leaves no half-built sibling behind: the whole
    /// replacement was renamed into place.
    #[test]
    fn a_rewrite_swaps_its_build_in_and_leaves_no_sibling() {
        let dir = scratch("rewrite-swap");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..4).map(message).collect::<Vec<_>>())
            .expect("the original log");
        let replacement: Vec<ChatMessage> = (0..2).map(|index| message(index + 50)).collect();
        history.rewrite(&replacement).expect("the rewrite");
        assert_eq!(history.len(), replacement.len());
        assert_eq!(
            history.window(0, replacement.len()).expect("the rows")[0].text,
            replacement[0].text
        );
        let parent = dir.parent().expect("the sessions directory");
        let prefix = format!("{}.rewrite-", dir.file_name().unwrap().to_string_lossy());
        let siblings: Vec<String> = fs::read_dir(parent)
            .expect("the parent")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&prefix))
            .collect();
        assert!(
            siblings.is_empty(),
            "the build is not left behind: {siblings:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two handles on one directory share the write lock, which is what makes
    /// a save and a compaction's swap serialize.
    #[test]
    fn histories_on_one_directory_share_the_write_lock() {
        let dir = scratch("lock-shared");
        let first = History::open(dir.clone());
        let second = History::open(dir.clone());
        assert!(Arc::ptr_eq(&first.lock, &second.lock));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A turn appended while the log is being compacted survives, whichever of
    /// the two wins the directory lock first.
    #[test]
    fn an_append_during_compaction_is_not_lost() {
        let dir = scratch("compact-race");
        let expected = legacy(&dir, 4);
        // Open both handles on this thread: a history open writes a manifest
        // with a process-wide temp name, so concurrent opens are not the seam
        // this test is about.
        let append_history = History::open(dir.clone());
        let compact_history = History::open(dir.clone());
        let lock = history_lock(&dir);
        let guard = lock
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let barrier = Arc::new(std::sync::Barrier::new(3));
        let (ready, waiting) = std::sync::mpsc::channel();
        let append_barrier = Arc::clone(&barrier);
        let append_ready = ready.clone();
        let append = std::thread::spawn(move || {
            let mut history = append_history;
            append_ready.send(()).expect("announce the append");
            append_barrier.wait();
            history
                .append(&[ChatMessage::user("late row")])
                .expect("the append");
        });
        let compact_barrier = Arc::clone(&barrier);
        let compact = std::thread::spawn(move || {
            let mut history = compact_history;
            ready.send(()).expect("announce the compaction");
            compact_barrier.wait();
            history.compact().expect("the compaction")
        });

        // Both workers are open and waiting; release them together.
        waiting.recv().expect("the append is ready");
        waiting.recv().expect("the compaction is ready");
        drop(guard);
        barrier.wait();
        append.join().expect("the append thread");
        let outcome = compact.join().expect("the compaction thread");
        assert!(
            matches!(outcome, Compaction::Rewritten { .. } | Compaction::Busy),
            "the compaction either rewrote or stood down: {outcome:?}"
        );

        let reopened = History::open(dir.clone());
        assert_eq!(
            reopened.len(),
            expected.len() + 1,
            "the turn appended during the rewrite survived"
        );
        assert_eq!(
            reopened.window(expected.len(), 1).expect("the late row")[0].text,
            "late row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The background snapshot reads the same pages as the live history.
    #[test]
    fn a_reader_reads_the_same_trajectory_pages() {
        let dir = scratch("reader-trajectory");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..200)
            .map(|index| {
                let mut row = message(index);
                row.tools = vec![ToolEvent {
                    name: format!("tool {index}"),
                    args: String::new(),
                    output: None,
                    ms: Some(index as u64),
                    at: None,
                }];
                row
            })
            .collect();
        history.append(&rows).expect("append");
        let reader = history.reader();
        assert_eq!(reader.trajectory_pages(), 2);
        let newest = reader.trajectory_page(0).expect("newest");
        assert_eq!(newest.len(), 200 - PAGE);
        assert_eq!(newest[0].name, format!("tool {PAGE}"));
        let older = reader.trajectory_page(1).expect("older");
        assert_eq!(older.len(), PAGE);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The trajectory is written too, one row per tool call.
    #[test]
    fn the_trajectory_is_written_beside_the_messages() {
        let dir = scratch("trajectory");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..12).map(message).collect();
        history.append(&rows).expect("append");
        let tools = history.trajectory_page_from_end(0).expect("trajectory");
        assert_eq!(tools.len(), 4, "every third message carries one tool");
        assert_eq!(tools[0].name, "bash");
        assert_eq!(tools[0].ms, Some(0));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A gate that fails one stage for paths containing one needle (#352).
    struct Injected {
        stage: IoStage,
        needle: &'static str,
        seen: std::sync::Mutex<Vec<(IoStage, String)>>,
    }

    impl CommitGate for Injected {
        fn pass(&self, stage: IoStage, path: &Path) -> io::Result<()> {
            self.seen
                .lock()
                .unwrap()
                .push((stage, path.display().to_string()));
            if stage == self.stage && path.to_string_lossy().contains(self.needle) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"));
            }
            Ok(())
        }
    }

    /// Register a failing gate for one history directory.
    fn inject(
        dir: &Path,
        stage: IoStage,
        needle: &'static str,
    ) -> (fault::Installed, Arc<Injected>) {
        let gate = Arc::new(Injected {
            stage,
            needle,
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let installed = fault::install(dir, Arc::clone(&gate) as Arc<dyn CommitGate>);
        (installed, gate)
    }

    /// Rewrite the manifest in place, the way a stale or corrupt one arrives.
    fn edit_manifest(dir: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
        let path = dir.join(MANIFEST);
        let bytes = fs::read(&path).expect("the manifest");
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("manifest json");
        edit(&mut value);
        fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("the edit");
    }

    /// A failed segment commit leaves no segment file and no manifest entry.
    #[test]
    fn a_failed_segment_commit_leaves_no_segment_and_no_manifest_reference() {
        let dir = scratch("segment-failure");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..4).map(message).collect::<Vec<_>>())
            .expect("the first segment");
        drop(history);
        let mut history = History::open(dir.clone());
        assert!(dir.join(MANIFEST).is_file(), "open publishes the index");
        let before = scan(&dir, "msg").len();
        let (installed, gate) = inject(&dir, IoStage::Sync, ".parquet");
        let error = history
            .append(&(4..8).map(message).collect::<Vec<_>>())
            .expect_err("the injected sync fails the segment commit");
        assert!(error.to_string().contains("sync"), "{error}");
        drop(installed);
        assert_eq!(history.len(), 4, "the failed save moved no row");
        assert_eq!(scan(&dir, "msg").len(), before, "no segment was installed");
        assert!(
            gate.seen
                .lock()
                .unwrap()
                .iter()
                .any(|(stage, _)| *stage == IoStage::Sync),
            "the gate saw the sync stage"
        );
        let manifest = load_manifest(&dir).expect("the previous manifest");
        for entry in &manifest.messages {
            assert!(
                dir.join(&entry.file).exists(),
                "the manifest never references a failed segment"
            );
        }
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), 4, "the next open still reads the old log");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A failed manifest commit leaves the PREVIOUS manifest and its files.
    #[test]
    fn a_failed_manifest_commit_keeps_the_previous_manifest_and_its_files() {
        let dir = scratch("manifest-failure");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..300).map(message).collect::<Vec<_>>())
            .expect("the first segment");
        drop(history);
        let mut history = History::open(dir.clone());
        let before = fs::read(dir.join(MANIFEST)).expect("the manifest");
        let (installed, gate) = inject(&dir, IoStage::Replace, MANIFEST);
        // 600 unsealed rows cross the seal target, so the save tries to publish
        // a fresh manifest - the stage this test fails.
        history
            .append(&(300..900).map(message).collect::<Vec<_>>())
            .expect("the save lands; the seal is best-effort");
        drop(installed);
        assert!(
            gate.seen
                .lock()
                .unwrap()
                .iter()
                .any(|(stage, path)| *stage == IoStage::Replace && path.contains(MANIFEST)),
            "the gate saw the manifest replacement"
        );
        assert_eq!(
            fs::read(dir.join(MANIFEST)).expect("the manifest"),
            before,
            "the previous manifest is untouched"
        );
        let manifest = load_manifest(&dir).expect("the previous manifest");
        for entry in &manifest.messages {
            assert!(
                dir.join(&entry.file).exists(),
                "the previous manifest still points at files that exist"
            );
        }
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), 900, "every saved row is readable");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A successful append is on disk for the next open, at the durable level.
    #[test]
    fn a_successful_append_is_seen_by_the_next_open() {
        let dir = scratch("append-visible");
        let mut history = History::open(dir.clone());
        let rows: Vec<ChatMessage> = (0..5).map(message).collect();
        history.append(&rows).expect("the append");
        let segments = scan(&dir, "msg");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].rows, rows.len());
        let name = segments[0]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let fields: Vec<&str> = name.trim_end_matches(".parquet").split('-').collect();
        assert_eq!(fields[0], "msg");
        assert_eq!(fields.len(), 4, "msg-stamp-pid-seq: {name}");
        assert_eq!(fields[1].len(), 13, "the millisecond stamp: {name}");
        assert_eq!(fields[2], format!("{:05}", std::process::id()));
        assert!(fields[3].len() >= 3, "the per-process sequence: {name}");
        drop(history);
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), rows.len());
        assert_eq!(
            reopened.window(0, rows.len()).expect("rows")[4].text,
            rows[4].text
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Legacy and pid-carrying segment names parse and sort in write order.
    #[test]
    fn legacy_and_sequence_names_parse_and_sort_together() {
        let mut names = vec![
            "msg-1700000000002-12345-001.parquet".to_string(),
            "msg-999.parquet".to_string(),
            "msg-1700000000001-12345-000.parquet".to_string(),
            "msg-1000.parquet".to_string(),
        ];
        names.sort_by(|a, b| (stamp_of(a), a).cmp(&(stamp_of(b), b)));
        assert_eq!(
            names,
            vec![
                "msg-999.parquet".to_string(),
                "msg-1000.parquet".to_string(),
                "msg-1700000000001-12345-000.parquet".to_string(),
                "msg-1700000000002-12345-001.parquet".to_string(),
            ],
            "the numeric first field orders legacy and current names together"
        );
        assert_eq!(stamp_of("msg-000.parquet"), 0);
        assert_eq!(
            stamp_of("msg-1700000000001-12345-000.parquet"),
            1_700_000_000_001
        );
        assert_eq!(stamp_of("not-a-segment"), u64::MAX);
    }

    /// A lock held by another process turns an append into the typed error.
    #[test]
    fn a_locked_history_directory_refuses_an_append_with_the_typed_error() {
        let dir = scratch("locked-append");
        let mut history = History::open(dir.clone());
        let held = FileLock::acquire(&dir).expect("the other process's lock");
        let error = history
            .append(&[message(0)])
            .expect_err("a locked append must not corrupt or hang");
        assert!(
            matches!(
                error.downcast_ref::<HistoryError>(),
                Some(HistoryError::Locked { .. })
            ),
            "a typed Locked error: {error}"
        );
        assert_eq!(history.len(), 0);
        assert!(scan(&dir, "msg").is_empty(), "nothing was written");
        drop(held);
        history
            .append(&[message(0)])
            .expect("the append succeeds after the lock is released");
        assert_eq!(history.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A second History (its own lock instance and sequence) cannot collide.
    #[test]
    fn two_histories_appending_to_one_directory_never_share_a_segment_name() {
        let dir = scratch("two-histories");
        let mut first = History::open(dir.clone());
        let mut second = History::open(dir.clone());
        for index in 0..4usize {
            let history = if index.is_multiple_of(2) {
                &mut first
            } else {
                &mut second
            };
            history.append(&[message(index)]).expect("the append");
        }
        let segments = scan(&dir, "msg");
        let names: HashSet<String> = segments
            .iter()
            .map(|segment| {
                segment
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert_eq!(segments.len(), 4, "four appends, four segment files");
        assert_eq!(names.len(), 4, "no two appends chose the same name");
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), 4);
        let texts: Vec<String> = reopened
            .window(0, 4)
            .expect("rows")
            .into_iter()
            .map(|row| row.text)
            .collect();
        assert_eq!(
            texts,
            (0..4)
                .map(|index| format!("turn {index} text"))
                .collect::<Vec<_>>(),
            "the name order is the write order"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A pre-existing final segment path is skipped, never renamed over.
    #[test]
    fn a_preexisting_segment_path_is_never_overwritten() {
        let dir = scratch("segment-collision");
        fs::create_dir_all(&dir).expect("the history directory");
        let first = segment_path(&dir, "msg");
        fs::write(&first, b"sentinel").expect("the blocker");
        let second = segment_path(&dir, "msg");
        assert_ne!(first, second, "the sequence is bumped instead of reused");
        write_messages(&second, 0, &[message(0)]).expect("the write");
        assert_eq!(
            fs::read(&first).unwrap(),
            b"sentinel",
            "nothing was overwritten"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A busy lock makes open read from a scan and skip the repair write.
    #[test]
    fn open_keeps_reading_while_another_holder_owns_the_lock() {
        let dir = scratch("open-busy");
        let expected = legacy(&dir, 3);
        let held = FileLock::acquire(&dir).expect("the other process's lock");
        let history = History::open(dir.clone());
        assert_eq!(history.len(), expected.len());
        assert!(
            !dir.join(MANIFEST).is_file(),
            "a busy lock skips the repair write"
        );
        drop(held);
        let repaired = History::open(dir.clone());
        assert!(
            dir.join(MANIFEST).is_file(),
            "the free lock lets the next open publish the index"
        );
        assert_eq!(repaired.len(), expected.len());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The sweep removes process-unique debris and never a live segment.
    #[test]
    fn sweep_archived_removes_process_debris_but_keeps_segments() {
        let dir = scratch("sweep");
        fs::create_dir_all(&dir).unwrap();
        let live = dir.join("msg-0000000000001-00001-000.parquet");
        fs::write(&live, b"live").unwrap();
        let debris = [
            dir.join("msg-000.parquet.tmp"),
            dir.join("msg-000.parquet.old"),
            dir.join(".msg-0000000000002-00001-001.parquet.tmp-12345-0"),
            dir.join(".msg-0000000000003-00001-002.parquet.old-12345-0"),
        ];
        for path in &debris {
            fs::write(path, b"debris").unwrap();
        }
        sweep_archived(&dir);
        assert!(live.exists(), "a live segment is not debris");
        for path in &debris {
            assert!(!path.exists(), "{} is debris", path.display());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A missing manifest is rebuilt from the footers and published at v2.
    #[test]
    fn a_missing_manifest_is_rebuilt_from_the_footers() {
        let dir = scratch("missing-manifest");
        let expected = legacy(&dir, 4);
        assert!(!dir.join(MANIFEST).exists());
        let history = History::open(dir.clone());
        assert_eq!(history.len(), expected.len());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        let manifest = load_manifest(&dir).expect("the rebuilt manifest");
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert_eq!(manifest.schema_version, SCHEMA_VERSION);
        assert_eq!(manifest.messages.len(), 4);
        for entry in &manifest.messages {
            assert!(dir.join(&entry.file).exists());
            assert_eq!(entry.schema_version, SCHEMA_VERSION);
        }
        let rows = history.window(0, expected.len()).expect("rows");
        for (actual, expected) in rows.iter().zip(expected.iter()) {
            same_row(actual, expected);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Stale row counts are rebuilt from the footers without losing a row.
    #[test]
    fn a_stale_manifest_is_rebuilt_from_the_footers() {
        let dir = scratch("stale-manifest");
        let expected = legacy(&dir, 3);
        drop(History::open(dir.clone()));
        edit_manifest(&dir, |value| {
            let entries = value["messages"].as_array_mut().expect("messages");
            entries[1]["rows"] = serde_json::json!(99);
            entries[1]["group_rows"] = serde_json::json!([99]);
        });
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        assert_eq!(history.len(), expected.len());
        let rows = history.window(0, expected.len()).expect("rows");
        for (actual, expected) in rows.iter().zip(expected.iter()) {
            same_row(actual, expected);
        }
        let manifest = load_manifest(&dir).expect("the rebuilt manifest");
        assert_eq!(
            manifest
                .messages
                .iter()
                .map(|entry| entry.rows)
                .sum::<usize>(),
            expected.len()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A listed segment that is gone means unavailable rows, not a shorter log.
    #[test]
    fn a_missing_listed_segment_is_unavailable_and_keeps_the_manifest() {
        let dir = scratch("missing-segment");
        let expected = legacy(&dir, 3);
        drop(History::open(dir.clone()));
        let before = fs::read(dir.join(MANIFEST)).expect("the manifest");
        let victim = load_manifest(&dir).expect("the manifest").messages[1]
            .file
            .clone();
        fs::remove_file(dir.join(&victim)).expect("remove a segment");
        let history = History::open(dir.clone());
        assert!(
            matches!(history.index_state(), IndexState::Unavailable { .. }),
            "a missing listed segment is diagnosed, not hidden"
        );
        assert_eq!(
            history.len(),
            expected.len(),
            "the promised row count is kept, not shortened"
        );
        assert_eq!(
            fs::read(dir.join(MANIFEST)).expect("the manifest"),
            before,
            "the manifest is kept while the rows are unavailable"
        );
        assert!(
            history.window(0, expected.len()).is_err(),
            "a read of the missing rows fails explicitly"
        );
        let report = diagnose(&dir);
        assert!(matches!(report.state, IndexState::Unavailable { .. }));
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.contains(&victim) && finding.contains("missing")),
            "the diagnosis names the missing segment: {:?}",
            report.findings
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// An unlisted tail segment is read in order and never duplicated.
    #[test]
    fn an_extra_tail_segment_is_read_without_duplicating_rows() {
        let dir = scratch("extra-tail");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..PAGE).map(message).collect::<Vec<_>>())
            .expect("the first segment");
        drop(history);
        let mut history = History::open(dir.clone());
        history
            .append(&(PAGE..PAGE + 3).map(message).collect::<Vec<_>>())
            .expect("the tail segment");
        drop(history);
        assert_eq!(
            load_manifest(&dir).expect("the manifest").messages.len(),
            1,
            "the tail is not in the manifest yet"
        );
        let reopened = History::open(dir.clone());
        assert_eq!(reopened.len(), PAGE + 3);
        let texts: Vec<String> = reopened
            .window(0, PAGE + 3)
            .expect("rows")
            .into_iter()
            .map(|row| row.text)
            .collect();
        assert_eq!(texts.len(), PAGE + 3);
        assert_eq!(texts[PAGE], format!("turn {PAGE} text"));
        assert_eq!(texts.last().unwrap(), &format!("turn {} text", PAGE + 2));
        let unique: HashSet<&String> = texts.iter().collect();
        assert_eq!(unique.len(), texts.len(), "no row is read twice");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A manifest whose row groups disagree with the file is rebuilt.
    #[test]
    fn invalid_row_group_metadata_is_rebuilt() {
        let dir = scratch("invalid-row-groups");
        let expected = legacy(&dir, 2);
        drop(History::open(dir.clone()));
        edit_manifest(&dir, |value| {
            let entries = value["messages"].as_array_mut().expect("messages");
            entries[0]["rows"] = serde_json::json!(2);
            entries[0]["group_rows"] = serde_json::json!([1, 1]);
        });
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        assert_eq!(history.len(), expected.len());
        let manifest = load_manifest(&dir).expect("the rebuilt manifest");
        assert!(
            manifest
                .messages
                .iter()
                .all(|entry| entry.rows > 0 && !entry.group_rows.is_empty()),
            "the rebuilt entries describe the files"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// An unknown manifest version is rebuilt, never guessed at.
    #[test]
    fn an_unsupported_manifest_version_is_rebuilt() {
        let dir = scratch("manifest-version");
        let expected = legacy(&dir, 2);
        drop(History::open(dir.clone()));
        edit_manifest(&dir, |value| value["version"] = serde_json::json!(99));
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        assert_eq!(history.len(), expected.len());
        assert_eq!(
            load_manifest(&dir).expect("the rebuilt manifest").version,
            MANIFEST_VERSION
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A duplicated manifest entry adds no row when the index is rebuilt.
    #[test]
    fn duplicate_manifest_entries_are_rebuilt_without_duplicating_rows() {
        let dir = scratch("duplicate-entries");
        let expected = legacy(&dir, 3);
        drop(History::open(dir.clone()));
        edit_manifest(&dir, |value| {
            let entries = value["messages"].as_array_mut().expect("messages");
            let first = entries[0].clone();
            entries.push(first);
        });
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        assert_eq!(
            history.len(),
            expected.len(),
            "a duplicate entry adds no row"
        );
        assert_eq!(
            load_manifest(&dir)
                .expect("the rebuilt manifest")
                .messages
                .len(),
            3
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Misordered entries are rebuilt into the segment write order.
    #[test]
    fn misordered_manifest_entries_are_rebuilt_in_stamp_order() {
        let dir = scratch("misordered-entries");
        let expected = legacy(&dir, 3);
        drop(History::open(dir.clone()));
        edit_manifest(&dir, |value| {
            let entries = value["messages"].as_array_mut().expect("messages");
            entries.swap(0, 2);
        });
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        let rows = history.window(0, expected.len()).expect("rows");
        for (actual, expected) in rows.iter().zip(expected.iter()) {
            same_row(actual, expected);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The offline walk names an unreadable unlisted segment (#356 seam).
    #[test]
    fn diagnose_reports_unlisted_unreadable_and_missing_segments() {
        let dir = scratch("diagnose");
        let expected = legacy(&dir, 2);
        drop(History::open(dir.clone()));
        let broken = "msg-0000000009999-00001-000.parquet";
        fs::write(dir.join(broken), b"not parquet").expect("a broken segment");
        let report = diagnose(&dir);
        assert!(matches!(report.state, IndexState::Unavailable { .. }));
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.contains(broken)),
            "the walk reports the unreadable file: {:?}",
            report.findings
        );
        let segment = report
            .segments
            .iter()
            .find(|segment| segment.file == broken)
            .expect("the broken segment is reported");
        assert!(!segment.readable);
        assert!(!segment.listed, "it was never in the manifest");
        assert_eq!(
            report
                .segments
                .iter()
                .filter(|segment| segment.listed && segment.kind == "msg")
                .count(),
            2
        );
        let history = History::open(dir.clone());
        assert!(matches!(
            history.index_state(),
            IndexState::Unavailable { .. }
        ));
        assert_eq!(history.len(), expected.len());
        assert!(matches!(
            history.diagnosis().state,
            IndexState::Unavailable { .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Every segment records the schema version in its Parquet metadata.
    #[test]
    fn every_segment_records_the_schema_version() {
        let dir = scratch("schema-metadata");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..3).map(message).collect::<Vec<_>>())
            .expect("the append");
        drop(history);
        drop(History::open(dir.clone()));
        let manifest = load_manifest(&dir).expect("the manifest");
        assert_eq!(manifest.schema_version, SCHEMA_VERSION);
        assert!(!manifest.messages.is_empty());
        for entry in &manifest.messages {
            assert_eq!(entry.schema_version, SCHEMA_VERSION);
            let file = fs::File::open(dir.join(&entry.file)).expect("the segment");
            let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("the footer");
            let values = builder
                .metadata()
                .file_metadata()
                .key_value_metadata()
                .expect("key/value metadata");
            let recorded = values
                .iter()
                .find(|value| value.key == SCHEMA_KEY)
                .expect("bsh.schema");
            let expected = SCHEMA_VERSION.to_string();
            assert_eq!(recorded.value.as_deref(), Some(expected.as_str()));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A healthy history diagnoses as valid once the index is published.
    #[test]
    fn a_healthy_history_diagnoses_as_valid() {
        let dir = scratch("diagnose-valid");
        let mut history = History::open(dir.clone());
        history
            .append(&(0..PAGE).map(message).collect::<Vec<_>>())
            .expect("the append");
        drop(history);
        let history = History::open(dir.clone());
        assert!(matches!(history.index_state(), IndexState::Rebuilt { .. }));
        let report = history.diagnosis();
        assert_eq!(report.state, IndexState::Valid);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(
            report.segments.iter().all(|segment| segment.listed
                && segment.readable
                && segment.consistent
                && segment.schema_supported),
            "{:?}",
            report.segments
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
