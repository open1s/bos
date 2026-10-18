//! File annotations: the comments a reader pins to a workspace's files.
//!
//! A note is one reader's remark about a file, anchored either to the whole
//! file or to one of its lines. Notes live beside the checkout at
//! `.bsh/annotations.json`, the same convention the workspace settings use,
//! so they travel with the repository a reader is already annotating instead
//! of hiding inside this machine's application data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One reader's remark about a file.
///
/// `line` of `None` anchors the note to the file itself; `Some(n)` anchors it
/// to the `n`th line, counted from one the way an editor numbers lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Note {
    /// The anchor the note is labelled with: a line number, or the whole file.
    pub(crate) fn anchor(&self) -> String {
        match (self.line, self.to) {
            (Some(from), Some(to)) if to > from => format!("L{from}-{to}"),
            (Some(line), _) => format!("L{line}"),
            (None, _) => String::from("·"),
        }
    }
}

/// Every note this workspace carries, keyed by the file's workspace-relative
/// path, which is the same name the file pane's tabs and rows already use.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notes(HashMap<String, Vec<Note>>);

impl Notes {
    /// Read a workspace's notes, treating absence as none yet written.
    pub(crate) fn load(root: &Path) -> Self {
        match std::fs::read(Self::path(root)) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Write the notes back to the workspace's `.bsh` directory, creating it
    /// on the first note the way the settings writer does.
    pub(crate) fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
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

    /// Remove the `index`th note from a file; a bad index is a no-op.
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

    /// Whether a file carries at least one note, which is what the tab's badge
    /// and the editor's toggle signal.
    pub(crate) fn has_notes(&self, rel: &Path) -> bool {
        !self.on(rel).is_empty()
    }
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
            Note {
                line: Some(3),
                text: String::from("watch this"),
                to: None,
            },
        );
        notes.save(&root).unwrap();
        let loaded = Notes::load(&root);
        assert_eq!(
            loaded.on(Path::new("src/lib.rs")),
            [Note {
                line: Some(3),
                text: String::from("watch this"),
                to: None,
            }]
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
        assert!(Notes::load(&root).on(Path::new("src/lib.rs")).is_empty());
    }

    #[test]
    fn removing_the_last_note_drops_the_file_entry() {
        let mut notes = Notes::default();
        let rel = Path::new("README.md");
        notes.add(
            rel,
            Note {
                line: None,
                text: String::from("first"),
                to: None,
            },
        );
        notes.add(
            rel,
            Note {
                line: None,
                text: String::from("second"),
                to: None,
            },
        );
        notes.remove(rel, 0);
        assert!(notes.has_notes(rel));
        notes.remove(rel, 0);
        assert!(!notes.has_notes(rel), "the file entry goes when empty");
        notes.remove(rel, 0);
        assert!(!notes.has_notes(rel), "a bad index is a no-op");
    }

    #[test]
    fn anchors_name_a_line_or_the_whole_file() {
        let whole = Note {
            line: None,
            text: String::new(),
            to: None,
        };
        let pinned = Note {
            line: Some(12),
            text: String::new(),
            to: None,
        };
        assert_eq!(whole.anchor(), "·");
        assert_eq!(pinned.anchor(), "L12");
    }

    #[test]
    fn offsets_count_into_line_numbers() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_at_offset(text, 0), 1);
        assert_eq!(line_at_offset(text, 4), 2);
        assert_eq!(line_at_offset(text, 8), 3);
        // Past the end clamps to the last line rather than inventing one.
        assert_eq!(line_at_offset(text, 999), 3);
    }
}
