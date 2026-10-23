//! The conversation, the trajectory, and the empty states.

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::tooltip::Tooltip;

/// What a click on a link in a message does.
///
/// The reference routes this through its `linkOpening` preference: `new-tab`
/// hands the URL to the system browser and `sidebar` loads it in the in-app
/// sidebar browser. This shell has no in-app browser host - `gpui-pre` ships no
/// webview - so both values land on the external channel, which is the one link
/// seam `App::open_url` actually offers. Before this the text views carried no
/// handler at all, so a link was inert.
fn open_link(url: &SharedString, _event: &ClickEvent, _window: &mut Window, cx: &mut App) {
    cx.open_url(url);
}
use gpui_kit::*;
use std::sync::Arc;

use super::shell::{ChangesHover, Shell};
use super::todo_panel::spinner;
use super::{glyph, icon, txt, DivExt, IconName};
use crate::i18n::{tr, Lang};
use crate::model::{ChatMessage, PlanStatus, Role, Session, ToolEvent};
use crate::settings::TranscriptPolicy;
use crate::theme;

/// Body-cache kinds: the transcript draws several independent markdown
/// surfaces, and each keeps its own parsed entry.
const BODY: u8 = 0;
const THINK: u8 = 1;
const COMPACTION: u8 = 2;
const STEP: u8 = 3;

/// A reasoning block's collapsed summary: the first line, with the emphasis
/// markers stripped.
///
/// `ReasoningRow` uses `firstLine(text).replaceAll("**", "")` for a settled block
/// and the last completed paragraph's first line while it streams; bsh has no
/// per-paragraph streaming state, so the settled rule is the drawn one.
pub(crate) fn think_summary(text: &str) -> String {
    text.lines().next().unwrap_or_default().replace("**", "")
}

/// Cap one rail preview the way the reference does: join the parts with a
/// space, collapse every whitespace run, then clip at `limit` with a trailing
/// ellipsis (`turn-navigation.js::preview`, over `PROMPT_PREVIEW_LIMIT` and
/// `RESPONSE_PREVIEW_LIMIT`).
///
/// The reference counts UTF-16 units; bsh counts characters, which differs only
/// for astral text.
pub(crate) fn rail_preview(parts: &[String], limit: usize) -> String {
    let mut text = String::new();
    let mut unread = false;
    for part in parts {
        if text.chars().count() >= limit * 2 {
            unread = true;
            break;
        }
        let clipped = part.chars().count() > limit * 2;
        let chunk: String = if clipped {
            part.chars().take(limit * 2).collect()
        } else {
            part.clone()
        };
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&chunk);
        if clipped {
            unread = true;
            break;
        }
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() > limit - 1 {
        let head: String = normalized.chars().take(limit - 1).collect();
        return format!("{}…", head.trim_end());
    }
    if unread {
        format!("{normalized}…")
    } else {
        normalized
    }
}

/// The line icon for a tool, by the name it registers under.
fn tool_icon(name: &str) -> IconName {
    match name {
        "bash" => IconName::SquareTerminal,
        "read_file" => IconName::FileText,
        "write_file" => IconName::FilePen,
        "list_dir" => IconName::Folder,
        "load_skill" => IconName::Sparkles,
        "update_plan" => IconName::ClipboardList,
        _ => IconName::Wrench,
    }
}

/// The kind word a tool row shows in place of its raw tool name.
///
/// The reference labels a step by what it DID - Code, Think, Bash, Read,
/// Write, Edit - rather than by the registry name of the tool that did it, so
/// two providers' differently-named file writers read as one kind.
pub(crate) fn tool_kind_key(name: &str) -> &'static str {
    match name {
        "bash" | "run_command" | "terminal" => "tool.kind.bash",
        "read_file" => "tool.kind.read",
        "write_file" => "tool.kind.write",
        "edit_file" | "apply_patch" => "tool.kind.edit",
        "list_dir" => "tool.kind.list",
        "load_skill" => "tool.kind.skill",
        "update_plan" => "tool.kind.plan",
        _ => "tool.kind.other",
    }
}

/// Whether a value carries anything worth showing as a summary.
fn meaningful(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_alphanumeric() || ch == '/' || ch == '_')
}
/// The one-line summary a tool row carries beside its kind.
///
/// The arguments arrive as the JSON the model sent, so the useful line is the
/// first well-known field's value (a command, a path, a query) rather than the
/// whole object; anything else falls back to the first string value, and then
/// to the raw arguments. Newlines collapse and long values clip, because this
/// line is a summary and the body holds the whole thing.
pub(crate) fn tool_summary(args: &str) -> String {
    const KEYS: [&str; 7] = [
        "command",
        "path",
        "file_path",
        "query",
        "code",
        "instruction",
        "prompt",
    ];
    for key in KEYS {
        if let Some(value) = json_field(args, key) {
            return clip_summary(&value);
        }
    }
    match first_json_string(args) {
        Some(value) => clip_summary(&value),
        None if !meaningful(args) => String::new(),
        None => clip_summary(args),
    }
}

/// The exact path a file tool was given.
///
/// The row's summary is CLIPPED for display (96 chars and a ellipsis), so using
/// it to open a file breaks on any path longer than that - the panel then
/// resolves a path that does not exist. This reads the argument itself.
pub(crate) fn tool_path(args: &str) -> Option<String> {
    for key in ["file_path", "path", "file"] {
        if let Some(value) = json_field(args, key) {
            return Some(value);
        }
    }
    None
}

/// The string value of `key` in an object literal, ignoring escapes' contents.
fn json_field(args: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = args.find(&needle)? + needle.len();
    let rest = args[at..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    quoted_span(rest).map(|(value, _)| value)
}

/// The value of `key` with its escapes decoded EXACTLY, so a body keeps its
/// newlines - a summary flattens them, a patch must not.
fn json_field_exact(args: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = args.find(&needle)? + needle.len();
    let rest = args[at..].trim_start().strip_prefix(':')?.trim_start();
    quoted_exact(rest)
}

/// Read one quoted string, decoding `\n` and `\t` as themselves.
fn quoted_exact(text: &str) -> Option<String> {
    let mut chars = text.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return None,
    }
    let mut value = String::new();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '"' => return Some(value),
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 't')) => value.push('\t'),
                Some((_, other)) => value.push(other),
                None => break,
            },
            other => value.push(other),
        }
    }
    None
}
/// The first quoted VALUE in the arguments, whatever its key.
///
/// A key is also a quoted string, so a span followed by `:` is skipped: the
/// first span whose remainder is not a colon is a value.
fn first_json_string(args: &str) -> Option<String> {
    let mut rest = args;
    while let Some((open, _)) = rest.char_indices().find(|(_, c)| *c == '"') {
        let tail = &rest[open..];
        let (value, after) = quoted_span(tail)?;
        if !after.trim_start().starts_with(':') {
            return Some(value);
        }
        rest = after;
    }
    None
}

/// Read one quoted string off the front of `text`, with what follows it.
fn quoted_span(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    match chars.next() {
        Some((_, '"')) => {}
        _ => return None,
    }
    let mut value = String::new();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '"' => return Some((value, &text[index + 1..])),
            '\\' => match chars.next() {
                Some((_, next)) => value.push(match next {
                    'n' | 't' => ' ',
                    other => other,
                }),
                None => break,
            },
            other => value.push(other),
        }
    }
    None
}
/// The language a step's body should be highlighted as, when the step carries
/// code at all.
///
/// A shell step's body IS the command, so it is bash; a file step's body is the
/// file it wrote or patched, so the language comes from that path's extension.
/// Nothing else claims a language, and a body with none renders as plain text
/// rather than pretending.
pub(crate) fn code_lang(name: &str, args: &str) -> Option<&'static str> {
    match name {
        "bash" | "run_command" | "terminal" => Some("bash"),
        "write_file" | "edit_file" | "apply_patch" | "read_file" => {
            let path = json_field(args, "path")
                .or_else(|| json_field(args, "file_path"))
                .unwrap_or_default();
            Some(match path.rsplit('.').next().unwrap_or_default() {
                "rs" => "rust",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" | "mjs" => "javascript",
                "py" => "python",
                "go" => "go",
                "json" => "json",
                "toml" => "toml",
                "yml" | "yaml" => "yaml",
                "md" => "markdown",
                "css" => "css",
                "html" => "html",
                "sh" => "bash",
                _ => {
                    // A file step with no language still has a body worth
                    // showing; markdown is the neutral fence that keeps it
                    // readable without claiming syntax it does not have.
                    "markdown"
                }
            })
        }
        _ => None,
    }
}

/// The bytes a step moved, for the size badge the reference puts on its rows.
///
/// It is the size of the payload the step carried - its output when it has one,
/// its arguments while it runs - not a disk size: bsh keeps no file handle, and
/// a badge that read the disk would be a second source of truth for the same
/// row.
pub(crate) fn tool_bytes(tool: &ToolEvent) -> usize {
    tool.output
        .as_deref()
        .map(str::len)
        .unwrap_or_else(|| tool.args.len())
}

/// A byte count the way a row badge shows it: 812 B, 7.2 KB.
pub(crate) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    }
}

/// A tool's output as the body should read it: the streams a command returned,
/// not the JSON envelope around them.
///
/// The shell tool answers `{stdout, stderr, exit_code, success}`, and showing
/// that object puts `\n` escapes in front of a reader who wants the text. A
/// failing command keeps its stderr in the same body - the red tone is the
/// row's own signal, and the body does not hide why it failed.
pub(crate) fn output_body(output: &str) -> String {
    let stdout = json_field_exact(output, "stdout");
    let stderr = json_field_exact(output, "stderr");
    if stdout.is_none() && stderr.is_none() {
        return output.to_string();
    }
    let mut text = stdout.unwrap_or_default();
    let err = stderr.unwrap_or_default();
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&err);
    }
    let trimmed = text.trim_end().to_string();
    if trimmed.is_empty() {
        // A silent success is still a result: say so rather than drawing an
        // empty box.
        return "(no output)".to_string();
    }
    trimmed
}
impl Shell {
    /// The text a step's path holds on disk now, when it can be read.
    ///
    /// A step records the path the model wrote, which may be absolute; the
    /// workspace loader resolves relative paths only, so an absolute one inside
    /// the workspace is trimmed to its relative form first. `None` is the honest
    /// answer for a file that moved, was deleted, or sits outside the workspace -
    /// the caller then falls back to the synthetic patch header.
    pub(crate) fn current_text_of(&self, path: &str) -> Option<String> {
        let root = self.state.workspace_root()?;
        let candidate = std::path::Path::new(path);
        let rel = match candidate.strip_prefix(&root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => candidate.to_path_buf(),
        };
        let bytes = crate::files::read(&root, &rel).ok()?;
        String::from_utf8(bytes).ok()
    }
}
/// The patch a write or an edit asked for, as `-` / `+` lines.
///
/// This is the SHAPE of the change the model requested, not a disk diff: bsh
/// keeps the arguments and no before-image, so a write shows its content as
/// additions and an edit shows the string it replaced against the one it put
/// there. It says what the step did; it does not pretend to know what the file
/// held.
pub(crate) fn tool_diff(name: &str, args: &str) -> Option<Vec<(char, String)>> {
    match name {
        "write_file" => {
            let content = json_field_exact(args, "content")?;
            Some(
                content
                    .lines()
                    .map(|line| ('+', line.to_string()))
                    .collect(),
            )
        }
        "edit_file" | "apply_patch" => {
            let mut lines: Vec<(char, String)> = Vec::new();
            if let Some(old) = json_field_exact(args, "old_string") {
                lines.extend(old.lines().map(|line| ('-', line.to_string())));
            }
            if let Some(new) = json_field_exact(args, "new_string") {
                lines.extend(new.lines().map(|line| ('+', line.to_string())));
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines)
            }
        }
        _ => None,
    }
}
/// The file-writing tools a turn's steps can carry.
///
/// The reference does not read this off the tool registry either: it asks the
/// host for the turn's changed files so a write through any pluggable file tool
/// or a shell command that touched a file, is the same fact. bsh has no such
/// host record, so the honest source is the step's own request, exactly like
/// `tool_diff`. The three names are what this crate's own tool switch already
/// treats as a write.
pub(crate) fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "write_file" | "edit_file" | "apply_patch" | "write" | "edit" | "patch" | "create_file"
    )
}

/// One file a turn wrote, as the changed-files card reports it.
///
/// `added` and `deleted` count the LINES the request carried, and `diff` is the
/// same shape a step row shows. Both come from the arguments, so a card here is
/// a summary of what the turn ASKED for - never a claim about what the disk
/// held, which bsh cannot know without a before-image.
#[derive(Clone)]
pub(crate) struct ChangedFile {
    /// The path exactly as the step named it.
    pub(crate) path: String,
    /// Lines the request added.
    pub(crate) added: usize,
    /// Lines the request removed.
    pub(crate) deleted: usize,
    /// The requested change, as `+` / `-` lines for the hover preview.
    pub(crate) diff: Vec<(char, String)>,
}

/// The files one message's own steps wrote, in the order they were touched.
///
/// Two edits to one file are ONE file, the way the reference's turn summary
/// groups them: the counts add up and the preview shows both changes in order.
/// A step with no readable path or no readable change is skipped rather than
/// shown as a nameless row.
pub(crate) fn turn_changes(tools: &[ToolEvent]) -> Vec<ChangedFile> {
    let mut files: Vec<ChangedFile> = Vec::new();
    for tool in tools {
        if !is_write_tool(&tool.name) {
            continue;
        }
        let Some(diff) = tool_diff(&tool.name, &tool.args) else {
            continue;
        };
        let Some(path) = tool_path(&tool.args) else {
            continue;
        };
        let added = diff.iter().filter(|(mark, _)| *mark == '+').count();
        let deleted = diff.iter().filter(|(mark, _)| *mark == '-').count();
        match files.iter_mut().find(|file| file.path == path) {
            Some(file) => {
                file.added += added;
                file.deleted += deleted;
                file.diff.extend(diff);
            }
            None => files.push(ChangedFile {
                path,
                added,
                deleted,
                diff,
            }),
        }
    }
    files
}

/// The file name a single-file card titles itself with.
///
/// Measured: the reference's `changes.singleTitle` names the BASENAME, because
/// the header's tile already carries the file's own kind icon and the full path
/// would ellipsise the one word that identifies it.
pub(crate) fn base_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// Collapse a value to one clipped line.
fn clip_summary(text: &str) -> String {
    let one_line = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    const MAX: usize = 96;
    if one_line.chars().count() <= MAX {
        return one_line;
    }
    let clipped: String = one_line.chars().take(MAX).collect();
    format!("{clipped}…")
}

/// How much further a mouse-wheel notch travels than a trackpad's own delta.
///
/// A trackpad sends precise pixels and its hardware already carries momentum, so
/// those go through 1:1. A mouse wheel sends whole LINES, one coarse step per
/// detent - at 1:1 a long transcript takes dozens of notches to cross, which is
/// what "the scroll needs to be fast" was about. Two is the smallest gain that
/// makes one notch a comfortable step without landing somewhere unrelated.
const MOUSE_GAIN: f32 = 2.0;

/// One wheel event's vertical delta in pixels.
///
/// Both kinds are resolved against the window's line height - the same conversion
/// the scroller itself would have done - and only the coarse one is gained.
fn wheel_delta(event: &ScrollWheelEvent, window: &mut Window) -> f32 {
    wheel_pixels(&event.delta, window.line_height())
}

/// The gained pixel delta for one wheel delta, split out so a test can pin it.
pub(crate) fn wheel_pixels(delta: &ScrollDelta, line_height: Pixels) -> f32 {
    let pixels = f32::from(delta.pixel_delta(line_height).y);
    match delta {
        ScrollDelta::Lines(_) => pixels * MOUSE_GAIN,
        _ => pixels,
    }
}

/// The chat transcript's wheel handler, named so the element chain stays small.
fn chat_wheel(
    this: &mut Shell,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    let dy = wheel_delta(event, window);
    this.wheel_chat(dy, cx);
    cx.stop_propagation();
}

/// The trajectory tab's wheel handler.
pub(crate) fn traj_wheel(
    this: &mut Shell,
    event: &ScrollWheelEvent,
    window: &mut Window,
    cx: &mut Context<Shell>,
) {
    let dy = wheel_delta(event, window);
    this.wheel_trajectory(dy, cx);
    cx.stop_propagation();
}

/// One transcript row's measured height, kept between frames.
///
/// The transcript is a flex column inside a `ScrollHandle` scroller whose child
/// indices ARE the rows (child 0 is the timestamp, then one per message), so a
/// row the viewport does not need is replaced by a spacer of its measured
/// height. Every consumer that addresses a child - the rail's
/// `scroll_to_top_of_item`, the paging probe, the tests - keeps working, while a
/// frame BUILDS only the rows near the viewport instead of the whole
/// conversation. A 400-message chat therefore costs its viewport, not the
/// transcript, on every repaint.
#[derive(Default)]
pub(crate) struct TranscriptLayout {
    /// The session shape the cached heights belong to.
    shape: Option<TranscriptShape>,
    /// Where `heights[1]` (the first loaded message) sits in the session.
    loaded_from: usize,
    /// One measured height per row, in scroller-child order.
    heights: Vec<Option<f32>>,
    /// Recently laid-out sessions, so switching back is a paint, not a
    /// measure. Newest first, capped at `SAVED_LAYOUTS`.
    saved: Vec<(TranscriptShape, usize, Vec<Option<f32>>)>,
    /// A one-shot request to re-measure every row exactly, raised when a width
    /// drag ENDS (during the drag the old heights are kept, so dragging a long
    /// chat does not re-measure it on every pointer move).
    remeasure: bool,
}

/// What a cached height is keyed on: the facts that decide whether the cached
/// row layout is still the one the height was measured for.
///
/// The session's `loaded_from` is deliberately NOT part of this: paging shifts
/// the rows by a known amount, so the measured heights stay valid and only the
/// prepended rows need measuring.
#[derive(Clone, PartialEq)]
struct TranscriptShape {
    session: String,
    /// The content width in bits: every wrapped line depends on it.
    width: u32,
    content_size: u32,
    lang: Lang,
    /// The whole transcript policy: fold, step grouping and the live-process
    /// rules all change how tall a turn draws.
    policy: TranscriptPolicy,
    compact: bool,
}

impl TranscriptShape {
    /// Whether two sessions' rows would lay out the same way.
    fn same_geometry(&self, other: &Self) -> bool {
        self.width == other.width && self.same_geometry_except_width(other)
    }

    /// The same question without the width: a drag changes the width every
    /// pointer move, and re-measuring the transcript on each of those frames is
    /// exactly what the window is meant to avoid.
    fn same_geometry_except_width(&self, other: &Self) -> bool {
        self.content_size == other.content_size
            && self.lang == other.lang
            && self.policy == other.policy
            && self.compact == other.compact
    }
}

/// What the cache could reuse this frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prepared {
    /// Every cached height is valid; the last frame's painted bounds are too.
    Exact,
    /// The heights are valid but the ROWS MOVED (a paged window was prepended
    /// or trimmed, or the session changed to a remembered layout), so last
    /// frame's child bounds address different rows. Decide visibility from the
    /// heights instead.
    Stale,
    /// Nothing was reusable: every row must be built to be measured. This is
    /// the one frame a new session (or a new width or font) costs.
    MeasureAll,
}

impl TranscriptLayout {
    /// Ask for one exact re-measure on the next frame (a width drag ended).
    pub(crate) fn request_remeasure(&mut self) {
        self.remeasure = true;
    }

    /// Reconcile the cache with the session about to be drawn.
    ///
    /// Three transitions are cheap and one is not. A plain append (a new turn)
    /// extends the table. A paged window prepended at the front SHIFTS the
    /// measured heights, so only the new page needs building. A session switch
    /// restores that session's last layout when its geometry still matches.
    /// Only a genuinely new session or geometry re-measures every row once.
    fn prepare(
        &mut self,
        shape: TranscriptShape,
        loaded_from: usize,
        rows: usize,
        freeze_width: bool,
    ) -> Prepared {
        if std::mem::take(&mut self.remeasure) {
            self.shape = Some(shape);
            self.loaded_from = loaded_from;
            self.heights = vec![None; rows];
            #[cfg(test)]
            LAYOUT_RESETS.with(|resets| resets.set(resets.get() + 1));
            return Prepared::MeasureAll;
        }
        let mut stale = false;
        // A resize drag changes the width every frame; the heights measured at
        // the PREVIOUS width are kept for the duration, and the rows on screen
        // re-measure themselves as they rebuild. The commit asks for the exact
        // pass.
        let frozen = freeze_width
            && self.shape.as_ref().is_some_and(|current| {
                current.session == shape.session && current.same_geometry_except_width(&shape)
            });
        if self.shape.as_ref() != Some(&shape) && !frozen {
            if let Some(old) = self.shape.take() {
                self.saved
                    .retain(|(saved, _, _)| saved.session != old.session);
                self.saved.insert(
                    0,
                    (old, self.loaded_from, std::mem::take(&mut self.heights)),
                );
                self.saved.truncate(SAVED_LAYOUTS);
            }
            let restored = self.saved.iter().position(|(saved, _, _)| {
                saved.session == shape.session && saved.same_geometry(&shape)
            });
            match restored {
                Some(index) => {
                    let (saved_shape, saved_from, heights) = self.saved.remove(index);
                    self.shape = Some(TranscriptShape {
                        session: shape.session,
                        ..saved_shape
                    });
                    self.loaded_from = saved_from;
                    self.heights = heights;
                    stale = true;
                }
                None => {
                    self.shape = Some(shape);
                    self.loaded_from = loaded_from;
                    self.heights = vec![None; rows];
                    #[cfg(test)]
                    LAYOUT_RESETS.with(|resets| resets.set(resets.get() + 1));
                    return Prepared::MeasureAll;
                }
            }
        }
        // A paged read moves the window's front: the rows that were measured are
        // the SAME rows, shifted by a known number, so their heights carry over.
        let delta = self.loaded_from as isize - loaded_from as isize;
        if delta != 0 {
            let timestamp = self.heights.first().copied().flatten();
            let mut messages: Vec<Option<f32>> = if self.heights.is_empty() {
                Vec::new()
            } else {
                self.heights.split_off(1)
            };
            if delta > 0 {
                let added = (delta as usize).min(rows.saturating_sub(1));
                messages.splice(0..0, std::iter::repeat_n(None, added));
            } else {
                let dropped = (-delta) as usize;
                messages.drain(0..dropped.min(messages.len()));
            }
            self.heights = Vec::with_capacity(rows);
            self.heights.push(timestamp);
            self.heights.extend(messages);
            self.loaded_from = loaded_from;
            stale = true;
        }
        match rows.cmp(&self.heights.len()) {
            std::cmp::Ordering::Less => self.heights.truncate(rows),
            std::cmp::Ordering::Greater => self.heights.resize(rows, None),
            std::cmp::Ordering::Equal => {}
        }
        if stale {
            Prepared::Stale
        } else {
            Prepared::Exact
        }
    }

    /// Take the heights the last painted frame measured.
    ///
    /// Every child is a row, and a spacer is only ever given a height that was
    /// already MEASURED (an unmeasured row is built instead), so reading the
    /// spacers back is a no-op and reading the built rows is the new
    /// measurement.
    fn observe(&mut self, scroll: &ScrollHandle) {
        for (index, height) in self.heights.iter_mut().enumerate() {
            if let Some(bounds) = scroll.bounds_for_item(index) {
                *height = Some(bounds.size.height.as_f32());
            }
        }
    }

    /// Adopt one session's measured rows for another that renders the same.
    ///
    /// A branch holds the messages of the conversation it was cut from, at the
    /// same width, content size and policy, so every height already measured for
    /// the source is the height the branch's row will take. Without this the
    /// branch is a genuinely NEW session: the first frame finds no cache, falls
    /// back to `MeasureAll`, and builds every row of a long chat - the visible
    /// stall right after the click.
    pub(crate) fn adopt(&mut self, from: &str, to: &str) {
        let source = if self
            .shape
            .as_ref()
            .is_some_and(|shape| shape.session == from)
        {
            self.shape
                .clone()
                .map(|shape| (shape, self.loaded_from, self.heights.clone()))
        } else {
            None
        }
        .or_else(|| {
            self.saved
                .iter()
                .find(|(shape, _, _)| shape.session == from)
                .cloned()
        });
        let Some((shape, loaded_from, heights)) = source else {
            return;
        };
        if heights.is_empty() {
            return;
        }
        let adopted = TranscriptShape {
            session: to.to_string(),
            ..shape
        };
        self.saved
            .retain(|(saved, _, _)| saved.session != adopted.session);
        self.saved.insert(0, (adopted, loaded_from, heights));
        self.saved.truncate(SAVED_LAYOUTS);
    }

    /// A row height to use before a row has ever been painted.
    ///
    /// The average of what HAS been measured is a far better placeholder than a
    /// constant for the scrollbar, and it is only ever used to place rows a
    /// frame has chosen not to build.
    fn estimate(&self) -> f32 {
        let mut total = 0.0;
        let mut count = 0usize;
        for height in self.heights.iter().flatten() {
            total += height;
            count += 1;
        }
        if count == 0 {
            64.0
        } else {
            (total / count as f32).clamp(24.0, 240.0)
        }
    }

    /// The cached height of one row.
    fn height(&self, index: usize) -> Option<f32> {
        self.heights.get(index).copied().flatten()
    }
}

/// Whether a message row carries images.
///
/// An image gallery can change a row's height AFTER it was measured, when a
/// data URL finishes decoding, so such a row is never culled.
fn has_images(session: &Session, index: usize) -> bool {
    index > 0
        && session
            .messages
            .get(index - 1)
            .is_some_and(|message| !message.images.is_empty())
}

/// A row kept at its measured height without building its contents.
fn transcript_spacer(height: f32) -> AnyElement {
    div()
        .flex_none()
        .w_full()
        .h(px(height.max(0.0)))
        .into_any_element()
}

/// Rows this close to the viewport are built too, so a small scroll never
/// reveals an empty frame.
const TRANSCRIPT_OVERDRAW: f32 = 480.0;
/// The scroller's top padding and the gap between rows, in one definition:
/// the window's geometry maths and the element tree must agree.
const TRANSCRIPT_PAD_Y: f32 = 24.0;
const TRANSCRIPT_GAP: f32 = 22.0;
/// How many sessions keep their measured layout for a fast switch back.
const SAVED_LAYOUTS: usize = 8;
/// The newest rows are always built: a stream writes into them and a turn's
/// fold state changes at the tail.
const TRANSCRIPT_TAIL_BUILT: usize = 4;

// How many message rows the last frame actually built, for the performance
// test. Per-thread, because tests run in parallel inside one process.
#[cfg(test)]
thread_local! {
    static ROWS_BUILT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Frames that had to re-measure the whole transcript. A paged read or a
    /// switch back must not add one.
    static LAYOUT_RESETS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The message rows built so far on this thread.
#[cfg(test)]
pub(crate) fn transcript_rows_built() -> usize {
    ROWS_BUILT.with(|built| built.get())
}

/// How many frames re-measured the whole transcript on this thread.
#[cfg(test)]
pub(crate) fn transcript_layout_resets() -> usize {
    LAYOUT_RESETS.with(|resets| resets.get())
}

/// The presentation facts every row of one frame shares.
///
/// A transcript resolves its settings once per frame: the policy, whether the
/// usage figure is suppressed, the content font size and the markdown style all
/// describe the same repaint, and asking per row turned a long conversation into
/// hundreds of resolutions and style allocations per frame.
struct RowChrome<'a> {
    /// How much of a completed turn the transcript view shows.
    policy: TranscriptPolicy,
    /// Whether the Compact performance preference hides the footer's usage.
    compact_usage: bool,
    /// The content font size the row's type scale derives from.
    content_size: u32,
    /// The markdown style every text view in this frame copies.
    style: &'a TextViewStyle,
}

/// The state of the TURN one transcript row belongs to.
///
/// A turn can hold two assistant rows when a steer landed mid-turn, so "is this
/// turn settled?" and "is this the row still being written?" are separate
/// facts: the settled policy decides how much detail a finished turn keeps, and
/// only the newest row of the live turn may claim the "Running…" line.
#[derive(Clone, Copy, Default)]
struct RowTurn {
    /// The fold policy treats this row's turn as settled.
    settled: bool,
    /// The live turn is still writing into this row.
    writing: bool,
}

impl Shell {
    /// The reference's own markdown scale, measured from its theme tokens.
    ///
    /// Headings are additive there - h1 21px, h2 19px, h3 18px and h4 the
    /// body size, each plus `content - 14` - while table cells sit at the
    /// secondary size and code blocks keep the fixed 11px/19px the reference
    /// pins. Every other field keeps its default, which folds the active theme
    /// in; only the highlight theme and the `is_dark` flag are named here so a
    /// dark window does not get light code colours.
    pub(crate) fn markdown_style(&self) -> TextViewStyle {
        let content = self.state.draft().global.general.content_font_size;
        let dark = theme::is_dark();
        let table = |weight: Option<FontWeight>| {
            let cell = StyleRefinement::default()
                .text_size(px(theme::small_size_at(content)))
                .line_height(px(theme::table_line_at(content)));
            match weight {
                Some(weight) => cell.font_weight(weight),
                None => cell,
            }
        };
        TextViewStyle {
            heading_base_font_size: px(theme::body_size_at(content)),
            heading_font_size: Some(Arc::new(move |level, _base| {
                px(theme::heading_size_at(content, level))
            })),
            table: table(None),
            table_head: table(Some(FontWeight::MEDIUM)),
            table_cell: table(None),
            // The reference writes a code block in its code face
            // (`--ds-font-family-code`), so the block asks for the same face the
            // trajectory's tool rows use instead of inheriting the UI one.
            code_block: StyleRefinement::default()
                .text_size(px(theme::CODE_BLOCK_SIZE))
                .line_height(px(theme::CODE_LINE))
                .font_family(theme::CODE_FONT),
            highlight_theme: if dark {
                HighlightTheme::default_dark()
            } else {
                HighlightTheme::default_light()
            },
            is_dark: dark,
            ..TextViewStyle::default()
        }
    }

    /// Which message rows this frame must build.
    ///
    /// `Some(height)` culls the row to a same-sized spacer; `None` builds it.
    /// Only the rows inside the PREFETCH BAND are built: the visible area plus
    /// TRANSCRIPT_OVERDRAW above and below, plus row 0 and the newest few. A row
    /// outside the band is a spacer at its measured height, or at the
    /// transcript's estimate while it has never been measured - so no frame ever
    /// has to lay out the whole history.
    fn transcript_plan(
        &self,
        session: &Session,
        rows: usize,
        content_size: u32,
        compact: bool,
        policy: TranscriptPolicy,
    ) -> Vec<Option<f32>> {
        let mut layout = self.transcript_layout.borrow_mut();
        layout.prepare(
            TranscriptShape {
                session: session.id.clone(),
                width: self.chat_w().to_bits(),
                content_size,
                lang: self.state.lang,
                policy,
                compact,
            },
            session.loaded_from,
            rows,
            self.chat_drag.is_some() || self.sidebar_drag.is_some(),
        );
        // One prefetch band for every state. A row inside the band is built; a
        // row outside it is a spacer at its MEASURED height when it has one and
        // at the transcript's estimate when it does not. Unmeasured rows are no
        // longer forced to build: doing that made the first frame of a long chat
        // - and every frame after a paging shift - lay out the whole history.
        layout.observe(&self.transcript_scroll);
        let view = self.transcript_scroll.bounds();
        let view_h = f32::from(view.size.height);
        let view_top = -f32::from(self.transcript_scroll.offset().y);
        let estimate = layout.estimate();
        let mut plans = Vec::with_capacity(rows);
        let mut top = TRANSCRIPT_PAD_Y;
        for index in 0..rows {
            let measured = layout.height(index).filter(|height| *height >= 8.0);
            let row_h = measured.unwrap_or(estimate);
            let forced = index == 0 || index + TRANSCRIPT_TAIL_BUILT >= rows;
            let in_band = view_h > 0.0
                && top + row_h > view_top - TRANSCRIPT_OVERDRAW
                && top < view_top + view_h + TRANSCRIPT_OVERDRAW;
            // An image row keeps building: its gallery can grow when a data URL
            // finishes decoding, so a spacer would go stale.
            let build = forced || has_images(session, index) || in_band;
            // A row outside the band is a spacer at its MEASURED height, or at
            // the estimate while it has never been measured: pushing `measured`
            // here would push None, which the consumer reads as "build it", and
            // the whole history came back.
            plans.push(if build { None } else { Some(row_h) });
            top += row_h + TRANSCRIPT_GAP;
        }
        plans
    }

    /// The scrolling conversation.
    pub(crate) fn render_transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.state.active_session() else {
            return self.empty_state("empty.none").into_any_element();
        };
        if session.messages.is_empty() {
            // The reference draws its hero in the composer stack instead, so a
            // blank session leaves the transcript itself empty.
            return div().into_any_element();
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(txt(
                    crate::state::stamp(session.created_at),
                    theme::CHIP_SIZE,
                    theme::faint(),
                ))
                .child(
                    div()
                        .id("copy-transcript")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(theme::hover())))
                        .child(icon(IconName::Copy, 12.0, theme::faint()))
                        .child(txt(
                            self.state.lang.tr("msg.copy"),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        ))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            let text = this.state.transcript_text();
                            if text.is_empty() {
                                return;
                            }
                            cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(text));
                            let copied = this.state.lang.tr("chat.copied");
                            this.state.notice(copied);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        );
        // `foldCompletedTurns` folds every closed turn but never the one still
        // streaming, so the last turn stays open while it runs.
        //
        // One settings resolution and one markdown style serve the whole frame:
        // `effective()` clones a resolved workspace, and the style allocates its
        // heading closure, so asking once per row made a long conversation pay
        // hundreds of clones on every repaint.
        let effective = self.state.settings.effective();
        let style = self.markdown_style();
        let chrome = RowChrome {
            policy: effective.transcript_view,
            compact_usage: effective.performance_usage == "compact",
            content_size: self.state.draft().global.general.content_font_size,
            style: &style,
        };
        let folded = chrome.policy.fold_completed_turns;
        let live = self
            .state
            .active_id
            .as_deref()
            .is_some_and(|id| self.state.is_streaming(id));
        // The frame's window: rows near the viewport and the newest ones are
        // built; every other row paints as a spacer of its measured height, so
        // the cost of a repaint follows the viewport, not the transcript.
        let rows_len = 1 + session.messages.len();
        let plans = self.transcript_plan(
            session,
            rows_len,
            chrome.content_size,
            chrome.compact_usage,
            chrome.policy,
        );
        rows.reserve(
            rows_len + session.compactions.len() + usize::from(session.compact_command.is_some()),
        );
        let last = session.messages.len().saturating_sub(1);
        // Where the reference anchors a compaction marker: at the checkpoint
        // EVENT's sequence, which is appended after the shadowed span and the
        // retained tail, and BEFORE anything the reader adds later. bsh records
        // the transcript length at commit (`record.after`), so the marker is
        // drawn after that many messages. A legacy record (`after == 0`) draws
        // at the end, which is still the slot the reader looks at right after
        // the command; a boundary-based placement would hide it above a tail
        // they are not looking at.
        let mut markers: Vec<(usize, usize, &crate::model::CompactionRecord)> = session
            .compactions
            .iter()
            .enumerate()
            .map(|(marker, record)| {
                (
                    crate::state::compaction_marker_position(
                        record,
                        session.messages.len(),
                        session.loaded_from,
                    ),
                    marker,
                    record,
                )
            })
            .collect();
        markers.sort_by_key(|(position, _, _)| *position);
        let mut pending = markers.into_iter().peekable();
        // One scan for the newest PROMPT, so no row has to look ahead over the
        // messages below it to decide whether its action row is revealed. A
        // steering row is not a prompt: it does not push the prompt above it
        // out of the newest slot.
        let last_user = session
            .messages
            .iter()
            .rposition(|message| message.role == Role::User && !message.steering);
        for (index, message) in session.messages.iter().enumerate() {
            while pending
                .peek()
                .is_some_and(|(position, _, _)| *position <= index)
            {
                let (_, marker, record) = pending.next().expect("the marker just peeked");
                rows.push(
                    self.compaction_row(record, marker, &chrome, cx)
                        .into_any_element(),
                );
            }
            if let Some(height) = plans.get(index + 1).copied().flatten() {
                rows.push(transcript_spacer(height));
                continue;
            }
            let settled = folded && !(live && index == last);
            // ONE running indicator per turn: after a steer the turn holds two
            // assistant rows, and both would otherwise report "Running…" while
            // only the newest is still writing.
            let row = RowTurn {
                settled,
                writing: live && index == last,
            };
            rows.push(
                self.message_row(index, message, row, last_user, &chrome, cx)
                    .into_any_element(),
            );
        }
        // Markers committed at (or beyond) the current end, and legacy records,
        // still read after the last message.
        for (_, marker, record) in pending {
            rows.push(
                self.compaction_row(record, marker, &chrome, cx)
                    .into_any_element(),
            );
        }
        // The `/compact` command card is the reference's own row while it runs
        // and after any outcome that produced no checkpoint: title "compact",
        // the running label, or the outcome text. A landed checkpoint clears
        // it and the marker above takes its place.
        if let Some(command) = &session.compact_command {
            rows.push(
                self.compact_command_row(command, &chrome)
                    .into_any_element(),
            );
        }
        // A steered message shows in the conversation IMMEDIATELY, before the
        // running step takes it: the reference keeps it in the next-step inbox
        // and renders it as a pending steering bubble, which is how the reader
        // sees their interjection land. The durable row replaces it when the
        // `Steered` delivery arrives.
        for queued in self.state.pending_steers(&session.id) {
            rows.push(self.pending_steer_row(queued).into_any_element());
        }
        // The reference puts a turn-jump rail down the transcript's right edge.
        // The scroll handle records each child's painted bounds, so a tick can jump
        // to a measured row without knowing any row's height up front.
        let turns = self.turn_indices();
        // `if (items.length < 2) return null`, and `@container (width <= 900px)`
        // hides the frame. The container is the conversation column, measured
        // from the window; before the first frame it is not narrow.
        let narrow = self.conversation_w > px(0.0)
            && self.conversation_w <= px(theme::TURN_RAIL_MIN_CONTAINER);

        let active = self.visible_turn(&turns);
        let last_ordinal = turns.len().saturating_sub(1);
        // Measured (TurnNavigator): a 28px frame parked 12px off the transcript's
        // right edge, capped at `min(band - 64, 420)` and scrolled when the marks
        // overflow. A mark is a 10px slot holding a 2px x 20px bar, right-aligned,
        // drawn at 60% while idle (border-l4), 90% on hover (label-tertiary) and
        // 100% in label-primary for the turn at the top.
        // The frame is exactly this tall, and the track with it: a max_h did not
        // cap the column (a flex child grew past it), so the marks were painted
        // below the frame's own box - and a mark outside its box cannot be
        // clicked. The height is computed here because the scroller needs it now.
        let marks_h = theme::TURN_RAIL_INSET * 2.0 + theme::TURN_PITCH * turns.len() as f32;
        let frame_h = marks_h.min(theme::TURN_RAIL_MAX_H);
        // The marks are a uniform 10px pitch, so the visible band is arithmetic
        // rather than a measurement. A mark outside it paints as a same-sized
        // blank: a hundred-turn conversation used to build a hundred tick
        // elements (hover groups, ids, click handlers) on every frame although
        // the 420px frame could only ever show forty-two of them.
        let rail_overflow = marks_h > theme::TURN_RAIL_MAX_H;
        let rail_view_h = f32::from(self.rail_scroll.bounds().size.height);
        let rail_top = -f32::from(self.rail_scroll.offset().y);
        let rail_window = |turn: usize| -> bool {
            if !rail_overflow || rail_view_h <= 0.0 {
                return true;
            }
            let y = theme::TURN_RAIL_INSET + theme::TURN_PITCH * turn as f32;
            y + theme::TURN_PITCH > rail_top - theme::TURN_PITCH * 3.0
                && y < rail_top + rail_view_h + theme::TURN_PITCH * 3.0
        };
        let track = div()
            .debug_selector(|| "turn-rail-track".to_string())
            .id("turn-rail-track")
            .flex_none()
            .h(px(frame_h))
            // `RAIL_INSET_PX`: the mark column pads each end of the frame.
            .py(px(theme::TURN_RAIL_INSET))
            .overflow_y_scroll()
            .track_scroll(&self.rail_scroll)
            .flex()
            .flex_col()
            .items_end()
            .children(turns.iter().enumerate().map(|(turn, index)| {
                if !rail_window(turn) {
                    // A same-sized blank keeps the track's scroll height and
                    // every child index, so a jump and the tail bias still
                    // address the mark they mean.
                    return div()
                        .flex_shrink_0()
                        .w(px(theme::TURN_RAIL_W))
                        .h(px(theme::TURN_PITCH))
                        .into_any_element();
                }
                let child = 1 + index; // child 0 is the timestamp and copy row
                let on = active == Some(turn);
                let group = SharedString::from(format!("turn-{turn}"));
                let bar_group = group.clone();
                let is_last = turn == last_ordinal;
                div()
                    .debug_selector(|| format!("turn-tick-{turn}"))
                    .id(SharedString::from(format!("turn-tick-{turn}")))
                    .group(group)
                    // The reference positions its marks absolutely; in a flex
                    // column with 1 as the default shrink they would squeeze to
                    // fit the 420 cap instead of overflowing into the scroller.
                    .flex_shrink_0()
                    .w(px(theme::TURN_RAIL_W))
                    .h(px(theme::TURN_PITCH))
                    .flex()
                    .items_center()
                    .justify_end()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        // `TurnMark` previews on pointer move and the frame
                        // clears it on leave; a mark's own leave is the same
                        // observable here.
                        if *hovered {
                            this.rail_hover = Some(turn);
                        } else if this.rail_hover == Some(turn) {
                            this.rail_hover = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .debug_selector(|| format!("turn-bar-{turn}"))
                            .h(px(2.0))
                            .w(px(if on { 20.0 } else { 12.0 }))
                            .rounded(px(2.0))
                            .bg(rgb(if on { theme::text() } else { theme::border() }))
                            .when(!on, |bar| {
                                bar.group_hover(bar_group, |s| {
                                    s.w(px(18.0)).bg(rgb(theme::faint()))
                                })
                            }),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.jump_to_turn(child, is_last);
                        cx.notify();
                    }))
                    .into_any_element()
            }));
        // Measured: the navigator overlays the column at `right: 12px`, so the
        // transcript keeps its full width underneath it. The frame is the
        // positioned box the marks scroll in and the preview card hangs off;
        // `TurnNavigator` renders both from the frame, so the card is never
        // clipped by the scroller.
        let offset = f32::from(self.rail_scroll.offset().y);
        // `rail_hover` holds the mark's ORDINAL in the rail (that is what the
        // hook stores), while `turns` holds MESSAGE indices. Searching `turns`
        // for an ordinal only ever matched even marks, so every odd mark - the
        // newest one always among them - drew no card at all.
        let preview = self.rail_hover.and_then(|ordinal| {
            let turn = *turns.get(ordinal)?;
            // The card tracks the hovered mark's centre within the frame
            // (`--turn-preview-center`).
            let centre = theme::TURN_RAIL_INSET
                + theme::TURN_PITCH * ordinal as f32
                + theme::TURN_PITCH / 2.0
                + offset;
            Some((turn, centre))
        });
        let frame = div()
            .debug_selector(|| "turn-rail-frame".to_string())
            .relative()
            .w(px(theme::TURN_RAIL_W))
            .flex()
            .flex_col()
            .child(track)
            .children(preview.map(|(turn, centre)| self.turn_preview(turn, centre, frame_h)));
        // A single turn has nothing to navigate between, and the reference hides
        // the whole frame under 900px of conversation width.
        let rail = (turns.len() >= 2 && !narrow).then(|| {
            div()
                .debug_selector(|| "turn-rail".to_string())
                .id("turn-rail")
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(theme::TURN_RAIL_RIGHT))
                .w(px(theme::TURN_RAIL_W))
                .flex()
                .flex_col()
                .items_end()
                .justify_center()
                .child(frame)
                .into_any_element()
        });
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_row()
            .relative()
            .child(
                div()
                    .debug_selector(|| "transcript".to_string())
                    .id("transcript")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    // The wheel is taken here rather than by the scroller: the
                    // delta becomes a target the frame loop eases towards, and
                    // taking it also stops the tail-follow from pinning the
                    // transcript back to the newest message.
                    .on_scroll_wheel(cx.listener(chat_wheel))
                    .on_scroll_wheel(cx.listener(|_this, _event, _window, cx| cx.notify()))
                    // Copy on selection: the press records where it started, and
                    // a release that travelled far enough takes the window's
                    // selection to the clipboard. A click is not a selection, so
                    // it writes nothing even when an older selection is on screen.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                            this.selection_start = Some(event.position);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, window, cx| {
                            let Some(start) = this.selection_start.take() else {
                                return;
                            };
                            let dx = event.position.x.as_f32() - start.x.as_f32();
                            let dy = event.position.y.as_f32() - start.y.as_f32();
                            if !super::shell::selection_drag(dx, dy) {
                                return;
                            }
                            if this.copy_selection(window, cx) {
                                cx.notify();
                            }
                        }),
                    )
                    .px(px(theme::INSET))
                    .py(px(TRANSCRIPT_PAD_Y))
                    .flex()
                    .flex_col()
                    .gap(px(TRANSCRIPT_GAP))
                    .children(rows),
            )
            .children(rail)
            // The hover preview lives in this relative frame, not inside the
            // scroller: a scroller clips absolute children, so a card hanging off
            // a row would be cut at the viewport edge.
            .children(self.changes_preview_overlay(cx))
            // While a window of older rows is loading, the pane shows the
            // shared busy mark rather than a frame that looks idle. It is an
            // overlay, so it adds no child and the turn rail's child indices
            // keep naming the messages while it shows.
            .when(self.older.is_busy(), |frame| {
                frame.child(
                    div()
                        .debug_selector(|| "older-loading".to_string())
                        .absolute()
                        .top(px(6.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded_full()
                                .bg(rgb(theme::page()))
                                .child(spinner()),
                        ),
                )
            })
            .into_any_element()
    }

    /// The rail's hover preview card, from `TurnNavigator .preview`: at most
    /// `min(300px, container - 120px)` wide and 100px tall, filled with
    /// `bg-layer-1` under `elevation-panel`, holding a one-line 13px/20px strong
    /// prompt over up to three 12px/18px label-caption response lines.
    fn turn_preview(&self, turn: usize, centre: f32, frame_h: f32) -> AnyElement {
        let (prompt, response) = self.turn_preview_texts(turn);
        let prompt = if prompt.is_empty() {
            self.state
                .lang
                .tr("chat.railTurn")
                .replace("{turn}", &turn.to_string())
        } else {
            prompt
        };
        // `top: clamp(0px, calc(var(--turn-preview-center) - 50px), calc(100% - 100px))`
        let top = (centre - theme::TURN_PREVIEW_H / 2.0)
            .clamp(0.0, (frame_h - theme::TURN_PREVIEW_H).max(0.0));
        // `width: min(300px, 100cqw - 120px)`. The frame only exists above a
        // 900px container, where `100cqw - 120px` already exceeds 300, so the
        // measured width is the 300px cap.
        let width = theme::TURN_PREVIEW_W;
        div()
            .debug_selector(|| "turn-preview".to_string())
            .id("turn-preview")
            .absolute()
            .top(px(top))
            // `right: calc(100% + 10px)`
            .right(px(theme::TURN_RAIL_W + theme::TURN_PREVIEW_GAP))
            .w(px(width))
            .max_h(px(theme::TURN_PREVIEW_H))
            .overflow_hidden()
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(theme::page()))
            .shadow_lg()
            .px(px(12.0))
            .py(px(10.0))
            .child(
                txt(prompt, 13.0, theme::text())
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(20.0))
                    .max_h(px(20.0))
                    .overflow_hidden(),
            )
            .when(!response.is_empty(), |card| {
                card.child(
                    txt(response, 12.0, theme::faint())
                        .line_height(px(18.0))
                        .max_h(px(54.0))
                        .mt(px(4.0))
                        .overflow_hidden(),
                )
            })
            .into_any_element()
    }

    /// The rail item's two previews (`turn-navigation.js`): the turn's user
    /// prompt and the last assistant text in the turn that has any - the
    /// reference takes `loaded.findLast((node) => responseText(node) !== "")`.
    fn turn_preview_texts(&self, turn: usize) -> (String, String) {
        let Some(session) = self.state.active_session() else {
            return (String::new(), String::new());
        };
        let end = self
            .turn_indices()
            .iter()
            .find(|index| **index > turn)
            .copied()
            .unwrap_or(session.messages.len());
        let prompt = session
            .messages
            .get(turn)
            .filter(|message| message.role == Role::User)
            .map(|message| {
                rail_preview(
                    std::slice::from_ref(&message.text),
                    theme::TURN_PREVIEW_PROMPT,
                )
            })
            .unwrap_or_default();
        let response = session
            .messages
            .get(turn..end)
            .unwrap_or_default()
            .iter()
            .rev()
            .find(|message| message.role == Role::Assistant && !message.text.is_empty())
            .map(|message| {
                rail_preview(
                    std::slice::from_ref(&message.text),
                    theme::TURN_PREVIEW_RESPONSE,
                )
            })
            .unwrap_or_default();
        (prompt, response)
    }

    /// The turn whose top is the last one at or above the transcript's top edge.
    pub(crate) fn visible_turn(&self, turns: &[usize]) -> Option<usize> {
        // At the tail the turn in front of the reader is the NEWEST one, even
        // though its top sits below the viewport's: the newest turn is the one
        // the last mark names, and highlighting the turn above it left the last
        // mark permanently dim beside a chat that had already ended.
        if !turns.is_empty() {
            let max = self.transcript_scroll.max_offset().y;
            let at_bottom = max > px(0.0) && self.transcript_scroll.offset().y <= -max + px(1.0);
            if self.following_tail || at_bottom {
                return Some(turns.len() - 1);
            }
        }
        let viewport = self.transcript_scroll.bounds();
        let offset = self.transcript_scroll.offset().y;
        let mut visible = None;
        for (turn, index) in turns.iter().enumerate() {
            let Some(child) = self.transcript_scroll.bounds_for_item(1 + index) else {
                continue;
            };
            if child.top() + offset <= viewport.top() + px(8.0) {
                visible = Some(turn);
            } else {
                // Rows are laid out in order, so the first one whose top is
                // below the viewport's top ends the search: every later turn is
                // below it too. Without this the rail scanned the whole
                // transcript once per frame.
                break;
            }
        }
        visible.or_else(|| turns.first().map(|_| 0))
    }

    /// The plan and tool trajectory for the active session.
    pub(crate) fn render_trajectory(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_trajectory_view(cx)
    }

    #[allow(dead_code)]
    fn render_trajectory_legacy(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.state.active_session() else {
            return self.empty_state("empty.none").into_any_element();
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(goal) = &session.goal {
            rows.push(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(
                        self.state.lang.tr("pane.goal"),
                        theme::CHIP_SIZE,
                        theme::muted(),
                    ))
                    .child(txt(
                        goal.objective.clone(),
                        theme::body_size_at(self.state.draft().global.general.content_font_size),
                        theme::text(),
                    ))
                    .into_any_element(),
            );
        }
        for item in &session.plan {
            let mark = match item.status {
                PlanStatus::Completed => "✓",
                PlanStatus::InProgress => "●",
                PlanStatus::Pending => "○",
            };
            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(glyph(mark, 12.0, theme::muted()))
                    .child(txt(
                        item.text.clone(),
                        theme::body_size_at(self.state.draft().global.general.content_font_size),
                        theme::text(),
                    ))
                    .into_any_element(),
            );
        }
        // Tool rows come from the Parquet trajectory log, paged: the trace of a
        // long run is scrollable without being resident. The in-memory rows are
        // only drawn while nothing is paged yet (a session older than the log).
        let style = self.markdown_style();
        let chrome = RowChrome {
            policy: self.state.settings.effective().transcript_view,
            compact_usage: self.state.settings.effective().performance_usage == "compact",
            content_size: self.state.draft().global.general.content_font_size,
            style: &style,
        };
        if self.trace_rows.is_empty() && session.messages.iter().any(|m| !m.tools.is_empty()) {
            for (index, message) in session.messages.iter().enumerate() {
                for (tool_index, tool) in message.tools.iter().enumerate() {
                    rows.push(
                        self.tool_row(index, tool_index, tool, false, &chrome, cx)
                            .into_any_element(),
                    );
                }
            }
        } else {
            for (index, row) in self.trace_rows.iter().enumerate() {
                rows.push(self.trace_row(index, row, &chrome, cx).into_any_element());
            }
        }
        if rows.is_empty() {
            return self.empty_state("empty.none").into_any_element();
        }
        div()
            .id("trajectory")
            .debug_selector(|| String::from("trajectory"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.traj_scroll)
            .on_scroll_wheel(cx.listener(traj_wheel))
            .px(px(theme::INSET))
            .py(px(24.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .children(rows)
            .into_any_element()
    }

    /// One message.
    /// `folded` is the policy a completed turn folds under: the reference's
    /// `foldCompletedTurns` reading, which suppresses a settled turn's live
    /// process while leaving the running turn's rows alone.
    /// One trajectory row read from the log: the same step shape the transcript
    /// draws, keyed by its place in the paged trace rather than by a message.
    fn trace_row(
        &self,
        index: usize,
        row: &crate::history::ToolRow,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tool = ToolEvent {
            name: row.name.clone(),
            args: row.args.clone(),
            output: row.output.clone(),
            ms: row.ms,
            at: None,
        };
        // A trace row is history: one summary line, opened on a click.
        self.tool_row(usize::MAX - index, 0, &tool, true, chrome, cx)
    }

    /// One steered message the running turn has not taken yet.
    ///
    /// The reference renders an inbox steering item with the same bubble it
    /// uses for an admitted user row (its `data-pending-steering` bubble), so
    /// the reader sees the interjection immediately; the bubble becomes the
    /// durable row when the step claims it. No action row: there is nothing to
    /// copy or retry until it is part of the conversation.
    fn pending_steer_row(&self, queued: &crate::state::Queued) -> Div {
        let seq = queued.seq;
        let content = self.state.draft().global.general.content_font_size;
        div()
            .debug_selector(move || format!("steer-pending-{seq}"))
            .w_full()
            .flex()
            .flex_row()
            .justify_end()
            .child(
                div()
                    .max_w(px((self.chat_w() - 2.0 * theme::INSET) * 0.82))
                    .px(px(16.0))
                    .py(px(10.0))
                    .rounded(px(20.0))
                    .bg(rgb(theme::user_bubble()))
                    .child(
                        txt(
                            crate::state::display_text(&queued.text, &queued.attachments),
                            theme::body_size_at(content),
                            theme::text(),
                        )
                        .line_height(px(theme::bubble_line_at(content))),
                    ),
            )
    }

    fn message_row(
        &self,
        index: usize,
        message: &ChatMessage,
        turn: RowTurn,
        // The newest prompt in the transcript. A prompt that has a later one
        // below it hides its action row until the pointer is on the turn, which
        // is the reference's own reveal rule for user rows.
        last_user: Option<usize>,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        #[cfg(test)]
        ROWS_BUILT.with(|built| built.set(built.get() + 1));
        if message.role == Role::User {
            let size = theme::body_size_at(chrome.content_size);
            let line = theme::bubble_line_at(chrome.content_size);
            // Measured (MessageItem .userRow): a column aligned to the end, 6px
            // between the bubble and its action row. The action row is the
            // reference's clock="start" form - the time FIRST, then copy - and
            // only its owner keeps it in view: a prompt with a later prompt
            // below it hides the row until the pointer is on the turn.
            let group = SharedString::from(format!("user-turn-{index}"));
            let mut actions = self.user_actions(index, message, chrome, cx);
            if last_user != Some(index) {
                actions = actions
                    .opacity(0.0)
                    .group_hover(group.clone(), |style| style.opacity(1.0));
            }
            return div()
                .w_full()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    div()
                        .group(group)
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(6.0))
                        .child(
                            // Measured (MessageItem): radius-xl 20 over `10px 16px`,
                            // the bubble fill, `line-height: 22px + delta`, capped at
                            // 82% of the row - and at `content * .702`, which only
                            // binds wider.
                            div()
                                // The reference draws an admitted steer with the
                                // SAME user bubble it draws a prompt with (its
                                // `data-chat-flow-kind=steering` shares the user
                                // styles), so the only difference is the identity
                                // a test can look up and the turn grouping the
                                // flag drives.
                                .debug_selector(move || {
                                    if message.steering {
                                        format!("steer-bubble-{index}")
                                    } else {
                                        format!("user-bubble-{index}")
                                    }
                                })
                                // 82% of the transcript's content box. The reference
                                // also caps at `content * .702`, which only binds wider.
                                .max_w(px((self.chat_w() - 2.0 * theme::INSET) * 0.82))
                                .px(px(16.0))
                                .py(px(10.0))
                                .rounded(px(20.0))
                                .bg(rgb(theme::user_bubble()))
                                .child(
                                    txt(message.text.clone(), size, theme::text())
                                        .line_height(px(line)),
                                ),
                        )
                        .child(actions),
                )
                .into_any_element();
        }

        let mut column = div().w_full().flex().flex_col().gap(px(8.0));

        if !message.reasoning.is_empty() {
            column = column.child(self.think_row(index, message, turn.settled, chrome, cx));
        }

        if message.text.is_empty() && message.error.is_none() {
            // An empty segment of a steered turn contributes no body: the turn
            // holds one row before the steer and one after it, and only the
            // newest is writing.
            if !turn.settled && turn.writing {
                column = column.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(glyph("●", 9.0, theme::accent()))
                        .child(txt(
                            match self.state.turn_elapsed() {
                                Some(elapsed) => {
                                    format!("{} · {elapsed}", self.state.lang.tr("msg.running"))
                                }
                                None => self.state.lang.tr("msg.running").to_string(),
                            },
                            theme::body_size_at(
                                self.state.draft().global.general.content_font_size,
                            ),
                            theme::muted(),
                        )),
                );
            }
        } else {
            column = column.child(
                {
                    let body = self.body_state(BODY, &message.text, cx);
                    TextView::new(&body)
                }
                .on_link_click(open_link)
                // Selectable, so the reader can quote a reply and so the
                // shell's copy-on-selection can take it from the clipboard
                // seam. The reference is a web page where every glyph is
                // selectable; a native text view has to say so.
                .selectable(true)
                .style(chrome.style.clone())
                .line_height(px(theme::body_line_at(chrome.content_size)))
                .text_size(px(theme::body_size_at(chrome.content_size)))
                .text_color(rgb(theme::text())),
            );
        }

        for (tool_index, tool) in message.tools.iter().enumerate() {
            column = column.child(self.tool_row(index, tool_index, tool, turn.settled, chrome, cx));
        }

        // The turn's changed files, as the reference's closing card. It sits after
        // the steps because it SUMMARISES them, and it is the only place a reader
        // can hover a file and see what changed in it.
        if let Some(files) = self.changed_files(index) {
            if let Some(card) = self.changes_card(index, &files, cx) {
                column = column.child(card);
            }
        }

        if let Some(error) = &message.error {
            column = column.child(
                div()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::error()))
                    .px(px(10.0))
                    .py(px(8.0))
                    .child(txt(error.clone(), theme::CHIP_SIZE, theme::error())),
            );
        }

        column
            .child(self.message_footer(index, message, chrome.compact_usage, cx))
            .into_any_element()
    }

    /// The reasoning disclosure, measured from the shared `DisclosureRow` over
    /// `ReasoningRow`.
    ///
    /// Collapsed it is a `24px + delta` row: a `16px + delta` leading box holding a
    /// `14px + delta` glyph with a 6px right margin (swapped for a chevron while the
    /// row is hovered), the secondary-size title on the same 24px line, then a 2px
    /// dot with 8px margins and a nowrap, ellipsised summary whose line is
    /// `20px + secondary delta`. Open, the complete reasoning renders in a body
    /// indented `22px + delta`. The row colour is label-tertiary, label-secondary
    /// under the pointer.
    /// The changed files of one message, from the per-frame cache.
    ///
    /// Parsing every write's arguments on every frame is the per-frame work the
    /// crate's own rule forbids, so the summary is cached on the steps it was
    /// built from.
    fn changed_files(&self, message: usize) -> Option<std::rc::Rc<Vec<ChangedFile>>> {
        let session = self.state.active_session()?;
        let tools = &session.messages.get(message)?.tools;
        let files = self.changes_cache.borrow_mut().get(message, tools);
        if files.is_empty() {
            return None;
        }
        Some(files)
    }

    /// Record the row the pointer is over and arm the 500ms preview clock.
    ///
    /// Moving WITHIN one row must not restart the clock on every pixel, so only a
    /// different row arms a new timer; the same row just follows the pointer.
    fn arm_changes_hover(
        &mut self,
        message: usize,
        file: usize,
        at: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        if let Some(current) = self.changes_hover {
            if current.message == message && current.file == file {
                self.changes_hover = Some(ChangesHover { message, file, at });
                return;
            }
        }
        self.changes_hover = Some(ChangesHover { message, file, at });
        self.changes_preview = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            // Decide WHAT to resolve on the UI thread, where the state is; the
            // resolving itself costs a git subprocess and belongs off it.
            let prepared = this.update(cx, |this, _cx| {
                let hover = this.changes_hover?;
                if hover.message != message || hover.file != file {
                    return None;
                }
                let files = this.changed_files(message)?;
                let changed = files.get(file)?;
                let root = this.state.workspace_root()?;
                // The reference titles the preview with the file's resolved path;
                // a step often records a short one ("x.md"), which names nothing
                // outside the directory the reader has in mind.
                let rel = crate::ui::diff_pane::workspace_relative(&root, &changed.path);
                let absolute = root.join(&rel);
                let display = if absolute.is_file() {
                    absolute.to_string_lossy().to_string()
                } else {
                    changed.path.clone()
                };
                Some((root, display, changed.diff.clone()))
            });
            let Ok(Some((root, path, changes))) = prepared else {
                return;
            };
            let resolving = path.clone();
            let rows = cx
                .background_executor()
                .spawn(async move {
                    crate::ui::diff_pane::preview_for_workspace(&root, &resolving, &changes)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // Still the same row? The pointer may have moved while the
                // subprocess ran, which is exactly what the delay exists to
                // avoid showing.
                let Some(hover) = this.changes_hover else {
                    return;
                };
                if hover.message != message || hover.file != file {
                    return;
                }
                this.changes_preview = Some((message, file, path, rows));
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop the hover state and its preview.
    fn clear_changes_hover(&mut self, cx: &mut Context<Self>) {
        if self.changes_hover.is_none() && self.changes_preview.is_none() {
            return;
        }
        self.changes_hover = None;
        self.changes_preview = None;
        cx.notify();
    }

    /// The hover preview, positioned at the pointer.
    ///
    /// It is drawn in the transcript's own relative frame rather than inside the
    /// scroller: a GPUI scroller clips absolute children, so a card hanging off a
    /// row would be cut at the viewport edge. The pointer's window position is
    /// therefore taken back to that frame through the scroller's bounds, which is
    /// the box the frame wraps.
    fn changes_preview_overlay(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        let (message, index, path, rows) = self.changes_preview.clone()?;
        let hover = self.changes_hover?;
        if hover.message != message || hover.file != index {
            return None;
        }
        if rows.is_empty() {
            return None;
        }
        const W: f32 = 460.0;
        const MAX_H: f32 = 320.0;
        let bounds = self.transcript_scroll.bounds();
        let origin_y = f32::from(bounds.origin.y);
        let view_w = f32::from(bounds.size.width);
        let view_h = f32::from(bounds.size.height);
        // The LEFT edge is the content's own edge, so the card lines up with the
        // row it explains rather than floating wherever the pointer happened to
        // sit.
        let x = theme::INSET.min((view_w - W - 8.0).max(8.0));
        // The panel's height is not measured before it is placed, so it is
        // estimated from the rows it will draw - enough to decide which side of
        // the pointer has room for it.
        const HEAD_H: f32 = 28.0;
        const ROW_H: f32 = 18.0;
        let panel_h = (HEAD_H + rows.len() as f32 * ROW_H).min(MAX_H);
        let y = crate::ui::diff_pane::preview_top(hover.at.1 - origin_y, panel_h, view_h);
        let lines: Vec<AnyElement> = rows
            .iter()
            .map(|row| {
                // The wash carries the row, but the MARKER states it: a reader
                // scanning a diff looks for the glyph, and a tint alone leaves
                // them hunting for whether a line was added or removed.
                let ink = match row.mark {
                    '-' | '+' => theme::text(),
                    _ => theme::muted(),
                };
                let fill = match row.mark {
                    '-' => theme::diff_del(),
                    '+' => theme::diff_add(),
                    _ => theme::page(),
                };
                let marker = match row.mark {
                    '-' => "\u{2212}",
                    '+' => "+",
                    _ => "",
                };
                let marker_ink = match row.mark {
                    '-' => theme::error(),
                    '+' => theme::success(),
                    _ => theme::faint(),
                };
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(ROW_H))
                    .bg(rgb(fill))
                    .child(
                        txt(
                            row.old.map(|n| n.to_string()).unwrap_or_default(),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        )
                        .w(px(30.0))
                        .pr(px(6.0))
                        .flex_none()
                        .line_height(px(ROW_H))
                        .text_align(gpui_kit::TextAlign::Right),
                    )
                    .child(
                        txt(
                            row.new.map(|n| n.to_string()).unwrap_or_default(),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        )
                        .w(px(30.0))
                        .pr(px(6.0))
                        .flex_none()
                        .line_height(px(ROW_H))
                        .text_align(gpui_kit::TextAlign::Right),
                    )
                    .child(
                        // The glyph the eye searches for, in its own colour.
                        txt(marker, theme::CHIP_SIZE, marker_ink)
                            .w(px(14.0))
                            .flex_none()
                            .line_height(px(ROW_H))
                            .text_align(gpui_kit::TextAlign::Center),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(
                                txt(row.text.clone(), theme::CHIP_SIZE, ink)
                                    .font_family(theme::CODE_FONT)
                                    .line_height(px(ROW_H)),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            super::hairline_all(div())
                .debug_selector(|| "changes-preview".to_string())
                .id("changes-preview")
                .absolute()
                .left(px(x))
                .top(px(y))
                .border_color(rgb(theme::border_soft()))
                .w(px(W))
                .max_h(px(MAX_H))
                .occlude()
                .rounded(px(12.0))
                .bg(rgb(theme::page()))
                .shadow_lg()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(rgb(theme::border_soft()))
                        .whitespace_nowrap()
                        .child(
                            txt(path.clone(), theme::CHIP_SIZE, theme::muted())
                                .font_family(theme::CODE_FONT),
                        ),
                )
                .child(
                    div()
                        .id("changes-preview-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        // Full-bleed rows, the way the reference draws a patch: a
                        // wash inset from the card edge reads as a highlighted
                        // block rather than as the line itself.
                        .py(px(4.0))
                        .children(lines),
                )
                .into_any_element(),
        )
    }

    /// The turn's changed files, as the reference's closing card.
    ///
    /// Measured (ChangedFiles.module.css): the card is a `.5px` border-l2 at
    /// radius-lg over the page fill; the header is a fixed 60 over `8px 10px` at
    /// gap 10 holding a 40x40 tile; the title is 13/20 at weight 500 and the stat
    /// line 10/16; a row is `min-height:24px` over `7px 18px 7px 14px` at a
    /// code-face 11/18; and the fold control is `10px 18px 10px 14px` at 12/18.
    ///
    /// A single changed file is only a header - the reference calls it a
    /// single-file card - and a list appears once a turn touched more than one.
    fn changes_card(
        &self,
        message: usize,
        files: &[ChangedFile],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let lang = self.state.lang;
        let first = files.first()?;
        let single = files.len() == 1;
        let foldable = files.len() > theme::CHANGES_COLLAPSED_ROWS;
        let expanded = self.changes_open.contains(&message);
        let shown: Vec<&ChangedFile> = if foldable && !expanded {
            files.iter().take(theme::CHANGES_COLLAPSED_ROWS).collect()
        } else {
            files.iter().collect()
        };
        let added: usize = files.iter().map(|file| file.added).sum();
        let deleted: usize = files.iter().map(|file| file.deleted).sum();
        let title = if single {
            crate::i18n::tr_args(
                lang,
                "changes.singleTitle",
                &[("name", base_name(&first.path))],
            )
        } else {
            crate::i18n::tr_args(lang, "changes.title", &[("count", files.len().to_string())])
        };
        // The counts a reader scans, in the two colours the card pairs them in.
        let counts = {
            let added =
                crate::i18n::tr_args(lang, "changes.added", &[("count", added.to_string())]);
            let deleted =
                crate::i18n::tr_args(lang, "changes.deleted", &[("count", deleted.to_string())]);
            div()
                .flex()
                .flex_row()
                .gap(px(6.0))
                .font_family(theme::CODE_FONT)
                .child(txt(added, theme::CHANGES_STAT_SIZE, theme::success()))
                .child(txt(deleted, theme::CHANGES_STAT_SIZE, theme::error()))
        };
        let header = {
            let hovered = self
                .changes_hover
                .is_some_and(|state| state.message == message);
            let hint = txt(
                lang.tr("presented.preview"),
                theme::CHANGES_STAT_SIZE,
                theme::faint(),
            );
            div()
                .debug_selector(move || format!("changes-header-{message}"))
                .id(SharedString::from(format!("changes-header-{message}")))
                .h(px(theme::CHANGES_HEADER_H))
                .px(px(10.0))
                .py(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .w_full()
                .bg(rgb(theme::changes_fill()))
                .cursor_pointer()
                .hover(|row| row.bg(rgb(theme::changes_hover())))
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                        this.arm_changes_hover(
                            message,
                            0,
                            (f32::from(event.position.x), f32::from(event.position.y)),
                            cx,
                        );
                    }),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    if !*hovered {
                        this.clear_changes_hover(cx);
                    }
                }))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    // The card summarises the WHOLE turn, so pressing it opens
                    // the turn's file list in the Diff pane rather than one
                    // file's comparison - the list is what the card is.
                    this.open_diff_turn(message, cx);
                }))
                .child(
                    // The tile: a 40x40 radius-10 plate holding the kind's glyph,
                    // which is the reference's FileTypeIcon slot.
                    div()
                        .w(px(theme::CHANGES_TILE))
                        .h(px(theme::CHANGES_TILE))
                        .flex_none()
                        .rounded(px(theme::CHANGES_TILE_RADIUS))
                        .bg(rgb(theme::changes_tile()))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Code, 20.0, theme::muted())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(
                                    txt(title, theme::CHANGES_TITLE_SIZE, theme::text())
                                        .font_weight(FontWeight::MEDIUM)
                                        .line_height(px(theme::CHANGES_TITLE_LINE)),
                                ),
                        )
                        .child(
                            div()
                                .h(px(theme::CHANGES_STAT_LINE))
                                .flex()
                                .flex_row()
                                .items_center()
                                // The counts swap for the preview hint under the
                                // pointer, which is the reference's own rule for
                                // telling the reader the row does something.
                                .child(if hovered {
                                    hint.into_any_element()
                                } else {
                                    counts.into_any_element()
                                }),
                        ),
                )
        };
        let mut card = super::hairline_all(div())
            .debug_selector(move || format!("changes-card-{message}"))
            .mt(px(4.0))
            .rounded(px(theme::RADIUS_LG))
            .bg(rgb(theme::page()))
            .border_color(rgb(theme::border_soft()))
            .flex()
            .flex_col()
            .overflow_hidden()
            .w_full()
            .child(header);
        if !single {
            let mut list = div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(rgb(theme::border_soft()));
            for (index, file) in shown.iter().enumerate() {
                list = list.child(self.changes_row(message, index, file, cx));
            }
            card = card.child(list);
        }
        if foldable {
            let open = expanded;
            card = card.child(
                div()
                    .debug_selector(move || format!("changes-toggle-{message}"))
                    .id(SharedString::from(format!("changes-toggle-{message}")))
                    .w_full()
                    .px(px(theme::CHANGES_ROW_PAD_R))
                    .pl(px(theme::CHANGES_ROW_PAD_L))
                    .py(px(10.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgb(theme::hover())))
                    .child(icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        14.0,
                        theme::faint(),
                    ))
                    .child(txt(
                        if open {
                            lang.tr("changes.collapse")
                        } else {
                            crate::i18n::tr_args(
                                lang,
                                "changes.all",
                                &[("count", files.len().to_string())],
                            )
                        },
                        12.0,
                        theme::faint(),
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if !this.changes_open.remove(&message) {
                            this.changes_open.insert(message);
                        }
                        cx.notify();
                    })),
            );
        }
        Some(card.into_any_element())
    }

    /// One file row inside the card.
    fn changes_row(
        &self,
        message: usize,
        index: usize,
        file: &ChangedFile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let path = file.path.clone();
        let added =
            crate::i18n::tr_args(lang, "changes.added", &[("count", file.added.to_string())]);
        let deleted = crate::i18n::tr_args(
            lang,
            "changes.deleted",
            &[("count", file.deleted.to_string())],
        );
        div()
            .debug_selector(move || format!("changes-row-{message}-{index}"))
            .id(SharedString::from(format!("changes-row-{message}-{index}")))
            .w_full()
            .min_h(px(24.0))
            .px(px(theme::CHANGES_ROW_PAD_R))
            .pl(px(theme::CHANGES_ROW_PAD_L))
            .py(px(theme::CHANGES_ROW_PAD_Y))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(10.0))
            .cursor_pointer()
            .font_family(theme::CODE_FONT)
            .hover(|row| row.bg(rgb(theme::hover())))
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    this.arm_changes_hover(
                        message,
                        index,
                        (f32::from(event.position.x), f32::from(event.position.y)),
                        cx,
                    );
                }),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.clear_changes_hover(cx);
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(txt(path.clone(), theme::CHANGES_PATH_SIZE, theme::muted())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    .child(
                        txt(added, theme::CHANGES_ROW_SIZE, theme::success())
                            .line_height(px(theme::CHANGES_ROW_LINE)),
                    )
                    .child(
                        txt(deleted, theme::CHANGES_ROW_SIZE, theme::error())
                            .line_height(px(theme::CHANGES_ROW_LINE)),
                    ),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // The row names ONE file, so the turn list opens with that
                // file already expanded - the comparison the reader asked for,
                // in the context of everything else the turn changed.
                this.open_diff_turn_at(message, &path, cx);
            }))
            .into_any_element()
    }

    /// The compaction marker: one line that says a span was condensed, and the
    /// summary the model was given in its place when expanded.
    ///
    /// Measured (MessageItem.module.css): the row is `2px 0` around a
    /// `24px + delta` button at radius-sm in label-tertiary, holding a
    /// `16px + delta` leading box with a 14px glyph, the title at the secondary
    /// size, a 2x2 caption-grey separator with 8px margins, and an ellipsised
    /// summary that takes the rest. The leading box swaps the context glyph for
    /// a chevron on hover, so the row reads as a control only when the pointer
    /// is on it.
    fn compaction_row(
        &self,
        record: &crate::model::CompactionRecord,
        index: usize,
        chrome: &RowChrome,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = chrome.content_size;
        let row_h = 24.0 + theme::delta_at(content);
        let small = theme::small_size_at(content);
        let quiet = theme::faint();
        let hover = theme::muted();
        let open = self.compaction_open.contains(&index);
        let group = SharedString::from(format!("compact-{index}"));
        let summary = crate::i18n::tr_args(
            self.state.lang,
            "msg.compaction.completed",
            &[
                ("items", record.items.to_string()),
                ("tokens", record.tokens.to_string()),
            ],
        );
        let title = if record.manual {
            self.state.lang.tr("msg.compaction.commandTitle")
        } else {
            self.state.lang.tr("msg.compaction")
        };
        let row = div()
            .debug_selector(move || format!("compaction-row-{index}"))
            .id(SharedString::from(format!("compaction-row-{index}")))
            .group(group.clone())
            .flex_shrink_0()
            .h(px(row_h))
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .debug_selector(move || format!("compaction-leading-{index}"))
                    .relative()
                    .flex_none()
                    .w(px(theme::DISCLOSURE_LEADING))
                    .h(px(theme::DISCLOSURE_LEADING))
                    .mr(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(IconName::CircleDashed, theme::DISCLOSURE_GLYPH, quiet)
                            .group_hover(group.clone(), |style| style.opacity(0.0)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(theme::DISCLOSURE_LEADING))
                            .h(px(theme::DISCLOSURE_LEADING))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.0)
                            .group_hover(group.clone(), |style| style.opacity(1.0))
                            .child(icon(
                                if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::DISCLOSURE_GLYPH,
                                hover,
                            )),
                    ),
            )
            .child(
                txt(title, small, quiet)
                    .debug_selector(move || format!("compaction-title-{index}"))
                    .flex_none()
                    .line_height(px(row_h))
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover))),
            )
            .child(
                div()
                    .debug_selector(move || format!("compaction-sep-{index}"))
                    .flex_none()
                    .w(px(2.0))
                    .h(px(2.0))
                    .mx(px(8.0))
                    .rounded(px(1.0))
                    .bg(rgb(quiet)),
            )
            .child(
                div()
                    .debug_selector(move || format!("compaction-summary-{index}"))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover)))
                    .child(txt(summary, small, quiet).line_height(px(row_h))),
            );
        let mut root = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(row.on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.compaction_open.remove(&index) {
                    this.compaction_open.insert(index);
                }
                cx.notify();
            })));
        if open {
            root = root.child(
                div()
                    .debug_selector(move || format!("compaction-body-{index}"))
                    .w_full()
                    .min_w_0()
                    .pl(px(22.0 + theme::delta_at(content)))
                    .py(px(4.0))
                    .child(
                        {
                            let body = self.body_state(COMPACTION, &record.summary, cx);
                            TextView::new(&body)
                        }
                        .style(chrome.style.clone())
                        .line_height(px(row_h))
                        .text_size(px(small))
                        .text_color(rgb(quiet)),
                    ),
            );
        }
        root
    }

    /// What the command card currently says, and whether it is an error.
    ///
    /// The row and this accessor share one source, so a test that asserts the
    /// wording reads the same string the frame paints - and a provider error,
    /// an empty summary and a shrink-gate refusal each keep their own line.
    #[cfg(test)]
    pub(crate) fn compact_command_summary(&self) -> Option<(String, bool)> {
        let command = self.state.active_session()?.compact_command.as_ref()?;
        Some(self.compact_command_text(command))
    }

    /// The text one command row carries.
    fn compact_command_text(&self, command: &crate::model::CompactCommand) -> (String, bool) {
        match &command.outcome {
            Some(outcome) => (outcome.text.clone(), outcome.error),
            None => (self.state.lang.tr("msg.compaction.running"), false),
        }
    }

    /// The `/compact` command card: the reference's `GenericCommandCard`.
    ///
    /// Measured from `GenericCommandCard.module.css` over the shared
    /// `DisclosureRow`: the same `24px + delta` row the checkpoint marker
    /// draws, the command name in the secondary size, the 2x2 caption separator
    /// with 8px margins, and a nowrap ellipsised summary. `data-error` paints
    /// that summary in the failure colour, which is exactly what the reference
    /// does for a `kind: "error"` result. The one difference the row carries is
    /// its glyph: the reference's `IconApiOutlineRegular` has no bsh equivalent,
    /// so the context mark the checkpoint row uses stands in.
    fn compact_command_row(
        &self,
        command: &crate::model::CompactCommand,
        chrome: &RowChrome<'_>,
    ) -> impl IntoElement {
        let content = chrome.content_size;
        let row_h = 24.0 + theme::delta_at(content);
        let small = theme::small_size_at(content);
        let quiet = theme::faint();
        let (summary, error) = self.compact_command_text(command);
        let title = self.state.lang.tr("msg.compaction.commandTitle");
        div()
            .debug_selector(|| String::from("compact-command-row"))
            .id("compact-command-row")
            .w_full()
            .min_w_0()
            .py(px(2.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(row_h))
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .debug_selector(|| String::from("compact-command-leading"))
                            .flex_none()
                            .w(px(theme::DISCLOSURE_LEADING))
                            .h(px(theme::DISCLOSURE_LEADING))
                            .mr(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::CircleDashed, theme::DISCLOSURE_GLYPH, quiet)),
                    )
                    .child(
                        txt(title, small, quiet)
                            .debug_selector(|| String::from("compact-command-title"))
                            .flex_none()
                            .line_height(px(row_h)),
                    )
                    .child(
                        div()
                            .debug_selector(|| String::from("compact-command-sep"))
                            .flex_none()
                            .w(px(2.0))
                            .h(px(2.0))
                            .mx(px(8.0))
                            .rounded(px(1.0))
                            .bg(rgb(quiet)),
                    )
                    .child(
                        div()
                            .debug_selector(|| String::from("compact-command-summary"))
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(
                                txt(summary, small, if error { theme::error() } else { quiet })
                                    .line_height(px(row_h)),
                            ),
                    ),
            )
    }

    fn think_row(
        &self,
        index: usize,
        message: &ChatMessage,
        folded: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let content = chrome.content_size;
        let row_h = theme::disclosure_row_at(content);
        let small = theme::small_size_at(content);
        let quiet = theme::faint();
        let hover = theme::muted();
        // The reference's preview is `!expanded && summary !== "" && (running ||
        // policy.settledReasoningPreview)`, so a policy without the preview shows
        // the settled block's title alone rather than a summary it drops.
        let open = self.think_open.contains(&index)
            || (folded && !chrome.policy.settled_reasoning_preview);
        let summary = think_summary(&message.reasoning);
        let group = SharedString::from(format!("think-{index}"));
        let row = div()
            .debug_selector(move || format!("think-row-{index}"))
            .id(SharedString::from(format!("think-row-{index}")))
            .group(group.clone())
            .flex_shrink_0()
            .h(px(row_h))
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .child(
                div()
                    .debug_selector(move || format!("think-leading-{index}"))
                    .relative()
                    .flex_none()
                    .w(px(theme::DISCLOSURE_LEADING))
                    .h(px(theme::DISCLOSURE_LEADING))
                    .mr(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(IconName::Lightbulb, theme::DISCLOSURE_GLYPH, quiet)
                            .group_hover(group.clone(), |style| style.opacity(0.0)),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w(px(theme::DISCLOSURE_LEADING))
                            .h(px(theme::DISCLOSURE_LEADING))
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.0)
                            .group_hover(group.clone(), |style| style.opacity(1.0))
                            .child(icon(
                                if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                theme::DISCLOSURE_GLYPH,
                                hover,
                            )),
                    ),
            )
            .child(
                txt(self.state.lang.tr("msg.thinking"), small, quiet)
                    .debug_selector(move || format!("think-title-{index}"))
                    .flex_none()
                    .line_height(px(row_h))
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover))),
            );
        // A settled summary is the first line with the emphasis markers stripped;
        // the reference hides the pair entirely when the summary is empty, or while
        // the block is open.
        let row = if open || summary.is_empty() {
            row
        } else {
            row.child(
                div()
                    .debug_selector(move || format!("think-dot-{index}"))
                    .flex_none()
                    .w(px(2.0))
                    .h(px(2.0))
                    .mx(px(8.0))
                    .rounded(px(1.0))
                    .bg(rgb(quiet)),
            )
            .child(
                div()
                    .debug_selector(move || format!("think-summary-{index}"))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .group_hover(group.clone(), |style| style.text_color(rgb(hover)))
                    .child(
                        txt(summary, small, quiet).line_height(px(theme::summary_line_at(content))),
                    ),
            )
        };
        let mut root = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(row.on_click(cx.listener(move |this, _event, _window, cx| {
                if !this.think_open.remove(&index) {
                    this.think_open.insert(index);
                }
                cx.notify();
            })));
        if open {
            root = root.child(
                div()
                    .debug_selector(move || format!("think-body-{index}"))
                    .min_w_0()
                    .pt(px(4.0))
                    .pb(px(4.0))
                    .pl(px(22.0 + theme::delta_at(content)))
                    .child(
                        {
                            let body = self.body_state(THINK, &message.reasoning, cx);
                            TextView::new(&body)
                        }
                        .on_link_click(open_link)
                        .selectable(true)
                        .style(chrome.style.clone())
                        .line_height(px(theme::summary_line_at(content)))
                        .text_size(px(small))
                        .text_color(rgb(theme::muted())),
                    ),
            );
        }
        root
    }

    /// A prompt's own action row: the time it was sent, then copy.
    ///
    /// Measured from `MessageIconActions` in its `clock="start"` form, which is
    /// the one the reference gives user rows: the clock LEADS (`.timeStart`, the
    /// secondary size, `padding-right: 12px`) and the copy control follows it at
    /// the 15px glyph a leading-clock row carries. There is no branch and no
    /// verdict here - a prompt is not something to branch from or rate.
    fn user_actions(
        &self,
        index: usize,
        message: &ChatMessage,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> Div {
        let lang = self.state.lang;
        let content = chrome.content_size;
        let delta = theme::delta_at(content);
        let side = 28.0 + delta;
        let mark = 15.0 + delta;
        let copied = self.copied_message == Some(index);
        let text = message.text.clone();
        // The submit instant stamps the prompt row, so the clock is the moment
        // the reader sent it rather than the moment the reply settled.
        let at = message
            .timing()
            .and_then(|timing| timing.start)
            .map(|start| start / 1000);
        let mut row = div()
            .debug_selector(move || format!("user-actions-{index}"))
            .flex()
            .flex_row()
            .items_center()
            .h(px(side))
            .gap(px(8.0));
        if let Some(at) = at {
            row = row.child(
                txt(
                    crate::state::clock_at(at, lang),
                    theme::small_size_at(content),
                    theme::faint(),
                )
                .line_height(px(24.0 + delta))
                .pr(px(12.0)),
            );
        }
        let copy = div()
            .id(SharedString::from(format!("user-copy-{index}")))
            .debug_selector(move || format!("user-copy-{index}"))
            .flex_none()
            .w(px(side))
            .h(px(side))
            .p(px(6.0))
            .rounded(px(theme::RADIUS_SM))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(|style| style.bg(rgb(theme::hover())))
            .child(
                icon(
                    if copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    },
                    mark,
                    if copied {
                        theme::success()
                    } else {
                        theme::faint()
                    },
                )
                .into_any_element(),
            )
            .tooltip({
                let label = if copied {
                    lang.tr("chat.copied")
                } else {
                    lang.tr("msg.copyMessage")
                };
                move |window, cx| Tooltip::new(label.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.copy_message(index, &text, cx);
            }));
        row.child(copy)
    }

    /// The assistant tail's action row, measured from MessageIconActions over
    /// TurnTailNodeView.
    ///
    /// The row is `28px + delta` tall and 8px apart, 20px under the tail - its
    /// own 16px column gap plus the measured 4px action margin - and pulled 6px
    /// left so the first button's own 6px padding lines its glyph up with the
    /// message text. Each action is a `28px + delta` square at radius-sm in
    /// label-tertiary, hovering to the interactive fill. Reading left to right:
    /// copy, good, bad, branch, rewind, then the turn's own figures - usage,
    /// duration and the clock - in an end group 8px apart.
    ///
    /// The branch is offered only on the last message of a completed turn,
    /// which is the reference's own rule (`branchUnavailable` when a later chat
    /// node exists); a row that cannot branch says so in its tooltip rather than
    /// hiding the control, so the reader learns the rule instead of missing it.
    fn message_footer(
        &self,
        index: usize,
        message: &ChatMessage,
        compact_usage: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.state.lang;
        let content = self.state.draft().global.general.content_font_size;
        let delta = theme::delta_at(content);
        let side = 28.0 + delta;
        // This is the assistant row, which is the reference's `[data-clock=end]`
        // variant: its glyphs are 17px, two above the 15px a clock-leading row
        // carries. The thumbs are text glyphs, not svgs, so they take the 13px
        // the emoji family needs to sit level with a 17px line icon.
        let mark = 17.0 + delta;
        let feedback = message
            .meta
            .as_ref()
            .and_then(|meta| meta.feedback.clone())
            .unwrap_or_default();
        let usage = message
            .meta
            .as_ref()
            .and_then(|meta| meta.usage.as_ref())
            .map(|usage| usage.total())
            .unwrap_or(0);
        let duration = message.timing().and_then(|timing| timing.ms);
        // The clock reads the turn's own opening instant, in seconds; a record
        // written before timing existed carries none and draws no clock.
        let at = message
            .timing()
            .and_then(|timing| timing.start)
            .map(|start| start / 1000);
        let session_id = self.state.active_id.clone().unwrap_or_default();
        let copied = self.copied_message == Some(index);
        let branchable = self.state.can_branch_at(&session_id, index);
        let text = message.text.clone();

        let action = move |id: String, element: AnyElement, hot: bool| -> Stateful<Div> {
            div()
                .id(SharedString::from(id.clone()))
                // The debug selector is the same name, so a click test can press
                // the control it asserts on.
                .debug_selector({
                    let selector = id.clone();
                    move || selector.clone()
                })
                .flex_none()
                .w(px(side))
                .h(px(side))
                .p(px(6.0))
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                // `[data-unavailable]:hover` keeps its own ink and no fill, so a
                // control that cannot act does not light up like one that can.
                .when(hot, |button| {
                    button.hover(|style| style.bg(rgb(theme::hover())))
                })
                .child(element)
        };

        let copy = action(
            format!("copy-message-{index}"),
            icon(
                if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                },
                mark,
                if copied {
                    theme::success()
                } else {
                    theme::faint()
                },
            )
            .into_any_element(),
            true,
        )
        .tooltip({
            let label = if copied {
                lang.tr("chat.copied")
            } else {
                lang.tr("msg.copyMessage")
            };
            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.copy_message(index, &text, cx);
        }));

        let good_id = session_id.clone();
        let good = action(
            format!("good-{index}"),
            glyph(
                "👍",
                13.0,
                if feedback == "good" {
                    theme::accent()
                } else {
                    theme::faint()
                },
            )
            .into_any_element(),
            true,
        )
        .tooltip({
            let label = lang.tr("msg.good");
            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.state.set_feedback(&good_id, index, "good");
            cx.notify();
        }));

        let bad_id = session_id.clone();
        let bad = action(
            format!("bad-{index}"),
            glyph(
                "👎",
                13.0,
                if feedback == "bad" {
                    theme::error()
                } else {
                    theme::faint()
                },
            )
            .into_any_element(),
            true,
        )
        .tooltip({
            let label = lang.tr("msg.bad");
            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.state.set_feedback(&bad_id, index, "bad");
            cx.notify();
        }));

        let branch_tip = if branchable {
            lang.tr("msg.branchMessage")
        } else {
            lang.tr("msg.branchUnavailable")
        };
        let branch_mark = icon(
            IconName::GitBranch,
            mark,
            if branchable {
                theme::faint()
            } else {
                theme::border()
            },
        );
        let mut branch = action(
            format!("branch-{index}"),
            branch_mark.into_any_element(),
            branchable,
        )
        .tooltip(move |window, cx| Tooltip::new(branch_tip.clone()).build(window, cx));
        if branchable {
            let branch_id = session_id.clone();
            branch = branch.on_click(cx.listener(move |this, _event, window, cx| {
                this.branch_into_session(&branch_id, index, window, cx);
            }));
        } else {
            // `[data-unavailable]`: the control keeps its slot so the row's
            // order never moves, but it is not a target.
            branch = branch.cursor_default().opacity(0.4);
        }

        let cut_id = session_id;
        let cut = action(
            format!("cut-{index}"),
            icon(IconName::RefreshCw, mark, theme::faint()).into_any_element(),
            true,
        )
        .tooltip({
            let label = lang.tr("msg.truncate");
            move |window, cx| Tooltip::new(label.clone()).build(window, cx)
        })
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.state.truncate_from(&cut_id, index);
            cx.notify();
        }));

        let mut row = div()
            .debug_selector(move || format!("message-footer-{index}"))
            .flex()
            .flex_row()
            .items_center()
            .h(px(side))
            .gap(px(8.0))
            // The tail's own 16px column gap plus the measured 4px action margin
            // is 20px; this row already sits inside an 8px column, so the extra
            // 12 makes the whole space match.
            .mt(px(12.0))
            .ml(px(-6.0))
            .child(copy)
            .child(good)
            .child(bad)
            .child(branch)
            .child(cut)
            .child(div().flex_1());
        // Performance & usage: the reference's Compact mode keeps the per-turn
        // usage reading to itself, so the footer's own token figure is the row
        // the preference governs.
        if usage > 0 && !compact_usage {
            row = row.child(txt(
                format!("{usage} {}", lang.tr("turn.tokens")),
                theme::small_size_at(content),
                theme::faint(),
            ));
        }
        // How long the turn took. The reference prints this in the turn's
        // process summary; the footer carries it beside usage so the cost and
        // the time of one turn read together.
        if let Some(ms) = duration.filter(|ms| *ms > 0) {
            row = row.child(txt(
                super::header::job_duration(ms, lang),
                theme::small_size_at(content) - 1.0,
                theme::faint(),
            ));
        }
        if let Some(at) = at {
            row = row.child(txt(
                crate::state::clock_at(at, lang),
                theme::small_size_at(content) - 1.0,
                theme::faint(),
            ));
        }
        row
    }

    /// One collapsible step row, in the reference's trace shape.
    ///
    /// Collapsed it is one line: a chevron, the KIND the step is (Code, Bash,
    /// Read, Write, ...), a `·`, then the step's own single-line summary. Open,
    /// the arguments and the output follow in the same box, indented under the
    /// row's own rail. The kind comes from the tool, but the row is labelled by
    /// what the step DID, which is what two providers' different tool names
    /// share.
    fn tool_row(
        &self,
        message_index: usize,
        tool_index: usize,
        tool: &ToolEvent,
        folded: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // `liveProcessDetail` governs the RUNNING turn: its steps stay open while
        // it works. A settled turn folds its process the way the reference's
        // `stepGrouping: collapsed` does - one summary line per step, opened on a
        // click - which is the shape a reader scans a finished trace in. Reading
        // the flag the other way round opened every body of every settled turn,
        // which is what made a long transcript a wall of arguments.
        let open = self.state.tool_open(message_index, tool_index)
            || (!folded && chrome.policy.live_process_detail);
        let lang = self.state.lang;
        let size = theme::small_size_at(chrome.content_size);
        let name = tool.name.clone();
        let kind = tr(lang, tool_kind_key(&name));
        let summary = tool_summary(&tool.args);
        let args = tool.args.clone();
        let output = tool.output.clone();
        let ms = tool.ms;
        let done = output.is_some();
        let mut row = div()
            // The nesting the trace shows: every step sits one rail in from the
            // message it belongs to.
            .debug_selector(move || format!("tool-{message_index}-{tool_index}"))
            .w_full()
            .pl(px(theme::TOOL_INDENT))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .border_l_1()
            .border_color(rgb(theme::border_soft()));
        // A file step's path inside the row is its own control, so the row-wide
        // fold handler is attached only when there is no link to protect.
        let opens_file = matches!(
            name.as_str(),
            "read_file" | "write_file" | "edit_file" | "apply_patch"
        );
        let head = div()
            .id(SharedString::from(format!(
                "tool-head-{message_index}-{tool_index}"
            )))
            .debug_selector({
                let selector = format!("tool-head-{message_index}-{tool_index}");
                move || selector.clone()
            })
            .ml(px(10.0))
            .py(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            // The disclosure lives on the chevron and the label, NOT on the whole
            // row: a file step's path inside the row is its own control that opens
            // the file, and a row-wide handler swallowed it (the live click
            // unfolded the step instead of opening the file).
            .child(
                div()
                    .id(SharedString::from(format!(
                        "tool-fold-{message_index}-{tool_index}"
                    )))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        // The head below folds on any press too; without this the
                        // chevron would toggle and the head would toggle it back.
                        cx.stop_propagation();
                        this.state.toggle_tool(message_index, tool_index);
                        cx.notify();
                    }))
                    .child(icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        12.0,
                        theme::faint(),
                    ))
                    .child(icon(tool_icon(&name), 13.0, theme::muted()))
                    .child(
                        txt(kind, size, theme::text())
                            .font_weight(FontWeight::MEDIUM)
                            .flex_none(),
                    ),
            )
            .when(!summary.is_empty(), |head| {
                // A file step's summary IS a path, and the reference makes it a
                // link: pressing it opens that file in the right panel's editor
                // instead of leaving the reader to find it in the tree.
                head.child(txt("·", size, theme::faint()).flex_none())
                    .child(
                        div()
                            // A stateful cell so the link can take a click; a
                            // plain div cannot.
                            .id(SharedString::from(format!(
                                "open-file-{message_index}-{tool_index}"
                            )))
                            .debug_selector(move || {
                                format!("open-file-{message_index}-{tool_index}")
                            })
                            .min_w_0()
                            .overflow_hidden()
                            .flex_1()
                            .when(opens_file, |cell| {
                                cell.cursor_pointer().hover(|s| s.underline()).on_click(
                                    cx.listener(move |this, _event, window, cx| {
                                        // The row below folds the step on any press;
                                        // the link is its own target.
                                        cx.stop_propagation();
                                        this.open_in_editor(message_index, tool_index, window, cx);
                                    }),
                                )
                            })
                            .child(
                                txt(summary, size, theme::muted())
                                    .whitespace_nowrap()
                                    .text_ellipsis(),
                            ),
                    )
            })
            .child(
                txt(
                    if done { "" } else { "●" },
                    theme::CHIP_SIZE,
                    theme::accent(),
                )
                .flex_none(),
            )
            .child(
                txt(
                    format_bytes(tool_bytes(tool)),
                    theme::CHIP_SIZE,
                    theme::faint(),
                )
                .flex_none(),
            )
            .when_some(ms, |head, ms| {
                head.child(txt(format!("{ms} ms"), theme::CHIP_SIZE, theme::faint()).flex_none())
            })
            // The whole row folds, file step or not: the path cell inside a file
            // step stops propagation, so the link still wins where it sits.
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.state.toggle_tool(message_index, tool_index);
                cx.notify();
            }));
        row = row.child(head);
        // The reference's own link out of a step: pressing it opens the matching
        // line in the Trajectory tab, so a reader who wants the whole call - the
        // arguments, the full output, the timing - does not have to unfold the
        // transcript and hunt for it there.
        row = row.child(
            div()
                .debug_selector(move || format!("inspect-{message_index}-{tool_index}"))
                .id(SharedString::from(format!(
                    "inspect-{message_index}-{tool_index}"
                )))
                .ml(px(10.0))
                .mb(px(4.0))
                .px(px(7.0))
                .h(px(22.0))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.0))
                .rounded(px(6.0))
                // No border: a bordered box in a 22px row reads as a heavy frame.
                // The fill carries the shape, and it appears under the pointer.
                .cursor_pointer()
                .hover(|chip| chip.bg(rgb(theme::hover())))
                .child(icon(IconName::Code, 11.0, theme::muted()))
                .child(txt(
                    tr(lang, "chat.inspect"),
                    theme::CHIP_SIZE,
                    theme::muted(),
                ))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.inspect_in_trajectory(message_index, tool_index);
                    cx.notify();
                })),
        );
        // A step that wrote or edited a file can open its change in the Diff
        // pane. The patch is the one the REQUEST carried - bsh keeps no
        // before-image - so the pane shows what the step asked for, which is
        // the same shape the row already previews inline.
        if let Some(lines) = tool_diff(&name, &args) {
            if let Some(path) = tool_path(&args) {
                // The change has usually landed, so the hunk is LOCATED in the
                // file as it is now: real line numbers and real context, not the
                // synthetic `@@ -1,N` a request alone can justify. A file that
                // cannot be read falls back to that synthetic header rather than
                // refusing to open.
                let current = self.current_text_of(&path);
                let patch = match current {
                    Some(text) => crate::ui::diff_pane::patch_for_located(&path, &lines, &text)
                        .or_else(|| crate::ui::diff_pane::patch_for(&path, &lines)),
                    None => crate::ui::diff_pane::patch_for(&path, &lines),
                };
                if let Some(patch) = patch {
                    let label = path.clone();
                    row = row.child(
                        div()
                            .debug_selector(move || {
                                format!("view-diff-{message_index}-{tool_index}")
                            })
                            .id(SharedString::from(format!(
                                "view-diff-{message_index}-{tool_index}"
                            )))
                            .ml(px(10.0))
                            .mb(px(4.0))
                            .px(px(7.0))
                            .h(px(22.0))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(5.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|chip| chip.bg(rgb(theme::hover())))
                            .child(icon(IconName::GitCompareArrows, 11.0, theme::muted()))
                            .child(txt(
                                tr(lang, "chat.viewDiff"),
                                theme::CHIP_SIZE,
                                theme::muted(),
                            ))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                cx.stop_propagation();
                                this.open_diff_text(&label, &patch, cx);
                            })),
                    );
                }
            }
        }
        // A step with no recorded arguments and no output used to unfold to nothing,
        // so a press looked ignored. Say so instead.
        if open && args.trim().is_empty() && output.is_none() {
            row = row.child(
                div()
                    .debug_selector(move || format!("tool-empty-{message_index}-{tool_index}"))
                    .ml(px(10.0))
                    .mb(px(6.0))
                    .child(txt(
                        tr(lang, "chat.stepEmpty"),
                        theme::CHIP_SIZE,
                        theme::faint(),
                    )),
            );
        }
        if open {
            let lang_key = code_lang(&name, &args);
            // The patch wins over the raw arguments when the step wrote or
            // edited: `-`/`+` lines say what changed, where the JSON would make
            // the reader diff it in their head.
            if let Some(lines) = tool_diff(&name, &args) {
                let mut diff = div()
                    .ml(px(10.0))
                    .mb(px(6.0))
                    .rounded(px(8.0))
                    // The fill is the box; a hairline around a block this wide
                    // doubled every edge in the transcript.
                    .bg(rgb(theme::new_session_fill()))
                    .px(px(10.0))
                    .py(px(7.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .max_h(px(theme::TOOL_BODY_MAX))
                    .overflow_hidden();
                for (mark, line) in lines.into_iter().take(60) {
                    let (tone, fill) = if mark == '+' {
                        (theme::success(), theme::diff_add())
                    } else {
                        (theme::error(), theme::diff_del())
                    };
                    diff = diff.child(div().px(px(4.0)).rounded(px(3.0)).bg(rgb(fill)).child(txt(
                        format!("{mark} {line}"),
                        theme::CHIP_SIZE,
                        tone,
                    )));
                }
                row = row.child(diff);
            } else if !args.is_empty() {
                // A command step carries its command in the arguments, so the
                // body shows that rather than the JSON envelope around it.
                let in_text = if lang_key == Some("bash") {
                    json_field(&args, "command").unwrap_or_else(|| args.clone())
                } else {
                    args.clone()
                };
                row = row.child(self.step_block("IN", &in_text, lang_key, true, chrome, cx));
            }
            if let Some(output) = output.clone() {
                row = row.child(self.step_block(
                    "OUT",
                    &output_body(&output),
                    None,
                    false,
                    chrome,
                    cx,
                ));
            }
        }
        row
    }

    /// One labelled body of a step: the label, a copy control, and the text -
    /// highlighted as code when the step carries a language, plain otherwise.
    fn step_block(
        &self,
        label: &'static str,
        text: &str,
        lang: Option<&'static str>,
        is_input: bool,
        chrome: &RowChrome<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let payload = text.to_string();
        let copy_id = format!("step-copy-{label}-{}-{is_input}", payload.len());

        let mut block = div()
            .ml(px(10.0))
            .mb(px(6.0))
            .rounded(px(8.0))
            .bg(rgb(theme::new_session_fill()))
            .px(px(10.0))
            .py(px(7.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(txt(label, theme::CHIP_SIZE, theme::faint()).flex_1())
                    .child(
                        div()
                            .id(SharedString::from(copy_id.clone()))
                            .debug_selector({
                                let selector = copy_id.clone();
                                move || selector.clone()
                            })
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::hover())))
                            .child(icon(IconName::Copy, 11.0, theme::faint()))
                            .on_click(cx.listener(move |_this, _event, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(payload.clone()));
                            })),
                    ),
            );
        let body = match lang {
            Some(lang) => {
                let body = self.body_state(STEP, &format!("```{lang}\n{text}\n```"), cx);
                TextView::new(&body)
            }
            .style(chrome.style.clone())
            .text_size(px(theme::CHIP_SIZE))
            .max_h(px(theme::TOOL_BODY_MAX))
            .overflow_hidden()
            .into_any_element(),
            None => txt(
                text.to_string(),
                theme::CHIP_SIZE,
                if is_input {
                    theme::muted()
                } else {
                    theme::faint()
                },
            )
            .line_height(px(theme::stat_line_at(chrome.content_size)))
            .max_h(px(theme::TOOL_BODY_MAX))
            .overflow_hidden()
            .into_any_element(),
        };
        block = block.child(body);
        block.into_any_element()
    }
    /// Nothing to show.
    fn empty_state(&self, key: &'static str) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(txt(
                self.state.lang.tr(key),
                theme::body_size_at(self.state.draft().global.general.content_font_size),
                theme::muted(),
            ))
    }
}
