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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

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
/// Bumped when the manifest shape changes; an older one falls back to a scan.
const MANIFEST_VERSION: u32 = 1;
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

/// One segment as the manifest stores it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestSegment {
    file: String,
    rows: usize,
    bytes: u64,
    sealed: bool,
    group_rows: Vec<usize>,
}

/// The per-directory metadata index.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    messages: Vec<ManifestSegment>,
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
    /// The manifest answers the common case without opening a single Parquet
    /// footer. A missing or invalid manifest falls back to scanning the
    /// directory, then writes a fresh one for the next launcher.
    pub(crate) fn open(dir: PathBuf) -> Self {
        recover_swap(&dir);
        if let Some(mut index) = read_manifest(&dir) {
            // Files not in the manifest are the unsealed tail written since the
            // last commit. They are few (the seal target bounds them), so only
            // their footers are opened.
            index
                .messages
                .extend(scan_missing(&dir, "msg", &index.listed));
            index
                .trajectory
                .extend(scan_missing(&dir, "traj", &index.listed));
            let mut history = Self::from_segments(dir, index.messages, index.trajectory);
            history.legacy = index.legacy;
            return history;
        }
        let messages = scan(&dir, "msg");
        let trajectory = scan(&dir, "traj");
        let mut history = Self::from_segments(dir, messages, trajectory);
        // No index at all means the old writer produced these files. Record
        // that in the index so the compacting decision survives a restart: the
        // manifest is written here, before anything has rewritten the log.
        history.legacy = history.rows > 0 || history.trajectory_rows > 0;
        let _ = history.write_manifest();
        history
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
            cache: RefCell::new(PageCache::new(CACHE_PAGES)),
        }
    }

    /// The per-directory read guard: a page read must not overlap a swap.
    fn read_guard(&self) -> RwLockReadGuard<'_, ()> {
        self.lock
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Write the metadata manifest atomically.
    fn write_manifest(&self) -> Result<()> {
        self.write_manifest_with(&self.messages, &self.trajectory)
    }

    /// Write the manifest for a candidate segment list before it is committed.
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
    pub(crate) fn rewrite(&mut self, messages: &[ChatMessage]) -> Result<()> {
        let lock = Arc::clone(&self.lock);
        let _guard = lock
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.dir.exists() {
            let _ = fs::remove_dir_all(&self.dir);
        }
        Arc::make_mut(&mut self.messages).clear();
        Arc::make_mut(&mut self.trajectory).clear();
        self.rows = 0;
        self.trajectory_rows = 0;
        self.unsealed_rows = 0;
        self.unsealed_bytes = 0;
        self.unsealed_trajectory_rows = 0;
        self.unsealed_trajectory_bytes = 0;
        self.cache.borrow_mut().clear();
        self.append_locked(messages)
    }

    /// Append the messages written since the last save, in one segment.
    pub(crate) fn append(&mut self, messages: &[ChatMessage]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
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
        let stamp = next_stamp();
        let path = self.dir.join(format!("msg-{stamp:013}.parquet"));
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
            let path = self.dir.join(format!("traj-{stamp:013}.parquet"));
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
        let _ = self.seal_tail();
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
        let path = self.dir.join(format!("msg-{:013}.parquet", next_stamp()));
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
        let path = self.dir.join(format!("traj-{:013}.parquet", next_stamp()));
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
        // A seal that died between archiving and committing leaves .old files
        // nothing lists; they are debris either way. The sweep removes
        // `*.parquet.tmp` too, so it must run under the write lock: an append
        // writes its segment to that exact temp name before renaming it, and
        // sweeping without the lock deleted a save that was in flight.
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

/// Every segment of one kind in a session directory, in write order.
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

fn stamp_of(name: &str) -> u64 {
    name.rsplit_once('-')
        .map(|(_, rest)| rest.trim_end_matches(".parquet"))
        .and_then(|stamp| stamp.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Move a seal's old files aside before its manifest is installed.
///
/// A crash between the manifest write and the delete would otherwise leave the
/// old `.parquet` files as unlisted tail files, which the next open would read
/// as duplicate rows. `.parquet.old` is ignored by the scanners.
fn archive_files(old: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let archived: Vec<PathBuf> = old
        .iter()
        .map(|path| path.with_extension("parquet.old"))
        .collect();
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
        if name.ends_with(".parquet.old")
            || name.ends_with(".parquet.tmp")
            || (name.starts_with(".manifest-") && name.ends_with(".tmp"))
        {
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
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = fs::remove_dir_all(entry.path());
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
        let path = dir.join(format!("msg-{:013}.parquet", next_stamp()));
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
        let path = dir.join(format!("traj-{:013}.parquet", next_stamp()));
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

/// Write the metadata index for a directory atomically.
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
        messages: messages.iter().map(manifest_segment).collect(),
        trajectory: trajectory.iter().map(manifest_segment).collect(),
        legacy,
    };
    let temporary = dir.join(format!(".manifest-{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(&manifest)?;
    fs::write(&temporary, bytes).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, dir.join(MANIFEST))
        .with_context(|| format!("installing {}", MANIFEST))?;
    Ok(())
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
    }
}

/// Read and validate the manifest. Any inconsistency falls back to a scan.
fn read_manifest(dir: &Path) -> Option<Index> {
    let bytes = fs::read(dir.join(MANIFEST)).ok()?;
    let manifest: Manifest = serde_json::from_slice(&bytes).ok()?;
    if manifest.version != MANIFEST_VERSION {
        return None;
    }
    let legacy = manifest.legacy;
    let mut listed = HashSet::new();
    let mut segments = |entries: Vec<ManifestSegment>| -> Option<Vec<Segment>> {
        let mut segments = Vec::with_capacity(entries.len());
        for entry in entries {
            let path = dir.join(&entry.file);
            let bytes = fs::metadata(&path).ok()?.len();
            if entry.rows != entry.group_rows.iter().sum::<usize>() {
                return None;
            }
            listed.insert(entry.file.clone());
            segments.push(Segment {
                path,
                start: 0,
                rows: entry.rows,
                bytes,
                sealed: entry.sealed,
                group_rows: entry.group_rows,
            });
        }
        Some(segments)
    };
    let messages = segments(manifest.messages)?;
    let trajectory = segments(manifest.trajectory)?;
    Some(Index {
        messages,
        trajectory,
        listed,
        legacy,
    })
}

fn scan(dir: &Path, prefix: &str) -> Vec<Segment> {
    scan_where(dir, prefix, None, true)
}

/// Scan only the files a manifest does not list, as the unsealed tail.
fn scan_missing(dir: &Path, prefix: &str, listed: &HashSet<String>) -> Vec<Segment> {
    scan_where(dir, prefix, Some(listed), false)
}

fn scan_where(
    dir: &Path,
    prefix: &str,
    listed: Option<&HashSet<String>>,
    sealed: bool,
) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return segments;
    };
    let mut paths: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.starts_with(prefix) && name.ends_with(".parquet")) {
                return None;
            }
            if listed.is_some_and(|listed| listed.contains(&name)) {
                return None;
            }
            Some((name.clone(), entry.path()))
        })
        .collect();
    // Numeric, not lexicographic: a legacy three-digit name and a millisecond
    // name sort wrongly by text once the epoch digit passes 9, which put
    // migrated history after the pages that followed it.
    paths.sort_by_key(|(name, _)| stamp_of(name));
    for (_, path) in paths {
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
            continue;
        };
        let groups = builder.metadata().num_row_groups();
        let group_rows: Vec<usize> = (0..groups)
            .map(|group| builder.metadata().row_group(group).num_rows() as usize)
            .collect();
        let rows: usize = group_rows.iter().sum();
        if rows == 0 {
            continue;
        }
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        segments.push(Segment {
            path,
            start: 0,
            rows,
            bytes,
            sealed,
            group_rows,
        });
    }
    segments
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
    // Compression is the one thing the writer properties are for; the row
    // group boundaries come from flush(), below, because a row group is the
    // page this store reads by.
    WriterProperties::builder()
        .set_compression(parquet::basic::Compression::SNAPPY)
        .build()
}

/// Write one segment, in row groups of at most PAGE rows.
///
/// Returns the rows in each group so the reader can locate a global row across
/// short segments instead of assuming every group is PAGE rows.
fn write_messages(path: &Path, base: usize, messages: &[ChatMessage]) -> Result<Vec<usize>> {
    let temporary = path.with_extension("parquet.tmp");
    let file = fs::File::create(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    let mut writer =
        ArrowWriter::try_new(BufWriter::new(file), message_schema(), Some(writer_props()))?;
    let mut group_rows = Vec::new();
    let outcome = (|| -> Result<Vec<usize>> {
        for chunk in message_chunks(messages) {
            writer.write(&message_batch(chunk, base)?)?;
            // A flush closes the row group: one page per group, which is what
            // the reader addresses.
            writer.flush()?;
            group_rows.push(chunk.len());
        }
        writer.close()?;
        fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
        Ok(group_rows)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
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

fn write_tools(path: &Path, tools: &[ToolRow]) -> Result<Vec<usize>> {
    let temporary = path.with_extension("parquet.tmp");
    let file = fs::File::create(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    let mut writer =
        ArrowWriter::try_new(BufWriter::new(file), tool_schema(), Some(writer_props()))?;
    let mut group_rows = Vec::new();
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
        )?;
        writer.write(&batch)?;
        writer.flush()?;
        group_rows.push(chunk.len());
    }
    let outcome = (|| -> Result<Vec<usize>> {
        writer.close()?;
        fs::rename(&temporary, path).with_context(|| format!("installing {}", path.display()))?;
        Ok(group_rows)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
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
}
