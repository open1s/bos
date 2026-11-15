//! Session persistence, one directory per workspace.
//!
//! One JSON file per session, written whole, at
//! `<root>/.bsh/sessions/<id>.json` - beside the workspace it belongs to, so a
//! checkout carries its own history and two machines do not share one session
//! list. A session whose workspace is unknown is written to the harness
//! directory instead, which is the only place left to put it.
//!
//! The directory is the database: a session that fails to parse is skipped
//! rather than aborting the load, so one damaged file cannot take the sidebar
//! down with it.
//!
//! A turn in flight is the exception to "written whole": its reply row changes
//! token by token and the Parquet log is append-only, so the mutable tail is
//! journaled to `inflight.json` beside the log and folded in on the next open.
//! The journal is only valid while its `inflight.lock` is held; the OS drops
//! that lock when the process dies, which is how a crashed turn is told from a
//! turn another window is still running. Both the document and the journal are
//! replaced by rename, and a log rewrite builds its replacement in a sibling
//! directory before swapping it in, so no kill can leave a session unreadable.

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::history::{History, Reader, ToolRow};
use crate::model::{ChatMessage, Role, Session};

/// Messages a session keeps in memory: two pages, which is a screen of
/// scrolling in each direction. Everything older stays in the log until the
/// reader scrolls to it.
pub(crate) const WINDOW: usize = 2 * crate::history::PAGE;

/// The in-flight turn's journal, beside the log it belongs to.
///
/// The log is append-only, so a row already written cannot be updated in
/// place. A turn streams into a row for minutes, and until this existed that
/// row lived only in memory: a kill during the run lost the whole answer, and
/// a kill during the save that finally wrote it could lose the log itself.
const INFLIGHT: &str = "inflight.json";

/// The lock that says a journal belongs to a LIVE turn.
///
/// A second launch - or a second window in another process - must not treat
/// a running turn's journal as a crash to recover. The owner holds an OS lock
/// for the turn's lifetime; the lock dies with the process, which is exactly
/// the distinction a pid cannot make reliably.
const INFLIGHT_LOCK: &str = "inflight.lock";

/// The journal is written here and renamed into place, so a reader never sees
/// half a JSON document.
const INFLIGHT_TMP: &str = "inflight.json.tmp";

/// Where unreadable journals are preserved instead of deleted.
const CORRUPT: &str = "corrupt";

/// The journal format this writer emits.
///
/// A journal written before the format carried integrity has version 0 and no
/// checksum. It still parses, and its checksum is not verified. A version above
/// this one is "unsupported", which is NOT corruption: its bytes are preserved
/// for the build that knows how to read them.
const INFLIGHT_VERSION: u32 = 1;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, 64-bit, over `bytes`.
///
/// The journal needs a checksum that notices a torn or edited file, not a
/// cryptographic one, so it is computed inline instead of adding a dependency.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The journal's checksum: FNV-1a over the deterministic bytes of `(base, rows)`.
fn checksum_of(base: usize, rows: &[ChatMessage]) -> serde_json::Result<String> {
    let bytes = serde_json::to_vec(&(base, rows))?;
    Ok(format!("{:016x}", fnv1a_64(&bytes)))
}

/// Why a journal cannot be replayed as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalDefect {
    /// The file holds no bytes at all.
    Empty,
    /// The JSON stops in the middle, as a kill during the write leaves it.
    Truncated,
    /// The bytes are not a journal.
    Unreadable,
    /// The declared checksum does not match the payload.
    ChecksumMismatch,
    /// A version this build does not know.
    UnsupportedVersion,
}

impl JournalDefect {
    /// The stable label the quarantine directory and record carry.
    fn label(self) -> &'static str {
        match self {
            JournalDefect::Empty => "empty",
            JournalDefect::Truncated => "truncated",
            JournalDefect::Unreadable => "unreadable",
            JournalDefect::ChecksumMismatch => "checksum-mismatch",
            JournalDefect::UnsupportedVersion => "unsupported-version",
        }
    }
}

/// A turn's uncommitted rows: the log already holds its first `base` rows, and
/// these supersede everything at or after that index.
#[derive(Debug, Serialize, Deserialize)]
struct Inflight {
    /// The format version; 0 is a journal written before integrity existed.
    #[serde(default)]
    version: u32,
    /// FNV-1a of `(base, rows)`, hex. Empty on a legacy (version 0) journal.
    #[serde(default)]
    checksum: String,
    /// The global row index the journal's rows start at.
    base: usize,
    /// The rows that supersede the log from `base` on.
    rows: Vec<ChatMessage>,
}

impl Inflight {
    /// Journal `base` and `rows` at the current version, with its checksum.
    fn new(base: usize, rows: Vec<ChatMessage>) -> serde_json::Result<Self> {
        let checksum = checksum_of(base, &rows)?;
        Ok(Self {
            version: INFLIGHT_VERSION,
            checksum,
            base,
            rows,
        })
    }

    /// Parse a journal, classifying every way its bytes can be untrustworthy.
    ///
    /// A version 0 journal is legacy: it parses and replays, and its checksum is
    /// not verified because it never carried one.
    fn parse(bytes: &[u8]) -> Result<Self, JournalDefect> {
        if bytes.is_empty() {
            return Err(JournalDefect::Empty);
        }
        match serde_json::from_slice::<Self>(bytes) {
            Ok(journal) => journal.verified(),
            Err(error) if error.is_eof() => Err(JournalDefect::Truncated),
            Err(_) => Err(JournalDefect::Unreadable),
        }
    }

    /// Check the version and, at version 1, the checksum.
    fn verified(self) -> Result<Self, JournalDefect> {
        if self.version > INFLIGHT_VERSION {
            return Err(JournalDefect::UnsupportedVersion);
        }
        if self.version == INFLIGHT_VERSION {
            let expected =
                checksum_of(self.base, &self.rows).map_err(|_| JournalDefect::Unreadable)?;
            if self.checksum != expected {
                return Err(JournalDefect::ChecksumMismatch);
            }
        }
        Ok(self)
    }
}

/// A chat window waiting to be read off the log.
///
/// The snapshot is built on the UI thread - it is only an index - and the
/// blocking Parquet read happens when OlderRead::read runs on the background
/// executor. Nothing here borrows the live history, so the reader never waits
/// while another thread decodes pages.
pub(crate) struct OlderRead {
    session: String,
    from: usize,
    floor: usize,
    reader: Reader,
    /// The directory the index came from, so a read that races a compaction's
    /// swap can reopen the history and retry instead of returning nothing.
    dir: PathBuf,
}

impl OlderRead {
    /// The session the window belongs to.
    pub(crate) fn session(&self) -> &str {
        &self.session
    }

    /// The first global row the window asks for.
    pub(crate) fn from(&self) -> usize {
        self.from
    }

    /// Decode the window. Blocking: background executor only.
    pub(crate) fn read(self) -> OlderPage {
        let len = self.floor - self.from;
        let rows = match self.reader.messages(self.from, len) {
            Ok(rows) => Ok(rows),
            Err(first) => self.retry(first),
        };
        OlderPage {
            failure: rows.as_ref().err().cloned(),
            session: self.session,
            from: self.from,
            rows: rows.unwrap_or_default(),
        }
    }

    /// Reopen the history ONCE and retry, the compaction-swap retry.
    ///
    /// A compaction renames the directory aside and back, so a read that finds
    /// its segment gone can be answered by the installed manifest. That is the
    /// only case this may recover: the reopened index has to still cover the
    /// rows the page asks for. A reopen that finds fewer rows (a segment that is
    /// gone or unreadable, not swapped) is the page's failure, never an empty
    /// window reported as success.
    fn retry<E: std::fmt::Display>(&self, first: E) -> Result<Vec<ChatMessage>, PageFailure> {
        let reopened = History::open(self.dir.clone());
        let covered = reopened.len() >= self.from.saturating_add(self.len());
        let mut second = None;
        if covered {
            match reopened.reader().messages(self.from, self.len()) {
                Ok(rows) => return Ok(rows),
                Err(error) => second = Some(error),
            }
        }
        Err(PageFailure {
            session: self.session.clone(),
            at: self.from,
            detail: failure_detail(
                &self.dir,
                self.from,
                self.len(),
                &first,
                second.as_ref(),
                covered,
            ),
            retried: true,
        })
    }

    /// The number of rows the window asks for.
    fn len(&self) -> usize {
        self.floor - self.from
    }
}

/// The rows one OlderRead decoded.
pub(crate) struct OlderPage {
    /// The session the window belongs to.
    pub(crate) session: String,
    /// The first global row the window holds.
    pub(crate) from: usize,
    /// The decoded rows, oldest first. Empty when the read failed.
    pub(crate) rows: Vec<ChatMessage>,
    /// Why the read failed, when it did.
    failure: Option<PageFailure>,
}

impl OlderPage {
    /// The failure this page carries, if the read did not succeed.
    ///
    /// A page that failed is not an empty page: the caller must surface this
    /// instead of showing nothing.
    pub(crate) fn failure(&self) -> Option<&PageFailure> {
        self.failure.as_ref()
    }
}

/// A page read that failed, carried by the page result.
///
/// The detail is the reader's own error text, which names the segment and the
/// row group it could not read (`opening <path>`, `row group N of <path>`),
/// prefixed with the window or page the caller asked for.
#[derive(Debug, Clone)]
pub(crate) struct PageFailure {
    /// The session whose log could not be read.
    pub(crate) session: String,
    /// The first global row asked for (a message window), or the page counted
    /// from the newest end (a trajectory page).
    pub(crate) at: usize,
    /// The reader's error text, naming the segment and the row group.
    pub(crate) detail: String,
    /// Whether the one bounded reopen-and-retry was attempted.
    pub(crate) retried: bool,
}

impl std::fmt::Display for PageFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "session {}: the page at {} could not be read (retried: {}): {}",
            self.session, self.at, self.retried, self.detail
        )
    }
}

/// Compose a page failure's detail from the reader's error.
///
/// The reader names the segment and the row group; the window is added here so
/// the caller can see which page failed. The history directory is appended only
/// when the error does not already name a path, so a bare footer or decode
/// error still says where to look.
fn failure_detail<F, S>(
    dir: &Path,
    at: usize,
    len: usize,
    first: &F,
    second: Option<&S>,
    covered: bool,
) -> String
where
    F: std::fmt::Display,
    S: std::fmt::Display,
{
    let mut detail = format!("rows {at}..{}: {first}", at.saturating_add(len));
    let directory = dir.display().to_string();
    if !detail.contains(&directory) {
        detail.push_str(&format!(" (in {directory})"));
    }
    match second {
        Some(second) if second.to_string() != first.to_string() => {
            // The reopen reported something else; both are evidence about the log.
            detail.push_str(&format!("; after reopening: {second}"));
        }
        Some(_) => {}
        None if !covered => detail.push_str(
            "; reopening the history found fewer rows than the page needs, \
             so a compaction swap does not explain it",
        ),
        None => {}
    }
    detail
}

/// A trajectory page waiting to be read off the log.
pub(crate) struct TraceRead {
    session: String,
    page: usize,
    reader: Reader,
    /// The directory the index came from, for the same retry as a chat read.
    dir: PathBuf,
}

impl TraceRead {
    /// Decode the page. Blocking: background executor only.
    pub(crate) fn read(self) -> TracePage {
        let rows = match self.reader.trajectory_page(self.page) {
            Ok(rows) => Ok(rows),
            Err(first) => self.retry(first),
        };
        TracePage {
            failure: rows.as_ref().err().cloned(),
            session: self.session,
            page: self.page,
            rows: rows.unwrap_or_default(),
        }
    }

    /// Reopen the history ONCE and retry, the compaction-swap retry.
    ///
    /// Same bound as the chat window: one reopen, the reopened index has to
    /// still hold the page, and only then is a second read attempted.
    fn retry<E: std::fmt::Display>(&self, first: E) -> Result<Vec<ToolRow>, PageFailure> {
        let reopened = History::open(self.dir.clone());
        let covered = reopened.trajectory_pages() > self.page;
        let mut second = None;
        if covered {
            match reopened.reader().trajectory_page(self.page) {
                Ok(rows) => return Ok(rows),
                Err(error) => second = Some(error),
            }
        }
        Err(PageFailure {
            session: self.session.clone(),
            at: self.page,
            detail: failure_detail(
                &self.dir,
                self.page,
                crate::history::PAGE,
                &first,
                second.as_ref(),
                covered,
            ),
            retried: true,
        })
    }
}

/// The trajectory rows one TraceRead decoded.
pub(crate) struct TracePage {
    /// The session the page belongs to.
    pub(crate) session: String,
    /// The page counted from the newest end.
    pub(crate) page: usize,
    /// The decoded rows, oldest first. Empty when the read failed.
    pub(crate) rows: Vec<ToolRow>,
    /// Why the read failed, when it did.
    failure: Option<PageFailure>,
}

impl TracePage {
    /// The failure this page carries, if the read did not succeed.
    ///
    /// A page that failed is not an empty page: the caller must surface this
    /// instead of showing nothing.
    pub(crate) fn failure(&self) -> Option<&PageFailure> {
        self.failure.as_ref()
    }
}

/// What one recovery pass did with one journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    /// The rows were folded into the log and the journal removed.
    Replayed,
    /// The log already held the journal's rows; the journal was removed.
    AlreadyCovered,
    /// The journal is valid but the replay failed; it stays for the next launch.
    Kept,
    /// The journal could not be trusted and was moved aside, bytes intact.
    Quarantined(QuarantineReason),
}

/// Why a journal was quarantined.
///
/// Each label is also the last component of the incident directory's name, so a
/// human can read the cause off the directory listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuarantineReason {
    /// The file held no bytes.
    Empty,
    /// The JSON stopped in the middle.
    Truncated,
    /// The bytes were not a journal at all.
    Unreadable,
    /// The checksum did not match the payload.
    ChecksumMismatch,
    /// The declared format version is newer than this build knows.
    UnsupportedVersion,
    /// The journal's base row is beyond the log it belongs to.
    BaseBeyondLog,
    /// Only the pre-rename temporary was left, as a kill before the rename is.
    InterruptedWrite,
}

impl QuarantineReason {
    /// The stable label the incident directory and its record carry.
    fn label(self) -> &'static str {
        match self {
            QuarantineReason::Empty => "empty",
            QuarantineReason::Truncated => "truncated",
            QuarantineReason::Unreadable => "unreadable",
            QuarantineReason::ChecksumMismatch => "checksum-mismatch",
            QuarantineReason::UnsupportedVersion => "unsupported-version",
            QuarantineReason::BaseBeyondLog => "base-beyond-log",
            QuarantineReason::InterruptedWrite => "interrupted-write",
        }
    }
}

impl From<JournalDefect> for QuarantineReason {
    fn from(defect: JournalDefect) -> Self {
        match defect {
            JournalDefect::Empty => QuarantineReason::Empty,
            JournalDefect::Truncated => QuarantineReason::Truncated,
            JournalDefect::Unreadable => QuarantineReason::Unreadable,
            JournalDefect::ChecksumMismatch => QuarantineReason::ChecksumMismatch,
            JournalDefect::UnsupportedVersion => QuarantineReason::UnsupportedVersion,
        }
    }
}

/// One journal's outcome, as the recovery report records it.
///
/// The record carries metadata only - never a message's contents. Its shape is
/// also what the `report.json` beside a quarantined journal holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecoveryRecord {
    /// The session the journal belonged to.
    pub(crate) session: String,
    /// Where the journal was read from (or moved from).
    pub(crate) source_path: String,
    /// What this pass did, or why the journal was quarantined.
    pub(crate) reason: String,
    /// The journal's declared format version; 0 is a legacy journal.
    pub(crate) version: u32,
    /// How many message rows it held - a count, never contents.
    pub(crate) rows: usize,
    /// When this pass classified it, in milliseconds since the Unix epoch.
    pub(crate) quarantined_at: u64,
}

/// What the last recovery pass found, so a caller can show it instead of
/// claiming a clean start.
///
/// Filled by `recover_inflight` at open; read it through
/// `Store::recovery_report`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryReport {
    /// Journals folded into their log.
    pub(crate) replayed: Vec<RecoveryRecord>,
    /// Journals kept for the next launch because the replay failed.
    pub(crate) kept: Vec<RecoveryRecord>,
    /// Journals moved into `corrupt/`, original bytes intact.
    pub(crate) quarantined: Vec<RecoveryRecord>,
}

/// The directory a workspace keeps its sessions in.
pub(crate) fn sessions_dir(root: &Path) -> PathBuf {
    root.join(".bsh").join("sessions")
}

/// Milliseconds since the Unix epoch, for a quarantine stamp.
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// Create a fresh incident directory under `base`, never reusing a name.
///
/// A name another process already took is not an error: the next suffix is
/// tried, so two processes quarantining in the same millisecond never contend.
fn incident_dir(base: &Path, name: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    let mut attempt = 0;
    loop {
        let candidate = if attempt == 0 {
            base.join(name)
        } else {
            base.join(format!("{name}-{attempt}"))
        };
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt += 1;
                if attempt > 64 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "no free incident directory name",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// The version and row count a journal's bytes declare, best effort.
///
/// Used for a journal that cannot be trusted: a truncated or edited file still
/// usually names its version and holds a `rows` array, and the report may only
/// carry counts - never contents.
fn partial_metadata(bytes: &[u8]) -> (u32, usize) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return (0, 0);
    };
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default() as u32;
    let rows = value
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |rows| rows.len());
    (version, rows)
}

/// Read one session document in full, streaming from its file.
fn read_document_at(path: &Path) -> Option<Session> {
    let file = fs::File::open(path).ok()?;
    serde_json::from_reader(BufReader::new(file)).ok()
}

/// The metadata a session row needs, without its full document.
///
/// Serde skips the transcript, plan, goal and archived table while parsing, so
/// startup never materializes a document's full JSON; the document is read by
/// `Store::ensure_document` when the session is selected or mutated.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SessionMeta {
    id: String,
    title: String,
    created_at: u64,
    updated_at: u64,
    workspace: String,
    provider: Option<String>,
    model: Option<String>,
    agent_preset: Option<String>,
    subagent: Option<crate::model::SubagentMeta>,
    pinned: bool,
    archived_at: Option<u64>,
}

/// The loaded set of sessions plus where each one is written.
pub(crate) struct Store {
    /// Where a session goes when its workspace is unknown.
    quarantine: PathBuf,
    /// Workspace id to the directory that workspace keeps its sessions in.
    roots: BTreeMap<String, PathBuf>,
    /// Every session row, newest first. Rows start as metadata only; the
    /// selected or mutated one is filled in on demand.
    pub(crate) sessions: Vec<Session>,
    /// Opened history indexes, so scrolling reuses the manifest and page cache
    /// instead of scanning a directory on every page.
    histories: RefCell<HashMap<PathBuf, History>>,
    /// The credential strings that must never reach a session file.
    ///
    /// A transcript is a record of what was said, and what was said sometimes
    /// QUOTES the configuration - a tool that printed the settings file puts the
    /// key in the conversation. The session document must not store a
    /// credential (the design keeps provider references only), so the live keys
    /// are scrubbed out of the serialized text at the one place a session is
    /// written. Only the exact configured values are replaced, never a pattern,
    /// so an ordinary sentence is not mangled by a guess at what a secret looks
    /// like.
    secrets: Vec<String>,
    /// The held lock of every journal this store owns, keyed by history
    /// directory. Dropping the handle releases the lock, which is how a crash
    /// becomes distinguishable from a live turn.
    inflight_locks: RefCell<HashMap<PathBuf, File>>,
    /// What the last recovery pass did with every journal it found, so nothing
    /// is silently dropped and a caller can show the state instead of claiming
    /// a clean start.
    recovery: RecoveryReport,
}

impl Store {
    /// Open the store over every registered workspace.
    ///
    /// The pairs are `(workspace id, workspace root)`: the id is how a session
    /// says where it lives, the root is where that turns into a path.
    pub(crate) fn open(workspaces: &[(String, String)]) -> Self {
        Self::open_over(workspaces, crate::settings::dir().join("sessions"))
    }

    /// Open a store over explicit workspace roots and an explicit quarantine.
    ///
    /// Tests point the quarantine at a scratch directory, so nothing under the
    /// user home is read or written.
    pub(crate) fn open_over(workspaces: &[(String, String)], quarantine: PathBuf) -> Self {
        let roots: BTreeMap<String, PathBuf> = workspaces
            .iter()
            .filter(|(id, root)| !id.trim().is_empty() && !root.trim().is_empty())
            .map(|(id, root)| (id.clone(), sessions_dir(Path::new(root))))
            .collect();
        let mut store = Self {
            quarantine,
            roots,
            sessions: Vec::new(),
            histories: RefCell::new(HashMap::new()),
            secrets: Vec::new(),
            inflight_locks: RefCell::new(HashMap::new()),
            recovery: RecoveryReport::default(),
        };
        let dirs: Vec<PathBuf> = store.roots.values().cloned().collect();
        store.read(&dirs);
        store.sort();
        store.recover_inflight();
        store
    }

    /// Open a store rooted at one explicit session directory.
    ///
    /// Test-only: it is how a test points the store at a scratch directory, so
    /// nothing under the user home is read or written. A real store is opened
    /// with `open` or `open_over`, because its roots come from the registry.
    /// The session directory a given session's files belong in.
    ///
    /// The workspace's own directory when the workspace is known, else the
    /// quarantine. A parked log has to be addressed through this rather than
    /// through a bare root, because the log sits BESIDE the document and the two
    /// have to land in the same place when a chat is put back.
    pub(crate) fn dir_for_id(&self, id: &str) -> PathBuf {
        match self.sessions.iter().find(|session| session.id == id) {
            Some(session) => self.dir_for(session).to_path_buf(),
            None => self.quarantine.clone(),
        }
    }

    /// The directory this store writes to when a workspace is unknown.
    pub(crate) fn root(&self) -> &Path {
        &self.quarantine
    }

    /// Whether this store was pointed away from the real sessions home.
    ///
    /// A test opens one, and the answer is what keeps a test's settings writes
    /// beside its own sessions rather than in the user's configuration.
    pub(crate) fn is_scratch(&self) -> bool {
        self.quarantine != crate::settings::dir().join("sessions")
    }

    /// What the last recovery pass found: journals replayed, kept for the next
    /// launch, and quarantined with their original bytes.
    ///
    /// The report is filled when the store opens, so a caller can show a real
    /// state instead of claiming a clean start.
    pub(crate) fn recovery_report(&self) -> &RecoveryReport {
        &self.recovery
    }

    /// Open a store over a scratch directory, for the tests: its roots come
    /// from the registry and its quarantine is a directory a test owns.
    #[cfg(test)]
    pub(crate) fn open_at(dir: PathBuf) -> Self {
        let mut store = Self {
            quarantine: dir,
            roots: BTreeMap::new(),
            sessions: Vec::new(),
            histories: RefCell::new(HashMap::new()),
            secrets: Vec::new(),
            inflight_locks: RefCell::new(HashMap::new()),
            recovery: RecoveryReport::default(),
        };
        store.read(&[]);
        store.sort();
        store.recover_inflight();
        store
    }

    /// Read every session's metadata, from each directory in turn.
    ///
    /// One directory may be named twice - a workspace rooted at the harness
    /// directory would collide with the quarantine - so a path is read once.
    /// Each document is streamed into `SessionMeta`; the transcript, plan, goal
    /// and archived table are skipped, and no history file is opened. A row is
    /// fully read only by `ensure_document`/`rehydrate` when it is selected.
    fn read(&mut self, dirs: &[PathBuf]) {
        let quarantine = self.quarantine.clone();
        let mut seen: BTreeSet<&Path> = BTreeSet::new();
        for dir in dirs.iter().chain(std::iter::once(&quarantine)) {
            if !seen.insert(dir.as_path()) {
                continue;
            }
            let _ = fs::create_dir_all(dir);
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                // Stream one document's metadata. Serde parses the fields the
                // row needs and discards the rest, so the full JSON is never
                // materialized in memory at startup.
                let Ok(file) = fs::File::open(&path) else {
                    continue;
                };
                let Ok(meta) = serde_json::from_reader::<_, SessionMeta>(BufReader::new(file))
                else {
                    continue;
                };
                let session = Session {
                    id: meta.id,
                    title: meta.title,
                    created_at: meta.created_at,
                    updated_at: meta.updated_at,
                    workspace: meta.workspace,
                    provider: meta.provider,
                    model: meta.model,
                    agent_preset: meta.agent_preset,
                    subagent: meta.subagent,
                    messages: Vec::new(),
                    last_injected_persona: None,
                    plan: Vec::new(),
                    goal: None,
                    plan_mode: false,
                    compacted: 0,
                    feedback: None,
                    pinned: meta.pinned,
                    archived_at: meta.archived_at,
                    archived: Vec::new(),
                    compactions: Vec::new(),
                    compact_command: None,
                    loaded_from: 0,
                    paged_from: None,
                    loaded: false,
                };
                // No history file is opened at startup. The selected row is
                // hydrated by `rehydrate`, which reads only its tail window; a
                // list-only row carries no transcript at all.
                self.sessions.push(session);
            }
        }
        self.dedupe_by_id();
    }

    /// Where one session is written: beside its workspace, else the quarantine.
    fn dir_for(&self, session: &Session) -> &Path {
        self.roots
            .get(&session.workspace)
            .map_or(self.quarantine.as_path(), PathBuf::as_path)
    }

    /// The cached History at a directory, opening its manifest once.
    fn history_at(&self, path: PathBuf) -> RefMut<'_, History> {
        let key = path.clone();
        let mut cache = self.histories.borrow_mut();
        cache
            .entry(path)
            .or_insert_with(|| History::open(key.clone()));
        RefMut::map(cache, move |map| {
            map.get_mut(&key).expect("the history was just inserted")
        })
    }

    /// The cached History for a session id.
    fn history_for(&self, id: &str) -> Option<RefMut<'_, History>> {
        let session = self.sessions.iter().find(|session| session.id == id)?;
        let path = History::dir_for(self.dir_for(session), id);
        Some(self.history_at(path))
    }

    /// A session's own directory, resolved from its workspace id.
    ///
    /// A trashed session is no longer in `sessions`, so `dir_for_id` would fall
    /// back to the quarantine root - which is why purging one used to leave its
    /// Parquet log behind. The workspace id outlives the session document, so
    /// the directory can still be named after it is gone.
    pub(crate) fn dir_for_workspace(&self, workspace: &str) -> PathBuf {
        self.roots
            .get(workspace)
            .cloned()
            .unwrap_or_else(|| self.quarantine.clone())
    }

    /// Declare the credentials that must not be written into a session file.
    pub(crate) fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets
            .into_iter()
            .map(|secret| secret.trim().to_string())
            .filter(|secret| secret.len() >= 6)
            .collect();
        self
    }

    /// Redact every configured credential from one serialized session.
    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }

    /// Keep one session per id.
    ///
    /// The same id can be on disk twice: a session written while its workspace
    /// was unknown landed in the quarantine, and a later save lands beside its
    /// workspace. Reading both would show the same conversation twice, so the
    /// newer copy wins and the older one is dropped from the list.
    fn dedupe_by_id(&mut self) {
        let mut newest: BTreeMap<String, usize> = BTreeMap::new();
        for (index, session) in self.sessions.iter().enumerate() {
            match newest.get(&session.id) {
                Some(&kept) if self.sessions[kept].updated_at >= session.updated_at => {}
                _ => {
                    newest.insert(session.id.clone(), index);
                }
            }
        }
        let mut kept: Vec<usize> = newest.into_values().collect();
        kept.sort_unstable();
        self.sessions = kept.into_iter().map(|i| self.sessions[i].clone()).collect();
    }

    /// Rebuild the workspace-to-directory map from the registry.
    ///
    /// The store keeps its own copy of which roots exist because it needs one to
    /// know where a session file lives. That copy is a CACHE of the settings
    /// registry, never a second truth: after a workspace is added or removed the
    /// registry is re-read through this, or a chat in a just-added workspace is
    /// saved into the quarantine directory (no root to name it) and a deleted
    /// workspace's chats keep reading as owned.
    pub(crate) fn set_roots(&mut self, workspaces: &[(String, String)]) {
        self.roots = workspaces
            .iter()
            .filter(|(id, root)| !id.trim().is_empty() && !root.trim().is_empty())
            .map(|(id, root)| (id.clone(), sessions_dir(Path::new(root))))
            .collect();
    }

    /// Attach the workspace-less sessions to the workspace in use.
    ///
    /// A session whose workspace id the registry no longer names belongs to no
    /// row the sidebar draws, so it is loaded and then invisible - history the
    /// reader cannot reach. Re-homing it is the only thing that can happen to
    /// it: there is no other workspace to attribute it to. The old file is
    /// removed, because the next save writes the session under its workspace and
    /// leaving the quarantined copy behind would put the same id on disk twice.
    pub(crate) fn rehome_orphans(&mut self, active: &str) -> usize {
        if active.trim().is_empty() || !self.roots.contains_key(active) {
            return 0;
        }
        let orphans: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| !self.roots.contains_key(&session.workspace))
            .map(|session| session.id.clone())
            .collect();
        for id in &orphans {
            let Some(index) = self.sessions.iter().position(|s| &s.id == id) else {
                continue;
            };
            let stale = self
                .dir_for(&self.sessions[index])
                .join(format!("{id}.json"));
            self.sessions[index].workspace = active.to_string();
            let session = self.sessions[index].clone();
            let _ = fs::remove_file(&stale);
            let _ = self.save(&session);
        }
        if orphans.is_empty() {
            return 0;
        }
        self.sort();
        orphans.len()
    }

    /// Sort newest-first, pinned sessions ahead of the rest.
    pub(crate) fn sort(&mut self) {
        self.sessions.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
    }

    /// Look up a session by id.
    pub(crate) fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// The path of a session's document.
    fn document_path(&self, session: &Session) -> PathBuf {
        self.dir_for(session).join(format!("{}.json", session.id))
    }

    /// Read one session document in full, for the selected or mutated row.
    fn read_document(&self, session: &Session) -> Option<Session> {
        read_document_at(&self.document_path(session))
    }

    /// Fill the metadata-only fields a startup row leaves empty.
    ///
    /// Startup streams only `SessionMeta`, so plan, goal and archived are not in
    /// memory until this runs. History fields and the in-memory transcript window
    /// are kept; the document's `messages` are always empty on disk.
    pub(crate) fn ensure_document(&mut self, id: &str) {
        let Some(at) = self.sessions.iter().position(|session| session.id == id) else {
            return;
        };
        if self.sessions[at].loaded {
            return;
        }
        let path = self.document_path(&self.sessions[at]);
        let Some(document) = read_document_at(&path) else {
            return;
        };
        let session = &mut self.sessions[at];
        session.title = document.title;
        session.created_at = document.created_at;
        session.updated_at = document.updated_at;
        session.workspace = document.workspace;
        session.provider = document.provider;
        session.model = document.model;
        session.agent_preset = document.agent_preset;
        session.subagent = document.subagent;
        session.pinned = document.pinned;
        session.archived_at = document.archived_at;
        session.plan = document.plan;
        session.goal = document.goal;
        session.plan_mode = document.plan_mode;
        session.compacted = document.compacted;
        session.feedback = document.feedback;
        session.archived = document.archived;
        session.compactions = document.compactions;
        session.compact_command = document.compact_command;
        // A document written before the Parquet log still carries its
        // transcript inline; keep it so the first save migrates it into the log.
        if session.messages.is_empty() && !document.messages.is_empty() {
            session.messages = document.messages;
        }
        session.loaded = true;
    }

    /// Look up a session by id for mutation.
    pub(crate) fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.ensure_document(id);
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Delete a session's file and forget it.
    ///
    /// Removing the row without the file would leave a session that reappears on
    /// the next launch, so the file goes in the same call; a file that cannot be
    /// removed still drops the row, because the reader asked for it to be gone.
    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let Some(index) = self.sessions.iter().position(|session| session.id == id) else {
            return false;
        };
        self.histories.borrow_mut().clear();
        // Every root is swept, not only the one this build resolved. A session
        // id is unique, so the same document under a second root is a stray
        // left by an earlier move between workspaces - and deleting only the
        // resolved copy left that stray on disk, which the NEXT LAUNCH read
        // back. That is why a removed chat came back after a restart.
        for dir in self.session_dirs() {
            let document = dir.join(format!("{id}.json"));
            let _ = fs::remove_file(dir.join(format!("{id}.json.tmp")));
            if document.is_file() {
                if let Err(error) = fs::remove_file(&document) {
                    log::warn!(
                        "session {id} document could not be removed ({}): {error}",
                        document.display()
                    );
                }
            }
            let history = History::dir_for(&dir, id);
            if history.is_dir() {
                // Release the journal lock before deleting its file: a held
                // handle keeps the lock (and on Windows the file) alive.
                self.drop_inflight(&history);
                let _ = fs::remove_dir_all(&history);
            }
        }
        self.sessions.remove(index);
        true
    }

    /// Every directory a session document can live in: each registered
    /// workspace root, then the harness directory. A path appears once.
    fn session_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.roots.values().cloned().collect();
        dirs.push(self.quarantine.clone());
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Drop the same session's document from every root except `keep`.
    ///
    /// A save writes a session to the root of the workspace it belongs to, so a
    /// copy anywhere else is stale by definition. Removing it here heals the
    /// strays that already exist instead of waiting for the delete that would
    /// otherwise resurrect the chat on the next launch.
    fn drop_strays(&self, id: &str, keep: &Path) {
        for dir in self.session_dirs() {
            if dir == keep {
                continue;
            }
            let document = dir.join(format!("{id}.json"));
            let _ = fs::remove_file(dir.join(format!("{id}.json.tmp")));
            if document.is_file() {
                log::warn!(
                    "dropping a stray copy of session {id} under {}",
                    dir.display()
                );
                let _ = fs::remove_file(&document);
            }
            let history = History::dir_for(&dir, id);
            if history.is_dir() {
                self.drop_inflight(&history);
                let _ = fs::remove_dir_all(&history);
            }
        }
    }

    /// Insert a session at the front.
    pub(crate) fn insert(&mut self, session: Session) {
        self.histories.borrow_mut().clear();
        self.sessions.insert(0, session);
    }

    /// Write a session: its transcript to the Parquet log, its document to JSON.
    ///
    /// The log is append-only, so a save costs the rows written since the last
    /// one rather than the history - and the document no longer embeds the
    /// transcript, which keeps a long conversation's metadata small and lets a
    /// session be listed without touching its log.
    pub(crate) fn save(&self, session: &Session) -> anyhow::Result<()> {
        let dir = self.dir_for(session);
        fs::create_dir_all(dir)?;
        let normalized = self.normalized(session);
        self.write_document(&normalized, dir)?;
        self.save_history(&normalized, dir)?;
        // A save has just committed the rows the journal was holding, so the
        // journal can go only when the log really covers it; a refused or
        // partial write must leave the journal for the next launch.
        self.release_inflight(&normalized, dir);
        // The document just written is the authority; the same id anywhere else
        // is a leftover that would resurrect this chat if it were deleted.
        self.drop_strays(&normalized.id, dir);
        Ok(())
    }

    /// Write only the session document, without touching the log.
    ///
    /// A turn's first act is to save its prompt this way: the document gains
    /// the title and the updated time, and the log gains the user row through
    /// the ordinary save. The reply itself is not written until it exists, so
    /// this path stays cheap and leaves the in-flight journal alone.
    pub(crate) fn save_document(&self, session: &Session) -> anyhow::Result<()> {
        let dir = self.dir_for(session);
        fs::create_dir_all(dir)?;
        let normalized = self.normalized(session);
        self.write_document(&normalized, dir)
    }

    /// Fill the metadata-only fields a save must not write back as empty.
    fn normalized(&self, session: &Session) -> Session {
        let mut normalized = session.clone();
        if !session.loaded {
            // A metadata-only row was never fully parsed. Merge the document's
            // plan/goal/archived so a pin, rename or archive does not rewrite
            // them as empty, and keep an inline pre-Parquet transcript so the
            // first save migrates it into the log.
            if let Some(full) = self.read_document(session) {
                normalized.plan = full.plan;
                normalized.goal = full.goal;
                normalized.plan_mode = full.plan_mode;
                normalized.compacted = full.compacted;
                normalized.feedback = full.feedback;
                normalized.archived = full.archived;
                normalized.compactions = full.compactions;
                normalized.compact_command = full.compact_command;
                if normalized.messages.is_empty() {
                    normalized.messages = full.messages;
                }
            }
        }
        normalized
    }

    /// Write the JSON document, which never embeds the transcript.
    ///
    /// The write is a durable commit, not a bare rename: the bytes are synced
    /// and, on Unix, the parent directory is synced too, so a documented save
    /// survives a power loss rather than only a process kill. (Windows cannot
    /// sync a directory, so the stronger guarantee is Unix-only.) A kill between
    /// the old file and the new one leaves the old document readable, because
    /// the loader silently skips a file that does not parse and that would read
    /// as the whole session disappearing.
    fn write_document(&self, session: &Session, dir: &Path) -> anyhow::Result<()> {
        let mut document = session.clone();
        document.messages.clear();
        let path = dir.join(format!("{}.json", session.id));
        // A temporary left by a crash of the pre-commit writer is debris; the
        // commit below uses its own uniquely named one.
        let _ = fs::remove_file(path.with_extension("json.tmp"));
        let text = self.redact(&serde_json::to_string_pretty(&document)?);
        // A session document carries the transcript and plan, so it is
        // owner-only on every host.
        hostkit::durable::commit(
            &path,
            text.as_bytes(),
            hostkit::durable::Privacy::OwnerOnly,
            &hostkit::durable::NoFaults,
        )?;
        Ok(())
    }

    /// Append what is new in the in-memory window to the session's log.
    fn save_history(&self, session: &Session, dir: &Path) -> anyhow::Result<()> {
        let mut history = self.history_at(History::dir_for(dir, &session.id));
        // The log is the transcript now, so it passes through the same
        // redaction the document did: a quoted credential must not reach
        // either one.
        let rows: Vec<crate::model::ChatMessage> = session
            .messages
            .iter()
            .map(|message| self.redact_message(message))
            .collect();
        let start = session.loaded_from;
        let end = start + rows.len();
        // A save never empties a log it did not write: a document that has not
        // been hydrated yet (a restore, a list-only read) carries no rows, and
        // treating that as a truncation would wipe the transcript.
        if rows.is_empty() && history.len() > 0 {
            return Ok(());
        }
        let count = history.len();
        if count == end {
            // The row COUNT is not the row CONTENT. Streaming mutates the last
            // row in place - text, reasoning, tool results, error - so a save
            // that lands after a placeholder was already appended (a goal
            // round, an automatic-compaction install, a stop after either) used
            // to return here as "nothing new" and the turn's output was never
            // written. A halted turn showed it plainly: the log stopped at an
            // empty assistant row.
            let unchanged = match (history.window(count.saturating_sub(1), 1), rows.last()) {
                (Ok(saved), Some(last)) => saved
                    .first()
                    .is_some_and(|saved| Self::same_transcript(saved, last)),
                _ => true,
            };
            if unchanged {
                return Ok(());
            }
        }
        // The common case: the log ends inside the in-memory window, so only
        // the rows after that point are new. Appending them keeps a save
        // O(new rows) even after a restored tail window has moved `start`
        // forward. The old code only appended when count == start (an empty
        // log from the window start), so a new turn after a tail restore
        // rewrote every page of the session.
        if count >= start && count < end {
            let skip = count - start;
            history.append(&rows[skip..])?;
            return Ok(());
        }
        // A window that claims to start at row 0 but does not reach the log's
        // end is not a truncation of THIS log - it is a list-only row that a
        // turn wrote into without hydrating the window. Writing it would drop
        // every row the window does not know about, which is how a whole
        // conversation could disappear.
        if start == 0 && count > end {
            log::warn!(
                "session {}: refusing to rewrite its log from {count} to {end} rows; the window was never hydrated",
                session.id
            );
            return Ok(());
        }
        // A truncation or a compaction changed the tail. The pages ABOVE the
        // window are untouched, so they are read back before the rewrite:
        // rebuilding from the window alone would drop them.
        let mut all = history.prefix(start)?;
        all.extend(rows);
        history.rewrite(&all)?;
        Ok(())
    }

    /// Whether two rows would write the same transcript content.
    ///
    /// Timing, parts and usage are display metadata that settles after a turn;
    /// the content is what a lost save would drop, so that is what decides
    /// whether the log has to be rewritten.
    fn same_transcript(a: &crate::model::ChatMessage, b: &crate::model::ChatMessage) -> bool {
        a.role == b.role
            && a.text == b.text
            && a.reasoning == b.reasoning
            && a.error == b.error
            && a.steering == b.steering
            && a.tools.len() == b.tools.len()
            && a.tools.iter().zip(&b.tools).all(|(left, right)| {
                left.name == right.name && left.args == right.args && left.output == right.output
            })
    }

    /// One message with every string run through the secret scrubber.
    fn redact_message(&self, message: &crate::model::ChatMessage) -> crate::model::ChatMessage {
        let mut rows = message.clone();
        rows.text = self.redact(&rows.text);
        rows.reasoning = self.redact(&rows.reasoning);
        rows.error = rows.error.as_deref().map(|text| self.redact(text));
        for tool in &mut rows.tools {
            tool.args = self.redact(&tool.args);
            tool.output = tool.output.as_deref().map(|text| self.redact(text));
        }
        rows
    }

    /// Journal the rows a running turn has not committed to the log yet.
    ///
    /// `base` is the global index the turn's mutable tail starts at. It is
    /// clamped to the log's current length, because a save that failed between
    /// the prompt and the first token leaves one more row to carry.
    pub(crate) fn checkpoint(&self, session: &Session, base: usize) -> anyhow::Result<()> {
        let len = match self.history_for(&session.id) {
            Some(history) => history.len(),
            None => return Ok(()),
        };
        let end = session.loaded_from.saturating_add(session.messages.len());
        if len > end {
            // The log is ahead of the loaded window. Replacing its tail from a
            // window that never read those rows would drop them.
            return Ok(());
        }
        let base = base.min(len);
        if base < session.loaded_from {
            return Ok(());
        }
        let skip = base - session.loaded_from;
        let Some(tail) = session.messages.get(skip..) else {
            return Ok(());
        };
        if tail.is_empty() {
            return Ok(());
        }
        let rows: Vec<ChatMessage> = tail
            .iter()
            .map(|message| self.redact_message(message))
            .collect();
        let dir = History::dir_for(self.dir_for(session), &session.id);
        self.lock_inflight(&dir)?;
        // The journal carries its own integrity: a torn or edited file is
        // detected at recovery instead of being replayed as rows.
        let journal = Inflight::new(base, rows)?;
        let temporary = dir.join(INFLIGHT_TMP);
        fs::write(&temporary, serde_json::to_vec(&journal)?)?;
        fs::rename(&temporary, dir.join(INFLIGHT))?;
        Ok(())
    }

    /// Drop the journal once the log covers it.
    ///
    /// A save that refuses or only partly writes (a metadata-only row, a full
    /// disk) must leave the journal in place: it is the only copy of the turn.
    /// The store calls this only AFTER `save_history` returned Ok, so the
    /// journal is released only once the log really carries its rows.
    fn release_inflight(&self, session: &Session, dir: &Path) {
        let history_dir = History::dir_for(dir, &session.id);
        let bytes = match fs::read(history_dir.join(INFLIGHT)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.drop_inflight(&history_dir);
                return;
            }
            Err(error) => {
                // A journal we cannot read is not a journal we may delete:
                // the save may not have covered it.
                log::warn!(
                    "session {}: the in-flight journal could not be read: {error}",
                    session.id
                );
                return;
            }
        };
        let Ok(journal) = Inflight::parse(&bytes) else {
            // A journal that cannot be verified is not a journal we may delete:
            // the save may not have covered the row it is the only copy of. It
            // stays for the next launch, which quarantines it with its bytes.
            log::warn!(
                "session {}: the in-flight journal could not be verified; keeping it",
                session.id
            );
            return;
        };
        let len = self
            .history_for(&session.id)
            .map(|history| history.len())
            .unwrap_or(0);
        if len >= journal.base.saturating_add(journal.rows.len()) {
            self.drop_inflight(&history_dir);
        }
    }

    /// Take the OS lock that marks a journal as a live turn's.
    fn lock_inflight(&self, dir: &Path) -> anyhow::Result<()> {
        let path = dir.join(INFLIGHT_LOCK);
        let mut locks = self.inflight_locks.borrow_mut();
        // A log rewrite swaps the whole history directory, which takes the
        // lock file with it while this handle still points at the old inode.
        // Re-lock the file that is on disk now instead of trusting the stale
        // handle: otherwise a second process would see an unlocked journal.
        if locks.contains_key(dir) && path.is_file() {
            return Ok(());
        }
        locks.remove(dir);
        fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        file.try_lock_exclusive()?;
        // The pid is a diagnostic for a human reading the directory; the lock
        // itself is the liveness test.
        let mut handle = &file;
        let _ = handle.write_all(std::process::id().to_string().as_bytes());
        locks.insert(dir.to_path_buf(), file);
        Ok(())
    }

    /// Release the lock and remove every file of one journal.
    fn drop_inflight(&self, dir: &Path) {
        // Dropping the handle releases the lock before the files go, which is
        // what Windows requires and what makes the removal safe everywhere.
        self.inflight_locks.borrow_mut().remove(dir);
        let _ = fs::remove_file(dir.join(INFLIGHT));
        let _ = fs::remove_file(dir.join(INFLIGHT_TMP));
        let _ = fs::remove_file(dir.join(INFLIGHT_LOCK));
    }

    /// Recover every journal a killed process left behind.
    ///
    /// A journal whose lock is still held belongs to a live turn - a second
    /// window must not append it early. The OS drops that lock when the owner
    /// dies, which is the one signal a pid cannot give reliably.
    ///
    /// Nothing unreadable is deleted here. A journal this pass cannot trust is
    /// moved into `corrupt/` with its original bytes; a valid journal whose
    /// replay fails stays where it is for the next launch. Both outcomes are
    /// recorded in `Store::recovery_report`.
    fn recover_inflight(&mut self) {
        let mut histories: Vec<PathBuf> = Vec::new();
        for dir in self.session_dirs() {
            let root = dir.join("history");
            let Ok(entries) = fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    histories.push(path);
                }
            }
        }
        histories.sort();
        histories.dedup();
        for history_dir in histories {
            let journal = history_dir.join(INFLIGHT);
            let temporary = history_dir.join(INFLIGHT_TMP);
            let has_journal = journal.is_file();
            // A kill between writing the temporary and renaming it leaves only
            // the temporary; its bytes are the only copy of that turn.
            let has_temporary = temporary.is_file();
            if !has_journal && !has_temporary {
                continue;
            }
            let Some(id) = history_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let lock_path = history_dir.join(INFLIGHT_LOCK);
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .ok();
            if let Some(lock) = &lock {
                if lock.try_lock_exclusive().is_err() {
                    // A live process owns this turn; its own save clears it.
                    continue;
                }
            }
            if has_journal && !self.sessions.iter().any(|session| session.id == id) {
                // The document is gone: a delete that crashed before removing
                // the log. Nothing can be recovered onto a session nobody can
                // select.
                let _ = fs::remove_file(&journal);
                let _ = fs::remove_file(&temporary);
                drop(lock);
                let _ = fs::remove_file(&lock_path);
                continue;
            }
            let recovered = if has_journal {
                self.recover_turn(&history_dir, &id)
            } else {
                let (version, rows) = partial_metadata(&fs::read(&temporary).unwrap_or_default());
                let record = self.recovery_record(
                    &id,
                    &temporary,
                    QuarantineReason::InterruptedWrite.label(),
                    version,
                    rows,
                );
                self.quarantine(
                    &history_dir,
                    &temporary,
                    QuarantineReason::InterruptedWrite,
                    record,
                )
            };
            drop(lock);
            if let Recovery::Quarantined(reason) = recovered {
                // The quarantine itself logged the one line a reader sees; this
                // names the classification for a debug trace.
                log::debug!("session {id}: journal classified as {}", reason.label());
            }
            if !matches!(recovered, Recovery::Kept) {
                let _ = fs::remove_file(&journal);
            }
            let _ = fs::remove_file(&lock_path);
        }
        let report = self.recovery_report();
        if !report.replayed.is_empty() || !report.kept.is_empty() || !report.quarantined.is_empty()
        {
            log::info!(
                "journal recovery: {} replayed, {} kept, {} quarantined",
                report.replayed.len(),
                report.kept.len(),
                report.quarantined.len()
            );
        }
    }

    /// The record one journal's outcome leaves behind.
    fn recovery_record(
        &self,
        id: &str,
        source: &Path,
        reason: &str,
        version: u32,
        rows: usize,
    ) -> RecoveryRecord {
        RecoveryRecord {
            session: id.to_string(),
            source_path: source.display().to_string(),
            reason: reason.to_string(),
            version,
            rows,
            quarantined_at: unix_ms(),
        }
    }

    /// Move one untrustworthy journal into a fresh incident directory.
    ///
    /// The order is the point: create the incident directory, move the journal
    /// in with ONE rename (so its bytes are never copied or rewritten), then
    /// write the report beside it through a temporary and a rename. A failure
    /// to create the directory returns `None` and leaves the journal where it
    /// is, because a journal that was not preserved must not be deleted.
    fn quarantine(
        &mut self,
        dir: &Path,
        source: &Path,
        reason: QuarantineReason,
        record: RecoveryRecord,
    ) -> Recovery {
        let name = format!("{}-{}-{}", unix_ms(), std::process::id(), reason.label());
        let incident = match incident_dir(&dir.join(CORRUPT), &name) {
            Ok(incident) => incident,
            Err(error) => {
                log::error!(
                    "session {}: a journal could not be quarantined ({error}); it is left in place",
                    record.session
                );
                return Recovery::Kept;
            }
        };
        let target = incident.join("journal.json");
        if let Err(error) = fs::rename(source, &target) {
            log::error!(
                "session {}: a journal could not be moved into {} ({error}); it is left in place",
                record.session,
                incident.display()
            );
            let _ = fs::remove_dir(&incident);
            return Recovery::Kept;
        }
        let report = match serde_json::to_vec_pretty(&record) {
            Ok(report) => report,
            Err(error) => {
                log::error!(
                    "session {}: a quarantine report could not be serialized: {error}",
                    record.session
                );
                let _ = fs::remove_dir(&incident);
                return Recovery::Kept;
            }
        };
        let report_tmp = incident.join("report.json.tmp");
        let installed = fs::write(&report_tmp, &report)
            .and_then(|()| fs::rename(&report_tmp, incident.join("report.json")));
        if let Err(error) = installed {
            log::error!(
                "session {}: a quarantine report could not be written: {error}",
                record.session
            );
        }
        log::warn!(
            "session {}: quarantined an interrupted turn's journal ({}) at {}",
            record.session,
            reason.label(),
            target.display()
        );
        self.recovery.quarantined.push(record);
        Recovery::Quarantined(reason)
    }

    /// Fold one interrupted turn's journal into its log, or preserve it.
    ///
    /// A journal that cannot be trusted is quarantined; one whose replay fails
    /// is kept where it is, because a journal still on disk gets another chance
    /// at the next launch while a removed one is gone forever.
    fn recover_turn(&mut self, dir: &Path, id: &str) -> Recovery {
        let source = dir.join(INFLIGHT);
        let bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "session {id}: an interrupted turn's journal could not be read: {error}"
                );
                let record =
                    self.recovery_record(id, &source, QuarantineReason::Unreadable.label(), 0, 0);
                return self.quarantine(dir, &source, QuarantineReason::Unreadable, record);
            }
        };
        let journal = match Inflight::parse(&bytes) {
            Ok(journal) => journal,
            Err(defect) => {
                let reason = QuarantineReason::from(defect);
                let (version, rows) = partial_metadata(&bytes);
                let record = self.recovery_record(id, &source, defect.label(), version, rows);
                return self.quarantine(dir, &source, reason, record);
            }
        };
        let version = journal.version;
        let journal_rows = journal.rows.len();
        let mut history = History::open(dir.to_path_buf());
        let len = history.len();
        if journal.rows.is_empty() {
            // Only an older writer could produce this; there is nothing to
            // place, and rewriting would truncate a log at `base`.
            return Recovery::AlreadyCovered;
        }
        // The journal can never reach past the log it was written against. A
        // base beyond the log is a journal from a directory that was replaced
        // (a fork, a rehome, a compaction) - there is nothing to place it on,
        // and it is evidence, so it is quarantined rather than dropped.
        if journal.base > len {
            let record = self.recovery_record(
                id,
                &source,
                QuarantineReason::BaseBeyondLog.label(),
                version,
                journal_rows,
            );
            return self.quarantine(dir, &source, QuarantineReason::BaseBeyondLog, record);
        }
        // A log that moved past the journal already carries newer rows. Rewriting
        // from the journal's base would delete them, so the log wins.
        if len > journal.base.saturating_add(journal_rows) {
            log::info!("session {id}: an interrupted turn's journal is behind the log");
            return Recovery::AlreadyCovered;
        }
        let mut rows = journal.rows;
        if let Some(last) = rows.last_mut() {
            if last.role == Role::Assistant {
                // The process died mid-turn: an answer-less row is a cancelled
                // turn, exactly as a stop records one, so the transcript says
                // why it ends and the next request does not replay an empty
                // assistant message.
                let empty = last.text.is_empty()
                    && last.reasoning.is_empty()
                    && last.tools.is_empty()
                    && last.error.is_none();
                if empty {
                    last.meta
                        .get_or_insert_with(crate::model::MessageMeta::default)
                        .cancelled = true;
                }
            }
        }
        let outcome = if len == journal.base {
            history.append(&rows)
        } else {
            match history.prefix(journal.base) {
                Ok(mut all) => {
                    all.extend(rows);
                    history.rewrite(&all)
                }
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) => {
                self.histories.borrow_mut().remove(dir);
                log::info!(
                    "session {id}: recovered {journal_rows} row(s) from a turn the process died in"
                );
                let record = self.recovery_record(id, &source, "replayed", version, journal_rows);
                self.recovery.replayed.push(record);
                Recovery::Replayed
            }
            Err(error) => {
                log::warn!(
                    "session {id}: an interrupted turn could not be recovered ({error}); its journal is kept"
                );
                let record =
                    self.recovery_record(id, &source, "replay-failed", version, journal_rows);
                self.recovery.kept.push(record);
                Recovery::Kept
            }
        }
    }

    /// Drop the messages BEHIND the reader once the window grows past a ceiling.
    ///
    /// Paging older rows in would otherwise leave every page the reader has
    /// passed through still resident, and a long chat would end up entirely in
    /// memory - the thing the paged log exists to avoid. The tail is what a
    /// reader returns to, so the oldest rows go first; they are still in the log
    /// and come back by paging up again.
    pub(crate) fn trim_front(&mut self, id: &str, keep: usize) -> usize {
        let Some(at) = self.sessions.iter().position(|session| session.id == id) else {
            return 0;
        };
        let session = &mut self.sessions[at];
        if session.messages.len() <= keep {
            return 0;
        }
        let drop = session.messages.len() - keep;
        session.messages.drain(..drop);
        // The window's start moves forward; the paging floor does not, because
        // the reader has still read every row above it.
        session.loaded_from += drop;
        drop
    }

    /// A trajectory page waiting to be read off the log.
    ///
    /// The trace tab reads its tool rows through this, so a long run does not
    /// have to be resident to be scrolled and the read can run on the
    /// background executor.
    pub(crate) fn trace_read(&self, id: &str, page: usize) -> Option<TraceRead> {
        let session = self.sessions.iter().find(|session| session.id == id)?;
        let dir = History::dir_for(self.dir_for(session), id);
        let history = self.history_for(id)?;
        if page >= history.trajectory_pages() {
            return None;
        }
        Some(TraceRead {
            session: id.to_string(),
            page,
            reader: history.reader(),
            dir,
        })
    }

    /// A window of older chat rows waiting to be read off the log.
    ///
    /// The floor is where paging has reached, which is NOT where the in-memory
    /// window starts once old rows have been dropped: reading from the window's
    /// start would re-fetch the tail the reader already passed.
    pub(crate) fn older_read(&self, id: &str, len: usize) -> Option<OlderRead> {
        let session = self.sessions.iter().find(|session| session.id == id)?;
        // The floor is where paging has REACHED - the lowest row ever brought
        // in - not the in-memory window's start: trim_front drops rows the
        // reader already passed, and flooring at the window's start would
        // re-fetch that passed region forever (the loop the paging test pins).
        let floor = session.paged_from.unwrap_or(session.loaded_from);
        if floor == 0 {
            return None;
        }
        let from = floor.saturating_sub(len);
        let dir = History::dir_for(self.dir_for(session), id);
        let history = self.history_for(id)?;
        Some(OlderRead {
            session: id.to_string(),
            from,
            floor,
            reader: history.reader(),
            dir,
        })
    }

    /// Put one decoded window above the loaded chat.
    ///
    /// A page that arrives after a newer one has already been applied is stale
    /// and dropped: applying it would duplicate rows the reader already sees.
    pub(crate) fn apply_older(&mut self, page: OlderPage) -> usize {
        let Some(at) = self.sessions.iter().position(|s| s.id == page.session) else {
            return 0;
        };
        let session = &mut self.sessions[at];
        // Stale against the paging floor, not the window's start: a trimmed
        // window starts lower than the rows this page would duplicate.
        let floor = session.paged_from.unwrap_or(session.loaded_from);
        if page.rows.is_empty() || page.from >= floor {
            return 0;
        }
        let added = page.rows.len();
        let mut slots = page.rows;
        slots.append(&mut session.messages);
        session.messages = slots;
        session.paged_from = Some(page.from);
        session.loaded_from = page.from;
        added
    }

    /// The directory a session's history lives in.
    pub(crate) fn history_dir(&self, id: &str) -> Option<PathBuf> {
        let session = self.sessions.iter().find(|session| session.id == id)?;
        Some(History::dir_for(self.dir_for(session), id))
    }

    /// Whether opening this history found a log worth rewriting once.
    pub(crate) fn history_needs_compaction(&self, id: &str) -> bool {
        let Some(path) = self.history_dir(id) else {
            return false;
        };
        self.history_at(path).needs_compaction()
    }

    /// Drop a cached history so the next read opens the current files.
    ///
    /// A compaction swaps the directory, so the cached index points at files
    /// that no longer exist; forgetting it is what makes the next page read
    /// see the sealed log.
    pub(crate) fn forget_history(&mut self, dir: &Path) {
        self.histories.borrow_mut().remove(dir);
    }

    /// Load the tail window of a session's log into memory.
    ///
    /// Startup reads no history. Selecting a row calls this once: it reads only
    /// the last `WINDOW` rows, and a row that already carries a window is left
    /// alone. A restore re-attaches a document to a log that never moved.
    /// Release an inactive session's cached History - its segment handles and
    /// decoded page cache - without touching the transcript window. The window
    /// is already bounded (see `trim_front`); the page cache is what grows with
    /// deep scrolling, and dropping it for a chat the reader left keeps that
    /// memory from accumulating across every visited session. The log still
    /// holds every row: paging that session again reopens the History cheaply.
    pub(crate) fn evict_history_cache(&mut self, id: &str) -> bool {
        let Some(at) = self.sessions.iter().position(|session| session.id == id) else {
            return false;
        };
        let dir = crate::history::History::dir_for(self.dir_for(&self.sessions[at]), id);
        self.histories.borrow_mut().remove(&dir).is_some()
    }

    /// How many session Histories (segment handles + page caches) are cached.
    #[cfg(test)]
    pub(crate) fn cached_histories(&self) -> usize {
        self.histories.borrow().len()
    }

    pub(crate) fn rehydrate(&mut self, id: &str) -> bool {
        let Some(at) = self.sessions.iter().position(|session| session.id == id) else {
            return false;
        };
        self.ensure_document(id);
        // A row that already carries a transcript window is not re-read. A
        // brand-new empty session has no history either; the read finds zero.
        if self.sessions[at].paged_from.is_some() || !self.sessions[at].messages.is_empty() {
            return false;
        }
        let (from, window) = {
            let Some(history) = self.history_for(id) else {
                return false;
            };
            let rows = history.len();
            if rows == 0 {
                return false;
            }
            let from = rows.saturating_sub(WINDOW);
            // The tail window could not be read. The row stays unhydrated, but
            // the failure is recorded instead of reading as an empty
            // conversation; the log's own error text names the segment and the
            // row group.
            let window = match history.window(from, rows - from) {
                Ok(window) => window,
                Err(error) => {
                    log::warn!(
                        "session {id}: the transcript window from row {from} of {rows} could not be read: {error}"
                    );
                    return false;
                }
            };
            (from, window)
        };
        let session = &mut self.sessions[at];
        // `compacted` is a GLOBAL index, so a window that starts at a different
        // global row needs no adjustment here; request_start maps it.
        session.loaded_from = from;
        session.paged_from = Some(from);
        session.messages = window;
        true
    }

    /// Copy one session's log to another: a fork has to carry the pages.
    ///
    /// The in-memory window is only part of a transcript, so a fork that saved
    /// the window would be a shortened copy of the chat it forked from. The
    /// segments are copied whole, which is a file copy per save the source had.
    pub(crate) fn copy_history(&self, from_id: &str, to_id: &str) {
        let Some(source) = self.sessions.iter().find(|session| session.id == from_id) else {
            return;
        };
        let from = History::dir_for(self.dir_for(source), from_id);
        let target_root = self
            .roots
            .get(&source.workspace)
            .map_or(self.quarantine.clone(), PathBuf::clone);
        let to = crate::history::History::dir_for(&target_root, to_id);
        let Ok(entries) = fs::read_dir(&from) else {
            return;
        };
        let _ = fs::create_dir_all(&to);
        for entry in entries.flatten() {
            // The journal describes a turn still running in the SOURCE
            // session. Copying it would make the fork's first launch append that
            // turn a second time.
            let name = entry.file_name();
            if name == INFLIGHT || name == INFLIGHT_LOCK || name == INFLIGHT_TMP {
                continue;
            }
            let _ = fs::copy(entry.path(), to.join(name));
        }
        self.histories.borrow_mut().remove(&to);
    }

    /// Append a page of older messages above the loaded window, synchronously.
    ///
    /// The app reads through Store::older_read on the background executor; this
    /// is the seam the paging tests drive, so the row placement and the floor
    /// arithmetic are tested without a UI.
    #[cfg(test)]
    pub(crate) fn load_older(&mut self, id: &str, len: usize) -> usize {
        let Some(read) = self.older_read(id, len) else {
            return 0;
        };
        self.apply_older(read.read())
    }

    /// Sessions belonging to a workspace, newest first.
    /// Child sessions, oldest first, for restoring the subagent menu.
    pub(crate) fn subagent_sessions(&self) -> Vec<String> {
        self.sessions
            .iter()
            .filter(|session| session.subagent.is_some())
            .map(|session| session.id.clone())
            .collect()
    }

    pub(crate) fn in_workspace(&self, workspace: &str) -> Vec<&Session> {
        let mut rows: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.workspace == workspace && s.archived_at.is_none() && s.subagent.is_none())
            .collect();
        rows.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        rows
    }

    /// Archived sessions, newest first.
    pub(crate) fn archived(&self) -> Vec<&Session> {
        let mut rows: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.archived_at.is_some() && s.subagent.is_none())
            .collect();
        rows.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        rows
    }
}

/// How the session tree is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SessionSort {
    /// Newest activity first.
    #[default]
    Updated,
    /// Newest creation first.
    Created,
    /// Title, A to Z, case-insensitively.
    Name,
}

impl SessionSort {
    /// Stable name used to persist the choice, distinct from the locale key.
    pub(crate) fn persisted(&self) -> &'static str {
        match self {
            SessionSort::Updated => "updated",
            SessionSort::Created => "created",
            SessionSort::Name => "name",
        }
    }

    /// Read a persisted name, falling back to the default order.
    pub(crate) fn from_persisted(key: &str) -> Self {
        match key {
            "created" => SessionSort::Created,
            "name" => SessionSort::Name,
            _ => SessionSort::Updated,
        }
    }

    /// The next view option in the cycle.
    /// The locale key that names this view option.
    pub(crate) fn key(self) -> &'static str {
        match self {
            SessionSort::Updated => "side.sortUpdated",
            SessionSort::Created => "side.sortCreated",
            SessionSort::Name => "side.sortName",
        }
    }
}

/// Reorder session rows by the chosen view option; pinned rows stay first.
pub(crate) fn sort_rows(rows: &mut Vec<&Session>, sort: SessionSort) {
    rows.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned).then_with(|| match sort {
            SessionSort::Updated => b.updated_at.cmp(&a.updated_at),
            SessionSort::Created => b.created_at.cmp(&a.created_at),
            SessionSort::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::{sort_rows, SessionSort};
    use crate::model::Session;

    /// A session carrying only what the view options read.
    fn row(title: &str, created: u64, updated: u64) -> Session {
        Session {
            id: title.to_string(),
            title: title.to_string(),
            created_at: created,
            updated_at: updated,
            workspace: "w".to_string(),
            provider: None,
            model: None,
            agent_preset: None,
            subagent: None,
            messages: Vec::new(),
            last_injected_persona: None,
            plan: Vec::new(),
            goal: None,
            plan_mode: false,
            compacted: 0,
            feedback: None,
            pinned: false,
            archived_at: None,
            archived: Vec::new(),
            compactions: Vec::new(),
            compact_command: None,
            loaded_from: 0,
            paged_from: None,
            loaded: true,
        }
    }

    fn titles(rows: &[&Session]) -> Vec<String> {
        rows.iter().map(|session| session.title.clone()).collect()
    }

    #[test]
    fn the_view_options_order_sessions_three_ways() {
        let (beta, alpha, gamma) = (
            row("beta", 10, 30),
            row("alpha", 30, 20),
            row("gamma", 20, 10),
        );
        let mut rows = vec![&beta, &alpha, &gamma];
        sort_rows(&mut rows, SessionSort::Updated);
        assert_eq!(titles(&rows), ["beta", "alpha", "gamma"]);
        sort_rows(&mut rows, SessionSort::Created);
        assert_eq!(titles(&rows), ["alpha", "gamma", "beta"]);
        sort_rows(&mut rows, SessionSort::Name);
        assert_eq!(titles(&rows), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn pinned_sessions_stay_on_top_under_every_view_option() {
        let (beta, alpha, pinned) = (
            row("beta", 10, 30),
            row("alpha", 30, 20),
            Session {
                pinned: true,
                ..row("gamma", 20, 10)
            },
        );
        let mut rows = vec![&beta, &alpha, &pinned];
        sort_rows(&mut rows, SessionSort::Name);
        assert_eq!(titles(&rows), ["gamma", "alpha", "beta"]);
    }

    /// A scratch directory no other test shares.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-settings-tests")
            .join(format!("store-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// One registered workspace, as an (id, root) pair.
    fn registry(root: &std::path::Path) -> Vec<(String, String)> {
        vec![("w1".to_string(), root.to_string_lossy().to_string())]
    }

    #[test]
    fn a_session_is_written_beside_the_workspace_it_belongs_to() {
        let base = scratch("roots");
        let root = base.join("proj");
        let store = super::Store::open_over(&registry(&root), base.join("quarantine"));

        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        store.save(&mine).expect("saves");
        assert!(
            super::sessions_dir(&root).join("a.json").exists(),
            "a session belongs beside its workspace, so a checkout carries its own \
             history instead of sharing one list across machines"
        );

        let mut orphan = row("b", 1, 1);
        orphan.workspace = "gone".to_string();
        store.save(&orphan).expect("saves");
        assert!(
            base.join("quarantine").join("b.json").exists(),
            "a session whose workspace is unknown goes to the harness directory"
        );
    }

    #[test]
    fn a_store_reopened_over_a_workspace_finds_its_sessions() {
        let base = scratch("reopen");
        let root = base.join("proj");
        let quarantine = base.join("quarantine");
        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        super::Store::open_over(&registry(&root), quarantine.clone())
            .save(&mine)
            .expect("saves");

        let reopened = super::Store::open_over(&registry(&root), quarantine);
        assert!(
            reopened.session("a").is_some(),
            "a session is found where it was written"
        );
    }

    #[test]
    fn a_workspace_rooted_at_the_quarantine_is_read_once() {
        let base = scratch("collide");
        // A workspace whose root IS the harness directory points its session
        // directory at the quarantine, so the same files are reached twice.
        let registry = [("w1".to_string(), base.to_string_lossy().to_string())];
        let store = super::Store::open_over(&registry, super::sessions_dir(&base));
        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        store.save(&mine).expect("saves");

        let reopened = super::Store::open_over(&registry, super::sessions_dir(&base));
        assert_eq!(
            reopened.sessions.len(),
            1,
            "one file must yield one session, not one per directory that reaches it"
        );
    }
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    /// A save of a session that was NOT hydrated must never truncate its log.
    ///
    /// Startup reads metadata only, so the in-memory window is empty while the
    /// log already holds the conversation. Writing a turn onto that row makes
    /// the index arithmetic look like a truncation (the log is longer than the
    /// window), and the "rewrite" arm then replaced every row with the two new
    /// ones.
    #[test]
    fn a_save_of_an_unhydrated_session_keeps_the_log() {
        let base = std::env::temp_dir().join(format!("bsh-unhydrated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("the scratch root");
        let mut session = crate::model::Session::new("w1", "long chat");
        session.id = "unhydrated".to_string();
        session.workspace = "w1".to_string();
        for index in 0..40 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("row {index}")));
        }
        {
            let store = Store::open_at(base.clone());
            store.save(&session).expect("the first save");
        }
        let mut store = Store::open_at(base.clone());
        {
            // The turn opens on a row whose window was never hydrated: the
            // document is loaded, the transcript is not.
            let row = store.session_mut("unhydrated").expect("the row");
            row.messages
                .push(crate::model::ChatMessage::user("new question"));
            row.messages.push(crate::model::ChatMessage::assistant());
        }
        let fresh = store.session("unhydrated").expect("the row").clone();
        store.save(&fresh).expect("the turn's save");

        let log =
            crate::history::History::open(crate::history::History::dir_for(&base, "unhydrated"));
        assert!(
            log.len() >= 40,
            "the old rows are still in the log, was {}",
            log.len()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A save after the assistant placeholder is already in the log must
    /// rewrite the tail, not skip it: the row COUNT is the same but the content
    /// is not, and skipping is how a turn's streamed text and reasoning were
    /// silently dropped - a halted turn showed it plainly.
    #[test]
    fn a_save_after_a_placeholder_rewrites_the_tail() {
        let base = std::env::temp_dir().join(format!("bsh-placeholder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("the scratch root");
        let mut session = crate::model::Session::new("w1", "halted");
        session.id = "halted".to_string();
        session.workspace = "w1".to_string();
        session
            .messages
            .push(crate::model::ChatMessage::user("ask"));
        session
            .messages
            .push(crate::model::ChatMessage::assistant());

        let store = Store::open_at(base.clone());
        store.save(&session).expect("the placeholder is written");
        {
            let before =
                crate::history::History::open(crate::history::History::dir_for(&base, "halted"));
            assert_eq!(before.len(), 2, "the placeholder is in the log");
            assert_eq!(before.page(0).expect("a page")[1].text, "");
        }

        session.messages.last_mut().expect("the reply").text = "the answer".to_string();
        session.messages.last_mut().expect("the reply").reasoning = "weigh it".to_string();
        store.save(&session).expect("the turn's save");

        let after =
            crate::history::History::open(crate::history::History::dir_for(&base, "halted"));
        let rows = after.page(0).expect("a page");
        assert_eq!(rows.len(), 2, "the same rows, now with content");
        assert_eq!(rows[1].text, "the answer", "the streamed text is written");
        assert_eq!(rows[1].reasoning, "weigh it", "and the reasoning with it");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A session whose workspace nobody registers is re-homed, not hidden.
    ///
    /// It loads, then the sidebar filter drops it from every row, so the history
    /// is on disk and unreachable. Re-homing is the only thing that can happen
    /// to it: there is no other workspace to attribute it to.
    #[test]
    fn an_orphaned_session_is_rehomed_to_the_workspace_in_use() {
        let base = std::env::temp_dir().join(format!("bsh-rehome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("the workspace root");
        let quarantine = base.join("quarantine");
        std::fs::create_dir_all(&quarantine).expect("the quarantine");

        let mut orphan = crate::model::Session::new("dead-workspace-id", "New chat");
        orphan.id = "orphan-1".to_string();
        let text = serde_json::to_string_pretty(&orphan).expect("serialize");
        std::fs::write(quarantine.join("orphan-1.json"), text).expect("the orphan file");

        let mut store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            quarantine.clone(),
        );
        assert_eq!(store.sessions.len(), 1, "the orphan is loaded");
        assert!(
            store.in_workspace("live").is_empty(),
            "and invisible: that is the defect this pins"
        );

        let moved = store.rehome_orphans("live");
        assert_eq!(moved, 1, "one session re-homed");
        assert_eq!(
            store.in_workspace("live").len(),
            1,
            "the history is reachable now"
        );
        assert!(
            !quarantine.join("orphan-1.json").exists(),
            "the quarantined copy is gone, so the id is not on disk twice"
        );
        let beside = crate::store::sessions_dir(&root).join("orphan-1.json");
        assert!(beside.exists(), "and it lives beside its workspace now");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A configured credential never reaches a session file.
    ///
    /// A transcript can quote the configuration - a tool that printed the
    /// settings file puts the key in the conversation - and a session document
    /// stores provider references, never a credential. Only the exact configured
    /// value is replaced, so ordinary text is not mangled.
    #[test]
    fn a_configured_credential_is_redacted_from_the_session_file() {
        let base = std::env::temp_dir().join(format!("bsh-redact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("the workspace root");

        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        )
        .with_secrets(vec!["sk-live-secret-123".to_string()]);

        let mut session = crate::model::Session::new("live", "New chat");
        session.id = "quotes-the-config".to_string();
        session.messages.push(crate::model::ChatMessage::user(
            "api_key = \"sk-live-secret-123\"\nbase_url = \"http://127.0.0.1:11436/v1\"",
        ));
        store.save(&session).expect("the save");

        let path = crate::store::sessions_dir(&root).join("quotes-the-config.json");
        let written = std::fs::read_to_string(&path).expect("the session file");
        assert!(
            !written.contains("sk-live-secret-123"),
            "the credential is not in the document: {written}"
        );

        // The transcript lives in the Parquet log now, so that is where the text
        // has to be scrubbed - and the log is the one place it could leak.
        let history = crate::history::History::open(crate::history::History::dir_for(
            &crate::store::sessions_dir(&root),
            "quotes-the-config",
        ));
        let rows = history.page(0).expect("a page of the log");
        assert_eq!(rows.len(), 1, "the message is in the log");
        assert!(
            !rows[0].text.contains("sk-live-secret-123"),
            "the credential is not in the log: {}",
            rows[0].text
        );
        assert!(
            rows[0].text.contains("***"),
            "the field survives, its value does not: {}",
            rows[0].text
        );
        assert!(
            rows[0].text.contains("http://127.0.0.1:11436/v1"),
            "and the rest of the quoted document is untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The paged history, end to end: a save leaves the transcript in the Parquet
    /// log, the document carries only metadata, a fresh store loads the tail window
    /// instead of the whole transcript, and scrolling up pages one more window in.
    #[test]
    fn a_session_pages_its_transcript_from_the_parquet_log() {
        use crate::model::ChatMessage;
        use crate::store::{Store, WINDOW};

        let base = std::env::temp_dir().join(format!("bsh-paged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");
        let sessions = crate::store::sessions_dir(&root);

        let mut session = crate::model::Session::new("live", "long chat");
        session.id = "paged".to_string();
        session.workspace = "live".to_string();
        for index in 0..600 {
            session
                .messages
                .push(ChatMessage::user(format!("message {index}")));
        }

        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        );
        store.save(&session).expect("the save");

        // The document is metadata now; the transcript is in the log.
        let text = std::fs::read_to_string(sessions.join("paged.json")).expect("document");
        assert!(
            !text.contains("message 599"),
            "the document does not embed the transcript"
        );
        let log =
            crate::history::History::open(crate::history::History::dir_for(&sessions, "paged"));
        assert_eq!(log.len(), 600, "the log holds every row");

        // A fresh store lists metadata; selecting the row opens only a window.
        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        );
        opened.rehydrate("paged");
        let loaded = opened.session("paged").expect("the session");
        assert_eq!(loaded.messages.len(), WINDOW, "only a window is in memory");
        assert_eq!(loaded.loaded_from, 600 - WINDOW, "and it is the tail");
        assert_eq!(
            loaded.messages.last().map(|m| m.text.as_str()),
            Some("message 599")
        );

        // Scrolling to the top pages the window before it in.
        let added = opened.load_older("paged", WINDOW);
        let after = opened.session("paged").expect("the session");
        assert_eq!(added, WINDOW, "a window arrives");
        assert_eq!(after.messages.len(), WINDOW * 2, "above the loaded window");
        assert_eq!(after.loaded_from, 600 - WINDOW * 2, "the window moved back");
        assert_eq!(
            after.messages[WINDOW].text,
            format!("message {}", 600 - WINDOW),
            "the old first row now sits after the paged-in ones"
        );

        // And pages until the start, then stops asking.
        while opened.load_older("paged", WINDOW) > 0 {}
        let all = opened.session("paged").expect("the session");
        assert_eq!(all.loaded_from, 0);
        assert_eq!(
            all.messages.len(),
            600,
            "the whole transcript is back in memory"
        );
        assert_eq!(
            opened.load_older("paged", WINDOW),
            0,
            "and nothing is left to page"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A document written before the Parquet log carries its transcript inline.
    /// Startup keeps it out of memory, but selecting the row restores it and the
    /// first save appends it to the log instead of losing it.
    #[test]
    fn an_inline_transcript_migrates_to_the_log() {
        use crate::store::{Store, WINDOW};
        use std::fs;

        let base = std::env::temp_dir().join(format!("bsh-inline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        let sessions = crate::store::sessions_dir(&root);
        std::fs::create_dir_all(&sessions).expect("sessions");
        let mut session = crate::model::Session::new("live", "inline");
        session.id = "inline".to_string();
        for index in 0..(WINDOW + 4) {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("old {index}")));
        }
        fs::write(
            sessions.join("inline.json"),
            serde_json::to_string(&session).expect("json"),
        )
        .expect("write the old document");

        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        assert!(
            store.session("inline").expect("row").messages.is_empty(),
            "startup does not read the inline transcript"
        );
        drop(store);

        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.rehydrate("inline");
        let loaded = opened.session("inline").expect("row");
        assert_eq!(loaded.messages.len(), WINDOW + 4);
        assert_eq!(loaded.messages[0].text, "old 0");
        let session = loaded.clone();
        opened.save(&session).expect("migrate");
        let log =
            crate::history::History::open(crate::history::History::dir_for(&sessions, "inline"));
        assert_eq!(log.len(), WINDOW + 4, "the first save wrote the log");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Startup streams only metadata: plan, goal and archived are not in memory
    /// until the selected row is fully loaded.
    #[test]
    fn startup_does_not_materialize_the_full_document() {
        use crate::store::{Store, WINDOW};

        let base = std::env::temp_dir().join(format!("bsh-meta-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "metadata");
        session.id = "metadata".to_string();
        for index in 0..(WINDOW * 2) {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("message {index}")));
        }
        session.plan = vec![crate::model::PlanItem {
            text: "keep this step".to_string(),
            status: crate::model::PlanStatus::Pending,
        }];
        session.goal = crate::model::Goal::new("keep this goal", None);
        // The session's own model choice rides the document as metadata, so a
        // metadata-only save must preserve it exactly like plan and goal.
        session.provider = Some("p".to_string());
        session.model = Some("beta".to_string());
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("save");

        let opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        let meta = opened.session("metadata").expect("the row");
        assert!(!meta.loaded, "startup read only metadata");
        assert!(meta.plan.is_empty(), "the plan was not parsed");
        assert!(meta.goal.is_none(), "the goal was not parsed");
        assert!(
            meta.messages.is_empty(),
            "no history page is read for a list-only row"
        );
        assert_eq!(meta.title, "metadata");
        assert_eq!(
            meta.provider.as_deref(),
            Some("p"),
            "the session choice is metadata, so startup reads it"
        );
        assert_eq!(meta.model.as_deref(), Some("beta"));

        // A metadata-only save (a rename here) must not rewrite the plan, goal
        // or archived table as empty, and must not clobber the session choice.
        let mut renamed = opened.session("metadata").expect("the row").clone();
        renamed.title = "renamed".to_string();
        opened.save(&renamed).expect("rename");
        let mut reopened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        assert_eq!(
            reopened.session("metadata").expect("row").title,
            "renamed",
            "the metadata edit landed"
        );
        reopened.rehydrate("metadata");
        let full = reopened.session("metadata").expect("the loaded row");
        assert!(full.loaded, "the selected row is fully read");
        assert_eq!(full.plan[0].text, "keep this step");
        assert_eq!(
            full.goal.as_ref().map(|goal| goal.objective.as_str()),
            Some("keep this goal")
        );
        assert_eq!(
            full.provider.as_deref(),
            Some("p"),
            "a metadata-only save does not clobber the session choice"
        );
        assert_eq!(full.model.as_deref(), Some("beta"));
        assert_eq!(full.messages.len(), WINDOW, "and the window opened");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The real save shape: one small segment per turn. Paging must read the
    /// correct rows even though almost every segment is shorter than PAGE.
    #[test]
    fn paging_survives_one_segment_per_turn() {
        use crate::store::{Store, WINDOW};

        let base = std::env::temp_dir().join(format!("bsh-turn-segments-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "per turn");
        session.id = "per-turn".to_string();
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        for turn in 0..200 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("turn {turn} user")));
            session
                .messages
                .push(crate::model::ChatMessage::assistant());
            session.messages.last_mut().expect("assistant").text = format!("turn {turn} answer");
            store.save(&session).expect("save the turn");
        }

        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.rehydrate("per-turn");
        let loaded = opened.session("per-turn").expect("session");
        assert_eq!(loaded.messages.len(), WINDOW, "the tail window");
        assert_eq!(
            loaded.messages.last().map(|m| m.text.as_str()),
            Some("turn 199 answer"),
            "the tail is the newest turn"
        );
        assert_eq!(loaded.loaded_from, 400 - WINDOW);

        // Page all the way up. Every window must be the right rows even though
        // each save left a two-row segment.
        while opened.load_older("per-turn", WINDOW) > 0 {}
        let all = opened.session("per-turn").expect("session");
        assert_eq!(all.loaded_from, 0);
        assert_eq!(all.messages.len(), 400);
        assert_eq!(all.messages[0].text, "turn 0 user");
        assert_eq!(all.messages[1].text, "turn 0 answer");
        assert_eq!(all.messages[WINDOW].text, "turn 128 user");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Evicting an inactive session's History releases its page cache but keeps
    /// the bounded window; paging it again reopens the log, so nothing is lost.
    #[test]
    fn evicting_a_history_cache_keeps_the_window_and_reopens() {
        use crate::model::ChatMessage;
        use crate::store::{Store, WINDOW};

        let base = std::env::temp_dir().join(format!("bsh-evict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "long chat");
        session.id = "evict".to_string();
        session.workspace = "live".to_string();
        for index in 0..600 {
            session
                .messages
                .push(ChatMessage::user(format!("message {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        );
        store.save(&session).expect("the save");

        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        );
        opened.rehydrate("evict");
        let window = opened.session("evict").unwrap().messages.len();
        assert_eq!(window, WINDOW);
        assert!(opened.cached_histories() >= 1, "rehydrate caches a History");

        // Eviction releases the page cache but keeps the window.
        assert!(opened.evict_history_cache("evict"));
        assert_eq!(opened.cached_histories(), 0, "the page cache is released");
        assert_eq!(
            opened.session("evict").unwrap().messages.len(),
            window,
            "the bounded window stays"
        );
        // Evicting an absent cache is a no-op.
        assert!(!opened.evict_history_cache("evict"));

        // Paging again reopens the History from the log.
        let added = opened.load_older("evict", WINDOW);
        assert_eq!(added, WINDOW, "the log still serves older pages");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A new turn after a restored tail window appends one segment. The old
    /// save path rewrote every page in that case, which is what would make a
    /// long chat quadratic to keep.
    #[test]
    fn a_save_after_a_tail_load_appends_one_segment() {
        use crate::store::{Store, WINDOW};

        let base = std::env::temp_dir().join(format!("bsh-append-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "append");
        session.id = "append".to_string();
        for index in 0..600 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("message {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("initial save");

        // A fresh open loads only the tail; adding a turn must append the delta,
        // not rebuild the log from loaded_from.
        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.rehydrate("append");
        let mut restored = opened.session("append").expect("session").clone();
        assert_eq!(restored.loaded_from, 600 - WINDOW, "the tail window");
        restored
            .messages
            .push(crate::model::ChatMessage::user("new turn"));
        opened.save(&restored).expect("save after the new turn");

        let dir = crate::store::sessions_dir(&root);
        let history_dir = crate::history::History::dir_for(&dir, "append");
        let segments = std::fs::read_dir(&history_dir)
            .expect("history dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("msg-"))
            .count();
        assert_eq!(
            segments, 2,
            "one more segment, not a rewrite of the whole log"
        );
        let log = crate::history::History::open(history_dir);
        assert_eq!(log.len(), 601);
        assert_eq!(log.window(600, 1).expect("the new row")[0].text, "new turn");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A truncation keeps the pages ABOVE the in-memory window: rebuilding the
    /// log from the window alone would silently drop them.
    #[test]
    fn a_truncation_keeps_the_pages_above_the_window() {
        let base = std::env::temp_dir().join(format!("bsh-truncate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "long chat");
        session.id = "cut".to_string();
        for index in 0..400 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("m {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("save");

        // Re-open: only the tail window is in memory.
        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.rehydrate("cut");
        let window = opened.session("cut").expect("session").messages.len();
        assert_eq!(window, WINDOW);
        // Cut the window in half and save: the log must keep rows 0..360.
        {
            let session = opened.session_mut("cut").expect("session");
            session.messages.truncate(WINDOW / 2);
        }
        let session = opened.session("cut").expect("session").clone();
        opened.save(&session).expect("save after the cut");

        let log = crate::history::History::open(crate::history::History::dir_for(
            &crate::store::sessions_dir(&root),
            "cut",
        ));
        assert_eq!(
            log.len(),
            400 - WINDOW + WINDOW / 2,
            "the pages above the window survive the truncation"
        );
        let first = log.page(0).expect("page");
        assert_eq!(first[0].text, "m 0", "and they are still the first rows");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A fork carries the whole log, not just the window in memory.
    #[test]
    fn a_fork_copies_the_log() {
        let base = std::env::temp_dir().join(format!("bsh-fork-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "chat");
        session.id = "source".to_string();
        for index in 0..300 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("m {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("save");

        let opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.copy_history("source", "copy");
        let dir = crate::store::sessions_dir(&root);
        let copy = crate::history::History::open(crate::history::History::dir_for(&dir, "copy"));
        assert_eq!(copy.len(), 300, "the fork can read every page");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The bin holds a chat entire: the park moves its Parquet pages with the
    /// document, a restore puts them back, and a purge leaves nothing behind.
    #[test]
    fn the_bin_carries_the_transcript_pages() {
        let base = std::env::temp_dir().join(format!("bsh-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");
        let dir = crate::store::sessions_dir(&root);

        let mut session = crate::model::Session::new("live", "parked chat");
        session.id = "parked".to_string();
        for index in 0..400 {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("m {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("save");
        let log = crate::history::History::dir_for(&dir, "parked");
        assert_eq!(crate::history::History::open(log.clone()).len(), 400);

        // Park it, the way a delete does: the pages move into the bin.
        let envelope = crate::trash::Envelope {
            kind: "session".to_string(),
            deleted_at: 1,
            workspace: "live".to_string(),
            workspace_name: "bos".to_string(),
            name: session.title.clone(),
            log: "log-parked".to_string(),
            payload: serde_json::to_value(&session).expect("payload"),
        };
        let bin = base.join("bin");
        crate::trash::park_log(&bin, &log, &envelope).expect("park");
        assert!(!log.exists(), "the pages left the workspace");
        let parked = crate::trash::log_dir(&bin, &envelope);
        assert_eq!(
            crate::history::History::open(parked.clone()).len(),
            400,
            "and the bin can read them"
        );

        // Purge: the parked pages go with the envelope.
        let _ = std::fs::remove_dir_all(crate::trash::log_dir(&bin, &envelope));
        assert!(!parked.exists(), "a purge leaves no pages behind");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Trimming bounds what a long scroll holds, and the log keeps everything.
    #[test]
    fn trimming_bounds_the_window() {
        use crate::store::WINDOW;

        let base = std::env::temp_dir().join(format!("bsh-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("workspace");

        let mut session = crate::model::Session::new("live", "long");
        session.id = "trim".to_string();
        for index in 0..(WINDOW * 12) {
            session
                .messages
                .push(crate::model::ChatMessage::user(format!("m {index}")));
        }
        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        store.save(&session).expect("save");

        let mut opened = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("q"),
        );
        opened.rehydrate("trim");
        // Page the whole chat up, trimming as the shell does. The floor falls
        // every round, so the loop reaches the top of the CHAT rather than the
        // top of the window in memory.
        let mut rounds = 0;
        while opened.load_older("trim", WINDOW) > 0 {
            opened.trim_front("trim", WINDOW * 4);
            rounds += 1;
            assert!(rounds < 64, "paging terminates");
        }
        let loaded = opened.session("trim").expect("session");
        assert_eq!(
            loaded.messages.len(),
            WINDOW * 4,
            "an exhaustive scroll still holds four windows"
        );
        assert_eq!(
            loaded.paged_from,
            Some(0),
            "and the reader reached the first row"
        );
        let dir = crate::store::sessions_dir(&root);
        let log = crate::history::History::open(crate::history::History::dir_for(&dir, "trim"));
        assert_eq!(log.len(), WINDOW * 12, "while the log keeps every row");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Two roots, one id: the store reads one row, and the delete must remove
    /// the document under BOTH roots. Removing only the copy the build resolved
    /// left the stray behind, and the next launch read it back - a deleted chat
    /// reappeared after a restart.
    #[test]
    fn deleting_a_session_sweeps_a_stray_copy_under_another_root() {
        let base = std::env::temp_dir().join(format!("bsh-stray-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("the scratch root");
        let first = base.join("one");
        let second = base.join("two");
        let registry = vec![
            ("w1".to_string(), first.to_string_lossy().to_string()),
            ("w2".to_string(), second.to_string_lossy().to_string()),
        ];
        let store = super::Store::open_over(&registry, base.join("quarantine"));
        let mut session = crate::model::Session::new("w1", "same-id");
        session.id = "same-id".to_string();
        store.save(&session).expect("saves");

        // A leftover under the other root, as an earlier move between
        // workspaces used to leave behind.
        std::fs::create_dir_all(super::sessions_dir(&second)).expect("the other root");
        std::fs::copy(
            super::sessions_dir(&first).join("same-id.json"),
            super::sessions_dir(&second).join("same-id.json"),
        )
        .expect("the stray copy");

        let mut store = super::Store::open_over(&registry, base.join("quarantine"));
        assert_eq!(store.sessions.len(), 1, "one row, deduplicated on read");
        assert!(store.remove("same-id"), "the delete happens");
        drop(store);

        let reopened = super::Store::open_over(&registry, base.join("quarantine"));
        assert!(
            reopened.sessions.is_empty(),
            "the deleted session does not come back: {:?}",
            reopened.sessions.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        assert!(
            !super::sessions_dir(&second).join("same-id.json").exists(),
            "the stray document is gone too"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A save heals a stray that already exists, so the next launch cannot
    /// resurrect a session whose deletion happened before this fix.
    #[test]
    fn a_save_drops_a_stray_copy_under_another_root() {
        let base = std::env::temp_dir().join(format!("bsh-stray-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("the scratch root");
        let first = base.join("one");
        let second = base.join("two");
        let registry = vec![
            ("w1".to_string(), first.to_string_lossy().to_string()),
            ("w2".to_string(), second.to_string_lossy().to_string()),
        ];
        let store = super::Store::open_over(&registry, base.join("quarantine"));
        let mut session = crate::model::Session::new("w1", "same-id");
        session.id = "same-id".to_string();
        store.save(&session).expect("saves");
        std::fs::create_dir_all(super::sessions_dir(&second)).expect("the other root");
        std::fs::copy(
            super::sessions_dir(&first).join("same-id.json"),
            super::sessions_dir(&second).join("same-id.json"),
        )
        .expect("the stray copy");

        store.save(&session).expect("saves again");
        assert!(
            !super::sessions_dir(&second).join("same-id.json").exists(),
            "the owning root keeps the document and every other root loses it"
        );
        assert!(super::sessions_dir(&first).join("same-id.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A session document is replaced atomically: a half-written temp is
    /// ignored, so a kill during the write cannot delete the whole session.
    #[test]
    fn a_session_document_is_replaced_atomically() {
        let base = journal_scratch("document-atomic");
        let mut store = super::Store::open_at(base.clone());
        let mut session = crate::model::Session::new("w", "atomic");
        session.id = "atomic".to_string();
        store.insert(session.clone());
        store.save(&session).expect("the document");
        let document = base.join("atomic.json");
        let temporary = document.with_extension("json.tmp");
        assert!(document.is_file(), "the document is installed");
        assert!(!temporary.exists(), "and no temporary is left behind");

        // A kill in the middle of the next write leaves only this.
        std::fs::write(&temporary, "{\"id\": \"ato").expect("the torn temporary");
        let reopened = super::Store::open_at(base.clone());
        assert!(
            reopened.session("atomic").is_some(),
            "the old document still lists the session"
        );
        reopened.save(&session).expect("the next save");
        assert!(
            !temporary.exists(),
            "the torn temporary is replaced by the next save"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A scratch directory for the journal tests, in target/.
    fn journal_scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-journal-tests")
            .join(format!("store-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// The prompt and the partial answer of a turn the process died in.
    fn crashed_turn(store: &mut super::Store, answer: &str) -> crate::model::Session {
        let mut session = crate::model::Session::new("w", "crashed");
        session.id = "crashed".to_string();
        store.insert(session.clone());
        store.save(&session).expect("the document");
        // The turn-start save commits the prompt by append.
        let mut prompt = crate::model::ChatMessage::user("what happened?");
        prompt.timing_mut().start = Some(1_000);
        session.messages.push(prompt);
        store.save(&session).expect("the prompt");
        // The reply row is the part the log cannot hold until it settles.
        let mut placeholder = crate::model::ChatMessage::assistant();
        placeholder.timing_mut().start = Some(1_000);
        placeholder.text = answer.to_string();
        session.messages.push(placeholder);
        store.checkpoint(&session, 1).expect("the journal");
        session
    }

    /// A kill mid-turn keeps the turn: the journal is folded into the log on
    /// the next launch, and the journal itself is gone.
    #[test]
    fn an_interrupted_turn_is_recovered_on_the_next_launch() {
        let base = journal_scratch("inflight-recover");
        let journal = {
            let mut store = super::Store::open_at(base.clone());
            let session = crashed_turn(&mut store, "half a reply");
            let dir = crate::history::History::dir_for(&base, &session.id);
            assert!(
                dir.join(super::INFLIGHT).is_file(),
                "the journal is written"
            );
            dir.join(super::INFLIGHT)
        };

        let mut reopened = super::Store::open_at(base.clone());
        assert!(!journal.exists(), "the journal is consumed");
        let id = "crashed".to_string();
        reopened.rehydrate(&id);
        let session = reopened.session(&id).expect("the session");
        assert_eq!(session.messages.len(), 2, "the prompt and its answer");
        assert_eq!(session.messages[0].text, "what happened?");
        assert_eq!(session.messages[1].text, "half a reply");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A second launch must not consume a journal a LIVE turn still owns.
    #[test]
    fn a_live_turns_journal_is_not_recovered_by_another_store() {
        let base = journal_scratch("inflight-live");
        let mut store = super::Store::open_at(base.clone());
        let session = crashed_turn(&mut store, "still streaming");
        let dir = crate::history::History::dir_for(&base, &session.id);

        let other = super::Store::open_at(base.clone());
        assert!(
            other.session("crashed").is_some(),
            "the second store still sees the session"
        );
        assert!(
            dir.join(super::INFLIGHT).is_file(),
            "the live turn's journal is left alone"
        );
        assert_eq!(
            crate::history::History::open(dir.clone()).len(),
            1,
            "and was not appended twice"
        );

        // The owner exits; the next launch is now the one that recovers it.
        drop(store);
        let mut reopened = super::Store::open_at(base.clone());
        reopened.rehydrate("crashed");
        assert_eq!(
            reopened
                .session("crashed")
                .expect("the session")
                .messages
                .len(),
            2,
            "the turn lands once its owner is gone"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A completed save commits the journal and removes it; a later launch
    /// must not append the same row a second time.
    #[test]
    fn a_clean_save_clears_the_journal_without_duplicating_rows() {
        let base = journal_scratch("inflight-clean");
        let session = {
            let mut store = super::Store::open_at(base.clone());
            let session = crashed_turn(&mut store, "the answer");
            store.save(&session).expect("the settled save");
            session
        };
        let dir = crate::history::History::dir_for(&base, &session.id);
        assert!(!dir.join(super::INFLIGHT).is_file(), "the journal is gone");
        assert_eq!(
            crate::history::History::open(dir).len(),
            2,
            "the rows were written exactly once"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A turn that produced nothing is marked cancelled, exactly as a stop
    /// records one, so the transcript does not show a silent empty reply.
    #[test]
    fn an_interrupted_empty_turn_is_marked_cancelled() {
        let base = journal_scratch("inflight-cancelled");
        {
            let mut store = super::Store::open_at(base.clone());
            crashed_turn(&mut store, "");
        }
        let mut reopened = super::Store::open_at(base.clone());
        reopened.rehydrate("crashed");
        let session = reopened.session("crashed").expect("the session");
        let last = session.messages.last().expect("the interrupted row");
        assert!(
            last.meta.as_ref().is_some_and(|meta| meta.cancelled),
            "the row says the turn never finished: {last:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A journal whose row count is behind the log replaces the tail, not the
    /// rows after it: a save that already appended the placeholder is updated
    /// in place instead of being appended twice.
    #[test]
    fn a_journal_on_a_saved_placeholder_replaces_it() {
        let base = journal_scratch("inflight-replace");
        let mut session = crate::model::Session::new("w", "crashed");
        session.id = "crashed".to_string();
        {
            let mut store = super::Store::open_at(base.clone());
            store.insert(session.clone());
            store.save(&session).expect("the document");
            let mut prompt = crate::model::ChatMessage::user("go");
            prompt.timing_mut().start = Some(1);
            session.messages.push(prompt);
            session
                .messages
                .push(crate::model::ChatMessage::assistant());
            store.save(&session).expect("the prompt and placeholder");
        }

        // A journal that carries the richer placeholder, written after the
        // save that committed the empty one.
        let mut settled = session.messages.last().expect("the placeholder").clone();
        settled.text = "the rest of the answer".to_string();
        let dir = crate::history::History::dir_for(&base, &session.id);
        std::fs::create_dir_all(&dir).expect("the log directory");
        std::fs::write(
            dir.join(super::INFLIGHT),
            serde_json::to_vec(&super::Inflight::new(1, vec![settled]).expect("the journal"))
                .expect("the journal bytes"),
        )
        .expect("the journal file");

        let mut reopened = super::Store::open_at(base.clone());
        reopened.rehydrate(&session.id);
        let rows = &reopened.session(&session.id).expect("the session").messages;
        assert_eq!(rows.len(), 2, "no duplicate assistant row");
        assert_eq!(rows[1].text, "the rest of the answer");
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- #350: journal recovery classifies, and never deletes what it cannot read ----

    /// A scratch root holding one crashed turn's journal, with its bytes.
    fn crashed_journal(
        name: &str,
        answer: &str,
    ) -> (std::path::PathBuf, std::path::PathBuf, Vec<u8>) {
        let base = journal_scratch(name);
        let (dir, bytes) = {
            let mut store = super::Store::open_at(base.clone());
            let session = crashed_turn(&mut store, answer);
            let dir = crate::history::History::dir_for(&base, &session.id);
            let bytes = std::fs::read(dir.join(super::INFLIGHT)).expect("the journal bytes");
            (dir, bytes)
        };
        (base, dir, bytes)
    }

    /// Replace a crashed turn's journal bytes.
    fn write_journal(dir: &std::path::Path, bytes: &[u8]) {
        std::fs::write(dir.join(super::INFLIGHT), bytes).expect("the journal file");
    }

    /// The one incident directory a quarantine created, with its report.
    fn only_quarantine(dir: &std::path::Path) -> (std::path::PathBuf, super::RecoveryRecord) {
        let corrupt = dir.join(super::CORRUPT);
        let mut incidents: Vec<std::path::PathBuf> = std::fs::read_dir(&corrupt)
            .expect("a corrupt directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        incidents.sort();
        assert_eq!(incidents.len(), 1, "one incident directory");
        let incident = incidents.remove(0);
        let text =
            std::fs::read_to_string(incident.join("report.json")).expect("the report exists");
        let report: super::RecoveryRecord = serde_json::from_str(&text).expect("the report parses");
        assert!(
            !incident.join("report.json.tmp").exists(),
            "the report is installed by rename, not left as a temporary"
        );
        (incident, report)
    }

    /// Launch over `base` and assert one quarantine with the expected reason,
    /// the original bytes intact beside a report naming the session.
    fn opened_quarantine(
        base: &std::path::Path,
        dir: &std::path::Path,
        bytes: &[u8],
        reason: &str,
    ) -> super::Store {
        let store = super::Store::open_at(base.to_path_buf());
        assert!(
            !dir.join(super::INFLIGHT).exists(),
            "the journal left its log directory"
        );
        let (incident, report) = only_quarantine(dir);
        assert_eq!(
            std::fs::read(incident.join("journal.json")).expect("the preserved journal"),
            bytes,
            "the original bytes are preserved"
        );
        assert_eq!(report.session, "crashed", "the report names the session");
        assert_eq!(report.reason, reason, "the report names the reason");
        assert_eq!(
            store.recovery_report().quarantined.len(),
            1,
            "the launch reports the quarantine"
        );
        assert_eq!(store.recovery_report().quarantined[0].reason, reason);
        store
    }

    #[test]
    fn fnv1a_64_matches_the_published_vectors() {
        assert_eq!(super::fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(super::fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn a_corrupt_journal_is_quarantined_with_its_bytes() {
        let (base, dir, _) = crashed_journal("corrupt", "half a reply");
        let bytes = b"this is not a journal".to_vec();
        write_journal(&dir, &bytes);
        let store = opened_quarantine(&base, &dir, &bytes, "unreadable");
        assert_eq!(store.recovery_report().quarantined[0].rows, 0);
    }

    #[test]
    fn a_truncated_journal_is_quarantined() {
        let (base, dir, original) = crashed_journal("truncated", "half a reply");
        let bytes = original[..original.len() / 2].to_vec();
        write_journal(&dir, &bytes);
        let store = opened_quarantine(&base, &dir, &bytes, "truncated");
        assert_eq!(
            store.recovery_report().quarantined[0].source_path,
            dir.join(super::INFLIGHT).display().to_string(),
            "the record says where the bytes came from"
        );
    }

    #[test]
    fn an_empty_journal_is_quarantined() {
        let (base, dir, _) = crashed_journal("empty", "half a reply");
        let bytes = Vec::new();
        write_journal(&dir, &bytes);
        opened_quarantine(&base, &dir, &bytes, "empty");
    }

    #[test]
    fn a_journal_whose_checksum_does_not_match_is_quarantined() {
        let (base, dir, original) = crashed_journal("checksum", "half a reply");
        let mut value: serde_json::Value =
            serde_json::from_slice(&original).expect("the journal json");
        let checksum = value["checksum"].as_str().expect("a checksum");
        let flipped = u64::from_str_radix(checksum, 16).expect("hex") ^ 1;
        value["checksum"] = serde_json::Value::String(format!("{flipped:016x}"));
        let bytes = serde_json::to_vec(&value).expect("the edited journal");
        write_journal(&dir, &bytes);
        let store = opened_quarantine(&base, &dir, &bytes, "checksum-mismatch");
        assert_eq!(
            store.recovery_report().quarantined[0].version,
            1,
            "the record keeps the declared version"
        );
        assert_eq!(
            store.recovery_report().quarantined[0].rows,
            1,
            "and the row COUNT, never the contents"
        );
    }

    #[test]
    fn a_journal_from_a_newer_version_is_quarantined_not_dropped() {
        let (base, dir, original) = crashed_journal("version", "half a reply");
        let mut value: serde_json::Value =
            serde_json::from_slice(&original).expect("the journal json");
        value["version"] = serde_json::json!(2);
        let bytes = serde_json::to_vec(&value).expect("the edited journal");
        write_journal(&dir, &bytes);
        let store = opened_quarantine(&base, &dir, &bytes, "unsupported-version");
        assert_eq!(
            store.recovery_report().quarantined[0].version,
            2,
            "an unknown version is preserved, not guessed at"
        );
    }

    #[test]
    fn a_journal_crashing_before_its_rename_is_quarantined() {
        let (base, dir, original) = crashed_journal("pre-rename", "half a reply");
        std::fs::rename(dir.join(super::INFLIGHT), dir.join(super::INFLIGHT_TMP))
            .expect("a kill before the rename");
        let store = opened_quarantine(&base, &dir, &original, "interrupted-write");
        assert!(
            !dir.join(super::INFLIGHT_TMP).exists(),
            "the temporary left the log directory"
        );
        assert!(
            store.recovery_report().quarantined[0]
                .source_path
                .ends_with(super::INFLIGHT_TMP),
            "the record names the temporary it preserved"
        );
    }

    #[test]
    fn a_valid_journal_replays_once_and_a_second_start_does_not_duplicate() {
        let (base, dir, _) = crashed_journal("replay-once", "half a reply");
        {
            let mut store = super::Store::open_at(base.clone());
            assert_eq!(
                store.recovery_report().replayed.len(),
                1,
                "the valid journal replayed"
            );
            assert_eq!(store.recovery_report().replayed[0].rows, 1);
            store.rehydrate("crashed");
            assert_eq!(
                store
                    .session("crashed")
                    .expect("the session")
                    .messages
                    .len(),
                2,
                "the prompt and its recovered answer"
            );
        }
        assert!(
            !dir.join(super::INFLIGHT).exists(),
            "the journal is consumed by the replay"
        );

        let mut second = super::Store::open_at(base.clone());
        assert!(
            second.recovery_report().replayed.is_empty(),
            "a second start has nothing to replay"
        );
        assert!(second.recovery_report().quarantined.is_empty());
        second.rehydrate("crashed");
        assert_eq!(
            second
                .session("crashed")
                .expect("the session")
                .messages
                .len(),
            2,
            "the rows are not duplicated by the second start"
        );
        assert_eq!(crate::history::History::open(dir).len(), 2);
    }

    #[test]
    fn a_legacy_journal_without_a_checksum_still_replays() {
        let (base, dir, original) = crashed_journal("legacy", "an answer");
        let mut value: serde_json::Value =
            serde_json::from_slice(&original).expect("the journal json");
        let object = value.as_object_mut().expect("an object");
        object.remove("version");
        object.remove("checksum");
        write_journal(
            &dir,
            &serde_json::to_vec(&value).expect("the legacy journal"),
        );
        let mut store = super::Store::open_at(base.clone());
        assert_eq!(
            store.recovery_report().replayed.len(),
            1,
            "a version 0 journal is still replayed"
        );
        assert_eq!(
            store.recovery_report().replayed[0].version,
            0,
            "and is reported as legacy"
        );
        store.rehydrate("crashed");
        let session = store.session("crashed").expect("the session");
        assert_eq!(session.messages[1].text, "an answer");
    }

    #[test]
    fn a_journal_for_a_deleted_session_is_debris() {
        let (base, dir, _) = crashed_journal("debris", "half a reply");
        std::fs::remove_file(base.join("crashed.json")).expect("the document");
        let store = super::Store::open_at(base.clone());
        assert!(
            !dir.join(super::INFLIGHT).exists(),
            "a deleted session's journal is debris"
        );
        assert!(
            !dir.join(super::CORRUPT).exists(),
            "and is not reported as corruption"
        );
        assert!(store.recovery_report().quarantined.is_empty());
        assert!(store.recovery_report().replayed.is_empty());
    }

    #[test]
    fn a_quarantine_report_never_records_message_contents() {
        let secret = "PRIVATE-ANSWER-DO-NOT-RECORD";
        let (base, dir, original) = crashed_journal("no-contents", secret);
        assert!(String::from_utf8_lossy(&original).contains(secret));
        let mut value: serde_json::Value =
            serde_json::from_slice(&original).expect("the journal json");
        value["checksum"] = serde_json::Value::String("0000000000000000".to_string());
        let bytes = serde_json::to_vec(&value).expect("the edited journal");
        write_journal(&dir, &bytes);
        let store = opened_quarantine(&base, &dir, &bytes, "checksum-mismatch");
        let (incident, _) = only_quarantine(&dir);
        let report = std::fs::read_to_string(incident.join("report.json")).expect("the report");
        assert!(
            !report.contains(secret),
            "the report holds counts, never contents: {report}"
        );
        assert_eq!(
            store.recovery_report().quarantined[0].rows,
            1,
            "but it does count the rows it could not replay"
        );
        let report: serde_json::Value = serde_json::from_str(&report).expect("the report json");
        for field in [
            "session",
            "source_path",
            "reason",
            "version",
            "quarantined_at",
        ] {
            assert!(report.get(field).is_some(), "the report carries {field}");
        }
    }

    #[test]
    fn a_journal_whose_replay_fails_is_kept_for_the_next_launch() {
        let base = journal_scratch("replay-fails");
        let mut session = crate::model::Session::new("w", "crashed");
        session.id = "crashed".to_string();
        {
            let mut store = super::Store::open_at(base.clone());
            store.insert(session.clone());
            store.save(&session).expect("the document");
            // One save of many rows seals one segment and lists it in the
            // manifest, which is what the replay has to read.
            for index in 0..600 {
                session
                    .messages
                    .push(crate::model::ChatMessage::user(format!("m {index}")));
            }
            store.save(&session).expect("the sealed log");
        }
        let dir = crate::history::History::dir_for(&base, &session.id);
        let mut settled = crate::model::ChatMessage::assistant();
        settled.text = "the rest of the answer".to_string();
        let bytes =
            serde_json::to_vec(&super::Inflight::new(599, vec![settled]).expect("the journal"))
                .expect("the journal bytes");
        write_journal(&dir, &bytes);
        // The listed segment is now a directory, so the replay's read of its
        // footer cannot succeed.
        let segment = listed_segment(&dir, "messages");
        std::fs::remove_file(&segment).expect("the segment");
        std::fs::create_dir(&segment).expect("a directory where the segment was");

        let store = super::Store::open_at(base.clone());
        assert!(
            dir.join(super::INFLIGHT).is_file(),
            "a failed replay keeps the journal for the next launch"
        );
        assert!(
            !dir.join(super::CORRUPT).exists(),
            "a retryable failure is not corruption"
        );
        let report = store.recovery_report();
        assert_eq!(report.kept.len(), 1);
        assert_eq!(report.kept[0].reason, "replay-failed");
        assert_eq!(
            std::fs::read(dir.join(super::INFLIGHT)).expect("the kept journal"),
            bytes
        );
    }

    // ---- #351: a failed page read is a failure, never an empty page ----

    /// A session of `rows` assistant turns, each with one tool call, saved in
    /// ONE append so both logs seal into a manifest-listed segment each.
    fn sealed_session(name: &str, rows: usize) -> (std::path::PathBuf, String, std::path::PathBuf) {
        let base = journal_scratch(name);
        let mut session = crate::model::Session::new("w", "sealed");
        session.id = "sealed".to_string();
        for index in 0..rows {
            let mut message = crate::model::ChatMessage::assistant();
            message.text = format!("answer {index}");
            message.tools.push(crate::model::ToolEvent {
                name: "bash".to_string(),
                args: "{}".to_string(),
                output: Some(format!("out {index}")),
                ms: Some(1),
                at: Some(index as u64),
            });
            session.messages.push(message);
        }
        {
            let mut store = super::Store::open_at(base.clone());
            store.insert(session.clone());
            store.save(&session).expect("the sealed save");
        }
        let dir = crate::history::History::dir_for(&base, &session.id);
        (base, session.id, dir)
    }

    /// One segment a manifest lists, by log kind (`messages`/`trajectory`).
    fn listed_segment(dir: &std::path::Path, kind: &str) -> std::path::PathBuf {
        let text = std::fs::read_to_string(dir.join("manifest.json")).expect("a manifest");
        let manifest: serde_json::Value = serde_json::from_str(&text).expect("manifest json");
        let name = manifest[kind][0]["file"]
            .as_str()
            .expect("a listed segment")
            .to_string();
        dir.join(name)
    }

    /// Replace a segment with a Parquet file of a different schema.
    fn write_foreign_parquet(path: &std::path::Path) {
        use arrow::array::Int64Array;
        use arrow::datatypes::{DataType, Field, Schema};
        use parquet::arrow::ArrowWriter;
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(
            (0..10)
                .map(|index| Field::new(format!("col{index}"), DataType::Int64, false))
                .collect::<Vec<_>>(),
        ));
        let columns: Vec<Arc<dyn arrow::array::Array>> = (0..10)
            .map(|_| {
                Arc::new(Int64Array::from((0..128).collect::<Vec<i64>>()))
                    as Arc<dyn arrow::array::Array>
            })
            .collect();
        let batch = arrow::record_batch::RecordBatch::try_new(schema.clone(), columns)
            .expect("a foreign batch");
        let file = std::fs::File::create(path).expect("the foreign segment");
        let mut writer = ArrowWriter::try_new(file, schema, None).expect("a foreign writer");
        writer.write(&batch).expect("the foreign rows");
        writer.close().expect("the foreign footer");
    }

    /// Corrupt every data page while leaving the file's footer readable.
    fn corrupt_data_pages(path: &std::path::Path) {
        let mut bytes = std::fs::read(path).expect("the segment bytes");
        let len = bytes.len();
        assert!(len > 1024, "the segment is big enough to have data pages");
        let footer_len =
            u32::from_le_bytes(bytes[len - 8..len - 4].try_into().expect("4 bytes")) as usize;
        let footer_start = len - 8 - footer_len;
        assert!(footer_start > 8, "the footer is not the whole file");
        for byte in &mut bytes[4..footer_start] {
            *byte = 0xff;
        }
        std::fs::write(path, &bytes).expect("the corrupted segment");
    }

    /// Truncate a segment so its footer is gone.
    fn truncate_segment(path: &std::path::Path) {
        let len = std::fs::metadata(path).expect("the segment").len();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the segment handle");
        file.set_len(len / 2).expect("the truncation");
    }

    /// One way a listed segment is damaged, by name.
    type Damage = (&'static str, fn(&std::path::Path));

    /// The failures injected into one segment, by name.
    fn damages() -> Vec<Damage> {
        vec![
            ("missing", |path| {
                std::fs::remove_file(path).expect("the segment");
            }),
            ("truncated", truncate_segment),
            ("foreign-schema", write_foreign_parquet),
            ("corrupt-row-group", corrupt_data_pages),
        ]
    }

    #[test]
    fn a_failed_message_window_is_a_page_failure_not_an_empty_page() {
        for (name, damage) in damages() {
            let (base, id, dir) = sealed_session("window-failure", 600);
            let mut store = super::Store::open_at(base.clone());
            store.rehydrate(&id);
            let read = store.older_read(&id, super::WINDOW).expect("a window");
            let segment = listed_segment(&dir, "messages");
            damage(&segment);

            let page = read.read();
            assert!(
                page.rows.is_empty(),
                "{name}: a failed page carries no rows"
            );
            let failure = page
                .failure()
                .unwrap_or_else(|| panic!("{name}: a failure, not an empty success"));
            assert!(failure.retried, "{name}: the one reopen was attempted");
            assert_eq!(failure.session, id, "{name}: the failure names the session");
            assert!(
                failure.detail.contains(&dir.display().to_string()),
                "{name}: the detail names the log: {}",
                failure.detail
            );
            assert!(
                failure.to_string().contains("could not be read"),
                "{name}: the failure is shown to a caller: {failure}"
            );
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn a_failed_trajectory_page_is_a_page_failure_not_an_empty_page() {
        for (name, damage) in damages() {
            let (base, id, dir) = sealed_session("trace-failure", 600);
            let mut store = super::Store::open_at(base.clone());
            store.rehydrate(&id);
            // Page 4 is the OLDEST page, so it starts at row 0 and reads row
            // group 0 - the group every damaged file below still describes.
            // (Page 0 is the newest, i.e. the last group, and a file whose
            // manifest claims more groups than it holds trips a panic inside
            // the Parquet reader; that belongs to history.rs, not this page.)
            let _ = store.trace_read(&id, 4).expect("a trajectory page");
            let segment = listed_segment(&dir, "trajectory");
            damage(&segment);

            let page = store
                .trace_read(&id, 4)
                .unwrap_or_else(|| panic!("{name}: the page is still listed"))
                .read();
            assert!(
                page.rows.is_empty(),
                "{name}: a failed page carries no rows"
            );
            let failure = page
                .failure()
                .unwrap_or_else(|| panic!("{name}: a failure, not an empty success"));
            assert!(failure.retried, "{name}: the one reopen was attempted");
            assert_eq!(failure.session, id, "{name}: the failure names the session");
            assert!(
                failure.detail.contains(&dir.display().to_string()),
                "{name}: the detail names the log: {}",
                failure.detail
            );
            let _ = std::fs::remove_dir_all(&base);
        }
    }

    #[test]
    fn a_page_past_the_end_is_still_an_empty_success() {
        let (base, id, dir) = sealed_session("past-end", 300);
        let store = super::Store::open_at(base.clone());
        let len = crate::history::History::open(dir.clone()).len();
        assert_eq!(len, 300, "the log holds the rows");
        let read = super::OlderRead {
            session: id.clone(),
            from: len,
            floor: len + 64,
            reader: crate::history::History::open(dir.clone()).reader(),
            dir: dir.clone(),
        };
        let page = read.read();
        assert!(
            page.failure().is_none(),
            "a page past the end is not a failure"
        );
        assert!(page.rows.is_empty(), "and it holds no rows");

        let trace = super::TraceRead {
            session: id,
            page: 64,
            reader: crate::history::History::open(dir.clone()).reader(),
            dir,
        };
        let trace = trace.read();
        assert!(
            trace.failure().is_none(),
            "a trajectory page past the end is not a failure either"
        );
        assert!(trace.rows.is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(&base);
    }
}
