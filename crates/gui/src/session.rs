//! Chat session model and JSON persistence under `~/.bos/gui/sessions/`.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use agent::tools::PlanItem;
use serde::{Deserialize, Serialize};

/// Who authored a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Role {
    /// The human user.
    User,
    /// The assistant (LLM).
    Assistant,
}

/// A tool invocation surfaced while streaming a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ToolEvent {
    /// Tool name reported by the model.
    pub(crate) name: String,
    /// Serialized tool arguments.
    pub(crate) args: String,
    /// Tool output captured when the ReAct loop finished the call
    /// (absent for records persisted before tool results were streamed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<String>,
    /// Wall-clock execution time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ms: Option<u64>,
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    /// Author of the message.
    pub(crate) role: Role,
    /// Visible response text.
    pub(crate) text: String,
    /// Reasoning/thinking trace, when the model produced one.
    #[serde(default)]
    pub(crate) reasoning: String,
    /// Tool calls made while producing the response.
    #[serde(default)]
    pub(crate) tools: Vec<ToolEvent>,
    /// Error description when the turn failed.
    #[serde(default)]
    pub(crate) error: Option<String>,
}

impl ChatMessage {
    /// Create a user message.
    pub(crate) fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            text: text.into(),
            reasoning: String::new(),
            tools: Vec::new(),
            error: None,
        }
    }

    /// Create an empty assistant message that streaming fills in.
    pub(crate) fn assistant() -> Self {
        Self {
            role: Role::Assistant,
            text: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            error: None,
        }
    }

    /// Whether the message carries nothing renderable.
    pub(crate) fn is_blank(&self) -> bool {
        self.text.is_empty()
            && self.reasoning.is_empty()
            && self.tools.is_empty()
            && self.error.is_none()
    }
}

/// A conversation session, persisted as one JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecord {
    /// Stable identifier (UUID).
    pub(crate) id: String,
    /// Short human title shown in the sidebar.
    pub(crate) title: String,
    /// Unix seconds when the session was created.
    pub(crate) created_at: u64,
    /// Unix seconds of the last update, used for ordering.
    pub(crate) updated_at: u64,
    /// Workspace this conversation belongs to (§24). Empty means "written before
    /// workspaces existed", which the summaries resolve to the workspace in use.
    #[serde(default)]
    pub(crate) workspace: String,
    /// The conversation, oldest first.
    pub(crate) messages: Vec<ChatMessage>,
    /// Working-plan snapshot from the agent's most recent turn, so the plan
    /// panel survives restarts (older session files simply have none).
    #[serde(default)]
    pub(crate) plan: Vec<PlanItem>,
    /// The turns the last compaction folded away, oldest first.
    ///
    /// Compaction rewrites the history into an anchor plus a summary, and a
    /// summary is lossy, so without this the folded turns are gone. Keeping them
    /// makes compaction recoverable rather than destructive; a later compaction
    /// replaces this, since it folds the current history including any earlier
    /// summary. Older session files simply have none.
    #[serde(default)]
    pub(crate) archived: Vec<ChatMessage>,
}

impl SessionRecord {
    /// Start a fresh, empty session.
    pub(crate) fn new() -> Self {
        let now = unix_now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: "New chat".to_string(),
            created_at: now,
            updated_at: now,
            workspace: String::new(),
            messages: Vec::new(),
            plan: Vec::new(),
            archived: Vec::new(),
        }
    }

    /// Bump the modification timestamp.
    pub(crate) fn touch(&mut self) {
        self.updated_at = unix_now();
    }

    /// Clone the messages before `index` into a brand-new session record.
    ///
    /// This is the branching counterpart to [`SessionRecord::truncate_from`]:
    /// instead of dropping the tail of this transcript, the kept prefix is
    /// copied into a fresh record — new id, `"… (branch)"` title, current
    /// timestamps — so both histories survive. The plan snapshot carries over
    /// because the branch continues the same work.
    pub(crate) fn fork_from(&self, index: usize) -> Result<Self, String> {
        let n = self.messages.len();
        if index == 0 {
            return Err("cannot branch an empty transcript".to_string());
        }
        if index > n {
            return Err(format!("index {index} out of range ({n} messages)"));
        }
        let now = unix_now();
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!("{} (branch)", self.title),
            created_at: now,
            updated_at: now,
            // A branch stays in the workspace its parent was written in, so the
            // fork cannot silently change the agent's tools or model.
            workspace: self.workspace.clone(),
            messages: self.messages[..index].to_vec(),
            plan: self.plan.clone(),
            // A branch starts with nothing folded of its own.
            archived: Vec::new(),
        })
    }

    /// Derive the sidebar title from the first user message.
    pub(crate) fn derive_title(&mut self) {
        if let Some(first) = self.messages.iter().find(|m| m.role == Role::User) {
            let trimmed = first.text.trim();
            const MAX: usize = 32;
            let title: String = trimmed.chars().take(MAX).collect();
            self.title = if trimmed.chars().count() > MAX {
                format!("{title}…")
            } else if title.is_empty() {
                "New chat".to_string()
            } else {
                title
            };
        }
    }

    /// Drop every message from `index` onward (edit-and-resend and
    /// "delete from here" both funnel through here). Returns how many
    /// messages survive; an out-of-range index changes nothing.
    pub(crate) fn truncate_from(&mut self, index: usize) -> Result<usize, String> {
        if index > self.messages.len() {
            return Err(format!(
                "truncate index {index} out of range (history has {})",
                self.messages.len()
            ));
        }
        self.messages.truncate(index);
        Ok(self.messages.len())
    }
}

/// Current time in unix seconds.
/// One place a search phrase was found.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SearchHit {
    /// Session the match belongs to.
    pub(crate) session: String,
    /// Its title, so a result is readable without a second lookup.
    pub(crate) title: String,
    /// Workspace it is filed under.
    pub(crate) workspace: String,
    /// Message index the match is in (0 when the title matched).
    pub(crate) index: usize,
    /// Text around the match, with ellipses where it was cut.
    pub(crate) snippet: String,
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format unix seconds as `YYYY-MM-DD HH:MM` in UTC (civil-from-days, so no
/// extra date-time dependency).
fn fmt_utc(unix: u64) -> String {
    let secs = unix as i64;
    let days = secs.div_euclid(86_400);
    let mins = secs.rem_euclid(86_400).div_euclid(60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        mins / 60,
        mins % 60
    )
}

/// Filesystem-safe filename slug for a session title.
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in title.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return "chat".to_string();
    }
    slug.chars()
        .take(48)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Clip text for a single Markdown line.
fn clip_md(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

impl SessionRecord {
    /// Render the session as portable Markdown (title, working plan, and the
    /// full transcript including reasoning, tool calls, and errors).
    pub(crate) fn to_markdown(&self) -> String {
        use agent::tools::PlanStatus;
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", self.title));
        out.push_str(&format!(
            "_BOS export · created {} UTC · updated {} UTC_\n",
            fmt_utc(self.created_at),
            fmt_utc(self.updated_at)
        ));
        if !self.plan.is_empty() {
            out.push_str("\n## Plan\n\n");
            for item in &self.plan {
                let boxc = match item.status {
                    PlanStatus::Completed => "[x]",
                    PlanStatus::InProgress => "[~]",
                    PlanStatus::Pending => "[ ]",
                };
                out.push_str(&format!("- {boxc} {}\n", item.text));
            }
        }
        out.push_str("\n## Conversation\n");
        let mut any = false;
        for m in &self.messages {
            if m.is_blank() {
                continue;
            }
            any = true;
            let who = match m.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            out.push_str(&format!("\n### {who}\n\n"));
            if !m.reasoning.is_empty() {
                out.push_str("**Reasoning**\n\n");
                for line in m.reasoning.lines() {
                    out.push_str(&format!("> {line}\n"));
                }
                out.push('\n');
            }
            for t in &m.tools {
                let args = t.args.lines().collect::<Vec<_>>().join(" ");
                out.push_str(&format!("- tool `{}` `{}`\n", t.name, clip_md(&args, 300)));
            }
            if !m.tools.is_empty() {
                out.push('\n');
            }
            if !m.text.is_empty() {
                out.push_str(m.text.trim_end());
                out.push_str("\n\n");
            }
            if let Some(err) = &m.error {
                out.push_str(&format!("**Error:** {err}\n\n"));
            }
        }
        if !any {
            out.push_str("\n_No messages yet._\n");
        }
        out.trim_end().to_string()
    }
}

impl SessionStore {
    /// Load one session by id; `None` when the file is missing or corrupt.
    pub(crate) fn load(&self, id: &str) -> Option<SessionRecord> {
        let bytes = fs::read(self.path_for(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Directory exports are written to (sibling `exports/` of `sessions/`).
    pub(crate) fn export_dir(&self) -> PathBuf {
        self.dir
            .parent()
            .map(|p| p.join("exports"))
            .unwrap_or_else(|| self.dir.join("exports"))
    }

    /// Write the session as Markdown under [`Self::export_dir`], returning the
    /// file path. Re-exporting the same session overwrites its previous file.
    pub(crate) fn export(&self, record: &SessionRecord) -> std::io::Result<PathBuf> {
        let dir = self.export_dir();
        fs::create_dir_all(&dir)?;
        let short = &record.id[..record.id.len().min(8)];
        let path = dir.join(format!("{}-{short}.md", slugify(&record.title)));
        fs::write(&path, record.to_markdown())?;
        Ok(path)
    }
}

/// Cached parses of session files, so a keystroke does not re-read the store.
///
/// `search` runs on every keystroke — the UI asks from two characters — and
/// re-reading and re-parsing every file dominated it. An entry is keyed by the
/// file's `(modified, length)`, so a rewritten session is re-read. The one case
/// that can serve stale data is a same-length rewrite inside the filesystem's
/// timestamp granularity; the invalidation test pins the behaviour that matters,
/// that differently sized rewrites are seen.
#[derive(Debug, Default)]
struct SearchCache {
    /// Parsed sessions by path, with the stamp they were parsed from.
    records: std::collections::HashMap<PathBuf, ((std::time::SystemTime, u64), Arc<CachedSession>)>,
}

/// A parsed session plus the lowercase copies search would otherwise rebuild.
///
/// The copies are what make a keystroke cheap: the pre-filter is then an exact
/// `str::contains` on text that is already lowered, for every needle including
/// CJK, instead of a scan that re-lowers every message. Memory is roughly the
/// session's text again, and the cache holds at most the live sessions.
#[derive(Debug)]
struct CachedSession {
    /// The parsed session.
    record: SessionRecord,
    /// `(text, reasoning)` lowercased, index-aligned with `record.messages`.
    lower: Vec<(String, String)>,
}

/// Directory-backed store of session JSON files.
#[derive(Debug, Clone)]
pub(crate) struct SessionStore {
    dir: PathBuf,
    /// Only the interactive path uses this; `load_all` stays a plain read.
    cache: Arc<Mutex<SearchCache>>,
}

impl SessionStore {
    /// Where sessions live, and therefore where anything kept beside them goes.
    pub(crate) fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// The conventional per-user location: `~/.bos/gui/sessions`.
    pub(crate) fn default_dir() -> PathBuf {
        PathBuf::from(shellexpand::tilde("~/.bos/gui/sessions").into_owned())
    }

    /// Create a store rooted at an explicit directory (used by tests).
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            cache: Arc::new(Mutex::new(SearchCache::default())),
        }
    }

    /// Every session, parsed from the cache when its file has not changed.
    ///
    /// The cache holds at most the live sessions: a file that is gone from the
    /// directory is dropped from it, so it cannot grow without bound.
    fn cached_sessions(&self) -> Vec<Arc<CachedSession>> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return out;
        };
        let mut found: Vec<(PathBuf, (std::time::SystemTime, u64))> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let stamp = (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len());
            found.push((path, stamp));
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        for (path, stamp) in &found {
            if let Some((cached, session)) = cache.records.get(path) {
                if cached == stamp {
                    out.push(Arc::clone(session));
                    continue;
                }
            }
            let Ok(bytes) = fs::read(path) else { continue };
            match serde_json::from_slice::<SessionRecord>(&bytes) {
                Ok(record) => {
                    let lower = record
                        .messages
                        .iter()
                        .map(|m| (m.text.to_lowercase(), m.reasoning.to_lowercase()))
                        .collect();
                    let session = Arc::new(CachedSession { record, lower });
                    cache
                        .records
                        .insert(path.clone(), (*stamp, Arc::clone(&session)));
                    out.push(session);
                }
                // A file that stopped parsing must not keep being served.
                Err(_) => {
                    cache.records.remove(path);
                }
            }
        }
        let live: std::collections::HashSet<&PathBuf> = found.iter().map(|(p, _)| p).collect();
        cache.records.retain(|path, _| live.contains(path));
        out
    }

    /// A store at the conventional per-user location.
    pub(crate) fn default_store() -> Self {
        Self::new(Self::default_dir())
    }

    /// Load every parseable session, newest first.
    ///
    /// Unreadable or corrupt files are skipped rather than failing startup.
    pub(crate) fn load_all(&self) -> Vec<SessionRecord> {
        let mut sessions = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return sessions;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            if let Ok(record) = serde_json::from_slice::<SessionRecord>(&bytes) {
                sessions.push(record);
            }
        }
        sessions.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
        sessions
    }

    /// Persist one session (atomically, via a temp file + rename).
    pub(crate) fn save(&self, record: &SessionRecord) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.path_for(&record.id);
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, data)?;
        fs::rename(tmp, path)
    }

    /// Delete a session file if it exists.
    /// Re-home a session into another workspace. Everything else is kept —
    /// its messages, its id, its title — because a move changes which
    /// configuration runs the chat, not what was said in it (§24.6).
    pub(crate) fn move_to(&self, id: &str, workspace: &str) -> std::io::Result<SessionRecord> {
        let mut record = self.load(id).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unknown session '{id}'"),
            )
        })?;
        record.workspace = workspace.to_string();
        self.save(&record)?;
        Ok(record)
    }

    /// Find a phrase in any stored chat — titles and message text (§17 parity: the
    /// harness keeps an FTS index; this scans, which is honest at these sizes and
    /// needs no second store that could drift out of step with the session files).
    pub(crate) fn search(&self, needle: &str, limit: usize) -> Vec<SearchHit> {
        let needle = needle.trim();
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let lower_needle: Vec<char> = needle.to_lowercase().chars().collect();
        let mut records = self.cached_sessions();
        // Newest first, then by id: a search result list that reshuffles between
        // identical queries is a result list nobody can use.
        records.sort_by(|a, b| {
            b.record
                .updated_at
                .cmp(&a.record.updated_at)
                .then_with(|| a.record.id.cmp(&b.record.id))
        });
        let mut hits = Vec::new();
        let lower_needle_text = needle.to_lowercase();
        for session in records {
            let record = &session.record;
            if hits.len() >= limit {
                break;
            }
            if let Some(hit) = Self::hit_for(record, &lower_needle, needle, None) {
                hits.push(hit);
                continue;
            }
            for (index, message) in record.messages.iter().enumerate() {
                if hits.len() >= limit {
                    break;
                }
                // The cached lowercase makes this an exact test, not merely a
                // necessary one: it is the same condition `hit_for` checks, done
                // without re-lowering the message. An earlier filter here was a
                // hand-rolled ASCII window scan that could only skip work; it was
                // sound, but slower than `str::contains` and useless for CJK.
                let (lower_text, lower_reasoning) = &session.lower[index];
                if !lower_text.contains(&lower_needle_text)
                    && (lower_reasoning.is_empty() || !lower_reasoning.contains(&lower_needle_text))
                {
                    continue;
                }
                let mut hit =
                    Self::hit_for(record, &lower_needle, needle, Some((index, &message.text)));
                if hit.is_none() && !message.reasoning.is_empty() {
                    hit = Self::hit_for(
                        record,
                        &lower_needle,
                        needle,
                        Some((index, &message.reasoning)),
                    );
                }
                if let Some(hit) = hit {
                    hits.push(hit);
                }
            }
        }
        hits
    }

    /// One match, or none. `title` matching reports index 0 with the title as the
    /// snippet, so a chat can be found by name from the same box.
    fn hit_for(
        record: &SessionRecord,
        lower_needle: &[char],
        needle: &str,
        body: Option<(usize, &str)>,
    ) -> Option<SearchHit> {
        let (index, text) = match body {
            Some((index, text)) => (index, text),
            None => (0usize, record.title.as_str()),
        };
        let chars: Vec<char> = text.chars().collect();
        let lower: Vec<char> = text.to_lowercase().chars().collect();
        // Lowercasing can change the length for a few scripts; when it does, the
        // character offsets no longer describe the original, so this says nothing
        // rather than pointing at the wrong characters.
        if lower.len() != chars.len() {
            return None;
        }
        if lower.windows(lower_needle.len()).all(|w| w != lower_needle) {
            return None;
        }
        let start = lower
            .windows(lower_needle.len())
            .position(|w| w == lower_needle)?
            .saturating_sub(40);
        let end = (start + lower_needle.len() + needle.chars().count() + 40).min(chars.len());
        let mut snippet: String = chars[start..end].iter().collect();
        if start > 0 {
            snippet.insert(0, '…');
        }
        if end < chars.len() {
            snippet.push('…');
        }
        Some(SearchHit {
            session: record.id.clone(),
            title: record.title.clone(),
            workspace: record.workspace.clone(),
            index,
            snippet,
        })
    }

    /// Where deleted sessions go, beside the store rather than into it.
    ///
    /// The name has no `.json` extension, so `load_all` and the search cache
    /// skip it exactly as they skip any other non-session entry.
    pub(crate) fn trash_dir(&self) -> PathBuf {
        self.dir.join(".trash")
    }

    /// Delete a session by moving it to the trash, so it can be brought back.
    ///
    /// Deleting a chat used to unlink the file, which made a mis-click
    /// irreversible and contradicted the rule this project already holds for
    /// workspaces, that removing one must never silently destroy sessions.
    pub(crate) fn delete(&self, id: &str) {
        let path = self.path_for(id);
        if !path.is_file() {
            return;
        }
        let trash = self.trash_dir();
        if fs::create_dir_all(&trash).is_err() {
            return;
        }
        let _ = fs::rename(&path, trash.join(format!("{id}.json")));
    }

    /// The sessions in the trash, newest first.
    ///
    /// Parsed rather than listed by filename, because the title and the time
    /// are what a person needs in order to recognise the chat they deleted
    /// (a bare UUID is not recognition).
    pub(crate) fn trashed(&self) -> Vec<SessionRecord> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(self.trash_dir()) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            if let Ok(record) = serde_json::from_slice::<SessionRecord>(&bytes) {
                out.push(record);
            }
        }
        out.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
        out
    }

    /// Bring a trashed session back into the store, returning its title.
    pub(crate) fn restore(&self, id: &str) -> Result<String, String> {
        let from = self.trash_dir().join(format!("{id}.json"));
        let Ok(bytes) = fs::read(&from) else {
            return Err("that chat is not in the trash".to_string());
        };
        let record: SessionRecord = serde_json::from_slice(&bytes)
            .map_err(|err| format!("the trashed chat is unreadable: {err}"))?;
        fs::create_dir_all(&self.dir).map_err(|err| format!("cannot create the store: {err}"))?;
        fs::rename(&from, self.path_for(id)).map_err(|err| format!("restore failed: {err}"))?;
        Ok(record.title)
    }

    /// Empty the trash for good, returning how many sessions were purged.
    ///
    /// This is the only place a session file is unlinked; everything else that
    /// removes one moves it here first.
    pub(crate) fn purge_trash(&self) -> usize {
        let mut purged = 0;
        let Ok(entries) = fs::read_dir(self.trash_dir()) else {
            return purged;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if fs::remove_file(&path).is_ok() {
                purged += 1;
            }
        }
        purged
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// What one chat amounts to at a glance.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SessionOverview {
    /// Messages the file holds, including turns a compaction folded.
    pub(crate) messages: usize,
    /// Turns written by the user.
    pub(crate) asks: usize,
    /// Turns written by the assistant.
    pub(crate) replies: usize,
    /// Tool calls recorded across every turn.
    pub(crate) tool_calls: usize,
    /// Turns that ended in an error.
    pub(crate) errors: usize,
    /// Visible characters across every turn.
    pub(crate) characters: usize,
    /// Reasoning characters, counted apart because the transcript hides them
    /// until asked, so folding them into the size would mislead.
    pub(crate) reasoning_characters: usize,
    /// Turns the last compaction folded away.
    pub(crate) archived: usize,
    /// Unix seconds when the chat was created.
    pub(crate) created_at: u64,
    /// Unix seconds of the last update.
    pub(crate) updated_at: u64,
    /// One entry per user turn, oldest first.
    pub(crate) outline: Vec<OutlineEntry>,
}

/// One user turn in a chat's outline.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct OutlineEntry {
    /// 1-based position among the user turns.
    pub(crate) n: usize,
    /// The ask, clipped for display.
    pub(crate) preview: String,
    /// Characters in the full ask.
    pub(crate) characters: usize,
}

/// Clip to `max` characters, marking that anything was dropped.
///
/// By characters, not bytes: a byte slice of a multi-byte character would
/// either panic on a non-boundary or produce mojibake, and the transcript is
/// explicitly multi-lingual.
fn clip_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

impl SessionRecord {
    /// Summarise this chat: counts, size, and one outline entry per ask.
    ///
    /// Every count includes the turns a compaction folded, because "how big is
    /// this chat" means the whole file, not just what survived the last fold;
    /// `archived` says how many of them are folded, so the reader can tell.
    pub(crate) fn overview(&self) -> SessionOverview {
        let all = || self.messages.iter().chain(self.archived.iter());
        SessionOverview {
            messages: self.messages.len() + self.archived.len(),
            asks: all().filter(|m| matches!(m.role, Role::User)).count(),
            replies: all().filter(|m| matches!(m.role, Role::Assistant)).count(),
            tool_calls: all().map(|m| m.tools.len()).sum(),
            errors: all().filter(|m| m.error.is_some()).count(),
            characters: all().map(|m| m.text.chars().count()).sum(),
            reasoning_characters: all().map(|m| m.reasoning.chars().count()).sum(),
            archived: self.archived.len(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            outline: all()
                .filter(|m| matches!(m.role, Role::User))
                .enumerate()
                .map(|(i, m)| OutlineEntry {
                    n: i + 1,
                    preview: clip_chars(&m.text, 80),
                    characters: m.text.chars().count(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_session_file_loads_without_a_workspace() {
        // A file written before workspaces existed must still load; the field
        // defaults to empty, which summaries resolve to the workspace in use.
        let json = r#"{"id":"s1","title":"t","created_at":1,"updated_at":2,"messages":[]}"#;
        let record: SessionRecord = serde_json::from_str(json).expect("legacy file loads");
        assert!(record.workspace.is_empty());
        assert!(SessionRecord::new().workspace.is_empty());
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let store = SessionStore::new(&dir);

        let mut record = SessionRecord::new();
        record.title = "hello".into();
        record.messages.push(ChatMessage::user("hi there"));
        let mut assistant = ChatMessage::assistant();
        assistant.text = "assistant reply".into();
        assistant.reasoning = "thinking".into();
        assistant.tools.push(ToolEvent {
            name: "search".into(),
            args: "{\"q\":1}".into(),
            output: Some("{\"hits\":[]}".into()),
            ms: Some(42),
        });
        record.messages.push(assistant);

        store.save(&record).expect("save");
        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, record.id);
        assert_eq!(loaded[0].title, "hello");
        assert_eq!(loaded[0].messages.len(), 2);
        assert_eq!(loaded[0].messages[0].role, Role::User);
        assert_eq!(loaded[0].messages[1].reasoning, "thinking");
        assert_eq!(loaded[0].messages[1].tools[0].name, "search");

        store.delete(&record.id);
        assert!(store.load_all().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncate_from_drops_tail_and_keeps_prefix() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("one"));
        record.messages.push(ChatMessage::assistant());
        record.messages.push(ChatMessage::user("two"));
        record.messages.push(ChatMessage::assistant());

        // Edit-and-resend at index 2 keeps the first exchange only.
        let left = record.truncate_from(2).expect("in range");
        assert_eq!(left, 2);
        assert_eq!(record.messages.len(), 2);
        assert_eq!(record.messages[0].text, "one");
        assert_eq!(record.messages[1].role, Role::Assistant);

        // Index == len is a no-op truncation (plain send path).
        let left = record.truncate_from(2).expect("exact len");
        assert_eq!(left, 2);

        // "Delete from here" on the first message empties the transcript.
        let left = record.truncate_from(0).expect("zero");
        assert_eq!(left, 0);
        assert!(record.messages.is_empty());
    }

    #[test]
    fn truncate_from_rejects_out_of_range() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("only"));
        let before = record.messages.len();
        let err = record.truncate_from(3).expect_err("out of range");
        assert!(err.contains("out of range"), "unexpected error: {err}");
        // A rejected truncation must not touch the history.
        assert_eq!(record.messages.len(), before);
    }

    #[test]
    fn load_skips_corrupt_files() {
        let dir = tmp_dir("corrupt");
        let store = SessionStore::new(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bad.json"), b"{not json").unwrap();

        let mut good = SessionRecord::new();
        good.title = "good".into();
        store.save(&good).unwrap();

        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title, "good");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn derive_title_truncates() {
        let mut record = SessionRecord::new();
        record
            .messages
            .push(ChatMessage::user("一二三四五六七八九十".repeat(10)));
        record.derive_title();
        assert!(record.title.chars().count() <= 33);
        assert!(record.title.ends_with('…'));
    }

    #[test]
    fn title_from_empty_conversation_stays_default() {
        let mut record = SessionRecord::new();
        record.derive_title();
        assert_eq!(record.title, "New chat");
    }

    /// The plan persists with the session, and older files written before
    /// the field existed still load with an empty plan.
    #[test]
    fn plan_roundtrips_and_legacy_files_load() {
        use agent::tools::PlanStatus;
        let dir = tmp_dir("plan");
        let store = SessionStore::new(&dir);

        let mut record = SessionRecord::new();
        record.plan.push(PlanItem {
            text: "wire the plan".into(),
            status: PlanStatus::InProgress,
        });
        store.save(&record).expect("save");
        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].plan.len(), 1);
        assert_eq!(loaded[0].plan[0].text, "wire the plan");
        assert_eq!(loaded[0].plan[0].status, PlanStatus::InProgress);
        store.delete(&record.id);

        // Legacy record: written before `plan` existed.
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("legacy.json"),
            br#"{"id":"legacy","title":"old","created_at":1,"updated_at":2,"messages":[]}"#,
        )
        .unwrap();
        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].plan.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fmt_utc_formats_epoch_and_known_timestamp() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00");
        assert_eq!(fmt_utc(1_700_000_000), "2023-11-14 22:13");
    }

    #[test]
    fn slugify_is_filesystem_safe() {
        assert_eq!(slugify("Hello, World! 42"), "hello-world-42");
        assert_eq!(slugify("   "), "chat");
        assert_eq!(slugify("..."), "chat");
        assert!(slugify(&"a very long title ".repeat(20)).chars().count() <= 48);
    }

    /// The Markdown export carries the title, plan, and every transcript part:
    /// user/assistant text, reasoning, tool calls, and errors.
    #[test]
    fn to_markdown_renders_plan_and_transcript() {
        use agent::tools::PlanStatus;
        let mut record = SessionRecord::new();
        record.title = "Export me".into();
        record.plan.push(PlanItem {
            text: "ship it".into(),
            status: PlanStatus::Completed,
        });
        record.plan.push(PlanItem {
            text: "polish".into(),
            status: PlanStatus::Pending,
        });
        record.messages.push(ChatMessage::user("do the thing"));
        let mut assistant = ChatMessage::assistant();
        assistant.reasoning = "thinking hard".into();
        assistant.tools.push(ToolEvent {
            name: "bash".into(),
            args: "{\"cmd\":\"ls\"}".into(),
            output: None,
            ms: None,
        });
        assistant.text = "done ✓".into();
        assistant.error = None;
        record.messages.push(assistant);
        let mut failed = ChatMessage::assistant();
        failed.error = Some("boom".into());
        record.messages.push(failed);

        let md = record.to_markdown();
        assert!(md.starts_with("# Export me"));
        assert!(md.contains("- [x] ship it"));
        assert!(md.contains("- [ ] polish"));
        assert!(md.contains("### User"));
        assert!(md.contains("do the thing"));
        assert!(md.contains("> thinking hard"));
        assert!(md.contains("- tool `bash` `{\"cmd\":\"ls\"}`"));
        assert!(md.contains("done ✓"));
        assert!(md.contains("**Error:** boom"));
        // Blank streaming placeholders never appear.
        let mut empty = SessionRecord::new();
        empty.messages.push(ChatMessage::assistant());
        assert!(empty.to_markdown().contains("_No messages yet._"));
    }

    /// Export writes `<slug>-<id8>.md` beside `sessions/` and re-exporting the
    /// same session overwrites the same file.
    #[test]
    fn export_writes_markdown_next_to_sessions_and_overwrites() {
        let dir = tmp_dir("export");
        let store = SessionStore::new(&dir);

        let mut record = SessionRecord::new();
        record.title = "My Chat".into();
        record.messages.push(ChatMessage::user("hi"));
        store.save(&record).unwrap();

        let path = store.export(&record).expect("export");
        assert_eq!(path.parent(), Some(store.export_dir().as_path()));
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("my-chat-"));
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("# My Chat"));

        record.messages.push(ChatMessage::user("again"));
        let again = store.export(&record).unwrap();
        assert_eq!(again, path);
        let second = fs::read_to_string(&path).unwrap();
        assert!(second.len() > first.len());

        // load-by-id returns the saved record, None for unknown ids.
        let loaded = store.load(&record.id).expect("load");
        assert_eq!(loaded.title, "My Chat");
        assert!(store.load("missing-id").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fork_clones_the_prefix_into_a_fresh_record() {
        let mut record = SessionRecord::new();
        record.title = "Base chat".to_string();
        record.messages.push(ChatMessage::user("q1"));
        record.messages.push(ChatMessage::assistant());
        record.messages.push(ChatMessage::user("q2"));
        record.messages.push(ChatMessage::assistant());
        record.plan.push(PlanItem {
            text: "step".to_string(),
            status: agent::tools::PlanStatus::Pending,
        });

        // Keeps messages[..index], carries the plan, new identity/title.
        let fork = record.fork_from(2).expect("fork at index 2");
        assert_ne!(fork.id, record.id);
        assert_eq!(fork.title, "Base chat (branch)");
        assert_eq!(fork.messages.len(), 2);
        assert_eq!(fork.messages[0].text, "q1");
        assert_eq!(fork.plan.len(), 1);
        assert!(record.messages.len() == 4, "source must be untouched");

        // Degenerate and out-of-range indices are refused.
        assert!(record.fork_from(0).is_err());
        assert!(record.fork_from(5).is_err());

        // The fork is a normal record: it survives its own store round-trip
        // and does not evict the source chat.
        let dir = tmp_dir("fork-roundtrip");
        let store = SessionStore::new(&dir);
        store.save(&record).expect("save source");
        store.save(&fork).expect("save fork");
        let mut all = store.load_all();
        all.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        assert_eq!(all.len(), 2);
        let reloaded = store.load(&fork.id).expect("load fork");
        assert_eq!(reloaded.messages.len(), 2);
        assert_eq!(reloaded.title, "Base chat (branch)");
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn deleting_a_session_moves_it_to_the_trash() {
        let dir = tmp_dir("trash");
        let store = SessionStore::new(&dir);
        let mut record = SessionRecord::new();
        record.title = "keep me maybe".to_string();
        store.save(&record).expect("saved");
        assert_eq!(store.load_all().len(), 1, "the store starts with it");

        store.delete(&record.id);
        assert!(store.load_all().is_empty(), "it left the store");
        let trashed = store.trashed();
        assert_eq!(trashed.len(), 1, "but it is in the trash");
        assert_eq!(trashed[0].title, "keep me maybe", "with its title intact");

        let title = store.restore(&record.id).expect("restore");
        assert_eq!(title, "keep me maybe");
        assert!(store.trashed().is_empty(), "the trash is empty again");
        assert_eq!(store.load_all().len(), 1, "and it is back in the store");
    }

    #[test]
    fn trashed_sessions_stay_out_of_the_store_and_its_search() {
        let dir = tmp_dir("trash-search");
        let store = SessionStore::new(&dir);
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("findable phrase"));
        store.save(&record).expect("saved");
        store.search("findable", 10);
        store.delete(&record.id);
        assert!(
            store.search("findable", 10).is_empty(),
            "a deleted chat must not come back through search"
        );
        assert_eq!(store.load_all().len(), 0);
    }

    #[test]
    fn purge_is_the_only_permanent_removal() {
        let dir = tmp_dir("purge");
        let store = SessionStore::new(&dir);
        let record = SessionRecord::new();
        store.save(&record).expect("saved");
        store.delete(&record.id);
        assert_eq!(store.purge_trash(), 1, "one session purged");
        assert!(store.trashed().is_empty());
        assert!(
            store.restore(&record.id).is_err(),
            "a purged chat cannot be restored"
        );
        assert_eq!(store.purge_trash(), 0, "purging an empty trash is a no-op");
    }

    #[test]
    fn restoring_something_that_was_never_trashed_says_so() {
        let dir = tmp_dir("restore-missing");
        let store = SessionStore::new(&dir);
        let err = store.restore("no-such-id").expect_err("must refuse");
        assert!(err.contains("not in the trash"), "{err}");
    }
    #[test]
    fn overview_counts_every_turn_including_folded_ones() {
        let mut record = SessionRecord::new();
        record.messages.push(ChatMessage::user("first ask"));
        let mut answer = ChatMessage::assistant();
        answer.reasoning = "thinking".to_string();
        answer.tools.push(ToolEvent {
            name: "search".into(),
            args: "{}".into(),
            output: None,
            ms: Some(3),
        });
        record.messages.push(answer);
        let mut failed = ChatMessage::assistant();
        failed.text = "failed reply".to_string();
        failed.error = Some("boom".to_string());
        record.messages.push(failed);
        record.archived.push(ChatMessage::user("folded ask"));

        let o = record.overview();
        assert_eq!(o.messages, 4, "the folded turn counts as a message");
        assert_eq!((o.asks, o.replies), (2, 2));
        assert_eq!(o.tool_calls, 1);
        assert_eq!(o.errors, 1);
        assert_eq!(o.archived, 1);
        assert_eq!(
            o.characters,
            9 + 12 + 10,
            "visible text only: the empty reply adds nothing"
        );
        assert_eq!(o.reasoning_characters, 8, "reasoning is counted apart");
        assert_eq!(o.outline.len(), 2, "one entry per ask");
        assert_eq!(
            (o.outline[0].n, o.outline[0].preview.as_str()),
            (1, "first ask")
        );
        assert_eq!(o.outline[1].n, 2);
    }

    #[test]
    fn overview_of_an_empty_chat_is_all_zeroes() {
        let o = SessionRecord::new().overview();
        assert_eq!(
            (o.messages, o.asks, o.replies, o.tool_calls, o.errors),
            (0, 0, 0, 0, 0)
        );
        assert_eq!(
            (o.characters, o.reasoning_characters, o.archived),
            (0, 0, 0)
        );
        assert!(o.outline.is_empty());
    }

    #[test]
    fn outline_previews_are_clipped_by_characters_not_bytes() {
        let mut record = SessionRecord::new();
        let long = "测".repeat(100);
        record.messages.push(ChatMessage::user(long.clone()));
        let o = record.overview();
        assert_eq!(o.outline[0].characters, 100, "the full ask is counted");
        assert_eq!(
            o.outline[0].preview.chars().count(),
            81,
            "80 characters plus the ellipsis"
        );
        assert!(o.outline[0].preview.ends_with('…'));
        assert!(long.starts_with(&o.outline[0].preview[..o.outline[0].preview.len() - 3]));
    }

    #[test]
    fn a_single_cjk_character_still_searches() {
        // One CJK character has no bigram, so the index has no opinion and the
        // scan has to answer. That fallback is the reason such a query works.
        let dir = std::env::temp_dir().join(format!(
            "bos-gui-test-{}-{}",
            "one-cjk-char",
            std::process::id()
        ));
        let store = SessionStore::new(&dir);
        let mut record = SessionRecord::new();
        record.id = "cjk".to_string();
        record.title = "窗口".to_string();
        record.messages.push(ChatMessage::user("会话窗口的滚动条"));
        store.save(&record).expect("save");
        assert_eq!(
            store.search("滚", 10).len(),
            1,
            "one character still finds it"
        );
        assert_eq!(store.search("滚动", 10).len(), 1, "two characters too");
        assert!(
            store.search("滚x", 10).is_empty(),
            "and a miss stays a miss"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_keeps_answering_the_same_way() {
        // Build a store, search, then add and rewrite sessions and search again:
        // the answers must be the scan's answers at every step, because the
        // index is only allowed to skip work.
        let dir = std::env::temp_dir().join(format!(
            "bos-gui-test-{}-{}",
            "index-parity",
            std::process::id()
        ));
        let store = SessionStore::new(&dir);
        for (id, title, body) in [
            ("a", "Deploy Notes", "How do I ship the GUI?"),
            ("b", "长会话窗口", "滚动条与 spacer"),
            ("c", "Mixed", "Cargo Build Insensitive"),
        ] {
            let mut record = SessionRecord::new();
            record.id = id.to_string();
            record.title = title.to_string();
            record.messages.push(ChatMessage::user(body));
            record.updated_at = 1_700_000_000 + id.len() as u64;
            store.save(&record).expect("save");
        }
        for needle in ["ship", "SHIP", "滚动", "spacer", "cargo build", "absent"] {
            let hits = store.search(needle, 10);
            assert_eq!(
                hits.len(),
                if needle == "absent" { 0 } else { 1 },
                "query {needle:?} through the index"
            );
        }
        // Rewrite one session: the index must notice and stop matching the old
        // text, which is the one failure mode that would hide a result.
        let mut rewritten = SessionRecord::new();
        rewritten.id = "a".to_string();
        rewritten.title = "Deploy Notes".to_string();
        rewritten
            .messages
            .push(ChatMessage::user("nothing about that verb"));
        store.save(&rewritten).expect("save");
        let _ = store.search("ship", 10);
        assert!(
            store.search("ship", 10).is_empty(),
            "the rewritten session stops matching"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a query costs on the real `search` path, at several corpus sizes.
    ///
    /// Timing is printed, never asserted: a timing assertion is a flaky test.
    /// The `simplified-scan` row is **not** the shipped path — it skips
    /// `hit_for` and therefore skips snippet extraction, which is the part that
    /// grows with message size. It is kept only because round 76 compared an
    /// index against exactly that row and drew a conclusion the comparison
    /// could not support; printing all three together makes the difference
    /// visible instead of argued.
    #[test]
    fn search_cost_is_reported_for_the_record() {
        for repeat in [6usize, 32, 128, 512] {
            let dir = std::env::temp_dir().join(format!(
                "bos-gui-test-cost-{}-{}",
                repeat,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let store = SessionStore::new(&dir);
            let body = "the quick brown fox jumps over the lazy dog ".repeat(repeat);
            for i in 0..300 {
                let mut record = SessionRecord::new();
                record.id = format!("cost-{i}");
                record.title = format!("session {i}");
                record.messages.push(ChatMessage::user(&body));
                store.save(&record).expect("save");
            }
            let _ = store.search("brown", 10); // warm the parse cache
            let hit = std::time::Instant::now();
            let hits = store.search("brown", 10);
            let hit = hit.elapsed();
            let miss = std::time::Instant::now();
            let misses = store.search("kangaroo", 10);
            let miss = miss.elapsed();
            let scan = std::time::Instant::now();
            let seen = store
                .cached_sessions()
                .iter()
                .filter(|s| {
                    s.record.title.to_lowercase().contains("brown")
                        || s.lower.iter().any(|(t, _)| t.contains("brown"))
                })
                .count();
            let scan = scan.elapsed();
            assert_eq!(hits.len(), 10, "the limit is honoured");
            assert!(misses.is_empty(), "and the miss is a miss");
            assert!(seen >= 10, "the simplified scan agrees matches exist");
            println!(
                "[search-cost] body={:>6} real-hit={:>11?} real-miss={:>11?} simplified-scan={:>11?}",
                body.len(),
                hit,
                miss,
                scan
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
