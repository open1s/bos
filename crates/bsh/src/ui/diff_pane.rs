//! The Diff pane: a patch or a conflicted file, compared in the right panel.
//!
//! Three sources feed one surface, and the difference between them is only what
//! has to be parsed:
//!
//! * a PATCH file (`.patch`, `.diff`, or any file whose bytes parse as a
//!   unified/Git diff) is read with [`DiffFile::parse`],
//! * a WORKING file with conflict markers is read with
//!   [`DiffFile::parse_conflicts`], which is the merge half: the kit renders
//!   each conflict with its own resolution commands and
//!   [`DiffState::resolved_text`] hands back the merged text to save,
//! * a file with NO changes is shown as itself (`DiffFile::unchanged`), so the
//!   pane answers "is this file dirty?" instead of going blank.
//!
//! The comparison itself - the row model, the hard wrapping, the line-number
//! lanes, the two view modes and the conflict commands - belongs to the kit's
//! `Diff` element. What this module owns is the loader, the two view
//! preferences and the save.

use std::path::{Path, PathBuf};

use gpui_kit::component::diff::{DiffFile, DiffMode, DiffState};
// Explicit, never `use gpui_kit::*`: with test-support the glob also brings in
// GPUI's own `test` macro, which shadows Rust's builtin `#[test]` and expands
// into itself until the recursion limit trips (a SIGBUS in the proc-macro
// parser, then `recursion limit reached while expanding #[test]`). The kit's
// own comment says as much: test modules must import their Kit types
// explicitly. This module keeps plain unit tests, so the glob is out.
use gpui_kit::Entity;

/// How a file was read, which decides whether the pane can save its result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffSource {
    /// A patch: read-only, because writing a patch back over itself would
    /// destroy the very file the reader came to compare.
    Patch,
    /// A working file whose conflict markers the pane can resolve and save.
    Merge,
}

/// The Diff pane's own state.
#[derive(Default)]
pub(crate) struct DiffView {
    /// The comparison on screen, or none before anything is opened.
    pub(crate) state: Option<Entity<DiffState>>,
    /// The workspace-relative file the comparison came from.
    pub(crate) path: Option<PathBuf>,
    /// How that file was read.
    pub(crate) source: Option<DiffSource>,
    /// Why the pane is empty, when it is. Every empty state names a reason:
    /// a parse failure that said nothing is a pane that looks broken.
    pub(crate) error: Option<String>,
    /// The resolved mode, so the toggle can draw its pressed state without
    /// reading the entity it does not own.
    pub(crate) mode: DiffMode,
    /// Whether the open comparison has an unresolved conflict, so the merge
    /// action knows whether there is anything to save.
    pub(crate) conflicts: usize,
    /// Whether every conflict has a resolution.
    pub(crate) resolved: bool,
}

impl DiffView {
    /// Whether a comparison is on screen.
    pub(crate) fn is_open(&self) -> bool {
        self.state.is_some()
    }

    /// Whether this comparison can be written back to disk.
    pub(crate) fn is_merge(&self) -> bool {
        self.source == Some(DiffSource::Merge) && self.resolved
    }

    /// Close the comparison and forget its path.
    pub(crate) fn clear(&mut self) {
        self.state = None;
        self.path = None;
        self.source = None;
        self.conflicts = 0;
        self.resolved = false;
    }
}

/// The view mode a stored preference names.
///
/// Unknown values - a hand-edited file, a value from a later version - fall
/// back to the default rather than being trusted, so a bad string cannot leave
/// the pane in a mode nothing draws.
pub(crate) fn mode_of(stored: &str) -> DiffMode {
    match stored {
        "unified" => DiffMode::Unified,
        "split" => DiffMode::Split,
        _ => DiffMode::Split,
    }
}

/// The preference string a mode is stored as. The round trip is pinned by test
/// because the two names are also the file's own vocabulary.
pub(crate) fn mode_key(mode: DiffMode) -> &'static str {
    match mode {
        DiffMode::Unified => "unified",
        // Split is the default, so a mode the kit adds later - the enum is
        // `non_exhaustive`, which is exactly that promise - is stored under the
        // name bsh draws it as rather than under a name nothing knows.
        _ => "split",
    }
}

/// Read a workspace file as a comparison.
///
/// The order matters and is the whole loader: a patch first, because a patch is
/// what the pane is for; then conflict markers, because a conflicted file is a
/// document the reader is mid-way through; then the file itself, so a file with
/// no changes still opens and says so.
pub(crate) fn load(rel: &str, bytes: &[u8]) -> Result<(Vec<DiffFile>, DiffSource), &'static str> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err("diff.binary");
    };
    if let Ok(files) = DiffFile::parse(text) {
        if !files.is_empty() {
            return Ok((files, DiffSource::Patch));
        }
    }
    if text.contains("<<<<<<<") {
        // A conflicted file is a document the reader is mid-way through, so it
        // is read as a merge - but only when it actually carries a marker: the
        // kit's conflict parser accepts any text (a file with no conflict at
        // all parses as a conflict file with zero conflicts), which would
        // otherwise turn every plain file into a Merge.
        match DiffFile::parse_conflicts(rel, text) {
            Ok(file) => return Ok((vec![file], DiffSource::Merge)),
            // Markers the parser refused: say so rather than showing the file
            // as unchanged and leaving the reader to find the markers.
            Err(_) => return Err("diff.error"),
        }
    }
    // The last resort, and the reason it exists: the pane answers "does this
    // file differ from what was committed?" for a file no patch was supplied
    // for, by showing it as its own single column.
    Ok((vec![DiffFile::unchanged(rel, text)], DiffSource::Patch))
}

/// The file a comparison is titled with, for the header and the tooltip.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// The language id the kit should highlight a path with.
///
/// It comes from `crate::files`, the same table the editor opens its buffers
/// with, so the pane and the editor can never disagree about what a `.tsx` is.
pub(crate) fn language_for(path: &Path) -> Option<&'static str> {
    crate::files::language_for(&display_name(path))
}

#[cfg(test)]
mod tests {
    // Explicit, not `use super::*`: a glob here would re-import any macro the
    // parent module brought in, and GPUI's `test` macro shadows the builtin
    // `#[test]` these unit tests rely on.
    use super::{load, mode_key, mode_of, DiffMode, DiffSource};

    /// The stored names round-trip, and an unknown one lands on the default.
    #[test]
    fn the_mode_preference_round_trips() {
        assert_eq!(mode_of("unified"), DiffMode::Unified);
        assert_eq!(mode_of("split"), DiffMode::Split);
        assert_eq!(mode_of(""), DiffMode::Split);
        assert_eq!(mode_of("side-by-side"), DiffMode::Split);
        assert_eq!(mode_key(DiffMode::Unified), "unified");
        assert_eq!(mode_key(DiffMode::Split), "split");
        assert_eq!(mode_of(mode_key(DiffMode::Unified)), DiffMode::Unified);
        assert_eq!(mode_of(mode_key(DiffMode::Split)), DiffMode::Split);
    }

    /// A unified patch parses as a patch, and reports both sides' counts.
    #[test]
    fn a_patch_loads_as_a_patch() {
        let patch = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1,3 +1,3 @@\n fn main() {\n-    let a = 1;\n+    let a = 2;\n }\n";
        let (files, source) = load("lib.rs", patch.as_bytes()).unwrap();
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].additions(), 1);
        assert_eq!(files[0].deletions(), 1);
    }

    /// Conflict markers load as a merge, so the pane can resolve and save.
    #[test]
    fn conflicts_load_as_a_merge() {
        let text = "one\n<<<<<<< ours\ntwo\n=======\nTWO\n>>>>>>> theirs\nthree\n";
        let (files, source) = load("a.txt", text.as_bytes()).unwrap();
        assert_eq!(source, DiffSource::Merge);
        assert_eq!(files[0].conflicts().len(), 1);
    }

    /// A file with no markers and no patch still opens, as itself.
    #[test]
    fn a_plain_file_opens_as_unchanged() {
        let (files, source) = load("notes.txt", b"hello\n").unwrap();
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].additions(), 0);
        assert_eq!(files[0].deletions(), 0);
    }

    /// A binary file is refused with a reason rather than shown as nothing.
    #[test]
    fn binary_files_are_refused_with_a_reason() {
        // A match, not `unwrap_err`: the kit's `DiffFile` carries no `Debug`,
        // and the Ok arm here is the failure worth naming.
        match load("a.bin", &[0xff, 0xfe, 0x00]) {
            Ok(_) => panic!("a binary file must be refused, never opened"),
            Err(error) => assert_eq!(error, "diff.binary"),
        }
    }
}
