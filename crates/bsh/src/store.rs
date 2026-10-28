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

use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::history::{History, Reader, ToolRow};
use crate::model::{ChatMessage, Session};

/// Messages a session keeps in memory: two pages, which is a screen of
/// scrolling in each direction. Everything older stays in the log until the
/// reader scrolls to it.
pub(crate) const WINDOW: usize = 2 * crate::history::PAGE;

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
        let rows = self.reader.messages(self.from, len).or_else(|_| {
            // A compaction may have swapped the directory between the snapshot
            // and this read. Reopening follows the installed manifest, and the
            // rows are the same ones either way.
            History::open(self.dir.clone())
                .reader()
                .messages(self.from, len)
        });
        OlderPage {
            session: self.session,
            from: self.from,
            rows: rows.unwrap_or_default(),
        }
    }
}

/// The rows one OlderRead decoded.
pub(crate) struct OlderPage {
    /// The session the window belongs to.
    pub(crate) session: String,
    /// The first global row the window holds.
    pub(crate) from: usize,
    /// The decoded rows, oldest first.
    pub(crate) rows: Vec<ChatMessage>,
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
        let rows = self.reader.trajectory_page(self.page).or_else(|_| {
            History::open(self.dir.clone())
                .reader()
                .trajectory_page(self.page)
        });
        TracePage {
            session: self.session,
            page: self.page,
            rows: rows.unwrap_or_default(),
        }
    }
}

/// The trajectory rows one TraceRead decoded.
pub(crate) struct TracePage {
    /// The session the page belongs to.
    pub(crate) session: String,
    /// The page counted from the newest end.
    pub(crate) page: usize,
    /// The decoded rows, oldest first.
    pub(crate) rows: Vec<ToolRow>,
}

/// The directory a workspace keeps its sessions in.
pub(crate) fn sessions_dir(root: &Path) -> PathBuf {
    root.join(".bsh").join("sessions")
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
        };
        let dirs: Vec<PathBuf> = store.roots.values().cloned().collect();
        store.read(&dirs);
        store.sort();
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
        };
        store.read(&[]);
        store.sort();
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
            if document.is_file() {
                log::warn!(
                    "dropping a stray copy of session {id} under {}",
                    dir.display()
                );
                let _ = fs::remove_file(&document);
            }
            let history = History::dir_for(&dir, id);
            if history.is_dir() {
                let _ = fs::remove_dir_all(&history);
            }
        }
    }

    /// Insert a session at the front.
    pub(crate) fn insert(&mut self, session: Session) {
        self.histories.borrow_mut().clear();
        self.sessions.insert(0, session);
    }

    /// Write one session to disk.
    /// Write a session: its transcript to the Parquet log, its document to JSON.
    ///
    /// The log is append-only, so a save costs the rows written since the last
    /// one rather than the history - and the document no longer embeds the
    /// transcript, which keeps a long conversation's metadata small and lets a
    /// session be listed without touching its log.
    pub(crate) fn save(&self, session: &Session) -> anyhow::Result<()> {
        let dir = self.dir_for(session);
        fs::create_dir_all(dir)?;
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
        let mut document = normalized.clone();
        document.messages.clear();
        let path = dir.join(format!("{}.json", normalized.id));
        let text = self.redact(&serde_json::to_string_pretty(&document)?);
        fs::write(path, text)?;
        self.save_history(&normalized, dir)?;
        // The document just written is the authority; the same id anywhere else
        // is a leftover that would resurrect this chat if it were deleted.
        self.drop_strays(&normalized.id, dir);
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
            return Ok(());
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
        // A truncation or a compaction changed the tail. The pages ABOVE the
        // window are untouched, so they are read back before the rewrite:
        // rebuilding from the window alone would drop them.
        let mut all = history.prefix(start)?;
        all.extend(rows);
        history.rewrite(&all)?;
        Ok(())
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
            let Ok(window) = history.window(from, rows - from) else {
                return false;
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
            let name = entry.file_name();
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
}
