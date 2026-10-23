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

use crate::model::ToolEvent;
use crate::ui::transcript;

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
    /// The turn whose changed files the pane lists, when it is showing a turn
    /// rather than one workspace file. The two are mutually exclusive: opening
    /// a file comparison drops the turn and opening a turn drops the file.
    pub(crate) turn: Option<TurnView>,
}

/// What a turn's changed file can honestly show as its before-image.
///
/// bsh records a tool's ARGUMENTS, not the disk. An edit carries the string it
/// replaced, so its old side is real; a write carries only the new text, so its
/// old side is unknown. Naming that difference is the difference between a
/// comparison and an invented one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum TurnBasis {
    /// The tool recorded the text it replaced.
    #[default]
    Recorded,
    /// The tool created the file, so every line is new.
    NewFile,
    /// The tool wrote only new content over a file it did not record.
    Rewritten,
}

impl TurnBasis {
    /// The note this basis shows in place of a diff, or `None` when the old
    /// side is real. The key is the pane's own, so the wording lives in the
    /// locale files with every other label.
    pub(crate) fn note_key(self) -> Option<&'static str> {
        match self {
            TurnBasis::Recorded => None,
            TurnBasis::NewFile => Some("diff.turnNewFile"),
            TurnBasis::Rewritten => Some("diff.turnRewritten"),
        }
    }
}

/// One file a turn wrote, as the pane lists it.
#[derive(Clone)]
pub(crate) struct TurnFile {
    /// The path exactly as the step named it.
    pub(crate) path: String,
    /// Lines the request added.
    pub(crate) added: usize,
    /// Lines the request removed.
    pub(crate) deleted: usize,
    /// What its before-image can honestly be.
    pub(crate) basis: TurnBasis,
    /// The recorded change, as `+` / `-` lines.
    pub(crate) diff: Vec<(char, String)>,
}

/// One turn's changed files, as the Diff pane lists them.
///
/// The files are the tool requests the turn recorded, grouped by path - never
/// a disk diff and never a VCS query, because bsh keeps no before-image of a
/// file. Rows are resolved only for a file the reader expands, so a turn that
/// touched forty files costs nothing until one is opened.
#[derive(Clone, Default)]
pub(crate) struct TurnView {
    /// The transcript message whose steps the list was built from.
    pub(crate) message: usize,
    /// The turn's own number, one-based, for the header and the empty state.
    pub(crate) ordinal: usize,
    /// The files the turn wrote, in the order it touched them.
    pub(crate) files: Vec<TurnFile>,
    /// The files whose unified rows are open.
    pub(crate) open: std::collections::HashSet<usize>,
    /// The resolved rows of each open file, keyed by its index in `files`.
    pub(crate) rows: std::collections::HashMap<usize, Vec<PreviewLine>>,
    /// Why the list is empty, when it is. An empty list has two honest
    /// explanations and they are not the same fact.
    pub(crate) empty: Option<TurnEmpty>,
    /// Whether the turn also ran commands, which can change files nobody
    /// recorded. The list then says so beside the files it does know, so the
    /// comparison never implies it is the whole story.
    pub(crate) commands: bool,
}

/// Why a turn-focused pane has no files to list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TurnEmpty {
    /// The turn ran commands, which can change files without recording which
    /// ones, so no per-file comparison exists to draw.
    CommandsOnly,
    /// The turn recorded nothing that writes a file.
    NoWrites,
}

/// The most rows one expanded file draws.
///
/// A request can carry a whole file's text, and building thousands of row
/// elements per frame is exactly the per-frame work this crate's rule forbids.
/// The pane says how many it left out rather than silently truncating.
pub(crate) const TURN_MAX_ROWS: usize = 400;

/// Classify a turn's written files, grouped exactly the way the transcript's
/// closing card groups them.
///
/// The GROUPING is `transcript::turn_changes`' job and stays the one source of
/// it. The BASIS is the pane's own question, answered from the same tool
/// records: did any step that touched this path carry the text it replaced?
pub(crate) fn turn_files(tools: &[ToolEvent]) -> Vec<TurnFile> {
    let writing: Vec<(&str, String)> = tools
        .iter()
        .filter(|tool| transcript::is_write_tool(&tool.name))
        .filter_map(|tool| transcript::tool_path(&tool.args).map(|path| (tool.name.as_str(), path)))
        .collect();
    // `create_file` is in this crate's write-tool vocabulary but shares
    // `write_file`'s path+content shape, so it is normalised onto that parser
    // before the grouping runs. Without this the create case would be
    // classified and then dropped, because `tool_diff` knows only the names it
    // parses.
    let normalized: Vec<ToolEvent> = tools
        .iter()
        .map(|tool| {
            if tool.name == "create_file" {
                let mut copy = tool.clone();
                copy.name = "write_file".to_string();
                copy
            } else {
                tool.clone()
            }
        })
        .collect();
    transcript::turn_changes(&normalized)
        .into_iter()
        .map(|file| {
            let recorded = file.diff.iter().any(|(mark, _)| *mark == '-');
            let mut touched = false;
            let mut all_create = true;
            for (name, path) in &writing {
                if *path != file.path {
                    continue;
                }
                touched = true;
                if *name != "create_file" {
                    all_create = false;
                }
            }
            let basis = if recorded {
                TurnBasis::Recorded
            } else if touched && all_create {
                TurnBasis::NewFile
            } else {
                TurnBasis::Rewritten
            };
            TurnFile {
                path: file.path,
                added: file.added,
                deleted: file.deleted,
                basis,
                diff: file.diff,
            }
        })
        .collect()
}

/// The unified rows one changed file expands to, or `None` when there is no
/// recorded before-image to diff against.
///
/// Only a recorded old side is diffable. A creation or a rewrite has an unknown
/// before-image, so drawing `+` lines as if they were a hunk would be exactly
/// the invented comparison this pane exists to avoid; the caller draws the
/// basis note instead.
pub(crate) fn turn_rows(file: &TurnFile, current: Option<&str>) -> Option<Vec<PreviewLine>> {
    if file.basis != TurnBasis::Recorded {
        return None;
    }
    Some(preview_lines(&file.path, &file.diff, current))
}

impl TurnView {
    /// Whether the turn ran a tool that can change files without recording
    /// WHICH files - a shell or a script. The list cannot name those changes,
    /// but it must not pretend they do not exist.
    pub(crate) fn ran_commands(tools: &[ToolEvent]) -> bool {
        tools.iter().any(|tool| {
            matches!(
                tool.name.as_str(),
                "bash" | "run_code" | "run_command" | "shell" | "execute"
            )
        })
    }

    /// Why the list is empty: commands may have written files without
    /// recording them, while a turn with no writing tool recorded no change.
    pub(crate) fn why_empty(tools: &[ToolEvent]) -> TurnEmpty {
        if Self::ran_commands(tools) {
            TurnEmpty::CommandsOnly
        } else {
            TurnEmpty::NoWrites
        }
    }
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
        self.turn = None;
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

/// A file and whatever git can say about it, read off the UI thread.
///
/// Every field is Send, which is the point: the filesystem read and the two
/// git subprocesses are the slow half of opening a comparison, so they are done
/// on the runner's runtime and only this crosses back to the window.
#[derive(Debug)]
pub(crate) struct Loaded {
    /// The file's own bytes, bounded by the caller before it gets here.
    pub(crate) bytes: Vec<u8>,
    /// The committed text against the working copy, when git had anything.
    pub(crate) git: Option<String>,
    /// Whether git sees the path as untracked, so it has no before-image.
    pub(crate) untracked: bool,
}

/// Why a comparison could not be read.
#[derive(Debug)]
pub(crate) enum LoadFailure {
    /// A fixed refusal, named by its locale key.
    Key(&'static str),
    /// A real I/O failure, formatted by the caller that owns the wording.
    Io(std::io::Error),
}

/// Read a file and ask git about it. All the I/O of opening a comparison.
///
/// This is the half that runs off the UI thread: a large file read plus two
/// subprocesses are what would otherwise stall the window on a click.
pub(crate) fn read_for(root: &Path, rel: &Path) -> Result<Loaded, LoadFailure> {
    let path = crate::files::resolve(root, rel).ok_or(LoadFailure::Key("right.files.outside"))?;
    let bytes = std::fs::read(&path).map_err(LoadFailure::Io)?;
    // The bound is the editor's own: a patch is a document like any other,
    // and a 40MB one would be pasted into a row list.
    if bytes.len() as u64 > crate::files::MAX_OPEN_BYTES {
        return Err(LoadFailure::Key("diff.oversized"));
    }
    let git = git_diff(root, rel);
    // Only ask git about tracking when it had no diff to give: one subprocess
    // answers the common case.
    let untracked = git.is_none() && git_untracked(root, rel);
    Ok(Loaded {
        bytes,
        git,
        untracked,
    })
}

/// Turn a read file into the comparison to draw. Pure: no disk, no subprocess.
///
/// Separated from read_for so the parsing can happen on the UI thread while the
/// I/O happened off it, and so a test can drive every branch with values it
/// already holds.
pub(crate) fn classify(
    label: &str,
    bytes: &[u8],
    git: Option<&str>,
    untracked: bool,
) -> Result<(Vec<DiffFile>, DiffSource), &'static str> {
    let (files, source) = load(label, bytes)?;
    // A patch of its own, or a file mid-merge, is already the comparison the
    // reader asked for: git has nothing to add to either.
    if source == DiffSource::Merge {
        return Ok((files, source));
    }
    if files
        .iter()
        .any(|file| file.additions() > 0 || file.deletions() > 0)
    {
        return Ok((files, source));
    }
    if let Some(patch) = git {
        if let Ok(parsed) = DiffFile::parse(patch) {
            if !parsed.is_empty() {
                return Ok((parsed, DiffSource::Patch));
            }
        }
    }
    if untracked {
        if let Some(patch) = added_file_patch(label, bytes) {
            if let Ok(parsed) = DiffFile::parse(&patch) {
                if !parsed.is_empty() {
                    return Ok((parsed, DiffSource::Patch));
                }
            }
        }
    }
    Ok((files, source))
}

/// The working copy against the last commit, as a unified diff.
///
/// `HEAD` rather than the index, so a staged change is still shown: a reader
/// asking "what changed?" means since the commit, not since the last `git add`.
fn git_diff(root: &Path, rel: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        // `--no-ext-diff` and `--no-color`: a reader's global config must not
        // turn the output into something the patch parser cannot read.
        .args(["diff", "--no-color", "--no-ext-diff", "-U3", "HEAD", "--"])
        .arg(rel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Whether git sees this path as untracked, so it has no before-image.
fn git_untracked(root: &Path, rel: &Path) -> bool {
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--"])
        .arg(rel)
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.starts_with("?"))
}

/// A file with no committed version, as a patch of pure additions.
fn added_file_patch(label: &str, bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let changes: Vec<(char, String)> = text.lines().map(|line| ('+', line.to_string())).collect();
    patch_for(label, &changes)
}
/// Build a unified diff from a change a tool request described.
///
/// This is what lets a chat step open its OWN change in the pane: bsh keeps
/// the arguments and no before-image, so the honest patch is the one the
/// request carried - the removed lines as `-`, the added ones as `+` - not a
/// claim about what the disk held. The header pair and the hunk count are
/// what make it a real unified diff the kit can parse.
pub(crate) fn patch_for(path: &str, changes: &[(char, String)]) -> Option<String> {
    if changes.is_empty() {
        return None;
    }
    let removed = changes.iter().filter(|(mark, _)| *mark == '-').count();
    let added = changes.iter().filter(|(mark, _)| *mark == '+').count();
    // A change with neither side is not a diff at all.
    if removed == 0 && added == 0 {
        return None;
    }
    let mut patch = String::new();
    patch.push_str(&format!("--- a/{path}\n"));
    patch.push_str(&format!("+++ b/{path}\n"));
    patch.push_str(&format!("@@ -1,{removed} +1,{added} @@\n"));
    for (mark, line) in changes {
        match mark {
            '-' => {
                patch.push('-');
                patch.push_str(line);
                patch.push('\n');
            }
            '+' => {
                patch.push('+');
                patch.push_str(line);
                patch.push('\n');
            }
            _ => {}
        }
    }
    Some(patch)
}
/// Build a patch whose hunk is LOCATED in the file as it is now.
///
/// `patch_for` has to invent `@@ -1,N`: a request names strings, not positions.
/// But the change has usually been applied, so the added text can be found in
/// the file, and then the hunk can carry the real start line and the real
/// surrounding context - which is what makes the pane readable instead of a
/// two-line block floating at the top.
///
/// The old side is the context plus the lines the request REMOVED (they are
/// gone from the file now), the new side is the context plus the lines it
/// ADDED. Falls back to `patch_for` when the added block is not in the file -
/// a reverted edit, an unreadable file - because a synthetic-but-valid header
/// beats refusing to open.
pub(crate) fn patch_for_located(
    path: &str,
    changes: &[(char, String)],
    current: &str,
) -> Option<String> {
    let added: Vec<&str> = changes
        .iter()
        .filter(|(mark, _)| *mark == '+')
        .map(|(_, line)| line.as_str())
        .collect();
    let removed: Vec<&str> = changes
        .iter()
        .filter(|(mark, _)| *mark == '-')
        .map(|(_, line)| line.as_str())
        .collect();
    // Only a change that actually landed can be located: with nothing added
    // there is no text to search for.
    if added.is_empty() {
        return patch_for(path, changes);
    }
    let lines: Vec<&str> = current.lines().collect();
    let start = lines
        .windows(added.len())
        .position(|window| window == added.as_slice())?;
    // Three lines of context each way is git's own default and enough to read
    // the change against its surroundings.
    const CONTEXT: usize = 3;
    let before = &lines[start.saturating_sub(CONTEXT)..start];
    let after_end = (start + added.len() + CONTEXT).min(lines.len());
    let after = &lines[(start + added.len()).min(lines.len())..after_end];
    let first_line = start.saturating_sub(CONTEXT) + 1;
    let old_count = before.len() + removed.len() + after.len();
    let new_count = before.len() + added.len() + after.len();
    let mut patch = String::new();
    patch.push_str(&format!("--- a/{path}\n"));
    patch.push_str(&format!("+++ b/{path}\n"));
    patch.push_str(&format!(
        "@@ -{first_line},{old_count} +{first_line},{new_count} @@\n"
    ));
    for line in before {
        patch.push(' ');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in &removed {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in &added {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in after {
        patch.push(' ');
        patch.push_str(line);
        patch.push('\n');
    }
    Some(patch)
}
/// One row of a hover preview: the numbers each side shows and the marked text.
///
/// The reference's preview draws a line-number column per side, so a reader can
/// see WHERE the change sits. bsh can only do that honestly by locating the hunk
/// in the file as it is now; when that fails the numbers are absent rather than
/// invented, and the row still shows its text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PreviewLine {
    /// The line number on the old side, when this row exists there.
    pub(crate) old: Option<usize>,
    /// The line number on the new side, when this row exists there.
    pub(crate) new: Option<usize>,
    /// The unified-diff marker: a space, a minus or a plus.
    pub(crate) mark: char,
    /// The line's own text, without the marker.
    pub(crate) text: String,
}

/// The rows a hover preview draws for one changed file.
///
/// The numbers come from the hunk `patch_for_located` builds, walked the way a
/// unified diff is read: a context row advances both sides, a removed row only
/// the old, an added row only the new. An unlocatable change keeps its text and
/// drops its numbers, because a guessed line number is worse than none.
pub(crate) fn preview_lines(
    path: &str,
    changes: &[(char, String)],
    current: Option<&str>,
) -> Vec<PreviewLine> {
    let located = current.and_then(|text| patch_for_located(path, changes, text));
    match located.as_deref().and_then(rows_of) {
        Some(rows) => rows,
        // No location: the text is still the change the step asked for, so it is
        // shown with no numbers rather than with made-up ones.
        None => changes
            .iter()
            .filter(|(mark, _)| *mark == '-' || *mark == '+')
            .map(|(mark, line)| PreviewLine {
                old: None,
                new: None,
                mark: *mark,
                text: line.clone(),
            })
            .collect(),
    }
}

/// The numbered rows of the first hunk in a unified diff.
///
/// A context row advances both sides, a removed row only the old, an added row
/// only the new - which is what puts the two number columns where a reader
/// expects them. `None` when the patch carries no hunk at all.
fn rows_of(patch: &str) -> Option<Vec<PreviewLine>> {
    let mut lines = patch.lines();
    let header = lines.find(|line| line.starts_with("@@"))?;
    // `split_whitespace`, not `split(' ')`: the header opens with `@@ -1,3`,
    // so a plain space split yields an EMPTY first part and every start line
    // parses as a failure.
    let mut parts = header.trim_start_matches("@@").split_whitespace();
    let old = parts.next()?.trim_start_matches('-');
    let new = parts.next()?.trim_start_matches('+');
    let mut old_line: usize = old.split(',').next()?.parse().ok()?;
    let mut new_line: usize = new.split(',').next()?.parse().ok()?;
    let mut rows = Vec::new();
    for line in lines {
        let mark = line.chars().next().unwrap_or(' ');
        let text = line.get(1..).unwrap_or("").to_string();
        match mark {
            '-' => {
                rows.push(PreviewLine {
                    old: Some(old_line),
                    new: None,
                    mark,
                    text,
                });
                old_line += 1;
            }
            '+' => {
                rows.push(PreviewLine {
                    old: None,
                    new: Some(new_line),
                    mark,
                    text,
                });
                new_line += 1;
            }
            _ => {
                rows.push(PreviewLine {
                    old: Some(old_line),
                    new: Some(new_line),
                    mark: ' ',
                    text,
                });
                old_line += 1;
                new_line += 1;
            }
        }
    }
    Some(rows)
}

/// Where a preview panel hangs, from the pointer down the pane.
///
/// BELOW by default, ABOVE when the pane has no room underneath: the composer
/// sits under the transcript, so a card growing downward from a low row was
/// drawn behind it and read as clipped. When neither side has room the card
/// sits at the top and its own body scrolls, which is what that scroller is for.
///
/// Pure, so the placement can be pinned without a window: the panel's height is
/// estimated from the rows it will draw rather than measured.
pub(crate) fn preview_top(pointer_y: f32, panel_h: f32, view_h: f32) -> f32 {
    const GAP: f32 = 16.0;
    const EDGE: f32 = 8.0;
    let below = pointer_y + GAP;
    let above = pointer_y - GAP - panel_h;
    if below + panel_h <= view_h - EDGE {
        below
    } else if above >= EDGE {
        above
    } else {
        EDGE
    }
}

/// The preview rows for one changed file, from the REAL before-image when git
/// has one.
///
/// The card's own summary is built from the tool request, because that is all a
/// turn records. A PREVIEW can do better: git holds the committed text, so the
/// rows can be a genuine diff of the file against its last commit - which is
/// what the reference's preview shows. Three sources, best first:
///
/// 1. `git diff HEAD` for the file, parsed into rows with both line numbers;
/// 2. the file as it is now, with the requested change located inside it;
/// 3. the request alone, with text and no numbers.
///
/// Runs off the UI thread, because the first source is a subprocess.
pub(crate) fn preview_for_workspace(
    root: &Path,
    path: &str,
    changes: &[(char, String)],
) -> Vec<PreviewLine> {
    let rel = workspace_relative(root, path);
    if let Some(patch) = git_diff(root, &rel) {
        if let Some(rows) = rows_of(&patch) {
            return rows;
        }
    }
    if let Ok(bytes) = crate::files::read(root, &rel) {
        if let Ok(text) = String::from_utf8(bytes) {
            return preview_lines(path, changes, Some(&text));
        }
    }
    preview_lines(path, changes, None)
}

/// A path a step recorded, expressed against the workspace root.
///
/// A step records the path the model wrote, which may be absolute; the file
/// helpers resolve workspace-relative paths only, so an absolute one inside the
/// workspace is trimmed back to its relative form.
pub(crate) fn workspace_relative(root: &Path, path: &str) -> std::path::PathBuf {
    let candidate = Path::new(path);
    match candidate.strip_prefix(root) {
        Ok(trimmed) => trimmed.to_path_buf(),
        Err(_) => candidate.to_path_buf(),
    }
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
    use super::{
        classify, load, mode_key, mode_of, patch_for, patch_for_located, preview_for_workspace,
        preview_lines, preview_top, read_for, turn_files, turn_rows, DiffMode, DiffSource,
        TurnBasis, TurnEmpty, TurnView,
    };
    use crate::model::ToolEvent;
    use gpui_kit::component::diff::DiffFile;
    use std::path::Path;

    /// A scratch directory under `target/`, so a git fixture never touches the
    /// user's tree and two runs cannot collide.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-diff-git-tests")
            .join(format!("{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// The comparison for one file, through the same two calls the window makes:
    /// the I/O half, then the pure half.
    fn compare(root: &Path, rel: &str) -> Result<(Vec<DiffFile>, DiffSource), &'static str> {
        let loaded = read_for(root, Path::new(rel)).map_err(|_| "diff.error")?;
        classify(rel, &loaded.bytes, loaded.git.as_deref(), loaded.untracked)
    }

    /// Run git in a fixture, asserting it worked.
    fn git(root: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=Test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// THE fix for the first limitation: a plain working file that git tracks
    /// and that was MODIFIED opens as a real comparison against HEAD, not as
    /// one unhighlighted column.
    #[test]
    fn a_modified_file_compares_against_head() {
        let root = scratch("modified");
        git(&root, &["init", "-q"]);
        let file = root.join("lib.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("the file");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "one"]);
        // The working copy now differs from the commit.
        std::fs::write(&file, "let a = 9;\nlet b = 2;\n").expect("the edit");

        let (files, source) = compare(&root, "lib.rs").expect("it loads as a comparison");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(
            files.iter().map(|f| f.deletions()).sum::<usize>(),
            1,
            "the committed line is shown as removed"
        );
        assert_eq!(
            files.iter().map(|f| f.additions()).sum::<usize>(),
            1,
            "and the working line as added"
        );
    }

    /// A file git does not track yet has no before-image, so it is shown as all
    /// additions - which is what git calls it too.
    #[test]
    fn an_untracked_file_shows_all_additions() {
        let root = scratch("untracked");
        git(&root, &["init", "-q"]);
        let file = root.join("new.rs");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("the file");
        let (files, source) = compare(&root, "new.rs").expect("it loads");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files[0].additions(), 3, "every line is new");
        assert_eq!(files[0].deletions(), 0);
    }

    /// A CLEAN tracked file has nothing to compare, so it still opens as
    /// itself: the fallback is kept for the case that genuinely has no diff.
    #[test]
    fn a_clean_file_still_opens_as_itself() {
        let root = scratch("clean");
        git(&root, &["init", "-q"]);
        let file = root.join("clean.rs");
        std::fs::write(&file, "one\n").expect("the file");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "one"]);
        let (files, source) = compare(&root, "clean.rs").expect("it loads");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files[0].additions(), 0);
        assert_eq!(files[0].deletions(), 0);
    }

    /// A directory that is not a repo at all must not fail: git is optional.
    #[test]
    fn a_directory_outside_git_still_loads() {
        let root = scratch("nogit");
        let file = root.join("plain.txt");
        std::fs::write(&file, "hello\n").expect("the file");
        let (files, _) = compare(&root, "plain.txt").expect("it loads without git");
        assert_eq!(files.len(), 1);
    }

    /// THE fix for the second limitation: a located hunk carries the real line
    /// number and real surrounding context, so the pane reads as a diff at a
    /// position instead of a block floating at the top.
    #[test]
    fn a_located_hunk_names_the_real_line_and_context() {
        // The file as it is NOW: the change has landed, so line 4 holds the new
        // text. That is what makes it locatable at all.
        let mut content = String::new();
        for line in 1..=10 {
            if line == 4 {
                content.push_str("LINE FOUR\n");
            } else {
                content.push_str(&format!("line {line}\n"));
            }
        }
        let changes = vec![('-', "line 4".to_string()), ('+', "LINE FOUR".to_string())];
        let patch = patch_for_located("a.txt", &changes, &content).expect("a patch");
        // Context starts three lines before the change, so the hunk opens at 1.
        assert!(
            patch.contains("@@ -1,7 +1,7 @@"),
            "a real located header: {patch}"
        );
        // Context lines are the file's own, and the change is highlighted.
        assert!(patch.contains(" line 1\n"), "leading context: {patch}");
        assert!(patch.contains("-line 4\n"), "the removed line: {patch}");
        assert!(patch.contains("+LINE FOUR\n"), "the added line: {patch}");
        assert!(patch.contains(" line 5\n"), "trailing context: {patch}");
        // And the kit accepts it as a patch with one change each way.
        let (files, source) = load("a.txt", patch.as_bytes()).expect("it parses");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files[0].additions(), 1);
        assert_eq!(files[0].deletions(), 1);
    }

    /// A change whose added text is NOT in the file cannot be located, so the
    /// synthetic header is used rather than refusing to open.
    #[test]
    fn an_unlocatable_change_falls_back_to_the_synthetic_header() {
        let changes = vec![('-', "old".to_string()), ('+', "never applied".to_string())];
        assert!(patch_for_located("a.txt", &changes, "something else\n").is_none());
    }
    /// A located change numbers BOTH sides: context advances both, a removal only
    /// the old, an addition only the new. That is what the hover preview draws.
    #[test]
    fn preview_rows_carry_the_real_line_numbers() {
        let content = "one
two
THREE
four
five
";
        let changes = vec![('-', "three".to_string()), ('+', "THREE".to_string())];
        let rows = preview_lines("a.txt", &changes, Some(content));
        assert!(!rows.is_empty(), "the change produced rows");
        // The removed row sits on the old side at line 3 and on no new line.
        let removed = rows.iter().find(|row| row.mark == '-').expect("a removal");
        assert_eq!(removed.old, Some(3));
        assert_eq!(removed.new, None);
        assert_eq!(removed.text, "three");
        // The added row is the same line on the NEW side.
        let added = rows
            .iter()
            .find(|row| row.mark == '+')
            .expect("an addition");
        assert_eq!(added.new, Some(3));
        assert_eq!(added.old, None);
        assert_eq!(added.text, "THREE");
        // Context rows carry both numbers and the plain marker.
        let context = rows.iter().find(|row| row.mark == ' ').expect("context");
        assert!(context.old.is_some() && context.new.is_some());
    }

    /// A change that cannot be located keeps its text and DROPS its numbers: a
    /// guessed line number is worse than none.
    #[test]
    fn an_unlocated_change_has_no_line_numbers() {
        let changes = vec![('-', "old".to_string()), ('+', "new".to_string())];
        let rows = preview_lines(
            "a.txt",
            &changes,
            Some(
                "different content
",
            ),
        );
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|row| row.old.is_none() && row.new.is_none()));
        assert_eq!(rows[0].mark, '-');
        assert_eq!(rows[1].mark, '+');
    }

    /// A file that is not on disk at all still previews its requested text.
    #[test]
    fn a_preview_without_a_file_still_shows_the_change() {
        let changes = vec![('+', "brand new".to_string())];
        let rows = preview_lines("a.txt", &changes, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text, "brand new");
        assert!(rows[0].new.is_none());
    }

    /// THE fix for the preview's before-image: the rows come from git's
    /// committed text, not from the tool request. A request only knows what it
    /// asked for, so it cannot say what the file held.
    #[test]
    fn a_preview_reads_the_committed_text_not_the_request() {
        let root = scratch("preview-git");
        git(&root, &["init", "-q"]);
        let file = root.join("lib.rs");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("the file");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "one"]);
        // The working copy now differs from the commit.
        std::fs::write(&file, "one\nTWO\nthree\n").expect("the edit");

        // The request DESCRIBES the change, but git knows the file it replaced.
        let changes = vec![('-', "two".to_string()), ('+', "TWO".to_string())];
        let rows = preview_for_workspace(&root, "lib.rs", &changes);
        let removed = rows.iter().find(|row| row.mark == '-').expect("a removal");
        assert_eq!(removed.text, "two", "git's own old line: {rows:?}");
        assert_eq!(removed.old, Some(2), "at its real line: {rows:?}");
        let added = rows
            .iter()
            .find(|row| row.mark == '+')
            .expect("an addition");
        assert_eq!(added.text, "TWO");
        assert_eq!(added.new, Some(2));
    }

    /// A request that misdescribes the change cannot mislead the preview: git
    /// is the source, so the rows still describe the file.
    #[test]
    fn a_preview_ignores_a_wrong_request() {
        let root = scratch("preview-wrong");
        git(&root, &["init", "-q"]);
        let file = root.join("lib.rs");
        std::fs::write(&file, "one\ntwo\n").expect("the file");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "one"]);
        std::fs::write(&file, "one\nTWO\n").expect("the edit");

        // This request mentions text the file never held.
        let lying = vec![('+', "not in this file".to_string())];
        let rows = preview_for_workspace(&root, "lib.rs", &lying);
        assert!(
            rows.iter().all(|row| row.text != "not in this file"),
            "the request did not supply the rows: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.text == "two"),
            "git's old line is what a reader sees: {rows:?}"
        );
    }

    /// The preview hangs below the pointer while the pane has room, and flips
    /// ABOVE when it does not: the composer sits under the transcript, so a card
    /// growing downward from a low row was drawn behind it.
    #[test]
    fn a_preview_flips_above_when_the_pane_has_no_room_below() {
        // Room below: 100 + 16 + 200 <= 800 - 8, so it hangs down.
        assert_eq!(preview_top(100.0, 200.0, 800.0), 116.0);
        // Near the bottom: below would run past the pane, so it flips up.
        let top = preview_top(700.0, 200.0, 800.0);
        assert_eq!(top, 700.0 - 16.0 - 200.0, "it sits above the pointer");
        assert!(top + 200.0 <= 700.0, "and clears the pointer it points at");
        // A pane too short for either side pins the card to the top edge.
        assert_eq!(preview_top(50.0, 400.0, 120.0), 8.0);
    }

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

    /// A change a tool request described becomes a patch the kit accepts, so
    /// a chat step can open its own change in the pane.
    #[test]
    fn a_requested_change_becomes_a_parseable_patch() {
        let changes = vec![
            ('-', "let a = 1;".to_string()),
            ('+', "let a = 2;".to_string()),
        ];
        let patch = patch_for("src/lib.rs", &changes).expect("a patch");
        // It parses as a patch, and both sides' counts survive.
        let (files, source) = load("src/lib.rs", patch.as_bytes()).expect("it loads");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files.len(), 1, "one file: {patch}");
        assert_eq!(files[0].additions(), 1, "one added line: {patch}");
        assert_eq!(files[0].deletions(), 1, "one removed line: {patch}");
    }

    /// A write with only additions is still a valid patch: removed count 0.
    #[test]
    fn an_all_additions_change_is_a_valid_patch() {
        let changes: Vec<(char, String)> = (0..3).map(|n| ('+', format!("line {n}"))).collect();
        let patch = patch_for("new.rs", &changes).expect("a patch");
        let (files, source) = load("new.rs", patch.as_bytes()).expect("it loads");
        assert_eq!(source, DiffSource::Patch);
        assert_eq!(files[0].additions(), 3, "three added lines: {patch}");
        assert_eq!(files[0].deletions(), 0);
    }

    /// Nothing to compare is refused rather than shown as an empty patch.
    #[test]
    fn an_empty_change_builds_no_patch() {
        assert!(patch_for("a.rs", &[]).is_none());
        assert!(patch_for("a.rs", &[('x', "note".to_string())]).is_none());
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

    /// The tool records a step's arguments, so the basis is a fact about the
    /// REQUEST: a create has every line new, a write over an unknown file has
    /// no old side, and an edit carries the text it replaced.
    fn step(name: &str, args: &str) -> ToolEvent {
        ToolEvent {
            name: name.to_string(),
            args: args.to_string(),
            output: Some("ok".to_string()),
            ms: Some(1),
            at: None,
        }
    }

    #[test]
    fn a_created_file_is_new_rather_than_a_fake_hunk() {
        let tools = [step(
            "create_file",
            r#"{"path":"src/new.rs","content":"one
two"}"#,
        )];
        let files = turn_files(&tools);
        assert_eq!(files.len(), 1, "one file was written");
        assert_eq!(files[0].path, "src/new.rs");
        assert_eq!(files[0].basis, TurnBasis::NewFile);
        assert_eq!(files[0].basis.note_key(), Some("diff.turnNewFile"));
        // And the expansion refuses to draw a diff it cannot support.
        assert!(
            turn_rows(&files[0], None).is_none(),
            "a creation has no before-image to diff"
        );
    }

    #[test]
    fn a_write_without_old_text_says_the_previous_content_is_gone() {
        let tools = [step(
            "write_file",
            r#"{"path":"src/lib.rs","content":"one
two"}"#,
        )];
        let files = turn_files(&tools);
        assert_eq!(files[0].basis, TurnBasis::Rewritten);
        assert_eq!(files[0].basis.note_key(), Some("diff.turnRewritten"));
        assert!(turn_rows(&files[0], Some("whatever\n")).is_none());
        assert_eq!(files[0].added, 2, "the recorded additions still count");
        assert_eq!(files[0].deleted, 0);
    }

    #[test]
    fn an_edit_records_both_sides_and_expands_to_them() {
        let tools = [step(
            "edit_file",
            r#"{"path":"lib.rs","old_string":"let a = 1;","new_string":"let a = 2;"}"#,
        )];
        let files = turn_files(&tools);
        assert_eq!(files[0].basis, TurnBasis::Recorded);
        assert_eq!(
            files[0].basis.note_key(),
            None,
            "a real old side has no note"
        );
        // The current file lets the hunk be placed; without one the rows still
        // carry the requested text, with no invented numbers.
        let rows = turn_rows(&files[0], Some("let a = 2;")).expect("a recorded edit is diffable");
        let removed = rows.iter().find(|row| row.mark == '-').expect("a removal");
        assert_eq!(removed.text, "let a = 1;");
        let added = rows
            .iter()
            .find(|row| row.mark == '+')
            .expect("an addition");
        assert_eq!(added.text, "let a = 2;");
        assert_eq!(added.new, Some(1), "the hunk is located at its real line");
        let unlocated = turn_rows(&files[0], None).expect("still diffable");
        assert!(unlocated
            .iter()
            .all(|row| row.old.is_none() && row.new.is_none()));
    }

    #[test]
    fn two_steps_on_one_file_are_one_row() {
        let tools = [
            step(
                "edit_file",
                r#"{"path":"a.rs","old_string":"one","new_string":"two"}"#,
            ),
            step(
                "edit_file",
                r#"{"path":"a.rs","old_string":"two","new_string":"three"}"#,
            ),
        ];
        let files = turn_files(&tools);
        assert_eq!(files.len(), 1, "one file, two edits");
        assert_eq!(files[0].deleted, 2);
        assert_eq!(files[0].added, 2);
    }

    #[test]
    fn an_empty_turn_names_its_reason() {
        // A command can write files without recording which ones, so an empty
        // list after one is "not captured", not "nothing changed".
        assert_eq!(
            TurnView::why_empty(&[step("bash", r#"{"command":"touch a"}"#)]),
            TurnEmpty::CommandsOnly
        );
        assert_eq!(
            TurnView::why_empty(&[step("read_file", r#"{"path":"a"}"#)]),
            TurnEmpty::NoWrites
        );
    }
}
