//! A durable, curated knowledge base the agent can query.
//!
//! Memory is what the agent *cannot help* recalling: short notes ranked by
//! keyword overlap and appended to the system prompt on every run. A knowledge
//! entry is the opposite — a named, curated note with a body and tags that is
//! **never** injected into the prompt. The agent reaches for it through the
//! `knowledge_search` tool registered here ([`tool`]), so a large, precise
//! corpus costs context only when it is asked for, and the user curates it in
//! one place.
//!
//! The store is durable (`~/.bos/gui/knowledge.json`, written atomically) and
//! every command takes `now_ms` from the caller, so nothing here reads the wall
//! clock and every behavior below is testable without waiting.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use agent::tools::{Tool, ToolError};

/// Most entries one store keeps. Past this, `add` is refused rather than
/// silently dropping the oldest: a note the user cannot see is worse than an
/// error that names the cap.
pub(crate) const MAX_ENTRIES: usize = 500;

/// Longest title, in characters.
pub(crate) const MAX_TITLE_CHARS: usize = 120;

/// Longest body, in characters.
pub(crate) const MAX_BODY_CHARS: usize = 4000;

/// Most tags on one entry.
pub(crate) const MAX_TAGS: usize = 8;

/// Longest single tag, in characters.
pub(crate) const MAX_TAG_CHARS: usize = 32;

/// Characters of an entry's body the *tool* hands the model. The pane shows the
/// whole body because it is the user's own text; a tool result is context, so
/// one entry cannot swallow the window, and truncation is stated rather than
/// silent.
pub(crate) const TOOL_BODY_CHARS: usize = 1200;

/// Matches the tool returns when the model does not ask for a count.
pub(crate) const TOOL_DEFAULT_LIMIT: usize = 5;

/// Most matches one tool call may return.
pub(crate) const TOOL_MAX_LIMIT: usize = 20;

/// The name the model sees.
pub(crate) const TOOL_NAME: &str = "knowledge_search";

/// Query token count past which extra words are ignored, so ranking stays
/// bounded no matter how long the question is.
const MAX_QUERY_TOKENS: usize = 8;

/// The shape version of `knowledge.json`.
const FILE_VERSION: u32 = 1;

/// One curated note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    /// Stable id, unique inside the store.
    pub(crate) id: String,
    /// Short name, the first thing a search matches and the first thing shown.
    pub(crate) title: String,
    /// Search keys, already normalized (lowercase, no blanks, deduplicated).
    pub(crate) tags: Vec<String>,
    /// The note itself.
    pub(crate) body: String,
    /// Epoch milliseconds the entry was written.
    pub(crate) created_ms: u64,
    /// Epoch milliseconds of the most recent edit.
    pub(crate) updated_ms: u64,
}

impl Entry {
    /// Whether this entry is still the shape the store accepts. A file edited
    /// by hand can hold anything, and an entry the store would refuse to create
    /// is not one it should serve.
    fn is_valid(&self) -> bool {
        !self.id.trim().is_empty()
            && !self.title.trim().is_empty()
            && self.title.chars().count() <= MAX_TITLE_CHARS
            && self.body.chars().count() <= MAX_BODY_CHARS
            && self.tags.len() <= MAX_TAGS
            && self
                .tags
                .iter()
                .all(|t| !t.trim().is_empty() && t.chars().count() <= MAX_TAG_CHARS)
    }
}

/// The bounds in force, so a surface that enforces them can say so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Caps {
    /// [`MAX_ENTRIES`].
    pub(crate) max_entries: usize,
    /// [`MAX_TITLE_CHARS`].
    pub(crate) max_title_chars: usize,
    /// [`MAX_BODY_CHARS`].
    pub(crate) max_body_chars: usize,
    /// [`MAX_TAGS`].
    pub(crate) max_tags: usize,
    /// [`MAX_TAG_CHARS`].
    pub(crate) max_tag_chars: usize,
    /// [`TOOL_BODY_CHARS`].
    pub(crate) tool_body_chars: usize,
    /// [`TOOL_MAX_LIMIT`].
    pub(crate) tool_max_limit: usize,
}

impl Caps {
    /// The bounds this build enforces.
    pub(crate) fn current() -> Self {
        Self {
            max_entries: MAX_ENTRIES,
            max_title_chars: MAX_TITLE_CHARS,
            max_body_chars: MAX_BODY_CHARS,
            max_tags: MAX_TAGS,
            max_tag_chars: MAX_TAG_CHARS,
            tool_body_chars: TOOL_BODY_CHARS,
            tool_max_limit: TOOL_MAX_LIMIT,
        }
    }
}

/// What a knowledge command answers with: the entries it selected, how many
/// exist, and the bounds — the same shape for list, search, add, update and
/// remove, so the pane redraws from one function.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Report {
    /// The caller's clock, echoed back.
    pub(crate) now_ms: u64,
    /// The query the matches answer, empty for a list.
    pub(crate) query: String,
    /// Entries in the store, whatever the query matched.
    pub(crate) total: usize,
    /// The selected entries, best first.
    pub(crate) matches: Vec<Entry>,
    /// The bounds in force.
    pub(crate) caps: Caps,
}

/// The on-disk shape.
#[derive(Debug, Default, Deserialize, Serialize)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
    #[serde(default)]
    seq: u64,
}

/// The durable store behind the Knowledge pane and the `knowledge_search` tool.
pub(crate) struct Store {
    path: PathBuf,
    file: File,
}

impl Store {
    /// The store the app opens: `~/.bos/gui/knowledge.json`.
    pub(crate) fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".bos")
            .join("gui")
            .join("knowledge.json")
    }

    /// Open the default store, tolerating anything a file can be.
    pub(crate) fn open() -> Self {
        Self::load(&Self::default_path())
    }

    /// Load `path`. A missing or unreadable file is an empty store rather than
    /// a failure to start, and entries that no longer validate are dropped on
    /// the way in — a hand-edited file must not be able to make every command
    /// fail.
    pub(crate) fn load(path: &Path) -> Self {
        let mut file: File = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        file.version = FILE_VERSION;
        file.entries.retain(Entry::is_valid);
        file.entries.truncate(MAX_ENTRIES);
        Self {
            path: path.to_path_buf(),
            file,
        }
    }

    /// Write the store durably: a temp file in the same directory, then a
    /// rename, so a crash mid-write cannot truncate the notes.
    pub(crate) fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&self.file).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Entries in the store.
    pub(crate) fn len(&self) -> usize {
        self.file.entries.len()
    }

    /// Add an entry. `tags` arrives as the user typed it (comma-separated).
    pub(crate) fn add(
        &mut self,
        title: &str,
        tags: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<Entry, String> {
        if self.file.entries.len() >= MAX_ENTRIES {
            return Err(format!(
                "at the cap of {MAX_ENTRIES} entries; delete one first"
            ));
        }
        let entry = Entry {
            id: String::new(),
            title: clean_title(title)?,
            tags: parse_tags(tags)?,
            body: clean_body(body)?,
            created_ms: now_ms,
            updated_ms: now_ms,
        };
        self.file.seq += 1;
        let entry = Entry {
            id: format!("kb-{now_ms}-{}", self.file.seq),
            ..entry
        };
        self.file.entries.push(entry.clone());
        Ok(entry)
    }

    /// Replace an entry's title, tags and body, keeping the id and the time it
    /// was first written.
    pub(crate) fn update(
        &mut self,
        id: &str,
        title: &str,
        tags: &str,
        body: &str,
        now_ms: u64,
    ) -> Result<Entry, String> {
        let entry = self
            .file
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("no knowledge entry with id {id}"))?;
        let title = clean_title(title)?;
        let tags = parse_tags(tags)?;
        let body = clean_body(body)?;
        entry.title = title;
        entry.tags = tags;
        entry.body = body;
        entry.updated_ms = now_ms;
        Ok(entry.clone())
    }

    /// Remove an entry, returning what was removed.
    pub(crate) fn remove(&mut self, id: &str) -> Result<Entry, String> {
        let at = self
            .file
            .entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| format!("no knowledge entry with id {id}"))?;
        Ok(self.file.entries.remove(at))
    }

    /// The entries a query selects, best first.
    ///
    /// Ranking is deliberate and stated so it can be argued with: a query token
    /// scores 4 in the title, 3 in the tags and 1 in the body, a **substring**
    /// match counts (so `start` finds `restart` — recall over precision, since
    /// the corpus is curated), ties go to the most recently edited and then to
    /// the id, and an empty query is a list of everything, newest first.
    /// `limit` is what the caller asked for; `None` is everything.
    pub(crate) fn matches(&self, query: &str, limit: Option<usize>) -> Vec<Entry> {
        let tokens = query_tokens(query);
        let mut hits: Vec<(u32, &Entry)> = self
            .file
            .entries
            .iter()
            .filter_map(|entry| {
                let score = score(entry, &tokens);
                (tokens.is_empty() || score > 0).then_some((score, entry))
            })
            .collect();
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.updated_ms.cmp(&a.1.updated_ms))
                .then(a.1.id.cmp(&b.1.id))
        });
        let take = limit.unwrap_or(hits.len()).min(hits.len());
        hits.into_iter()
            .take(take)
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// What a command answers with.
    pub(crate) fn report(&self, query: &str, limit: Option<usize>, now_ms: u64) -> Report {
        Report {
            now_ms,
            query: query.to_string(),
            total: self.file.entries.len(),
            matches: self.matches(query, limit),
            caps: Caps::current(),
        }
    }
}

/// A title is trimmed and required, and it has a ceiling.
fn clean_title(title: &str) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("a knowledge entry needs a title".to_string());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("a title is at most {MAX_TITLE_CHARS} characters"));
    }
    Ok(title.to_string())
}

/// A body is trimmed and bounded; an empty one is allowed, because a title and
/// its tags are already a usable index entry.
fn clean_body(body: &str) -> Result<String, String> {
    let body = body.trim();
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(format!("a body is at most {MAX_BODY_CHARS} characters"));
    }
    Ok(body.to_string())
}

/// Comma-separated tags become lowercase keys: trimmed, blanks dropped,
/// duplicates dropped, order kept. A list or a tag past the caps is refused
/// rather than silently shortened.
fn parse_tags(raw: &str) -> Result<Vec<String>, String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in raw.split(',') {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() || tags.contains(&tag) {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            return Err(format!("a tag is at most {MAX_TAG_CHARS} characters"));
        }
        tags.push(tag);
    }
    if tags.len() > MAX_TAGS {
        return Err(format!("at most {MAX_TAGS} tags per entry"));
    }
    Ok(tags)
}

/// A query's search tokens: lowercased, split on anything that is not a word
/// character, deduplicated, and capped so ranking stays bounded.
fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '/')
    {
        if token.is_empty() || tokens.iter().any(|t| t == token) {
            continue;
        }
        if tokens.len() >= MAX_QUERY_TOKENS {
            break;
        }
        tokens.push(token.to_string());
    }
    tokens
}

/// How well `entry` answers the query's tokens: title 4, tags 3, body 1.
fn score(entry: &Entry, tokens: &[String]) -> u32 {
    if tokens.is_empty() {
        return 0;
    }
    let title = entry.title.to_lowercase();
    let tags = entry.tags.join(" ");
    let body = entry.body.to_lowercase();
    let mut score = 0;
    for token in tokens {
        if title.contains(token.as_str()) {
            score += 4;
        }
        if tags.contains(token.as_str()) {
            score += 3;
        }
        if body.contains(token.as_str()) {
            score += 1;
        }
    }
    score
}

/// `text` limited to `max` characters, saying whether anything was dropped.
fn clamp(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let mut kept: String = text.chars().take(max.saturating_sub(1)).collect();
    kept.push('…');
    (kept, true)
}

/// The `knowledge_search` tool: the store, as the model sees it.
struct KnowledgeTool {
    store: Arc<Mutex<Store>>,
}

impl Tool for KnowledgeTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn description(&self) -> String {
        "Search this workspace's curated knowledge base — named notes with tags \
         kept by the user, which are not part of the prompt. Use it before \
         answering anything about this project's conventions, decisions or \
         setup. Pass \"*\" as the query to list the newest entries."
            .to_string()
    }

    fn json_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Words to look for; title hits rank above tag \
                                    hits, which rank above body hits. \"*\" lists \
                                    the newest entries."
                },
                "limit": {
                    "type": "integer",
                    "description": "Most entries to return.",
                    "minimum": 1,
                    "maximum": TOOL_MAX_LIMIT
                }
            },
            "required": ["query"]
        })
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let query = input
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return Err(ToolError::InvalidInput(
                "knowledge_search needs a query (\".*\" lists the newest entries)".to_string(),
            ));
        }
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(TOOL_DEFAULT_LIMIT as u64)
            .clamp(1, TOOL_MAX_LIMIT as u64) as usize;
        let store = self
            .store
            .lock()
            .map_err(|_| ToolError::Failed("the knowledge store lock is poisoned".to_string()))?;
        let list_all = query == "*";
        let hits = store.matches(if list_all { "" } else { query }, Some(limit));
        let total = store.len();
        let truncated = hits
            .iter()
            .any(|e| e.body.chars().count() > TOOL_BODY_CHARS);
        let matches: Vec<Value> = hits
            .iter()
            .map(|entry| {
                let (body, clamped) = clamp(&entry.body, TOOL_BODY_CHARS);
                json!({
                    "id": entry.id,
                    "title": entry.title,
                    "tags": entry.tags,
                    "body": body,
                    "truncated": clamped,
                    "updated_ms": entry.updated_ms,
                })
            })
            .collect();
        let note = if matches.is_empty() {
            format!("no knowledge entry matches \"{query}\"; the store holds {total}")
        } else if list_all {
            format!("newest {} of {total} knowledge entries", matches.len())
        } else {
            format!("{} of {total} knowledge entries match", matches.len())
        };
        Ok(json!({
            "query": query,
            "count": matches.len(),
            "total": total,
            "matches": matches,
            "truncated": truncated,
            "note": note,
        }))
    }
}

/// The knowledge tool backed by `store`.
pub(crate) fn tool(store: Arc<Mutex<Store>>) -> Arc<dyn Tool> {
    Arc::new(KnowledgeTool { store })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store on a fresh temp path, with the file removed first.
    fn store(name: &str) -> Store {
        let path = std::env::temp_dir().join(format!("bos-kb-{name}.json"));
        let _ = std::fs::remove_file(&path);
        Store::load(&path)
    }

    #[test]
    fn a_knowledge_entry_needs_a_title() {
        let mut store = store("needs-title");
        assert!(store.add("   ", "", "body", 1).is_err());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn a_blank_title_cannot_be_saved_over_a_good_one() {
        let mut store = store("blank-update");
        let entry = store.add("Keep me", "tag", "body", 1).unwrap();
        assert!(store.update(&entry.id, "", "tag", "body", 2).is_err());
        assert_eq!(store.matches("", None)[0].title, "Keep me");
    }

    #[test]
    fn the_title_and_body_are_bounded() {
        let mut store = store("bounded");
        let long_title = "t".repeat(MAX_TITLE_CHARS + 1);
        let long_body = "b".repeat(MAX_BODY_CHARS + 1);
        assert!(store.add(&long_title, "", "body", 1).is_err());
        assert!(store.add("title", "", &long_body, 1).is_err());
        assert!(store
            .add(
                &"t".repeat(MAX_TITLE_CHARS),
                "",
                &"b".repeat(MAX_BODY_CHARS),
                1
            )
            .is_ok());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn tags_are_normalised_deduplicated_and_capped() {
        assert_eq!(
            parse_tags(" Rust, rust ,, GUI , rust ").unwrap(),
            vec!["rust".to_string(), "gui".to_string()]
        );
        assert!(parse_tags("a,b,c,d,e,f,g,h,i").is_err());
        assert!(parse_tags(&"x".repeat(MAX_TAG_CHARS + 1)).is_err());
        assert!(parse_tags("").unwrap().is_empty());
    }

    #[test]
    fn the_store_refuses_an_entry_past_its_cap() {
        let mut store = store("cap");
        for i in 0..MAX_ENTRIES {
            store.add(&format!("entry {i}"), "", "body", 1).unwrap();
        }
        let err = store.add("one too many", "", "body", 1).unwrap_err();
        assert!(err.contains(&MAX_ENTRIES.to_string()), "{err}");
        assert_eq!(store.len(), MAX_ENTRIES);
    }

    #[test]
    fn a_title_hit_outranks_a_tag_hit_which_outranks_a_body_hit() {
        let mut store = store("ranking");
        store.add("plain", "", "mentions restart once", 1).unwrap();
        store.add("about restart", "", "nothing else", 2).unwrap();
        store
            .add("unrelated", "restart", "nothing else", 3)
            .unwrap();
        let titles: Vec<String> = store
            .matches("restart", None)
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(titles, vec!["about restart", "unrelated", "plain"]);
    }

    #[test]
    fn matching_is_case_insensitive_and_a_substring() {
        let mut store = store("substring");
        store
            .add("The restart ritual", "", "run make again", 1)
            .unwrap();
        assert_eq!(store.matches("RESTART", None).len(), 1);
        assert_eq!(store.matches("start", None).len(), 1);
        assert_eq!(store.matches("missing", None).len(), 0);
    }

    #[test]
    fn ties_break_toward_the_most_recently_edited() {
        let mut store = store("ties");
        let older = store.add("same words", "", "body", 1).unwrap();
        store.add("same words", "", "body", 2).unwrap();
        let hits = store.matches("words", None);
        assert_eq!(hits[0].created_ms, 2);
        store
            .update(&older.id, "same words", "", "body", 9)
            .unwrap();
        assert_eq!(store.matches("words", None)[0].created_ms, 1);
    }

    #[test]
    fn an_empty_query_lists_the_newest_first_and_a_limit_is_honoured() {
        let mut store = store("list");
        store.add("first", "", "body", 1).unwrap();
        store.add("second", "", "body", 2).unwrap();
        store.add("third", "", "body", 3).unwrap();
        let all: Vec<String> = store
            .matches("", None)
            .iter()
            .map(|e| e.title.clone())
            .collect();
        assert_eq!(all, vec!["third", "second", "first"]);
        assert_eq!(store.matches("", Some(2)).len(), 2);
        assert_eq!(store.matches("body", Some(1)).len(), 1);
    }

    #[test]
    fn an_entry_survives_a_restart_and_keeps_when_it_was_written() {
        let path = std::env::temp_dir().join("bos-kb-restart.json");
        let _ = std::fs::remove_file(&path);
        let mut store = Store::load(&path);
        let entry = store.add("Durable", "tag", "body", 7).unwrap();
        store.save().unwrap();
        let mut reloaded = Store::load(&path);
        assert_eq!(reloaded.matches("durable", None).len(), 1);
        let updated = reloaded
            .update(&entry.id, "Durable", "tag", "edited", 8)
            .unwrap();
        assert_eq!(updated.created_ms, 7);
        assert_eq!(updated.updated_ms, 8);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_or_corrupt_file_is_an_empty_store() {
        let missing = store("missing");
        assert_eq!(missing.len(), 0);
        let path = std::env::temp_dir().join("bos-kb-corrupt.json");
        std::fs::write(&path, "{ not json").unwrap();
        let corrupt = Store::load(&path);
        assert_eq!(corrupt.len(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_drops_entries_that_no_longer_validate() {
        let path = std::env::temp_dir().join("bos-kb-hand-edited.json");
        // Hermetic: this test asserts a *count*, so a file left behind by an
        // earlier run must not be part of the answer.
        let _ = std::fs::remove_file(&path);
        let mut store = Store::load(&path);
        store.add("good", "tag", "body", 1).unwrap();
        store.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut file: File = serde_json::from_str(&text).unwrap();
        file.entries.push(Entry {
            id: "kb-bad".to_string(),
            title: "  ".to_string(),
            tags: vec![],
            body: "no title".to_string(),
            created_ms: 0,
            updated_ms: 0,
        });
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(Store::load(&path).len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn touching_an_unknown_entry_says_so() {
        let mut store = store("unknown");
        assert!(store.remove("kb-nope").is_err());
        assert!(store.update("kb-nope", "t", "", "b", 1).is_err());
        let entry = store.add("real", "", "body", 1).unwrap();
        assert_eq!(store.remove(&entry.id).unwrap().title, "real");
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn the_report_carries_the_total_the_query_and_the_bounds() {
        let mut store = store("report");
        store.add("one", "alpha", "body", 1).unwrap();
        store.add("two", "", "body", 2).unwrap();
        let report = store.report("alpha", None, 42);
        assert_eq!(report.now_ms, 42);
        assert_eq!(report.query, "alpha");
        assert_eq!(report.total, 2);
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.caps.max_entries, MAX_ENTRIES);
        assert_eq!(report.caps.tool_body_chars, TOOL_BODY_CHARS);
        let listed = store.report("", None, 0);
        assert_eq!(listed.matches.len(), 2);
    }

    #[test]
    fn ids_are_unique_inside_one_millisecond() {
        let mut store = store("ids");
        let a = store.add("a", "", "body", 5).unwrap();
        let b = store.add("b", "", "body", 5).unwrap();
        assert_ne!(a.id, b.id);
    }

    /// The tool the agent gets, over a store holding one long and one short
    /// entry.
    fn tool_with(name: &str) -> (Arc<dyn Tool>, Arc<Mutex<Store>>) {
        let mut store = store(name);
        store
            .add("Deploying", "ops", &"x".repeat(TOOL_BODY_CHARS + 50), 1)
            .unwrap();
        store.add("Testing", "ci", "run the suite", 2).unwrap();
        let shared = Arc::new(Mutex::new(store));
        (tool(Arc::clone(&shared)), shared)
    }

    #[test]
    fn the_tool_is_named_and_schemad_for_a_query() {
        let (tool, _) = tool_with("tool-name");
        assert_eq!(tool.name(), TOOL_NAME);
        let schema = tool.json_schema();
        assert_eq!(schema["required"][0], "query");
        assert!(tool.description().contains("knowledge base"));
    }

    #[test]
    fn the_tool_finds_an_entry_and_says_what_it_dropped() {
        let (tool, _) = tool_with("tool-search");
        let out = tool.run(&json!({ "query": "deploying" })).unwrap();
        assert_eq!(out["count"], 1);
        assert_eq!(out["total"], 2);
        assert_eq!(out["matches"][0]["title"], "Deploying");
        assert_eq!(out["matches"][0]["tags"][0], "ops");
        assert_eq!(out["truncated"], true);
        assert_eq!(out["matches"][0]["truncated"], true);
        assert!(out["matches"][0]["body"].as_str().unwrap().chars().count() <= TOOL_BODY_CHARS);
        assert!(out["note"]
            .as_str()
            .unwrap()
            .contains("of 2 knowledge entries"));
    }

    #[test]
    fn the_tool_leaves_a_short_body_alone_and_defaults_its_limit() {
        let (tool, _) = tool_with("tool-short");
        let out = tool.run(&json!({ "query": "testing" })).unwrap();
        assert_eq!(out["matches"][0]["body"], "run the suite");
        assert_eq!(out["truncated"], false);
        assert_eq!(out["matches"][0]["truncated"], false);
        // No limit asked for: the newest five at most, here both.
        let all = tool.run(&json!({ "query": "*" })).unwrap();
        assert_eq!(all["count"], 2);
        assert_eq!(all["matches"][0]["title"], "Testing");
        assert!(all["note"].as_str().unwrap().contains("newest 2 of 2"));
    }

    #[test]
    fn the_tool_honours_a_limit_and_clamps_an_absurd_one() {
        let (tool, _) = tool_with("tool-limit");
        let one = tool.run(&json!({ "query": "*", "limit": 1 })).unwrap();
        assert_eq!(one["count"], 1);
        assert_eq!(one["matches"][0]["title"], "Testing");
        let huge = tool.run(&json!({ "query": "*", "limit": 9999 })).unwrap();
        assert_eq!(huge["count"], 2);
    }

    #[test]
    fn the_tool_refuses_a_query_that_says_nothing() {
        let (tool, _) = tool_with("tool-empty");
        assert!(tool.run(&json!({})).is_err());
        assert!(tool.run(&json!({ "query": "   " })).is_err());
        let out = tool.run(&json!({ "query": "nothing like this" })).unwrap();
        assert_eq!(out["count"], 0);
        assert!(out["note"].as_str().unwrap().contains("the store holds 2"));
    }
}
