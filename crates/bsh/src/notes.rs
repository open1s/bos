//! File annotations: the comments a reader pins to a workspace's files.
//!
//! A note is one reader's remark about a file, anchored either to the whole
//! file or to a run of lines. Notes live beside the checkout at
//! `.bsh/annotations.json`, the same convention the workspace settings use,
//! so they travel with the repository a reader is already annotating instead
//! of hiding inside this machine's application data.
//!
//! Three facts make a note survive the session that wrote it: the anchor line,
//! the QUOTE of the code it was written against, and the resolved flag. The
//! line is the fast answer, the quote is the durable one - [`Notes::reanchor`]
//! moves a note back onto its code after an edit above it shifted every line -
//! and the flag is the review's own state, so a suggestion that has been dealt
//! with stops asking for attention without being deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many source lines a stored quote keeps.
///
/// A note is a remark about a place, not a copy of the file: eight lines is
/// enough to recognise the place after an edit and to show the comment's
/// subject in the panel, while the sidecar stays small.
pub(crate) const QUOTE_LINES: usize = 8;

/// How many bytes a stored quote keeps, before it is cut with an ellipsis.
pub(crate) const QUOTE_BYTES: usize = 400;

/// One reader's remark about a file.
///
/// `line` of `None` anchors the note to the file itself; `Some(n)` anchors it
/// to the `n`th line, counted from one the way an editor numbers lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct Note {
    /// The first line the note is pinned to, or none for the file as a whole.
    #[serde(default)]
    pub(crate) line: Option<usize>,
    /// The LAST line, when the note covers a range (the reference's `lines 12-18`).
    /// Absent on single-line notes and on records written before ranges existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<usize>,
    /// What the note says.
    pub(crate) text: String,
    /// The code the note was written against, bounded by [`QUOTE_LINES`] and
    /// [`QUOTE_BYTES`]. This is what lets the note follow its code when an
    /// edit above it moves every anchor; records written before quotes existed
    /// load with an empty one and fall back to their line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) quote: String,
    /// Whether the reader has dealt with the remark. A resolved note stops
    /// asking for attention but stays in the file's history.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) resolved: bool,
    /// Whether this note has already been sent to the model. The review that
    /// follows a comment is a request, not a subscription, so a note that went
    /// out once is not sent again by a later comment's own review.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) sent: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Note {
    /// A fresh, open note pinned to `line`..`to` and quoting `quote`.
    pub(crate) fn new(
        line: Option<usize>,
        to: Option<usize>,
        text: impl Into<String>,
        quote: impl Into<String>,
    ) -> Self {
        Self {
            line,
            to,
            text: text.into(),
            quote: quote.into(),
            resolved: false,
            sent: false,
        }
    }

    /// The anchor the note is labelled with: a line number, or the whole file.
    pub(crate) fn anchor(&self) -> String {
        match (self.line, self.to) {
            (Some(from), Some(to)) if to > from => format!("L{from}-{to}"),
            (Some(line), _) => format!("L{line}"),
            (None, _) => String::from("·"),
        }
    }

    /// The last line the note covers, for a caller that wants the range.
    pub(crate) fn end(&self) -> Option<usize> {
        self.line.map(|line| self.to.unwrap_or(line).max(line))
    }
}

/// Every note this workspace carries, keyed by the file's workspace-relative
/// path, which is the same name the file pane's tabs and rows already use.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notes(HashMap<String, Vec<Note>>);

impl Notes {
    /// Read a workspace's notes, treating absence as none yet written.
    pub(crate) fn load(root: &Path) -> Self {
        let Ok(bytes) = crate::extension::notes::store().read(root) else {
            return Self::default();
        };
        if bytes.is_empty() {
            return Self::default();
        }
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Write the notes back to the workspace's `.bsh` directory, creating it
    /// on the first note the way the settings writer does.
    pub(crate) fn save(&self, root: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        crate::extension::notes::store().write(root, &bytes)
    }

    /// The notes file's path in the workspace.
    ///
    /// One definition, owned here, so the loader, the writer, and the reveal
    /// paths cannot drift apart.
    pub(crate) fn path(root: &Path) -> PathBuf {
        root.join(".bsh").join("annotations.json")
    }

    /// The notes on one file, in the order they were written.
    pub(crate) fn on(&self, rel: &Path) -> &[Note] {
        let key = rel.to_string_lossy().to_string();
        self.0.get(&key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Add a note to a file and return the store, so a caller can save it.
    pub(crate) fn add(&mut self, rel: &Path, note: Note) {
        let key = rel.to_string_lossy().to_string();
        self.0.entry(key).or_default().push(note);
    }

    /// Move a file's notes with it.
    ///
    /// A rename the reader made in the tree must not orphan the remarks pinned
    /// to the old path: notes are keyed by that path, so the key moves with it.
    pub(crate) fn rename(&mut self, from: &Path, to: &Path) {
        let from = from.to_string_lossy().to_string();
        let to = to.to_string_lossy().to_string();
        if let Some(notes) = self.0.remove(&from) {
            self.0.insert(to, notes);
        }
    }

    /// Drop a file's notes, for when the file they described is gone.
    pub(crate) fn forget(&mut self, rel: &Path) {
        self.0.remove(&rel.to_string_lossy().to_string());
    }

    /// Remove the `index`th note from a file; a bad index is a no-op.
    pub(crate) fn remove(&mut self, rel: &Path, index: usize) {
        let key = rel.to_string_lossy().to_string();
        if let Some(notes) = self.0.get_mut(&key) {
            if index < notes.len() {
                notes.remove(index);
            }
            if notes.is_empty() {
                self.0.remove(&key);
            }
        }
    }

    /// Open or resolve the `index`th note; a bad index is a no-op.
    pub(crate) fn set_resolved(&mut self, rel: &Path, index: usize, resolved: bool) {
        let key = rel.to_string_lossy().to_string();
        if let Some(note) = self.0.get_mut(&key).and_then(|notes| notes.get_mut(index)) {
            note.resolved = resolved;
        }
    }

    /// Mark the given notes as sent, so a later review does not repeat them.
    pub(crate) fn mark_sent(&mut self, rel: &Path, indices: &[usize]) {
        let key = rel.to_string_lossy().to_string();
        let Some(notes) = self.0.get_mut(&key) else {
            return;
        };
        for index in indices {
            if let Some(note) = notes.get_mut(*index) {
                note.sent = true;
            }
        }
    }

    /// Whether a file carries at least one note, which is what the tab's badge
    /// and the editor's toggle signal.
    pub(crate) fn has_notes(&self, rel: &Path) -> bool {
        !self.on(rel).is_empty()
    }

    /// How many remarks on a file are still open, which is what the tab badge
    /// counts: a file whose comments are all resolved is caught up with.
    pub(crate) fn open_count(&self, rel: &Path) -> usize {
        self.on(rel).iter().filter(|note| !note.resolved).count()
    }

    /// Move a file's notes back onto their code after an edit shifted them.
    ///
    /// The line is the anchor a note is FASTEST to use and the first to go
    /// stale: one inserted line moves every remark below it while the code they
    /// describe is untouched. The quote is the durable half - the first line of
    /// the code the reader marked - so it is searched for when the note's own
    /// line no longer carries it, preferring the occurrence nearest the old
    /// anchor so two identical lines do not steal each other's remarks.
    ///
    /// Returns whether anything moved, so a caller only writes the sidecar when
    /// there is something new to write.
    pub(crate) fn reanchor(&mut self, rel: &Path, text: &str) -> bool {
        let key = rel.to_string_lossy().to_string();
        let Some(notes) = self.0.get_mut(&key) else {
            return false;
        };
        let mut moved = false;
        for note in notes.iter_mut() {
            let (Some(line), Some(head)) = (note.line, note.quote.lines().next()) else {
                continue;
            };
            let head = head.trim();
            if head.is_empty() || line_text(text, line).trim() == head {
                continue;
            }
            let Some(found) = find_line(text, head, line) else {
                continue;
            };
            if found == line {
                continue;
            }
            let span = note.end().unwrap_or(line).saturating_sub(line);
            note.line = Some(found);
            note.to = (span > 0).then_some(found + span);
            moved = true;
        }
        moved
    }
}

/// The text of one one-based line, without its break. A line past the end is
/// empty rather than a panic.
pub(crate) fn line_text(text: &str, line: usize) -> &str {
    let start = line_start_offset(text, line);
    if start >= text.len() {
        return "";
    }
    let rest = &text[start..];
    let end = rest.find('\n').map(|at| start + at).unwrap_or(text.len());
    &text[start..end]
}

/// The nearest line whose trimmed text is `head`, preferring `near`.
fn find_line(text: &str, head: &str, near: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (index, line) in text.split('\n').enumerate() {
        if line.trim() != head {
            continue;
        }
        let number = index + 1;
        let distance = number.abs_diff(near);
        if best.is_none_or(|(_, closest)| distance < closest) {
            best = Some((number, distance));
        }
    }
    best.map(|(number, _)| number)
}

/// The quote stored with a note, cut to [`QUOTE_LINES`] and [`QUOTE_BYTES`].
///
/// The quote is taken from the buffer the reader selected in, not from a fresh
/// read: it has to be the text they were looking at, and the buffer already is.
pub(crate) fn quote_of(text: &str, from: usize, to: usize) -> String {
    let from = from.max(1);
    let to = to.max(from);
    let mut out = String::new();
    let mut lines = 0;
    let mut next = from;
    while next <= to {
        let line = line_text(text, next).trim_end();
        // A selection can begin or end on a blank line; leading blanks say
        // nothing about the code, so they are skipped instead of quoted.
        if line.trim().is_empty() && out.is_empty() {
            next += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        lines += 1;
        if lines >= QUOTE_LINES || out.len() >= QUOTE_BYTES {
            if out.len() > QUOTE_BYTES {
                let cut = out
                    .char_indices()
                    .take_while(|(at, _)| *at <= QUOTE_BYTES)
                    .last()
                    .map(|(at, _)| at)
                    .unwrap_or(0);
                out.truncate(cut);
            }
            if next < to {
                out.push('…');
            }
            break;
        }
        next += 1;
    }
    out
}

/// The code a note is about, read from `text` at its CURRENT anchor.
///
/// The stored quote is a snapshot; after a re-anchor or an edit the buffer is
/// the truth, so the excerpt is re-read from it whenever the anchor still
/// resolves. A note with no anchor, or whose lines are empty, falls back to the
/// snapshot it was written with.
pub(crate) fn excerpt(note: &Note, text: &str) -> String {
    let Some(from) = note.line else {
        return String::new();
    };
    let to = note.end().unwrap_or(from).max(from);
    let current = quote_of(text, from, to);
    if current.trim().is_empty() {
        note.quote.clone()
    } else {
        current
    }
}

/// The prompt a file's open remarks become when the reader asks the model.
///
/// Resolved remarks are left out: the review asks for what is still wanted, and
/// a finished remark is history. Each entry names its anchor and carries the
/// code excerpt, so the model reads the place rather than trusting a line
/// number that an edit may have moved, and a trailing instruction says what to
/// do with the list instead of leaving the model to infer it.
pub(crate) fn review_prompt(rel: &Path, notes: &[Note], text: &str) -> String {
    let open: Vec<&Note> = notes.iter().filter(|note| !note.resolved).collect();
    if open.is_empty() {
        return String::new();
    }
    let mut prompt = format!("Comments on `{}`:\n", rel.display());
    for note in open {
        prompt.push('\n');
        prompt.push_str(&format!("### {}\n", note.anchor()));
        let code = excerpt(note, text);
        if !code.trim().is_empty() {
            prompt.push_str("```\n");
            prompt.push_str(&code);
            if !code.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str("```\n");
        }
        prompt.push_str(note.text.trim());
        prompt.push('\n');
    }
    prompt.push_str(
        "\nApply what these comments ask for in this file and keep the rest of the file \
unchanged. Ask me if a comment is ambiguous.\n",
    );
    prompt
}

/// The byte offset a one-based line starts at, the anchor a note points to.
pub(crate) fn line_start_offset(text: &str, line: usize) -> usize {
    let mut number = 1;
    for (offset, ch) in text.char_indices() {
        if number == line {
            return offset;
        }
        if ch == '\n' {
            number += 1;
        }
    }
    text.len()
}

/// Turn a buffer's byte offset into a one-based line number, the anchor a note
/// records. An offset past the end lands on the last line.
pub(crate) fn line_at_offset(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut line = 1;
    for (index, byte) in text.bytes().enumerate() {
        if index >= offset {
            break;
        }
        if byte == b'\n' {
            line += 1;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-notes-{label}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_note_round_trips_through_the_workspace_sidecar() {
        let root = scratch("round-trip");
        let mut notes = Notes::default();
        notes.add(
            Path::new("src/lib.rs"),
            Note::new(Some(3), None, "watch this", "let x = 1;"),
        );
        notes.save(&root).unwrap();
        let loaded = Notes::load(&root);
        assert_eq!(
            loaded.on(Path::new("src/lib.rs")),
            [Note::new(Some(3), None, "watch this", "let x = 1;")]
        );
        assert_eq!(
            Notes::path(&root),
            root.join(".bsh").join("annotations.json")
        );
    }

    #[test]
    fn a_missing_sidecar_reads_as_no_notes() {
        let root = scratch("absent");
        assert!(Notes::load(&root).on(Path::new("src/lib.rs")).is_empty());
    }

    #[test]
    fn a_sidecar_written_before_quotes_existed_still_loads() {
        let root = scratch("legacy");
        std::fs::create_dir_all(root.join(".bsh")).unwrap();
        std::fs::write(
            root.join(".bsh").join("annotations.json"),
            r#"{"src/lib.rs":[{"line":2,"text":"old"}]}"#,
        )
        .unwrap();
        let notes = Notes::load(&root);
        let note = &notes.on(Path::new("src/lib.rs"))[0];
        assert_eq!(note.line, Some(2));
        assert_eq!(note.text, "old");
        assert!(note.quote.is_empty());
        assert!(!note.resolved);
        assert!(!note.sent);
    }

    #[test]
    fn removing_the_last_note_drops_the_file_entry() {
        let mut notes = Notes::default();
        let rel = Path::new("README.md");
        notes.add(rel, Note::new(None, None, "first", ""));
        notes.add(rel, Note::new(None, None, "second", ""));
        notes.remove(rel, 0);
        assert!(notes.has_notes(rel));
        notes.remove(rel, 0);
        assert!(!notes.has_notes(rel), "the file entry goes when empty");
        notes.remove(rel, 0);
        assert!(!notes.has_notes(rel), "a bad index is a no-op");
    }

    #[test]
    fn anchors_name_a_line_or_the_whole_file() {
        let whole = Note::new(None, None, "", "");
        let pinned = Note::new(Some(12), None, "", "");
        let range = Note::new(Some(12), Some(18), "", "");
        assert_eq!(whole.anchor(), "·");
        assert_eq!(pinned.anchor(), "L12");
        assert_eq!(range.anchor(), "L12-18");
    }

    #[test]
    fn offsets_count_into_line_numbers() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_at_offset(text, 0), 1);
        assert_eq!(line_at_offset(text, 4), 2);
        assert_eq!(line_at_offset(text, 8), 3);
        assert_eq!(line_at_offset(text, 999), 3);
        assert_eq!(line_text(text, 2), "two");
        assert_eq!(line_text(text, 99), "");
    }

    #[test]
    fn resolving_and_sending_move_only_the_note_named() {
        let mut notes = Notes::default();
        let rel = Path::new("a.rs");
        notes.add(rel, Note::new(Some(1), None, "one", "a"));
        notes.add(rel, Note::new(Some(2), None, "two", "b"));
        notes.set_resolved(rel, 1, true);
        notes.mark_sent(rel, &[0]);
        let on = notes.on(rel);
        assert!(!on[0].resolved && on[0].sent);
        assert!(on[1].resolved && !on[1].sent);
        assert_eq!(notes.open_count(rel), 1, "a resolved note is not open");
        notes.set_resolved(rel, 9, true);
        assert_eq!(notes.open_count(rel), 1, "a bad index is a no-op");
    }

    #[test]
    fn a_quote_keeps_the_selected_lines_and_bounds_itself() {
        let text = (1..=20).map(|n| format!("line {n}\n")).collect::<String>();
        assert_eq!(quote_of(&text, 3, 5), "line 3\nline 4\nline 5");
        let long = quote_of(&text, 1, 20);
        assert_eq!(long.lines().count(), QUOTE_LINES);
        assert!(long.ends_with('…'), "the cut is marked: {long:?}");
        assert_eq!(quote_of("   \nreal code\n", 1, 2), "real code");
    }

    #[test]
    fn a_note_follows_its_code_when_lines_move() {
        let edited = "fn main() {\n    println!(\"hi\");\n}\n";
        let mut notes = Notes::default();
        let rel = Path::new("main.rs");
        notes.add(
            rel,
            Note::new(Some(2), None, "use a log", "    println!(\"hi\");"),
        );
        assert!(!notes.reanchor(rel, edited), "the anchor is already right");
        let moved = "// header\n// header\nfn main() {\n    println!(\"hi\");\n}\n";
        assert!(notes.reanchor(rel, moved));
        assert_eq!(notes.on(rel)[0].line, Some(4));
        assert!(
            !notes.reanchor(rel, moved),
            "a settled note does not move twice"
        );
    }

    #[test]
    fn two_identical_lines_do_not_steal_each_others_notes() {
        let mut notes = Notes::default();
        let rel = Path::new("same.rs");
        notes.add(rel, Note::new(Some(5), None, "the x", "x"));
        let moved = "x\nx\nx\nx\nx\nx\nx\nx\n";
        assert!(notes.reanchor(rel, moved));
        assert_eq!(notes.on(rel)[0].line, Some(5), "the nearest x wins");
    }

    #[test]
    fn the_review_names_each_place_and_quotes_its_code() {
        let text = "fn main() {\n    println!(\"hi\");\n}\n";
        let notes = vec![
            Note::new(Some(2), None, "use a log", ""),
            Note::new(None, None, "split this file", ""),
            Note {
                resolved: true,
                ..Note::new(Some(1), None, "already done", "")
            },
        ];
        let prompt = review_prompt(Path::new("src/main.rs"), &notes, text);
        assert!(prompt.contains("`src/main.rs`"), "the path: {prompt}");
        assert!(prompt.contains("### L2"), "the anchor: {prompt}");
        assert!(
            prompt.contains("    println!(\"hi\");"),
            "the code: {prompt}"
        );
        assert!(prompt.contains("use a log"), "the remark: {prompt}");
        assert!(prompt.contains("### ·"), "the file-wide remark: {prompt}");
        assert!(
            !prompt.contains("already done"),
            "resolved is history: {prompt}"
        );
    }

    #[test]
    fn an_empty_review_says_nothing() {
        let only_done = vec![Note {
            resolved: true,
            ..Note::new(Some(1), None, "done", "")
        }];
        assert!(review_prompt(Path::new("a.rs"), &only_done, "x").is_empty());
    }
}
