//! Chat session model and JSON persistence under `~/.bos/gui/sessions/`.

use std::fs;
use std::path::PathBuf;
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
    /// The conversation, oldest first.
    pub(crate) messages: Vec<ChatMessage>,
    /// Working-plan snapshot from the agent's most recent turn, so the plan
    /// panel survives restarts (older session files simply have none).
    #[serde(default)]
    pub(crate) plan: Vec<PlanItem>,
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
            messages: Vec::new(),
            plan: Vec::new(),
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
            messages: self.messages[..index].to_vec(),
            plan: self.plan.clone(),
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

/// Directory-backed store of session JSON files.
#[derive(Debug, Clone)]
pub(crate) struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    /// The conventional per-user location: `~/.bos/gui/sessions`.
    pub(crate) fn default_dir() -> PathBuf {
        PathBuf::from(shellexpand::tilde("~/.bos/gui/sessions").into_owned())
    }

    /// Create a store rooted at an explicit directory (used by tests).
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
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
    pub(crate) fn delete(&self, id: &str) {
        let _ = fs::remove_file(self.path_for(id));
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
