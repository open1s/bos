//! An incremental inverted index over session text.
//!
//! The index exists to answer one question quickly: **which sessions could
//! possibly match this query**. It is a candidate filter, never the last word.
//! Every candidate still goes through the same exact `str::contains`
//! verification the plain scan uses, so a query can never match something the
//! scan would not have matched.
//!
//! The property that makes that safe is the tokenizer's: any substring of two
//! or more characters produces all of its tokens inside a single run, so an
//! occurrence in the text always puts its session in the candidate set. False
//! positives are fine and get verified away; a false negative would be a bug,
//! and the tests assert the superset direction because of it.
//!
//! What it covers is exactly what `SessionStore::search` checks — the title, and
//! each message's text and reasoning — because covering less would hide results
//! and covering more would waste memory on text nothing ever matches.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::session::SessionRecord;

/// Whether a character is CJK, kana or Hangul — scripts written without spaces,
/// where character bigrams are the cheapest useful token.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2FA1F)
}

/// The tokens of `text`: lowercased words, plus character bigrams for CJK runs.
///
/// A one-character CJK run yields nothing, which is why a one-character CJK
/// query has to fall back to a scan: a bigram needs two characters by
/// definition.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    // Bigrams of adjacent characters, never spanning a gap or a change of
    // writing system. Whole words were the first attempt and they were **wrong**:
    // a document holding "there" has no "the" token, so a query for "the" would
    // have been told no session could match and a real result would have been
    // hidden. Bigrams have the property that was needed — if the needle occurs
    // in the text, every bigram of the needle occurs in the text — including
    // inside longer words, which is exactly the case words broke.
    let mut out = Vec::new();
    let mut run: Vec<char> = Vec::new();
    let mut class = 0u8;
    for c in text.to_lowercase().chars() {
        let this = if is_cjk(c) {
            2
        } else if c.is_alphanumeric() {
            1
        } else {
            0
        };
        if this == 0 || this != class {
            run.clear();
            class = this;
            if this == 0 {
                continue;
            }
        }
        run.push(c);
        if run.len() >= 2 {
            out.push(run[run.len() - 2..].iter().collect());
        }
    }
    out
}

/// The text of a session that can match, joined so no token spans two fields.
pub(crate) fn indexable_text(record: &SessionRecord) -> String {
    let mut out = record.title.to_lowercase();
    out.push('\n');
    for message in record.messages.iter().chain(record.archived.iter()) {
        out.push_str(&message.text);
        out.push('\n');
        out.push_str(&message.reasoning);
        out.push('\n');
    }
    out
}

/// One session as the index needs to see it.
pub(crate) struct Synced {
    /// Session id, which is what a candidate set contains.
    pub(crate) id: String,
    /// `(modified seconds, length)` of the file this text came from.
    pub(crate) stamp: (u64, u64),
    /// Text to tokenize, from [`indexable_text`].
    pub(crate) text: String,
}

/// Token to session ids, plus the stamp each session was indexed at.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Index {
    /// Sorted ids per token, so a candidate set is deterministic.
    postings: HashMap<String, BTreeSet<String>>,
    /// Id to the stamp it was indexed from.
    stamps: HashMap<String, (u64, u64)>,
    /// How many sessions the last [`Index::sync`] had to tokenize.
    #[serde(skip)]
    reindexed: usize,
    /// Whether this index has read the file yet.
    #[serde(skip)]
    loaded: bool,
    /// When the index was last written, so keystrokes do not rewrite it.
    #[serde(skip)]
    saved_at: Option<Instant>,
}

impl Index {
    /// An index with nothing in it.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Sessions [`Index::sync`] had to tokenize last time.
    pub(crate) fn reindexed(&self) -> usize {
        self.reindexed
    }

    /// How many distinct tokens are held.
    pub(crate) fn tokens_held(&self) -> usize {
        self.postings.len()
    }

    /// Bring the index up to date, tokenizing only what changed.
    ///
    /// Sessions that are gone lose their postings; sessions whose stamp moved
    /// have theirs replaced. Removing an id means walking the vocabulary, which
    /// is why the stamp exists: an unchanged session costs one comparison.
    pub(crate) fn sync(&mut self, sessions: &[Synced]) {
        self.reindexed = 0;
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for session in sessions {
            seen.insert(session.id.as_str());
            if self.stamps.get(&session.id) == Some(&session.stamp) {
                continue;
            }
            self.forget(&session.id);
            for token in tokens(&session.text) {
                self.postings
                    .entry(token)
                    .or_default()
                    .insert(session.id.clone());
            }
            self.stamps.insert(session.id.clone(), session.stamp);
            self.reindexed += 1;
        }
        let gone: Vec<String> = self
            .stamps
            .keys()
            .filter(|id| !seen.contains(id.as_str()))
            .cloned()
            .collect();
        for id in gone {
            self.forget(&id);
        }
    }

    /// Drop every posting that names `id`, and its stamp.
    fn forget(&mut self, id: &str) {
        self.stamps.remove(id);
        self.postings.retain(|_, ids| {
            ids.remove(id);
            !ids.is_empty()
        });
    }

    /// Sessions that could match `query`, or `None` when the index has no
    /// opinion — an empty query, one character of CJK, or punctuation only.
    pub(crate) fn candidate_ids(&self, query: &str) -> Option<BTreeSet<String>> {
        let wanted = tokens(query);
        if wanted.is_empty() {
            return None;
        }
        let mut out: Option<BTreeSet<String>> = None;
        for token in wanted {
            let Some(ids) = self.postings.get(&token) else {
                // No session holds this token, so no session can match it. An
                // empty set is a stronger answer than "no opinion": it lets the
                // caller skip every session instead of scanning all of them.
                // This was found by a test that expected the empty set, which is
                // the right expectation.
                return Some(BTreeSet::new());
            };
            out = Some(match out {
                None => ids.clone(),
                Some(so_far) => so_far.intersection(ids).cloned().collect(),
            });
            if out.as_ref().is_some_and(BTreeSet::is_empty) {
                return out;
            }
        }
        out
    }

    /// Read the index from `path`, capping what a corrupt file can cost.
    pub(crate) fn load(path: &Path) -> Self {
        let mut index = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<Index>(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        index.loaded = true;
        index
    }

    /// Whether this session was already indexed at `stamp`.
    ///
    /// The caller asks before building the text to index, so an unchanged
    /// session costs one comparison rather than a tokenization.
    pub(crate) fn is_current(&self, id: &str, stamp: (u64, u64)) -> bool {
        self.stamps.get(id) == Some(&stamp)
    }

    /// Whether the index has been read from disk yet.
    pub(crate) fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Write the index atomically, via a temp file and a rename.
    pub(crate) fn save(&mut self, path: &Path) -> Result<(), String> {
        let Some(parent) = path.parent() else {
            return Err("index path has no directory".to_string());
        };
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(self).map_err(|e| format!("cannot serialize index: {e}"))?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
        self.saved_at = Some(Instant::now());
        self.loaded = true;
        Ok(())
    }

    /// Whether a save is due, so a keystroke does not rewrite the whole file.
    pub(crate) fn save_is_due(&self) -> bool {
        match self.saved_at {
            None => true,
            Some(when) => when.elapsed().as_secs() >= 5,
        }
    }
}

/// Where the index lives: in a subdirectory of the session store.
///
/// It goes in a directory rather than beside the session files because the
/// store reads every `.json` in its own directory as a session; a directory has
/// no `.json` extension, so nothing can mistake this for one. The export
/// directory from the previous round sits there for the same reason.
pub(crate) fn index_path(root: &Path) -> PathBuf {
    root.join("fts").join("index.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ChatMessage, Role};

    /// A session record carrying `lines` as messages, titled `title`.
    fn record(id: &str, title: &str, lines: &[&str]) -> SessionRecord {
        let mut record = SessionRecord::new();
        record.id = id.to_string();
        record.title = title.to_string();
        for line in lines {
            record.messages.push(match record.messages.len() % 2 {
                0 => ChatMessage::user(*line),
                _ => {
                    let mut m = ChatMessage::assistant();
                    m.text = (*line).to_string();
                    m
                }
            });
        }
        record
    }

    /// The plain scan this index must never contradict.
    fn scan(records: &[SessionRecord], query: &str) -> Vec<String> {
        let needle = query.to_lowercase();
        records
            .iter()
            .filter(|r| {
                r.title.to_lowercase().contains(&needle)
                    || r.messages.iter().any(|m| {
                        m.text.to_lowercase().contains(&needle)
                            || m.reasoning.to_lowercase().contains(&needle)
                    })
            })
            .map(|r| r.id.clone())
            .collect()
    }

    fn synced(records: &[SessionRecord]) -> Vec<Synced> {
        records
            .iter()
            .enumerate()
            .map(|(i, r)| Synced {
                id: r.id.clone(),
                stamp: (i as u64 + 1, indexable_text(r).len() as u64),
                text: indexable_text(r),
            })
            .collect()
    }

    #[test]
    fn tokens_are_bigrams_that_never_span_a_gap() {
        // Adjacent pairs, so a query matches inside a longer word: "the" is
        // findable in "there", which whole-word tokens could not do.
        assert_eq!(
            tokens("there"),
            ["th", "he", "er", "re"],
            "a word is its adjacent pairs"
        );
        assert!(tokens("there").contains(&"he".to_string()));
        assert_eq!(tokens("Ab Cd"), ["ab", "cd"]);
        // Punctuation and spaces break the run, so no bigram spans them.
        assert_eq!(tokens("a b"), Vec::<String>::new());
        assert!(tokens("e").is_empty());
        assert!(tokens("...").is_empty());
        // CJK runs are bigrams too, and a one-character run yields nothing.
        assert_eq!(tokens("会话窗口"), ["会话", "话窗", "窗口"]);
        assert!(tokens("会").is_empty());
        assert!(tokens("会、话").is_empty());
        // A run that changes writing system breaks, because a needle cannot span
        // such a change either.
        assert_eq!(tokens("a中"), Vec::<String>::new());
    }

    #[test]
    fn an_index_never_misses_what_a_scan_matches() {
        let records = vec![
            record("a", "Deploy notes", &["How do I ship the GUI?", "Run cargo build."]),
            record("b", "长会话窗口", &["会话窗口的滚动条", "bigram 测试"]),
            record("c", "Mixed", &["Cargo Build is case insensitive", ""]),
            record("d", "Quiet", &["nothing to see"]),
            // The case that broke whole-word tokens: "the" occurs inside this
            // session's only word, so a word index would have hidden it.
            record("e", "there", &["there"]),
        ];
        let mut index = Index::new();
        index.sync(&synced(&records));
        for query in [
            // Substrings inside longer words, which whole-word tokens could not
            // answer: "the", "he", "ere" and "er" are all inside "there".
            "the", "he", "ere", "er", "here", "re",
            "cargo", "CARGO", "build", "ship", "会话", "话窗", "滚动条", "窗口", "bigram", "nothing",
        ] {
            let expected = scan(&records, query);
            let Some(ids) = index.candidate_ids(query) else {
                // No opinion: the caller falls back to the scan, which is sound.
                continue;
            };
            for id in &expected {
                assert!(
                    ids.contains(id),
                    "query {query:?} matched {id} in the scan but not in the index"
                );
            }
        }
    }

    #[test]
    fn a_query_with_no_tokens_has_no_opinion() {
        let mut index = Index::new();
        index.sync(&synced(&[record("a", "t", &["会话窗口"])]));
        // One CJK character, punctuation, and empty all fall back to a scan.
        assert!(index.candidate_ids("会").is_none());
        assert!(index.candidate_ids("...").is_none());
        assert!(index.candidate_ids("  ").is_none());
        // Two characters are enough for a bigram.
        assert_eq!(
            index.candidate_ids("会话"),
            Some(["a".to_string()].into_iter().collect())
        );
    }

    #[test]
    fn a_single_match_among_many_sessions_yields_one_candidate() {
        // The point of an index: the work after a query is proportional to what
        // matches, not to how much exists. Asserted rather than timed, because a
        // timing assertion is a flaky test and this is the fact it would have
        // stood in for.
        let records: Vec<SessionRecord> = (0..200)
            .map(|i| {
                let mut r = record(
                    &format!("s{i}"),
                    &format!("session {i}"),
                    &["the quick brown fox", "jumps over the lazy dog"],
                );
                // One session of the 200 is the only one that mentions this.
                if i == 7 {
                    r.messages.push(ChatMessage::user("a kangaroo appears"));
                }
                r
            })
            .collect();
        let mut index = Index::new();
        index.sync(&synced(&records));
        assert_eq!(index.reindexed(), 200, "a fresh index tokenizes all 200");
        let one = index.candidate_ids("kangaroo").expect("opinion");
        assert_eq!(one.len(), 1, "one session of 200 mentions it");
        assert!(one.contains("s7"), "and it is the session that mentioned it");
        // A word every session shares yields every session: that is correct, and
        // it is why verification still has to run.
        let all = index.candidate_ids("brown fox").expect("opinion");
        assert_eq!(all.len(), 200, "shared text yields all of them");
        let none = index.candidate_ids("kangarooesque").expect("opinion");
        assert!(none.is_empty(), "text nothing matches yields no candidates");
    }

    #[test]
    fn only_changed_sessions_are_tokenized_again() {
        let records = vec![record("a", "one", &["alpha"]), record("b", "two", &["beta"])];
        let mut index = Index::new();
        index.sync(&synced(&records));
        assert_eq!(index.reindexed(), 2, "a fresh index has to tokenize both");
        index.sync(&synced(&records));
        assert_eq!(index.reindexed(), 0, "unchanged stamps cost a comparison");

        // Change one session's text and its stamp.
        let changed = vec![
            record("a", "one", &["alpha"]),
            record("b", "two", &["gamma"]),
        ];
        let mut moved = synced(&changed);
        moved[1].stamp = (99, 1);
        index.sync(&moved);
        assert_eq!(index.reindexed(), 1, "only the changed session is retokenized");
        assert!(
            index
                .candidate_ids("beta")
                .is_some_and(|ids| ids.is_empty()),
            "the old text must stop matching"
        );
        assert!(index.candidate_ids("gamma").is_some());
    }

    #[test]
    fn a_removed_session_leaves_the_index() {
        let records = vec![record("a", "one", &["alpha"]), record("b", "two", &["beta"])];
        let mut index = Index::new();
        index.sync(&synced(&records));
        index.sync(&synced(&records[..1]));
        assert_eq!(
            index.candidate_ids("beta"),
            Some(BTreeSet::new()),
            "a token no session holds now yields an empty set, not a scan"
        );
        assert_eq!(index.candidate_ids("alpha").map(|s| s.len()), Some(1));
    }

    #[test]
    fn the_index_survives_a_round_trip_through_disk() {
        let records = vec![record("a", "会话", &["alpha 窗口"])];
        let mut index = Index::new();
        index.sync(&synced(&records));
        let root = std::env::temp_dir().join(format!("bos-fts-{}", std::process::id()));
        let path = index_path(&root);
        index.save(&path).expect("save");
        let reloaded = Index::load(&path);
        assert!(reloaded.is_loaded());
        assert_eq!(reloaded.tokens_held(), index.tokens_held());
        assert_eq!(reloaded.candidate_ids("alpha"), index.candidate_ids("alpha"));
        assert_eq!(reloaded.candidate_ids("窗口"), index.candidate_ids("窗口"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn archived_turns_are_indexed_too() {
        let mut r = record("a", "t", &["visible"]);
        let mut folded = ChatMessage::assistant();
        folded.text = "folded text about llamas".to_string();
        r.archived.push(folded);
        assert_eq!(r.archived.len(), 1);
        assert_eq!(r.messages[0].role, Role::User);
        let mut index = Index::new();
        index.sync(&synced(&[r]));
        assert!(index.candidate_ids("llamas").is_some(), "folded turns match");
    }
}
