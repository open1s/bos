//! Chat session model and JSON persistence under `~/.bos/gui/sessions/`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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
        }
    }

    /// Bump the modification timestamp.
    pub(crate) fn touch(&mut self) {
        self.updated_at = unix_now();
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
}

/// Current time in unix seconds.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
}
