//! The right panel: Workspace Files, and the frame the terminal and browser
//! panes will hang off.
//!
//! The panel is the reference's own right column: a tab strip that carries one
//! chip per open pane, a header of controls on its right edge, and either the
//! Start screen's three cards or the body of the pane the reader chose. Only
//! Workspace Files is built here so far; the terminal and the browser take the
//! same tab, so what they need is a tab chip and a body, not a second frame.
//!
//! Workspace Files is a browser, not a tree: one directory is listed at a time,
//! directories lead files, and choosing a directory replaces the listing. That
//! is the reference's own shape, and it is also the one that survives a
//! five-thousand-file workspace without a virtual list.
//!
//! Everything the pane reads and writes goes through [`crate::files`], which
//! confines every path to the workspace root. The panel never joins a path
//! itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::component::diff::{Diff, DiffEvent, DiffFile, DiffMode, DiffState};
use gpui_kit::component::input::{
    Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::Focusable;
use gpui_kit::*;

use super::diff_pane::{self, DiffSource};
use super::shell::{Shell, SideDrag};
use super::{hairline_left, handle_bar, icon, txt, DivExt, IconName};
use crate::files::{self, Kind};
use crate::model::Role;
use crate::terminal::KeyOutcome;
use crate::theme;

// Save the file the panel is showing, bound to cmd-s.
//
// The editor surface carries no save button - the panel's body is the
// editor, and a bar repeating the tab chip's name and one control read as
// chrome - so the keyboard shortcut is the whole affordance, the way a
// native document editor does it.
actions!(bsh_files, [SaveFile, TerminalTab, BrowserTab]);

/// How close in time two blank-space presses must be to read as a double-click.
///
/// macOS tracks its own double-click interval, but the harness's synthetic
/// clicks carry no timing, so the pane measures it: a second press inside this
/// window, and inside [`SELF_DOUBLE_PX`] of the first, is a double-click.
const SELF_DOUBLE_MS: u64 = 400;

/// How close in space two blank-space presses must be to read as a double-click.
///
/// Roughly the width of a fingertip's drift between two presses of the same
/// spot; a press on the far side of the blank area is a different intention.
const SELF_DOUBLE_PX: f32 = 6.0;

/// The top-left a document-anchored overlay sits at, in the editor's own
/// coordinates.
///
/// The kit reports text boxes in WINDOW space - `range_to_bounds` is
/// `text_bounds().origin + glyph offset` - while `absolute()` places a child
/// against its PARENT. The editor's box is that parent, so subtracting its
/// origin is the whole conversion. Without it the offset was added twice
/// (once by the kit, once by the panel sitting to the window's right), which
/// is why the note affordance only showed while the editor happened to start
/// at the window's left edge.
///
/// The point is the middle of the part of `bounds` the document SHOWS: the
/// control sits on the text the reader marked rather than hanging off one
/// edge, and it is pulled back inside the editor so a selection on the last
/// line cannot push it out of the document.
fn document_point(bounds: Bounds<Pixels>, editor: Bounds<Pixels>, size: f32) -> (f32, f32) {
    // A long line runs past the right edge, so the horizontal middle is taken
    // from the slice the editor actually shows; the vertical middle is the
    // line's own, which `anchor_line` has already picked from the visible rows.
    let visible = bounds.intersect(&editor);
    let center_x = if f32::from(visible.size.width) > 0.0 {
        visible.center().x
    } else {
        bounds.center().x
    };
    let x = f32::from(center_x - editor.origin.x) - size / 2.0;
    let y = f32::from(bounds.center().y - editor.origin.y) - size / 2.0;
    let max_x = (f32::from(editor.size.width) - size - 4.0).max(4.0);
    let max_y = (f32::from(editor.size.height) - size - 4.0).max(4.0);
    (x.clamp(4.0, max_x), y.clamp(4.0, max_y))
}

/// The line the note affordance rides: the middle of the part of the selection
/// the viewport shows, as a one-based line number, or `None` when the marks are
/// entirely outside the laid-out rows.
///
/// The whole-range box cannot be the answer. The kit lays out only the visible
/// lines, so a selection whose first or last line has scrolled out arrives
/// with a degenerate box - which made the + vanish the moment the reader
/// scrolled, even while most of the marked text was still on screen. Picking a
/// line that IS laid out and asking for that line's own slice keeps the
/// control on the document: it follows the selection through the scroll, and it
/// goes only when the marks do.
pub(crate) fn anchor_line(
    text: &str,
    range: &std::ops::Range<usize>,
    rows: &std::ops::Range<usize>,
) -> Option<usize> {
    if range.start >= range.end || rows.start >= rows.end {
        return None;
    }
    let first = crate::notes::line_at_offset(text, range.start);
    // The end of a selection that stops at a line break points at the next
    // line, which would name a row the reader never marked.
    let last = crate::notes::line_at_offset(text, range.end.saturating_sub(1));
    // `rows` is zero-based and half-open; the line numbers are one-based.
    let visible_first = rows.start + 1;
    let visible_last = rows.end;
    let from = first.max(visible_first);
    let to = last.min(visible_last);
    if from > to {
        return None;
    }
    Some(from + (to - from) / 2)
}

/// [`document_point`]'s conversion for an overlay that keeps the measured
/// box's own top-left instead of centring on it - a note card hangs UNDER its
/// anchor line, so it wants the line's bottom edge, not its middle.
fn document_origin(bounds: Bounds<Pixels>, editor: Bounds<Pixels>) -> (f32, f32) {
    (
        f32::from(bounds.origin.x - editor.origin.x),
        f32::from(bounds.origin.y - editor.origin.y),
    )
}

/// The wording the panel reports an I/O failure with.
///
/// The reference prints its own `fileReferences` error text beside the row
/// that failed; bsh has no remote to quote, so it prints what the operating
/// system said, which is the more useful half of the same idea.
fn io_msg(error: &std::io::Error) -> String {
    error.to_string()
}

/// The terminal pane's measured geometry for one frame.
///
/// The prepaint pass records it, the terminal clock hands its grid to the PTY,
/// and the mouse handlers turn a pointer position into the cell under it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerminalFrame {
    /// The grid's top-left corner on the window, in logical pixels.
    pub(crate) origin: Point<Pixels>,
    /// One cell's width.
    pub(crate) cell_w: Pixels,
    /// One cell's height.
    pub(crate) cell_h: Pixels,
    /// The grid this frame measured.
    pub(crate) cols: u16,
    /// The rows this frame measured.
    pub(crate) rows: u16,
    /// The size the grid was measured at.
    pub(crate) font_size: f32,
}

/// The bytes a keystroke sends to a real terminal.
///
/// Printable characters go as their UTF-8, Enter as a carriage return (which
/// is what a terminal's line discipline expects), and the editing and arrow
/// keys as the CSI sequences xterm defines. A cmd chord is deliberately not
/// translated: on macOS it belongs to the application, not to the shell.
fn terminal_key_bytes(keystroke: &Keystroke) -> Option<Vec<u8>> {
    if keystroke.modifiers.platform {
        return None;
    }
    let control = keystroke.modifiers.control;
    let alt = keystroke.modifiers.alt;
    let key = keystroke.key.as_str();
    if control && key == "space" {
        return Some(vec![0x00]);
    }
    let mut bytes: Vec<u8> = match key {
        "enter" => b"\r".to_vec(),
        "backspace" => vec![0x7f],
        "tab" => b"\t".to_vec(),
        "escape" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "insert" => b"\x1b[2~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        _ => {
            if control {
                let upper = key.to_ascii_uppercase();
                let first = upper.as_bytes().first().copied()?;
                let byte = if upper.len() == 1 && first.is_ascii_uppercase() {
                    first & 0x1f
                } else {
                    match upper.as_str() {
                        "@" => 0x00,
                        "2" => 0x00,
                        "3" => 0x1b,
                        "4" => 0x1c,
                        "5" => 0x1d,
                        "6" => 0x1e,
                        "7" => 0x1f,
                        "/" => 0x1f,
                        _ => return None,
                    }
                };
                vec![byte]
            } else {
                let text = keystroke.key_char.as_deref().unwrap_or(key);
                let mut chars = text.chars();
                let ch = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                let mut buffer = [0u8; 4];
                ch.encode_utf8(&mut buffer).as_bytes().to_vec()
            }
        }
    };
    if alt && key != "escape" {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Whether a keystroke asks for the terminal's selection on the clipboard.
///
/// Both spellings a terminal user reaches for work: Cmd-C on macOS and
/// Ctrl-Shift-C where Control is the modifier that owns the clipboard.
fn terminal_copy_shortcut(keystroke: &Keystroke) -> bool {
    let key = keystroke.key.as_str();
    (keystroke.modifiers.platform && key == "c")
        || (keystroke.modifiers.control && keystroke.modifiers.shift && key == "c")
}

impl gpui::EntityInputHandler for Shell {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        Some(gpui::UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// A committed composition is the only text the shell should see; the
    /// marked preview belongs to the IME's own candidate window, not the PTY.
    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            return;
        }
        if let Some(terminal) = self.right.terminal.as_mut() {
            terminal.send_key(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

/// Paint one snapshot of the terminal grid into the pane's bounds.
///
/// Each row is shaped as one line with a run per style span, so the cursor's
/// inverse cell and every coloured span cost one shape call for the row rather
/// than one per cell. The block cursor is a plain cell in the grid - painted,
/// not blinked - so no clock owns it.
fn paint_terminal_grid(
    bounds: Bounds<Pixels>,
    frame: &TerminalFrame,
    snapshot: &crate::terminal::Snapshot,
    selection: Option<crate::terminal::Selection>,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(bounds, rgb(theme::page())));
    let default_fg: Hsla = rgb(theme::text()).into();
    let default_bg: Hsla = rgb(theme::page()).into();
    let selected_bg: Hsla = rgb(theme::selected_row()).into();
    // The four faces a run can be shaped in, built once per paint: a grid is
    // mostly monochrome text, but a single styled cell used to rebuild the Font
    // (and its family string) for every run it opened.
    let fonts = grid_fonts();
    for (row_index, cells) in snapshot.rows.iter().enumerate() {
        let document_row = snapshot.first_row + row_index as u64;
        let mut text = String::with_capacity(cells.len());
        let mut runs: Vec<TextRun> = Vec::new();
        let mut style: Option<(Hsla, Hsla, bool, bool, bool, bool)> = None;
        for (col, cell) in cells.iter().take(snapshot.cols).enumerate() {
            // The trailing half of a double-width glyph is covered by that
            // glyph's own advance; it must never paint a second character.
            if cell.is_continuation() {
                continue;
            }
            let mut fg = terminal_color(cell.fg, default_fg);
            let mut bg = terminal_color(cell.bg, default_bg);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            if snapshot.cursor == Some((row_index, col)) {
                fg = default_bg;
                bg = default_fg;
            } else if selection.is_some_and(|selection| selection.contains(document_row, col)) {
                bg = selected_bg;
            }
            if cell.dim() {
                fg.a *= 0.6;
            }
            cell.push_text(&mut text);
            let len = cell.text_len();
            let key = (
                fg,
                bg,
                cell.bold(),
                cell.italic(),
                cell.underline(),
                cell.strike(),
            );
            if style == Some(key) {
                if let Some(run) = runs.last_mut() {
                    run.len += len;
                }
            } else {
                let font = fonts[(cell.bold() as usize) | ((cell.italic() as usize) << 1)].clone();
                runs.push(TextRun {
                    len,
                    font,
                    color: fg,
                    background_color: Some(bg),
                    underline: cell.underline().then_some(UnderlineStyle {
                        color: Some(fg),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    strikethrough: cell.strike().then_some(StrikethroughStyle {
                        color: Some(fg),
                        thickness: px(1.0),
                    }),
                });
                style = Some(key);
            }
        }
        if text.is_empty() {
            continue;
        }
        let origin = point(
            bounds.origin.x,
            bounds.origin.y + frame.cell_h * row_index as f32,
        );
        let line = window
            .text_system()
            .shape_line(text.into(), px(frame.font_size), &runs, None);
        let _ = line.paint_background(origin, frame.cell_h, TextAlign::Left, None, window, cx);
        let _ = line.paint(origin, frame.cell_h, TextAlign::Left, None, window, cx);
    }
}

/// The four monospace faces a grid run can carry, indexed by
/// `bold | italic << 1`.
///
/// Measured once per paint rather than once per run: a Font owns its family
/// string, and a line of tinted text opens a run per style change.
fn grid_fonts() -> [Font; 4] {
    let face = |weight: FontWeight, style: FontStyle| Font {
        family: "monospace".into(),
        weight,
        style,
        ..Default::default()
    };
    [
        face(FontWeight::NORMAL, FontStyle::Normal),
        face(FontWeight::BOLD, FontStyle::Normal),
        face(FontWeight::NORMAL, FontStyle::Italic),
        face(FontWeight::BOLD, FontStyle::Italic),
    ]
}

/// Resolve a cell colour against the theme's default for that slot.
fn terminal_color(color: crate::terminal::Color, fallback: Hsla) -> Hsla {
    use crate::terminal::Color;
    match color {
        Color::Default => fallback,
        Color::Indexed(index) => rgb(indexed_color(index)).into(),
        Color::Rgb(red, green, blue) => {
            let hex = ((red as u32) << 16) | ((green as u32) << 8) | blue as u32;
            rgb(hex).into()
        }
    }
}

/// The xterm 256-colour palette as 0xRRGGBB.
///
/// The first sixteen are xterm's own ANSI values, 16..=231 are the 6x6x6 cube
/// and 232..=255 the grey ramp. A shell's default colours come from the theme
/// instead, so this table is only what a program asked for by number.
fn indexed_color(index: u8) -> u32 {
    const ANSI: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let value = (index - 16) as u32;
            let steps = [0u32, 95, 135, 175, 215, 255];
            let red = steps[(value / 36) as usize];
            let green = steps[((value % 36) / 6) as usize];
            let blue = steps[(value % 6) as usize];
            (red << 16) | (green << 8) | blue
        }
        _ => {
            let grey = 8 + (index - 232) as u32 * 10;
            (grey << 16) | (grey << 8) | grey
        }
    }
}

/// Which pane the right panel is showing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RightTab {
    /// The workspace file browser and editor.
    Files,
    /// The managed terminal: a real interactive shell on a persistent PTY,
    /// drawn from its own cell grid.
    Terminal,
    /// The comparison surface: a patch, a conflicted file, or a file on its
    /// own, drawn in unified or split. It is the one pane whose VIEW the reader
    /// switches from the strip, so its two controls sit in the top bar.
    Diff,
    /// The builtin browser. Not built yet; see [`RightTab::Terminal`].
    Browser,
}

impl RightTab {
    /// The tab's label key in the string table.
    pub(crate) fn key(self) -> &'static str {
        match self {
            RightTab::Files => "right.tab.files",
            RightTab::Terminal => "right.tab.terminal",
            RightTab::Diff => "right.tab.diff",
            RightTab::Browser => "right.tab.browser",
        }
    }

    /// The tab's leading glyph.
    pub(crate) fn glyph(self) -> IconName {
        match self {
            RightTab::Files => IconName::FolderOpen,
            RightTab::Terminal => IconName::SquareTerminal,
            RightTab::Diff => IconName::GitCompareArrows,
            RightTab::Browser => IconName::Globe,
        }
    }

    /// The pane's stable id stem, for the strip's controls and the tests that
    /// press them. The label key is dotted ("right.tab.diff"), which makes a
    /// poor element id, so the two are separate facts.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            RightTab::Files => "files",
            RightTab::Terminal => "terminal",
            RightTab::Diff => "diff",
            RightTab::Browser => "browser",
        }
    }

    /// Every pane the Start screen offers, in the order it lists them.
    const ALL: [RightTab; 4] = [
        RightTab::Files,
        RightTab::Diff,
        RightTab::Terminal,
        RightTab::Browser,
    ];
}

/// The right panel's own state: whether it is open, which pane is up, and the
/// width the reader dragged out.
#[derive(Default)]
pub(crate) struct RightPanel {
    /// Whether the panel is on screen.
    pub(crate) open: bool,
    /// The pane showing, or none for the Start screen.
    pub(crate) tab: Option<RightTab>,
    /// The width a reader dragged out, before it is committed.
    pub(crate) user_w: Option<f32>,
    /// Whether the panel has taken the whole column, which is what its header's
    /// maximise control does.
    pub(crate) maximized: bool,
    /// The resize in flight, if any.
    pub(crate) drag: Option<SideDrag>,
    /// The managed terminal, spawned when the Terminal tab is first opened.
    pub(crate) terminal: Option<crate::terminal::Terminal>,
    /// The terminal's text selection, in the grid's document coordinates.
    pub(crate) terminal_selection: Option<crate::terminal::Selection>,
    /// The cell a terminal press started on, while a drag is in flight.
    pub(crate) terminal_anchor: Option<(u64, usize)>,
    /// The comparison surface's own state: what it is showing, and how.
    pub(crate) diff: super::diff_pane::DiffView,
}

/// The workspace browser's place in the tree, and the files open beside it.
///
/// The pane holds three things at once: the expandable directory tree the
/// reader walks, the directories it has listed so far, and the editor tabs
/// they opened out of it. Keeping all three is what makes the tabs worth
/// having - a reader can step back to the tree and open a second file without
/// losing the first, which is what the reference's dock does and what a file
/// browser that swaps its own body for an editor cannot do.
pub(crate) struct FilesPane {
    /// The workspace root's own listing, from which every branch descends.
    pub(crate) roots: Vec<files::Row>,
    /// The directories the reader has expanded, relative to the root, in the
    /// order they were opened so the chevrons never reshuffle under a hand.
    pub(crate) expanded: Vec<PathBuf>,
    /// Each expanded directory's listing, cached so a keystroke never walks
    /// the filesystem and a failed listing still shows what was there.
    pub(crate) children: HashMap<PathBuf, Vec<files::Row>>,
    /// The failure the last listing or mutation hit, which the pane reports.
    pub(crate) error: Option<String>,
    /// Whether hidden entries are listed.
    pub(crate) hidden: bool,
    /// The open editor tabs, in the order they were opened.
    pub(crate) tabs: Vec<FileTab>,
    /// Which tab the panel is showing, or none for the tree itself: the Files
    /// chip is the tree's own tab, so the tree and any open document are two
    /// surfaces the strip switches between, never two columns sharing a row.
    pub(crate) active: Option<usize>,
    /// The name typed into the New file field.
    pub(crate) new_name: Entity<InputState>,
    /// The entry being renamed, relative to the root, with the field that is
    /// renaming it. A rel rather than a name, because two branches can hold
    /// the same name and only one of them is being renamed.
    pub(crate) renaming: Option<PathBuf>,
    /// The rename field's engine, built once so a switching row keeps focus.
    pub(crate) rename_name: Entity<InputState>,
    /// The entry whose delete is armed, relative to the root: the first press
    /// arms it, the second deletes, which is the reference's own two-press
    /// guard. A rel rather than a name for the same reason `renaming` is.
    pub(crate) deleting: Option<PathBuf>,
    /// Whether the new-file field is showing. The tree keeps no permanent
    /// create row: a double-click on its blank space is what asks for one, and
    /// it goes away once the file is made or the tree is left.
    pub(crate) new_file_open: bool,
    /// Where and when the blank space was last pressed, so two presses close
    /// together in space and time read as a double-click.
    ///
    /// gpui's [`gpui::ClickEvent::click_count`] is the natural signal, but the
    /// test harness's synthetic clicks always report a count of one, and the
    /// gesture is worth driving from a test. So the blank handler tracks the
    /// prior press itself: a second press within [`SELF_DOUBLE_MS`] milliseconds
    /// and [`SELF_DOUBLE_PX`] pixels of the last one opens the field.
    pub(crate) last_blank_press: Option<(gpui::Point<gpui::Pixels>, std::time::Instant)>,
    /// The notes the reader has pinned to this workspace's files, loaded from
    /// the workspace's own `.bsh` directory and written back on every change.
    pub(crate) notes: crate::notes::Notes,
    /// Whether the notes drawer is showing over the document.
    pub(crate) notes_open: bool,
    /// Whether the inline comment card is up over the document.
    pub(crate) comment_open: bool,
    /// The note being typed into the drawer's field.
    pub(crate) note_text: Entity<TextareaState>,
}

/// One file open in the editor area, as the tab strip carries it.
pub(crate) struct FileTab {
    /// Its path relative to the workspace root, which is what the pane joins
    /// onto the root for every later read and write.
    pub(crate) rel: PathBuf,
    /// The editing engine, or none for an image or a file the editor refused.
    pub(crate) editor: Option<Entity<EditorState>>,
    /// How it opened, so the save path and the fallback message agree.
    pub(crate) kind: Kind,
    /// Whether the buffer holds changes the disk does not.
    pub(crate) dirty: bool,
}

impl FileTab {
    /// The name the tab is labelled with: the file's own name, not its path,
    /// because a tab bar that spelled out every path would be unreadable at
    /// the panel's width. The full path stays in the editor header above it.
    pub(crate) fn label(&self) -> String {
        self.rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.rel.display().to_string())
    }
}

impl FilesPane {
    /// The pane as it opens: at the workspace root, with nothing listed and no
    /// tabs, which is the state the reference's own dock starts in.
    pub(crate) fn new(
        new_name: Entity<InputState>,
        rename_name: Entity<InputState>,
        note_text: Entity<TextareaState>,
    ) -> Self {
        Self {
            roots: Vec::new(),
            expanded: Vec::new(),
            children: HashMap::new(),
            error: None,
            hidden: false,
            tabs: Vec::new(),
            active: None,
            new_name,
            renaming: None,
            rename_name,
            deleting: None,
            new_file_open: false,
            last_blank_press: None,
            notes: crate::notes::Notes::default(),
            notes_open: false,
            comment_open: false,
            note_text,
        }
    }

    /// The tab the panel is showing, if any.
    pub(crate) fn active_tab(&self) -> Option<&FileTab> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    /// The index of the tab already showing `rel`, if any, so opening a file a
    /// second time focuses it instead of stacking a duplicate.
    pub(crate) fn tab_index(&self, rel: &Path) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.rel == rel)
    }
}

/// Turn what the reader typed into a URL worth asking the policy about.
///
/// A bare host gets https, which is what a browser does when the scheme is
/// omitted: "example.com" is a request the reader meant, while "file:/x" and
/// "javascript:..." keep their scheme and are refused by the transport's own
/// rule with a sentence naming the scheme.
fn normalize_address(typed: &str) -> String {
    let text = typed.trim();
    if text.is_empty() {
        return String::new();
    }
    if text.contains("://") || text.to_ascii_lowercase().starts_with("http:") {
        return text.to_string();
    }
    format!("https://{}", text.trim_start_matches('/'))
}

impl Shell {
    /// Whether the panel is the column at the window's top-left corner.
    ///
    /// The panel is the LAST column of the row, so it only reaches x=0 when the
    /// sidebar is folded AND the panel is wide enough to squeeze the main
    /// column to nothing - a maximised panel asks for exactly the row's width,
    /// which with no sidebar is the whole window. That is the state the band is
    /// for: the panel's tab strip shares the traffic-light strip and starts
    /// after the lights. A plain (non-maximised) panel is capped at half its
    /// space, so it always has the main column beside it and never owns the
    /// corner. The zero-width main column is not rendered in this state; see
    /// `Shell::render`.
    pub(crate) fn right_panel_owns_the_corner(&self) -> bool {
        self.right.open
            && self.sidebar_folded()
            && self.viewport_w > 0.0
            && self.right_panel_w() >= self.viewport_w
    }

    /// The titlebar band the panel owns when it is the window's corner column.
    ///
    /// The band IS the bar: the tab strip is drawn in the same 52px the
    /// traffic lights sit in, so the panel reads as a titlebar rather than an
    /// empty strip with the tabs pushed below it. The bar starts after the
    /// lights; the stretch left over - exactly the inset's width - is the
    /// drag region, so the corner still moves the window while the lights stay
    /// uncovered and clickable (macOS draws them over the content, and this
    /// strip only asks for the move). Only the column that owns the corner
    /// draws it - see [`Shell::right_panel_owns_the_corner`] - which is why a
    /// folded panel leaves no dead hole behind: the corner goes back to the
    /// sidebar or the main column, which own it in their own right.
    fn right_panel_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let zone = theme::traffic_light_inset();
        div()
            .debug_selector(|| String::from("titlebar-band"))
            .w_full()
            .h(px(zone.bottom))
            .flex_none()
            .flex()
            .flex_row()
            .child(
                div()
                    .debug_selector(|| String::from("titlebar-drag"))
                    .id("titlebar-drag")
                    .w(px(zone.right))
                    .h_full()
                    .flex_none()
                    .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
                        // The same call AppKit's titlebar makes for a
                        // system-dragged window: the band reads as the title
                        // bar, not as a control.
                        window.start_window_move();
                    }),
            )
            .child(self.right_panel_bar(cx, true))
    }

    /// The right panel column and its resize strip, absent while it is folded.
    pub(crate) fn render_right_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.right.open {
            return None;
        }
        let width = self.right_panel_w();
        // The panel is the last column, so it normally sits beside the main
        // column and away from the window's corner. The one state where it owns
        // that corner is the maximised panel with the sidebar folded, and there
        // its header takes the traffic-light band the sidebar gave up, through
        // the one shared inset, like the header does with its folded left pad.
        let owns_corner = self.right_panel_owns_the_corner();
        let column = div()
            .debug_selector(|| String::from("right-panel"))
            // The panel is a fixed column BESIDE a flex-1 main column, so a
            // width that does not fit does not shrink the chat - the row
            // overflows the window and the panel's right edge lands off screen,
            // taking its controls with it. The resolved width cannot know its
            // sibling, so the floor of a real panel is enforced here, where the
            // flex row is the thing doing the fitting.
            .min_w(px(theme::RIGHT_PANEL_MIN))
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::page()))
            // One header, in one of its two placements: the band (bar and drag
            // region sharing the traffic-light strip) when the panel owns the
            // corner, or the plain 40px bar below it when it does not. They are
            // alternatives, never both, so the strip is reserved exactly once.
            .when(owns_corner, |column| {
                column.child(self.right_panel_titlebar(cx))
            })
            .when(!owns_corner, |column| {
                column.child(self.right_panel_bar(cx, false))
            })
            // The reference hangs the same `.5px` rule on this seam that it
            // hangs on the sidebar's, so the panel reads as a column of the
            // same family.
            .child({
                let started = std::time::Instant::now();
                let body = self.right_panel_body(cx);
                let took = started.elapsed();
                if took.as_millis() >= 2 {
                    eprintln!(
                        "BSHPROBE right_panel_body {:?} tab={:?}",
                        took, self.right.tab
                    );
                }
                body
            });
        Some(
            div()
                .debug_selector(|| String::from("right-panel-frame"))
                .relative()
                .w(px(width))
                .h_full()
                .flex_shrink_0()
                // Moving anywhere else in the column puts the bar away; the
                // handle's own move stops propagation, so the two never fight.
                .on_mouse_move(cx.listener(|this, _event: &MouseMoveEvent, _window, cx| {
                    this.hover_right_seam(false, cx);
                }))
                .child(hairline_left(column).border_color(rgb(theme::border_soft())))
                .child(self.render_right_panel_handle(cx)),
        )
    }

    /// The reference's DragHandle for the panel: an 8px col-resize strip
    /// straddling the seam, drawn only while the column is up, carrying the
    /// same reveal bar the content column's handles do: invisible until the
    /// pointer is on the seam (or a drag holds it), then the measured 2x72.
    fn render_right_panel_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("right-panel-handle")
            .debug_selector(|| String::from("right-panel-handle"))
            .occlude()
            .absolute()
            .top(px(0.0))
            .bottom(px(0.0))
            .left(px(-crate::theme::SIDEBAR_HANDLE_W / 2.0))
            .w(px(crate::theme::SIDEBAR_HANDLE_W))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.begin_right_panel_drag(event.position.x.as_f32(), cx);
                }),
            )
            // The bar follows the pointer; stopping the move here is what keeps
            // the column's own move from clearing it in the same dispatch.
            .on_mouse_move(cx.listener(|this, _event: &MouseMoveEvent, _window, cx| {
                this.hover_right_seam(true, cx);
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.commit_right_panel_width(event.position.x.as_f32(), cx);
                }),
            )
            .child(handle_bar(
                "right-panel-bar",
                if self.right.drag.is_some() || self.right_hover {
                    1.0
                } else {
                    0.0
                },
            ))
    }

    /// The panel's bar: the Start screen's title, or the tab strip and the
    /// controls the reference draws on every panel header.
    ///
    /// `band` selects the placement, but both share ONE 28px content row, so a
    /// 28px tab chip, a 26px file chip and a 26px control share one centre
    /// line. The plain 40px header centres that row at y=6 (the open-sidebar
    /// state); the traffic-light band starts it at
    /// [`theme::RIGHT_BAR_BAND_PAD`] (10) so the row's centre is the window
    /// buttons' own centre line (24), not the 52px strip's centre (26) - a
    /// screenshot showed the row sitting high against the lights.
    fn right_panel_bar(&self, cx: &mut Context<Self>, band: bool) -> impl IntoElement {
        let bar = div()
            // The header, not the seam's reveal bar: that one already carries
            // "right-panel-bar", and two elements sharing a selector made the
            // strip's bounds unreadable (debug_bounds returned the 2x72 bar).
            .debug_selector(|| String::from("right-panel-header"))
            .flex()
            .flex_row()
            .items_start()
            .px(px(theme::RIGHT_PAD))
            .when(band, |bar| {
                bar.h(px(theme::traffic_light_inset().bottom))
                    .pt(px(theme::RIGHT_BAR_BAND_PAD))
                    .flex_1()
                    .min_w_0()
            })
            .when(!band, |bar| {
                bar.h(px(theme::RIGHT_BAR_H))
                    .pt(px(theme::RIGHT_BAR_TOP_PAD))
                    .flex_none()
            });
        // The one row every placement uses: its 28px height is what gives the
        // 28px and 26px chips the same vertical centre inside it.
        let row = div()
            .h(px(theme::RIGHT_TAB_H))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0));
        let content = match self.right.tab {
            None => row
                .child(icon(IconName::Compass, 15.0, theme::muted()))
                .child(
                    txt(
                        self.state.lang.tr("right.start"),
                        theme::SIDEBAR_SIZE,
                        theme::text(),
                    )
                    .font_weight(FontWeight::MEDIUM),
                )
                .child(div().flex_1())
                .child(self.right_panel_control(
                    "right-maximize",
                    if self.right.maximized {
                        IconName::Minimize2
                    } else {
                        IconName::Maximize2
                    },
                    cx,
                ))
                .child(self.right_panel_close(cx)),
            Some(tab) => {
                // The strip is a real tab list: the open pane carries its
                // label, every other pane keeps its glyph one press away. It
                // used to carry only the pane that was already open, which left
                // the Start screen as the only door to a pane - and Diff, whose
                // body needs no picker, had no door at all from here.
                let mut content = row;
                for pane in RightTab::ALL {
                    content = content.child(if pane == tab {
                        self.right_tab_chip(pane, cx).into_any_element()
                    } else {
                        self.right_pane_chip(pane, cx).into_any_element()
                    });
                }
                // The files open out of the Files pane join the panel's own tab
                // strip, right after its chip: the reference's dock carries
                // every open pane and document in one strip, and a second strip
                // inside the pane would read as two unrelated rows.
                if tab == RightTab::Files {
                    for (index, file_tab) in self.files.tabs.iter().enumerate() {
                        content = content.child(self.files_tab_chip(index, file_tab, cx));
                    }
                }
                content
                    .child(
                        div()
                            .id("right-new-tab")
                            .debug_selector(|| String::from("right-new-tab"))
                            .w(px(26.0))
                            .h(px(26.0))
                            .rounded(px(theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Plus, 14.0, theme::faint()))
                            // A new tab lands back on the Start screen, which is
                            // where a pane without a chip of its own would be
                            // picked.
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.right.tab = None;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    // No strip control for the review drawer: the + that follows
                    // a selection is its door now, so the strip carries only the
                    // panel's own tabs and controls.
                    .child(self.right_panel_control(
                        "right-maximize",
                        if self.right.maximized {
                            IconName::Minimize2
                        } else {
                            IconName::Maximize2
                        },
                        cx,
                    ))
                    // The rightmost control is the reference's own collapse
                    // control; a second panel-toggle before it would be the same
                    // action wearing two glyphs.
                    .child(self.right_panel_close(cx))
            }
        };
        bar.child(content)
    }

    /// One pane's chip in the strip, for every pane that is not the open one.
    ///
    /// Its glyph and its tooltip are the same pair the Start card carries, so
    /// the strip and the Start screen name a pane the same way, and pressing it
    /// is the same call the Start card makes.
    fn right_pane_chip(&self, tab: RightTab, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self.state.lang.tr(tab.key());
        div()
            .id(SharedString::from(format!("right-pane-{}", tab.slug())))
            .debug_selector(move || format!("right-pane-{}", tab.slug()))
            .h(px(theme::RIGHT_TAB_H))
            .w(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .child(icon(tab.glyph(), 14.0, theme::muted()))
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.open_right_tab(tab, cx);
                // A terminal is a text surface: opening it puts the cursor
                // where the reader's next keystroke will land.
                if tab == RightTab::Terminal {
                    window.focus(&this.terminal_focus, cx);
                }
            }))
    }

    /// The one open tab, with the close that drops the pane back to Start.
    ///
    /// For the Files pane the chip is also the tree's own tab: when a document
    /// holds the body, the chip sits unselected and pressing it hands the body
    /// back, which is how the reference's dock moves between a pane and the
    /// documents opened out of it.
    fn right_tab_chip(&self, tab: RightTab, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let selected = match tab {
            RightTab::Files => self.files.active.is_none(),
            _ => true,
        };
        div()
            .id("right-tab")
            .debug_selector(|| String::from("right-tab"))
            .h(px(theme::RIGHT_TAB_H))
            .px(px(8.0))
            .rounded(px(theme::RADIUS_SM))
            .when(selected, |chip| chip.bg(rgb(theme::selected_row())))
            .when(!selected, |chip| chip.cursor_pointer())
            .when(!selected, |chip| chip.hover(|s| s.bg(rgb(theme::hover()))))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(icon(
                tab.glyph(),
                14.0,
                if selected {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .child(txt(
                lang.tr(tab.key()),
                theme::CHIP_SIZE,
                if selected {
                    theme::text()
                } else {
                    theme::muted()
                },
            ))
            .child(
                div()
                    .id("right-tab-close")
                    .debug_selector(|| String::from("right-tab-close"))
                    .ml(px(2.0))
                    .px(px(3.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::X, 12.0, theme::faint()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.tab = None;
                        cx.notify();
                    })),
            )
            .when(tab == RightTab::Files && !selected, |chip| {
                chip.on_click(cx.listener(|this, _event, _window, cx| {
                    this.focus_tree(cx);
                }))
            })
    }

    /// A small square control on the panel header.
    fn right_panel_control(
        &self,
        id: &'static str,
        mark: IconName,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .debug_selector(|| id.to_string())
            .id(id)
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(mark, 14.0, theme::muted()))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // The glyph is the contract: Maximise takes the column, Minimise
                // gives it back, and neither one closes the panel - the
                // collapse control beside this one does that. Both used to fold
                // the panel, so the maximise icon was lying.
                this.right.maximized = !this.right.maximized;
                cx.notify();
            }))
    }

    /// The panel's collapse control, the reference's own right-edge control.
    ///
    /// The reference labels this slot with the panel-right glyph, not an X:
    /// it collapses the whole column back behind the header's show control,
    /// and the icon says that, so a reader looking for "where did the panel
    /// go" finds the same shape they used to open it.
    fn right_panel_close(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| String::from("right-close"))
            .id("right-close")
            .w(px(26.0))
            .h(px(26.0))
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(IconName::PanelRightClose, 14.0, theme::muted()))
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.right.open = false;
                this.right.tab = None;
                cx.notify();
            }))
    }

    /// The panel body: the Start screen, or the chosen pane.
    fn right_panel_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let __all = std::time::Instant::now();
        let body = div()
            .debug_selector(|| String::from("right-panel-body"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        match self.right.tab {
            None => body.child(self.right_start(cx)),
            Some(RightTab::Files) => {
                let t = std::time::Instant::now();
                let pane = body.child(self.files_pane(cx));
                let took = t.elapsed();
                if took.as_millis() >= 2 {
                    eprintln!("BSHPROBE files_pane {:?}", took);
                }
                let _ = __all;
                pane
            }
            Some(RightTab::Terminal) => body.child(self.terminal_pane(cx)),
            Some(RightTab::Diff) => body.child(self.diff_pane(cx)),
            Some(RightTab::Browser) => body.child(self.browser_pane(cx)),
        }
    }

    /// A pane that has not been built yet says so, rather than drawing an
    /// empty box the reader has to interpret.
    fn right_pane_placeholder(&self, tab: RightTab) -> impl IntoElement {
        let lang = self.state.lang;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(theme::RIGHT_PAD * 2.0))
            .child(icon(tab.glyph(), 22.0, theme::faint()))
            .child(
                txt(lang.tr(tab.key()), theme::SIDEBAR_SIZE, theme::text())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                txt(lang.tr("right.notBuilt"), theme::CHIP_SIZE, theme::faint())
                    .text_align(TextAlign::Center),
            )
    }

    /// The built-in Browser pane.
    ///
    /// It is a text view, not a rendered page, and it draws only what the
    /// document model actually holds: the address bar, the four navigation
    /// controls, the loading or failure line, and the extracted title, URL,
    /// text and interactive elements. Nothing here paints a page the browser
    /// did not read.
    fn browser_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.browser_pane.clone().unwrap_or_default();
        let mut pane = div()
            .debug_selector(|| String::from("browser-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme::page()));
        pane = pane.child(
            div()
                .flex_none()
                .w_full()
                .h(px(36.0))
                .px(px(theme::RIGHT_PAD))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(self.browser_icon(
                    "browser-back",
                    IconName::ArrowLeft,
                    view.can_back,
                    "right.browser.back",
                    cx,
                    |this, cx| this.browser_history(-1, cx),
                ))
                .child(self.browser_icon(
                    "browser-forward",
                    IconName::ArrowRight,
                    view.can_forward,
                    "right.browser.forward",
                    cx,
                    |this, cx| this.browser_history(1, cx),
                ))
                .child(self.browser_icon(
                    "browser-reload",
                    IconName::RotateCw,
                    view.snapshot.is_some(),
                    "right.browser.reload",
                    cx,
                    |this, cx| this.browser_reload(cx),
                ))
                .child(
                    div()
                        .debug_selector(|| String::from("browser-address"))
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.browser_url).appearance(false).h(px(26.0))),
                )
                .child(self.browser_go_control(cx)),
        );
        pane.child(self.browser_body(view, cx))
    }

    /// One navigation control: a glyph that dims when the action cannot run.
    fn browser_icon(
        &self,
        id: &'static str,
        glyph: IconName,
        enabled: bool,
        label: &'static str,
        cx: &mut Context<Self>,
        action: fn(&mut Shell, &mut Context<Shell>),
    ) -> impl IntoElement {
        let tooltip = self.state.lang.tr(label);
        let mut button = div()
            .id(id)
            .debug_selector(move || String::from(id))
            .w(px(26.0))
            .h(px(26.0))
            .flex_none()
            .rounded(px(theme::RADIUS_SM))
            .flex()
            .items_center()
            .justify_center()
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx));
        if enabled {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(rgb(theme::hover())))
                .child(icon(glyph, 14.0, theme::muted()))
                .on_click(cx.listener(move |this, _event, _window, cx| action(this, cx)));
        } else {
            button = button.child(icon(glyph, 14.0, theme::faint()));
        }
        button
    }

    /// The Go control.
    fn browser_go_control(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("browser-go")
            .debug_selector(|| String::from("browser-go"))
            .h(px(26.0))
            .px(px(10.0))
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(rgb(theme::border()))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(theme::hover())))
            .flex()
            .items_center()
            .child(
                txt(
                    self.state.lang.tr("right.browser.go"),
                    theme::CHIP_SIZE,
                    theme::text(),
                )
                .font_weight(FontWeight::MEDIUM),
            )
            .on_click(cx.listener(|this, _event, _window, cx| this.browser_submit(cx)))
    }

    /// The pane's body: the empty state, the loading line, a failure, or the
    /// page itself.
    fn browser_body(&self, view: crate::browser::PaneView, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let state_id = format!("browser-state-{}", view.status.as_str());
        let mut body = div()
            .debug_selector(move || state_id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        if let Some(failure) = view.failure.clone() {
            body = body.child(self.browser_failure(failure));
        }
        match view.status {
            crate::browser::Status::Empty => body
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .px(px(theme::RIGHT_PAD * 2.0))
                        .child(icon(IconName::Globe, 22.0, theme::faint()))
                        .child(
                            txt(
                                lang.tr("right.browser.empty"),
                                theme::CHIP_SIZE,
                                theme::faint(),
                            )
                            .text_align(TextAlign::Center)
                            .line_height(px(18.0)),
                        ),
                )
                .into_any_element(),
            crate::browser::Status::Loading => {
                let line = crate::i18n::tr_args(
                    lang,
                    "right.browser.loading",
                    &[("url", view.url.clone())],
                );
                body.child(
                    div()
                        .debug_selector(|| String::from("browser-loading"))
                        .w_full()
                        .px(px(theme::RIGHT_PAD))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(icon(IconName::RotateCw, 12.0, theme::muted()))
                        .child(txt(line, theme::CHIP_SIZE, theme::muted()).truncate()),
                )
                .into_any_element()
            }
            crate::browser::Status::Ready | crate::browser::Status::Failed => match view.snapshot {
                Some(snapshot) => body
                    .child(self.browser_page(snapshot, cx))
                    .into_any_element(),
                None => body
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(txt(
                                lang.tr("right.browser.empty"),
                                theme::CHIP_SIZE,
                                theme::faint(),
                            )),
                    )
                    .into_any_element(),
            },
        }
    }

    /// The failure line: what happened, and the rule when the policy was the
    /// reason. It sits above the page, because a failed navigation keeps the
    /// page the reader was already reading.
    fn browser_failure(&self, failure: crate::browser::Failure) -> impl IntoElement {
        let lang = self.state.lang;
        let mut note = div()
            .debug_selector(|| String::from("browser-failure"))
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .py(px(8.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .bg(rgb(theme::hover()))
            .child(
                txt(lang.tr(failure.key()), theme::CHIP_SIZE, theme::error())
                    .font_weight(FontWeight::MEDIUM),
            )
            .child(
                txt(failure.message.clone(), theme::CHIP_SIZE, theme::text()).line_height(px(16.0)),
            );
        if let Some(rule) = failure.rule.clone() {
            note = note.child(txt(
                crate::i18n::tr_args(lang, "right.browser.rule", &[("rule", rule)]),
                theme::RIGHT_HINT_SIZE,
                theme::faint(),
            ));
        }
        note
    }

    /// The extracted page: title, URL, capped text and the interactive rows.
    fn browser_page(
        &self,
        snapshot: crate::browser::Snapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.state.lang;
        let mut page = div()
            .id("browser-page")
            .debug_selector(|| String::from("browser-page"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(theme::RIGHT_PAD))
            .py(px(8.0))
            .overflow_y_scroll();
        page = page
            .child(
                div()
                    .debug_selector(|| String::from("browser-title"))
                    .w_full()
                    .child(
                        txt(snapshot.title.clone(), theme::SIDEBAR_SIZE, theme::text())
                            .font_weight(FontWeight::MEDIUM)
                            .line_height(px(19.0)),
                    )
                    .child(
                        txt(snapshot.url.clone(), theme::RIGHT_HINT_SIZE, theme::faint())
                            .truncate()
                            .line_height(px(15.0)),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| String::from("browser-text"))
                    .w_full()
                    .child(
                        txt(snapshot.text.clone(), theme::CHIP_SIZE, theme::text())
                            .line_height(px(18.0)),
                    ),
            );
        if snapshot.text_truncated {
            let note = crate::i18n::tr_args(
                lang,
                "right.browser.capped",
                &[
                    ("cap", snapshot.text_cap.to_string()),
                    ("total", snapshot.text_total_chars.to_string()),
                ],
            );
            page =
                page.child(txt(note, theme::RIGHT_HINT_SIZE, theme::faint()).line_height(px(15.0)));
        }
        // The element rows: the refs the model may pass back, and the only
        // place the pane shows that a page has controls at all.
        let heading = crate::i18n::tr_args(
            lang,
            "right.browser.elements",
            &[("count", snapshot.elements_total.to_string())],
        );
        page = page.child(div().w_full().pt(px(4.0)).child(
            txt(heading, theme::RIGHT_HINT_SIZE, theme::muted()).font_weight(FontWeight::MEDIUM),
        ));
        if snapshot.elements.is_empty() {
            page = page.child(
                txt(
                    lang.tr("right.browser.noElements"),
                    theme::RIGHT_HINT_SIZE,
                    theme::faint(),
                )
                .line_height(px(15.0)),
            );
        }
        for element in snapshot.elements.iter() {
            page = page.child(self.browser_element_row(element.clone(), cx));
        }
        if snapshot.elements_total > snapshot.elements.len() {
            let note = crate::i18n::tr_args(
                lang,
                "right.browser.cappedElements",
                &[
                    ("shown", snapshot.elements.len().to_string()),
                    ("total", snapshot.elements_total.to_string()),
                ],
            );
            page =
                page.child(txt(note, theme::RIGHT_HINT_SIZE, theme::faint()).line_height(px(15.0)));
        }
        page
    }

    /// One interactive element as a row.
    ///
    /// A link row is a real control: pressing it navigates through the same
    /// gate the address bar uses. Every other row is a readout, because this
    /// view cannot type into a field from the reader's side without a per-field
    /// editor - the model does that through browser_type, and drawing a
    /// pointer cursor on a row that does nothing would be the exact "control
    /// that only looks wired" this crate refuses.
    fn browser_element_row(
        &self,
        element: crate::browser::ElementView,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let reference = element.reference.clone();
        let id = element.reference.clone();
        let mut row = div()
            .id(SharedString::from(format!("browser-element-{id}")))
            .debug_selector(move || format!("browser-element-{id}"))
            .w_full()
            .min_h(px(22.0))
            .px(px(4.0))
            .rounded(px(4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                txt(reference, theme::RIGHT_HINT_SIZE, theme::accent())
                    .font_family("monospace")
                    .flex_none(),
            )
            .child(txt(element.kind, theme::RIGHT_HINT_SIZE, theme::faint()).flex_none());
        let label = if let Some(value) = element.value.clone() {
            if value.is_empty() {
                element.label.clone()
            } else {
                format!("{} = {}", element.label, value)
            }
        } else {
            element.label.clone()
        };
        row = row.child(txt(label, theme::CHIP_SIZE, theme::text()).truncate());
        if let Some(href) = element.href.clone() {
            if element.kind == "link" {
                row = row
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(theme::hover())))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.browser_go(href.clone(), cx);
                    }));
            }
        }
        row
    }

    /// The managed terminal's surface: one full-bleed cell grid painted from
    /// the shell's own screen.
    ///
    /// The shell is an interactive process on a persistent PTY, so its prompt,
    /// its colours, its line editor and its history are the ones on screen:
    /// bsh draws no prompt of its own and has no separate input line. Typing is
    /// captured by this pane and written to the PTY as raw bytes, and the
    /// parser above turns the shell's answer into the cells painted here.
    ///
    /// The pane's cell metrics are measured at prepaint time into
    /// terminal_frame, so the terminal clock can resize the PTY from what the
    /// pane actually occupies rather than from a constant, and the pointer
    /// handlers can turn a drag position into the cell under it.
    fn terminal_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.right.terminal.is_none() {
            return self
                .right_pane_placeholder(RightTab::Terminal)
                .into_any_element();
        }
        let snapshot = std::rc::Rc::clone(&self.terminal_snapshot);
        // The IME reaches a view only through the window's input handler, which
        // is set during paint; without it a Chinese keyboard composes nothing
        // the shell can read.
        let ime_focus = self.terminal_focus.clone();
        let ime_panel = cx.entity();
        let measured = std::rc::Rc::clone(&self.terminal_frame);
        let font_size = theme::BODY_SIZE - 1.0;
        let selection = self.right.terminal_selection;
        let mut pane = div()
            .debug_selector(|| String::from("terminal-pane"))
            .id("terminal-pane")
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme::page()))
            .cursor_text()
            .track_focus(&self.terminal_focus)
            // Tab is bound in this context (see terminal_tab): without a
            // binding, the macOS input handler hands it to AppKit's key-view
            // loop and focus leaves the pane instead of the shell completing.
            .key_context("terminal")
            .on_action(cx.listener(|this, _: &TerminalTab, _window, cx| {
                this.terminal_tab(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.terminal_key(event, cx);
            }))
            // A press clears the old selection and arms a new drag; the move
            // handler grows it while the button is held, and the release keeps
            // whatever was selected. A click alone therefore selects nothing.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.terminal_focus, cx);
                    this.terminal_press(event.position, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    this.terminal_drag(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.terminal_release(cx);
                }),
            )
            // The wheel browses the retained scrollback; the pane stays at the
            // bottom until the reader scrolls away from it.
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.terminal_wheel(event, cx);
            }));
        if let Some((text, is_error)) = self.terminal_status_text() {
            pane = pane.child(
                div()
                    .debug_selector(|| String::from("terminal-status"))
                    .id("terminal-status")
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .bg(rgb(theme::page()))
                    .child(txt(
                        text,
                        theme::CHIP_SIZE,
                        if is_error {
                            theme::error()
                        } else {
                            theme::faint()
                        },
                    )),
            );
        }
        pane.child(
            canvas(
                move |bounds, window, _cx| {
                    let font = Font {
                        family: "monospace".into(),
                        ..Default::default()
                    };
                    let font_id = window.text_system().resolve_font(&font);
                    let mut cell_w = window
                        .text_system()
                        .layout_width(font_id, px(font_size), 'M');
                    if cell_w <= px(0.0) {
                        cell_w = px(font_size * 0.6);
                    }
                    let cell_h = px(font_size * 1.4);
                    let cols = ((bounds.size.width / cell_w).floor() as i32).clamp(1, 500) as u16;
                    let rows = ((bounds.size.height / cell_h).floor() as i32).clamp(1, 300) as u16;
                    let frame = TerminalFrame {
                        origin: bounds.origin,
                        cell_w,
                        cell_h,
                        cols,
                        rows,
                        font_size,
                    };
                    measured.set(Some(frame));
                    frame
                },
                move |bounds, frame, window, cx| {
                    window.handle_input(
                        &ime_focus,
                        gpui::ElementInputHandler::new(bounds, ime_panel.clone()),
                        cx,
                    );
                    paint_terminal_grid(bounds, &frame, &snapshot, selection, window, cx);
                },
            )
            .flex_1()
            .min_h(px(0.0))
            .w_full(),
        )
        .into_any_element()
    }

    /// One keystroke to the shell.
    ///
    /// Every key is translated to the bytes a real terminal sends: printable
    /// characters as their UTF-8, Enter as a carriage return, the arrows and
    /// the editing block as their CSI sequences, and control chords as C0. A
    /// cmd chord is left alone so the shell does not swallow app shortcuts.
    pub(crate) fn terminal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        // Copy takes the chord only while there is a selection: with none it
        // falls through to the shell, so Cmd-C still means what it always did.
        if terminal_copy_shortcut(&event.keystroke) && self.terminal_copy(cx) {
            cx.stop_propagation();
            return;
        }
        let Some(bytes) = terminal_key_bytes(&event.keystroke) else {
            crate::terminal::trace_key(&event.keystroke.key, None);
            return;
        };
        crate::terminal::trace_key(&event.keystroke.key, Some(&bytes));
        let Some(terminal) = self.right.terminal.as_mut() else {
            return;
        };
        match terminal.send_key(&bytes) {
            KeyOutcome::Sent => cx.stop_propagation(),
            KeyOutcome::Refused => {
                // The status line names the refusal, so the pane has to redraw.
                cx.notify();
                cx.stop_propagation();
            }
            // No shell to write to: the status line already says why.
            KeyOutcome::Closed => {}
        }
    }

    /// Tab belongs to the shell.
    ///
    /// The macOS input handler passes an unbound Tab to AppKit's key-view
    /// loop, which moves focus out of the pane before the shell sees the byte,
    /// so Tab is bound to this action in the pane's `terminal` key context and
    /// forwarded exactly as a keystroke would be.
    pub(crate) fn terminal_tab(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.right.terminal.as_mut() else {
            return;
        };
        match terminal.send_key(b"\t") {
            KeyOutcome::Sent => cx.stop_propagation(),
            KeyOutcome::Refused => {
                cx.notify();
                cx.stop_propagation();
            }
            KeyOutcome::Closed => {}
        }
    }

    /// The cell a window position names, in the grid's document row and column.
    fn terminal_cell(&self, position: Point<Pixels>) -> Option<(u64, usize)> {
        let frame = self.terminal_frame.get()?;
        let snapshot = &self.terminal_snapshot;
        if snapshot.rows.is_empty() || snapshot.cols == 0 {
            return None;
        }
        let x = position.x - frame.origin.x;
        let y = position.y - frame.origin.y;
        if x < px(0.0) || y < px(0.0) {
            return None;
        }
        let col = (x / frame.cell_w).floor() as usize;
        let row = (y / frame.cell_h).floor() as usize;
        if col >= snapshot.cols || row >= snapshot.rows.len() {
            return None;
        }
        Some((snapshot.first_row + row as u64, col))
    }

    /// A press clears the old selection and arms a new drag.
    pub(crate) fn terminal_press(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.right.terminal_selection = None;
        self.right.terminal_anchor = self.terminal_cell(position);
        cx.notify();
    }

    /// A move with the button held grows the selection from the press.
    pub(crate) fn terminal_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(anchor) = self.right.terminal_anchor else {
            return;
        };
        let Some(head) = self.terminal_cell(position) else {
            return;
        };
        let selection = (head != anchor).then(|| crate::terminal::Selection::new(anchor, head));
        if self.right.terminal_selection != selection {
            self.right.terminal_selection = selection;
            cx.notify();
        }
    }

    /// A release keeps whatever the drag selected; a click alone leaves none.
    pub(crate) fn terminal_release(&mut self, cx: &mut Context<Self>) {
        self.right.terminal_anchor = None;
        cx.notify();
    }

    /// The selected text, when a selection covers any.
    pub(crate) fn terminal_selection_text(&self) -> Option<String> {
        let selection = self.right.terminal_selection?;
        let terminal = self.right.terminal.as_ref()?;
        let text = terminal.selection_text(&selection);
        (!text.is_empty()).then_some(text)
    }

    /// Put the selection on the clipboard; whether there was text to copy.
    pub(crate) fn terminal_copy(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.terminal_selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// The wheel over the pane: one cell per line of scroll, browsing the
    /// retained scrollback and returning towards the bottom on the way back.
    pub(crate) fn terminal_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let cell_h = self
            .terminal_frame
            .get()
            .map(|frame| frame.cell_h)
            .unwrap_or(px(17.0));
        let pixels = event.delta.pixel_delta(cell_h);
        let lines = (f32::from(pixels.y) / f32::from(cell_h)).round() as i32;
        if lines == 0 {
            return;
        }
        let moved = match self.right.terminal.as_mut() {
            Some(terminal) => terminal.scroll_view(lines),
            None => false,
        };
        if moved {
            self.sync_terminal(cx);
            cx.stop_propagation();
        }
    }

    /// What the pane says when there is no shell to draw.
    ///
    /// A refused start reports the policy's own copy, and a shell that failed
    /// or exited reports the runtime reason beside it. Both are errors: a
    /// terminal that cannot take a command is not a blank terminal.
    pub(crate) fn terminal_status_text(&self) -> Option<(String, bool)> {
        let terminal = self.right.terminal.as_ref()?;
        let lang = self.state.lang;
        if terminal.refused {
            let mut text = lang.tr("right.terminal.refused");
            if let Some(reason) = terminal.failure.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            // A refused start spawned nothing, so there is no shell whose
            // confinement could be described: the refusal is the whole line.
            return Some((text, true));
        }
        // A RUNNING shell carries no chrome: the pane's status line is for what
        // went wrong, and a standing warning band under a working shell would be
        // prose explaining state. The sandbox answer reaches the reader two
        // other ways instead: the shell's own BSH_SANDBOX, and this line while
        // the pane has something to report (below).
        let mut parts: Vec<String> = Vec::new();
        // A preset that turned the sandbox off is a standing security fact, not
        // a transient error: say it while the shell runs, not only after exit.
        if terminal.sandbox_off_by_preset {
            parts.push(lang.tr("right.terminal.sandboxOff"));
        }
        if let Some(error) = terminal.policy_error() {
            parts.push(format!(
                "{}: {error}",
                lang.tr("right.terminal.policyError")
            ));
        }
        if let Some(refusal) = terminal.last_refusal() {
            let mut text = format!(
                "{}: {}",
                lang.tr("right.terminal.inputRefused"),
                refusal.line
            );
            if let Some(reason) = refusal.reason.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            parts.push(text);
        }
        // The audit's compact face: a count, and only once something was
        // actually refused or redacted. No other chrome is added for a policy.
        let blocked = terminal.blocked_decisions();
        if blocked > 0 {
            parts.push(crate::i18n::tr_args(
                lang,
                "right.terminal.policyBlocked",
                &[("count", blocked.to_string())],
            ));
        }
        if !terminal.is_running() {
            let mut text = lang.tr("right.terminal.notRunning");
            if let Some(reason) = terminal.failure.as_ref() {
                text.push_str(": ");
                text.push_str(reason);
            }
            // A dead pane must say how to come back: Enter starts a new shell
            // (see Terminal::restart). Without this the pane looked hung after
            // `exit`, because nothing on screen named the way out.
            if !terminal.refused {
                text.push_str(" \u{b7} ");
                text.push_str(&lang.tr("right.terminal.restartHint"));
            }
            // A pane that is already speaking says what the spec answered too:
            // the refusal to confine, or the tier the shell actually ran under.
            // The preset-off sentence was already added above; it is not a
            // backend limitation and must not be dressed as one.
            if !terminal.sandbox_off_by_preset {
                if let Some(note) = terminal.sandbox_note() {
                    text.push_str(" \u{b7} ");
                    text.push_str(&format!(
                        "{}: {note}",
                        lang.tr("right.terminal.unsandboxed")
                    ));
                } else if let Some(label) = terminal.sandbox_label() {
                    text.push_str(" \u{b7} ");
                    text.push_str(&crate::i18n::tr_args(
                        lang,
                        "right.terminal.sandboxTier",
                        &[("tier", label.to_string())],
                    ));
                }
            }
            parts.insert(0, text);
        }
        if parts.is_empty() {
            return None;
        }
        Some((parts.join(" \u{b7} "), true))
    }

    /// The Start screen: the panel's headline over its three cards.
    ///
    /// Measured from the reference's own guide: a centred column with a 14px
    /// gap and 24px side padding, and an invisible 10% spacer after the last
    /// card that biases the whole stack upward instead of dead-centring it.
    fn right_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut stack = div()
            .id("right-start")
            .debug_selector(|| String::from("right-start"))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.0))
            .px(px(24.0));
        stack = stack.child(
            txt(
                self.state.lang.tr("right.start"),
                theme::SIDEBAR_SIZE,
                theme::muted(),
            )
            .mb(px(16.0)),
        );
        for tab in RightTab::ALL {
            stack = stack.child(self.right_start_card(tab, cx));
        }
        // The reference's `::after { flex: 0 10% }`: a spacer that takes no
        // content but takes space, so the visible stack sits above the optical
        // centre of the panel rather than dead-centred in it.
        stack.child(div().flex_none().h(relative(0.10)))
    }

    /// One Start card: glyph, title group, and the shortcut hint trailing.
    ///
    /// Measured: 380px wide (max-width 100%), min-height 56, 14px/20px
    /// padding, a 14px gap, a `.5px` border at the panel's own radius, and a
    /// 26px icon. The whole card is the target.
    fn right_start_card(&self, tab: RightTab, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        // A pane with no key binding draws no hint: the shortcut is a fact
        // about the shell, and printing one nothing dispatches would be the
        // exact "control that only looks wired" this crate refuses.
        let hint = match tab {
            RightTab::Files => Some("\u{2318}P"),
            RightTab::Diff => None,
            RightTab::Terminal => Some("\u{2325}`"),
            RightTab::Browser => Some("\u{2318}T"),
        };
        div()
            .id(SharedString::from(format!("right-card-{}", tab.key())))
            .debug_selector(|| format!("right-card-{}", tab.key()))
            .w(px(theme::RIGHT_CARD_W))
            .max_w(px(theme::RIGHT_CARD_W))
            .min_h(px(theme::RIGHT_CARD_H))
            .px(px(20.0))
            .py(px(14.0))
            .rounded(px(theme::RADIUS_LG))
            .border_1()
            .border_color(rgb(theme::border_soft()))
            .bg(rgb(theme::new_session_fill()))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(theme::hover())))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .child(
                div()
                    .w(px(26.0))
                    .h(px(26.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(tab.glyph(), 20.0, theme::muted())),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        txt(lang.tr(tab.key()), theme::SIDEBAR_SIZE, theme::text())
                            .line_height(px(20.0))
                            .truncate()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        txt(
                            lang.tr(match tab {
                                RightTab::Files => "right.files.hint",
                                RightTab::Diff => "right.diff.hint",
                                RightTab::Terminal => "right.terminal.hint",
                                RightTab::Browser => "right.browser.hint",
                            }),
                            theme::RIGHT_HINT_SIZE,
                            theme::faint(),
                        )
                        .line_height(px(15.0))
                        .truncate()
                        .flex_1()
                        .min_w_0(),
                    ),
            )
            .when_some(hint, |card, hint| {
                card.child(
                    txt(hint, theme::RIGHT_HINT_SIZE, theme::faint())
                        .font_family("monospace")
                        .flex_none(),
                )
            })
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.open_right_tab(tab, cx);
                // A terminal is a text surface: opening it puts the cursor
                // where the reader's next keystroke will land.
                if tab == RightTab::Terminal {
                    window.focus(&this.terminal_focus, cx);
                }
            }))
    }

    /// The Workspace Files pane: one full-width surface, the tree or the
    /// document, with only the chrome that surface needs.
    ///
    /// Measured from the reference's own dock: an open document takes the whole
    /// panel and the tree comes back through the Files chip. A document's
    /// surface is the editor and nothing else - the tab chip already names the
    /// file and cmd-s saves it, so a path bar and a save button over it would
    /// only repeat what the reader already has.
    fn files_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = div()
            .debug_selector(|| String::from("files-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        match self.state.workspace_root() {
            None => pane.child(self.files_no_workspace()),
            Some(root) => {
                pane.when(self.files.active_tab().is_none(), |pane| {
                    pane.child(self.files_header(&root, cx))
                })
                // The new-file row appears only when a double-click on the
                // tree's blank space asked for it (see `blank_clicked`).
                .when(
                    self.files.active_tab().is_none() && self.files.new_file_open,
                    |pane| pane.child(self.files_new_row(cx)),
                )
                // A rename is its own row rather than a tenant of the create
                // row: arming one from a tree row has to show the field whether
                // or not the create field happens to be open. It used to be
                // nested inside it, so the pencil set a flag nothing drew and a
                // rename could not be typed at all.
                .when(
                    self.files.active_tab().is_none() && self.files.renaming.is_some(),
                    |pane| pane.child(self.files_rename_row(cx)),
                )
                // The save-failure line stays on both surfaces: feedback is
                // not chrome, and it only renders when something went wrong.
                .when_some(self.files.error.clone(), |pane, error| {
                    pane.child(
                        div()
                            .debug_selector(|| String::from("files-error"))
                            .px(px(theme::RIGHT_PAD))
                            .pb(px(4.0))
                            .flex_none()
                            .child(
                                txt(error, theme::CHIP_SIZE, theme::error()).line_height(px(16.0)),
                            ),
                    )
                })
                .when_some(self.files.active_tab(), |pane, _open| {
                    pane.child(self.files_editor(&root, cx))
                })
                .when(self.files.active_tab().is_none(), |pane| {
                    pane.child(self.files_listing(cx))
                })
            }
        }
    }

    /// The pane with no workspace selected.
    fn files_no_workspace(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(theme::RIGHT_PAD * 2.0))
            .child(icon(IconName::FolderClosed, 22.0, theme::faint()))
            .child(
                txt(
                    self.state.lang.tr("right.files.noWorkspace"),
                    theme::CHIP_SIZE,
                    theme::faint(),
                )
                .text_align(TextAlign::Center)
                .line_height(px(18.0)),
            )
    }

    /// The 38px header bar, plus the rename row while one is in flight.
    ///
    /// The new-file field that used to sit here as a permanent strip is gone:
    /// the reader asks for it with a double-click on the tree's blank space
    /// (see [`Shell::blank_double_clicked`]), so the tree's own chrome is just
    /// the path and its tools.
    fn files_header(&self, root: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let path = root.display().to_string();
        // The header itself is the reference's measured 38px bar: path on the
        // left, tools on the right, a .5px rule under it. The new-file and
        // rename fields no longer ride here — the gesture that summons them
        // lives on the tree's blank space (see [`Shell::blank_double_clicked`]),
        // so this bar stays the measured 38px and nothing more.
        div()
            .debug_selector(|| String::from("files-header"))
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| String::from("files-path"))
                    .w_full()
                    .h(px(theme::FILES_HEADER_H))
                    .flex_none()
                    // Measured: padding 0 6px 0 16px, gap 4.
                    .pl(px(16.0))
                    .pr(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .border_b_1()
                    .border_color(rgb(theme::border_soft()))
                    .child(icon(IconName::FolderOpen, 13.0, theme::faint()).flex_none())
                    .child(
                        txt(path, theme::CHIP_SIZE, theme::muted())
                            .line_height(px(18.0))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap(),
                    )
                    .child(
                        div()
                            .id("files-refresh")
                            .debug_selector(|| String::from("files-refresh"))
                            // Measured tool: 28x28, radius-sm, 6px padding, 15px svg.
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::RefreshCw, 15.0, theme::muted()))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.refresh_files(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("files-hidden")
                            .debug_selector(|| String::from("files-hidden"))
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(theme::RADIUS_SM))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .when(self.files.hidden, |s| s.bg(rgb(theme::hover())))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(
                                IconName::Eye,
                                15.0,
                                if self.files.hidden {
                                    theme::text()
                                } else {
                                    theme::muted()
                                },
                            ))
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.files.hidden = !this.files.hidden;
                                this.refresh_files(cx);
                            })),
                    ),
            )
    }

    /// The new-file field, shown only after a double-click on the tree's blank
    /// space.
    ///
    /// The permanent manage strip is gone: creating a file is a gesture, not
    /// chrome that sits on screen. This row is that gesture made visible, and
    /// it carries the same rename field below it when a rename is in flight so
    /// both mutations share one place to type.
    fn files_new_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        div()
            .debug_selector(|| String::from("files-manage"))
            .w_full()
            .flex_none()
            .px(px(theme::RIGHT_PAD))
            .pt(px(6.0))
            .pb(px(2.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .debug_selector(|| String::from("files-new"))
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .debug_selector(|| String::from("files-new-input"))
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&self.files.new_name)
                                    .appearance(false)
                                    .h(px(28.0)),
                            ),
                    )
                    .child(
                        div()
                            .id("files-create")
                            .debug_selector(|| String::from("files-create"))
                            .h(px(28.0))
                            .px(px(10.0))
                            .rounded(px(theme::RADIUS_SM))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(icon(IconName::Plus, 13.0, theme::text()))
                            .child(
                                txt(lang.tr("right.files.new"), theme::CHIP_SIZE, theme::text())
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.create_file(window, cx);
                            })),
                    ),
            )
    }

    /// The rename row, which only exists while a row is being renamed.
    fn files_rename_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let target = self
            .files
            .renaming
            .clone()
            .unwrap_or_default()
            .display()
            .to_string();
        div()
            .debug_selector(|| String::from("files-rename"))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                txt(target, theme::CHIP_SIZE, theme::faint())
                    .flex_none()
                    .max_w(px(120.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap(),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.files.rename_name)
                        .appearance(false)
                        .h(px(28.0)),
                ),
            )
            .child(
                div()
                    .id("files-rename-confirm")
                    .debug_selector(|| String::from("files-rename-confirm"))
                    .h(px(28.0))
                    .px(px(10.0))
                    .rounded(px(theme::RADIUS_SM))
                    .border_1()
                    .border_color(rgb(theme::accent()))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(
                        txt(
                            lang.tr("right.files.renameConfirm"),
                            theme::CHIP_SIZE,
                            theme::text(),
                        )
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.rename_file(window, cx);
                    })),
            )
    }

    /// The directory tree: the workspace root's entries, each expanded branch
    /// nested under its parent at 18px a level.
    ///
    /// Measured from the reference's own FilesBody: nested `.level` lists with
    /// `.level .level { padding-left: 18px }`. The walk is depth-first over
    /// cached listings only, so a keystroke never touches the filesystem.
    fn files_listing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div()
            .id("files-list")
            .debug_selector(|| String::from("files-list"))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .w_full()
            .flex()
            .flex_col();
        // Measured body padding: 8px top, 8px left, 0 right.
        list = list.pt(px(8.0)).pl(px(8.0));
        if self.files.roots.is_empty() {
            list = list.child(div().pr(px(theme::RIGHT_PAD)).pb(px(8.0)).child(txt(
                self.state.lang.tr("right.files.empty"),
                theme::CHIP_SIZE,
                theme::faint(),
            )));
        }
        let mut stack: Vec<(&files::Row, PathBuf, usize)> = self
            .files
            .roots
            .iter()
            .map(|row| (row, PathBuf::new(), 0))
            .collect();
        // Reversed so a pop walks the entries in listing order.
        stack.reverse();
        while let Some((row, parent, depth)) = stack.pop() {
            let rel = parent.join(&row.name);
            list = list.child(self.files_tree_row(row, &rel, depth, cx));
            if row.dir {
                let expanded = self.files.expanded.iter().any(|p| p == &rel);
                if expanded {
                    if let Some(children) = self.files.children.get(&rel) {
                        for child in children.iter().rev() {
                            stack.push((child, rel.clone(), depth + 1));
                        }
                    }
                }
            }
        }
        // A double-click on the space below the last row asks for a new file
        // there. gpui exposes no on_double_click, and its ClickEvent's
        // click_count is unreliable for a synthesized press, so the pane reads
        // the gesture itself: two presses close together in time and space.
        // The blank div fills the pane's remaining height, so the gesture has
        // somewhere to land that is never a row.
        list.child(
            div()
                .id("files-blank")
                .debug_selector(|| String::from("files-blank"))
                .w_full()
                .flex_1()
                .min_h(px(48.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                    let gpui::ClickEvent::Mouse(mouse) = event else {
                        return;
                    };
                    let now = std::time::Instant::now();
                    let here = mouse.up.position;
                    let is_double = match this.files.last_blank_press {
                        Some((there, then)) => {
                            let near = (here - there).magnitude() < f64::from(SELF_DOUBLE_PX);
                            let soon = now.duration_since(then)
                                < std::time::Duration::from_millis(SELF_DOUBLE_MS);
                            near && soon
                        }
                        None => false,
                    };
                    // A double-click consumes the press it was built from, so
                    // the next click starts a fresh sequence rather than reading
                    // as yet another double-click. The same reset applies when
                    // a lone press closes the field, so the reader's next
                    // double-click isn't mistaken for a single.
                    let consumed = is_double || this.files.new_file_open;
                    this.files.last_blank_press = if consumed { None } else { Some((here, now)) };
                    if is_double {
                        this.blank_double_clicked(window, cx);
                    } else if this.files.new_file_open {
                        // A lone press on blank space retires the new-file
                        // field, so the tree stays quiet when the reader is
                        // just clearing the gesture away.
                        this.files.new_file_open = false;
                        cx.notify();
                    }
                })),
        )
    }

    /// One row of the tree, with the actions that appear on hover.
    ///
    /// Measured from the reference's own `.row`: `padding: 5px 10px`, a 6px
    /// gap, and a `radius-md` rounded hover fill. The height is left to the
    /// 13px/1.5 line box rather than pinned, so a bigger system font grows the
    /// row instead of clipping inside it. A directory carries a chevron that
    /// turns with its branch, and `depth` sets the 18px-per-level indent the
    /// reference nests its lists with.
    fn files_tree_row(
        &self,
        row: &files::Row,
        rel: &Path,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = row.name.clone();
        let dir = row.dir;
        let rel_click = rel.to_path_buf();
        let rel_diff = rel.to_path_buf();
        let rel_rename = rel.to_path_buf();
        let rel_delete = rel.to_path_buf();
        let rel_toggle = rel.to_path_buf();
        // Element ids and the hover group key on the whole path: two branches
        // can hold a file of the same name, and a duplicated id shares one
        // element state between them - the hover fill and both action groups
        // would light up together. The debug selector stays name-based, which
        // is what the click tests address.
        let rel_key = rel.to_string_lossy().to_string();
        let group = SharedString::from(format!("file-{rel_key}"));
        let row_id = SharedString::from(format!("file-row-{rel_key}"));
        // The debug selector stays name-based: it is a test address, not
        // element state, and the click tests name their row by file name.
        let debug_id = SharedString::from(format!("file-row-{name}"));
        let rename_id = SharedString::from(format!("rename-{rel_key}"));
        let delete_id = SharedString::from(format!("delete-{rel_key}"));
        let row_group = group.clone();
        let size = row.size;
        let deleting = self.files.deleting.as_deref() == Some(rel);
        let renaming = self.files.renaming.as_deref() == Some(rel);
        let expanded = self.files.expanded.iter().any(|p| p == rel);
        let glyph = file_glyph(row);
        let shell = cx.entity();
        div()
            .debug_selector(|| debug_id.to_string())
            .id(row_id)
            .group(group)
            .w_full()
            .min_h(px(0.0))
            .rounded(px(theme::RADIUS_SM))
            .pl(px(10.0 + 18.0 * depth as f32))
            .pr(px(10.0))
            .py(px(5.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .when(renaming, |row| row.bg(rgb(theme::selected_row())))
            .when(!renaming, |row| row.hover(|s| s.bg(rgb(theme::hover()))))
            .when(dir, |row| {
                row.child(
                    icon(
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        12.0,
                        theme::faint(),
                    )
                    .flex_none(),
                )
            })
            .when(!dir, |row| row.child(div().w(px(12.0)).flex_none()))
            .child(icon(glyph, 14.0, theme::faint()).flex_none())
            .child(
                txt(name.clone(), theme::SIDEBAR_SIZE, theme::text())
                    .truncate()
                    .flex_1(),
            )
            // The size is what a reader scans for, so it holds the row's right
            // edge until an action fades over it.
            .child(
                txt(size_label(size, dir), theme::CHIP_SIZE, theme::faint())
                    .flex_none()
                    .group_hover(row_group.clone(), |s| s.opacity(0.0)),
            )
            .child(
                div()
                    .absolute()
                    .right(px(theme::RIGHT_PAD))
                    .top(px(0.0))
                    .h(px(theme::RIGHT_ROW_H))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    // A directory has nothing to compare, so the action is a file's
                    // only. It is the pane's entry point: the Diff tab alone can
                    // never load anything, because a comparison needs a FILE.
                    .when(!dir, |strip| {
                        strip.child(
                            div()
                                .debug_selector(|| format!("diff-{}", row.name))
                                .id(SharedString::from(format!("diff-{}", row.name.clone())))
                                .px(px(4.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .opacity(0.0)
                                .group_hover(row_group.clone(), |s| s.opacity(1.0))
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .child(icon(IconName::GitCompareArrows, 12.0, theme::faint()))
                                .on_click({
                                    let shell = shell.clone();
                                    let rel = rel_diff.clone();
                                    move |_event, _window, cx| {
                                        cx.stop_propagation();
                                        shell.update(cx, |this, cx| {
                                            this.open_diff(&rel, cx);
                                        });
                                    }
                                }),
                        )
                    })
                    .child(
                        div()
                            .id(rename_id)
                            .debug_selector(|| format!("rename-{}", row.name))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(row_group.clone(), |s| s.opacity(1.0))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Pencil, 12.0, theme::faint()))
                            .on_click({
                                let shell = shell.clone();
                                let rel = rel_rename.clone();
                                move |_event, window, cx| {
                                    cx.stop_propagation();
                                    shell.update(cx, |this, cx| {
                                        this.begin_rename(&rel, window, cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .id(delete_id)
                            .debug_selector(|| format!("delete-{}", row.name))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(row_group.clone(), |s| s.opacity(1.0))
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(
                                IconName::Trash,
                                12.0,
                                if deleting {
                                    theme::error()
                                } else {
                                    theme::faint()
                                },
                            ))
                            .on_click({
                                let shell = shell.clone();
                                let rel = rel_delete.clone();
                                move |_event, _window, cx| {
                                    cx.stop_propagation();
                                    shell.update(cx, |this, cx| {
                                        this.press_delete(&rel, cx);
                                    });
                                }
                            }),
                    ),
            )
            .on_click(cx.listener(move |this, _event, window, cx| {
                if dir {
                    this.toggle_dir(&rel_toggle, cx);
                } else {
                    this.open_file(&rel_click, window, cx);
                }
            }))
    }

    /// One open file's chip, drawn in the panel's own tab strip.
    ///
    /// A chip carries the file's own name, the dirty dot when its buffer has
    /// changes, and the close that drops it. Pressing a chip focuses that tab
    /// rather than reopening the file, which is what makes the strip a strip
    /// and not a list of buttons.
    fn files_tab_chip(
        &self,
        index: usize,
        tab: &FileTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.files.active == Some(index);
        let name = tab.label();
        let chip_name = name.clone();
        let dirty = tab.dirty;
        let active_color = if active {
            theme::text()
        } else {
            theme::muted()
        };
        let weight = if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::default()
        };
        let close_name = name.clone();
        div()
            .debug_selector(move || format!("file-tab-{chip_name}"))
            .id(SharedString::from(format!("file-tab-{index}")))
            .h(px(26.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .when(active, |chip| chip.bg(rgb(theme::selected_row())))
            .hover(|s| s.bg(rgb(theme::hover())))
            .child(icon(file_glyph_for_name(&name), 12.0, theme::faint()).flex_none())
            .child(txt(name, theme::CHIP_SIZE, active_color).font_weight(weight))
            // A note glyph rides the chip the way the dirty dot does, so a
            // reader sees at a glance that the file carries remarks.
            .when(self.files.notes.has_notes(&tab.rel), |chip| {
                chip.child(icon(IconName::MessageSquare, 11.0, theme::muted()).flex_none())
            })
            // The dirty dot sits after the name and before the close, so it
            // reads as a property of the file rather than of the chip's edge.
            .when(dirty, |chip| {
                chip.child(
                    div()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(rgb(theme::warn()))
                        .flex_none(),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("file-tab-close-{index}")))
                    .debug_selector(move || format!("file-tab-close-{close_name}"))
                    .px(px(3.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(icon(IconName::X, 11.0, theme::faint()))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.close_tab(index, cx);
                    })),
            )
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.focus_tab(index, cx);
            }))
    }

    /// Raise the review card for the current selection.
    pub(crate) fn open_comment_card(&mut self, cx: &mut Context<Self>) {
        self.files.comment_open = true;
        self.files.notes_open = true;
        cx.notify();
    }

    /// Every pinned comment, drawn at the line it is anchored to.
    ///
    /// The reference interleaves these into the code so the text below shifts
    /// down; a kit `Editor` paints one block and offers no seam between two
    /// lines, so bsh puts each card at its anchor's own pixels instead. Same
    /// place, same caption, same menu - the code simply does not move.
    fn notes_inline(&self, rel: &Path, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(editor) = self.files.active_tab().and_then(|tab| tab.editor.clone()) else {
            return Vec::new();
        };
        // Every anchor is measured against the same buffer, so the buffer is
        // read ONCE: reading it inside the loop cloned the whole file per note
        // per frame. The read is also scoped, because `cx.listener` below needs
        // the context mutably.
        let tops: Vec<(usize, f32)> = {
            let state = editor.read(cx);
            let text = state.value();
            // The kit answers in window space; the cards are positioned inside
            // the editor, so its own box is what they are measured against. No
            // box yet means no first layout, and no card can be placed either.
            let editor_box = state.text_bounds();
            self.files
                .notes
                .on(rel)
                .iter()
                .enumerate()
                .filter_map(|(index, note)| {
                    let line = note.line?;
                    let start = crate::notes::line_start_offset(text.as_ref(), line);
                    let bounds = state.range_to_bounds(&(start..start))?;
                    let (_, y) = document_origin(bounds, editor_box?);
                    Some((index, y + f32::from(bounds.size.height) + 6.0))
                })
                .collect()
        };
        let mut out = Vec::with_capacity(tops.len());
        for (index, top) in tops {
            let note = &self.files.notes.on(rel)[index];
            let anchor = note.anchor();
            let body = note.text.clone();
            let del_rel = rel.to_path_buf();
            out.push(
                div()
                    .id(SharedString::from(format!("note-inline-{index}")))
                    .debug_selector(move || format!("note-inline-{index}"))
                    .absolute()
                    .top(px(top))
                    .left(px(theme::RIGHT_PAD + 24.0))
                    .right(px(16.0))
                    .max_w(px(520.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(theme::border_soft()))
                    .bg(rgb(theme::page()))
                    .shadow_lg()
                    .px(px(10.0))
                    .py(px(7.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(txt(body, 12.0, theme::text()).line_height(px(18.0)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(txt(anchor, 11.0, theme::faint()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(SharedString::from(format!("note-delete-{index}")))
                                    .debug_selector(move || format!("note-delete-{index}"))
                                    .w(px(20.0))
                                    .h(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::hover())))
                                    .child(icon(IconName::X, 11.0, theme::faint()))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.delete_note(&del_rel, index, cx);
                                    })),
                            ),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// The inline review card, shaped like the reference's: a titled box over the
    /// document holding one growing field, the anchor it will carry, and Cancel /
    /// Comment. Writing a note is a mode the card makes obvious; the strip's
    /// Comments control is its door.
    /// The little + that follows a line selection, and the press that opens the
    /// comment card: the reference puts the affordance where the reader's hand
    /// already is, instead of asking them to find a toolbar first.
    fn comment_plus(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.files.active_tab()?.editor.clone()?;
        let state = editor.read(cx);
        let range = state.selected_range();
        if range.start >= range.end {
            return None;
        }
        // The control rides a line of the selection that the viewport actually
        // holds, so it follows the marks through a scroll instead of vanishing
        // with them. `anchor_line` names that line; its own slice of the
        // selection is the box to centre on, and `document_point` converts that
        // box's window coordinates into the editor's.
        let text = state.value();
        let rows = state.visible_row_range()?;
        let line = anchor_line(text.as_ref(), &range, &rows)?;
        let start = crate::notes::line_start_offset(text.as_ref(), line);
        let line_end = text.as_ref()[start..]
            .find('\n')
            .map(|at| start + at)
            .unwrap_or(text.len());
        // The part of the selection on that line. A selection that stops at the
        // line break leaves an empty slice there, so the line's first character
        // becomes the anchor instead of a zero-width box.
        let from = range.start.max(start).min(line_end);
        let to = range.end.min(line_end).max(from + 1).min(text.len());
        let bounds = state
            .range_to_bounds(&(from..to))
            .or_else(|| state.range_to_bounds(&(start..(start + 1).min(text.len()))))?;
        let editor = state.text_bounds()?;
        let (left, top) = document_point(bounds, editor, 20.0);
        Some(
            div()
                .debug_selector(|| String::from("files-comment-plus"))
                .id("files-comment-plus")
                .absolute()
                .top(px(top))
                .left(px(left))
                .w(px(20.0))
                .h(px(20.0))
                .rounded_full()
                .bg(rgb(theme::accent()))
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .child(icon(IconName::Plus, 12.0, theme::page()))
                // The press routes: this is the control a reader actually presses,
                // and doing so is what surfaced the duplicate-field crash. The
                // harness cannot drive it - a synthetic press through the focused
                // editor lands on the editor - so the tests call the action it
                // fires and assert the control is on screen and on the selection.
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.open_comment_card(cx);
                }))
                .into_any_element(),
        )
    }
    fn comment_card(&self, rel: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let lines = self
            .files
            .active_tab()
            .and_then(|tab| tab.editor.clone())
            .map(|editor| {
                let state = editor.read(cx);
                let range = state.selected_range();
                let text = state.value();
                (
                    crate::notes::line_at_offset(text.as_ref(), range.start),
                    crate::notes::line_at_offset(text.as_ref(), range.end),
                )
            });
        let anchor = match lines {
            Some((from, end)) if end > from => lang
                .tr("right.files.commentLines")
                .replace("{from}", &from.to_string())
                .replace("{to}", &end.to_string()),
            Some((from, _)) => lang
                .tr("right.files.commentLine")
                .replace("{line}", &from.to_string()),
            None => lang.tr("right.files.commentFile"),
        };

        // The card hangs directly under the lines being commented, the way the
        // reference puts it: `range_to_bounds` gives the selection's own box, so
        // the card starts one gap below its bottom edge. The box is in window
        // space, so it is converted into the editor's before it is used as a
        // `top()` - the same conversion the + uses.
        let selection_bottom = self
            .files
            .active_tab()
            .and_then(|tab| tab.editor.clone())
            .and_then(|editor| {
                let state = editor.read(cx);
                let selection = state.range_to_bounds(&state.selected_range())?;
                let editor_box = state.text_bounds()?;
                // The BOTTOM edge, from the normalised corners: the kit's box
                // has a negative height when the range ends on a shorter line,
                // and adding that put the card above the lines it describes.
                let bottom = selection.origin.y.max(selection.bottom_right().y);
                Some(f32::from(bottom - editor_box.origin.y) + 8.0)
            });
        let send_rel = rel.to_path_buf();
        let can_send = !self.files.note_text.read(cx).value().trim().is_empty();
        div()
            .debug_selector(|| String::from("files-comment-card"))
            .absolute()
            // Over the document's text area, so it must take the pointer ahead of
            // the editor: without this the focused input ate every press and the
            // card's own buttons never fired.
            .occlude()
            // Beside the lines when the editor can say where they are, and at the
            // top otherwise (a tab with no editor, or before the first layout).
            .when_some(selection_bottom, |card, top| card.top(px(top)))
            .when(selection_bottom.is_none(), |card| card.top(px(20.0)))
            // Left and right bounds instead of a fixed width: a 460px card anchored
            // at the panel's left edge ran past the window, which put its buttons
            // off screen - the probe put Comment at x=1850 in a 1640 window.
            .left(px(theme::RIGHT_PAD + 24.0))
            .right(px(16.0))
            .flex()
            .items_start()
            .child(
                div()
                    .w_full()
                    .max_w(px(460.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(rgb(theme::border()))
                    .bg(rgb(theme::page()))
                    .shadow_lg()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        txt(lang.tr("right.files.commentTitle"), 13.0, theme::text())
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        div()
                            .debug_selector(|| String::from("files-comment-field"))
                            .h(px(76.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .rounded(px(theme::RADIUS_SM))
                            .border_1()
                            .border_color(rgb(theme::border()))
                            .child(Textarea::new(&self.files.note_text).appearance(false)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(txt(anchor, 12.0, theme::faint()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("files-comment-cancel")
                                    .debug_selector(|| String::from("files-comment-cancel"))
                                    .px(px(10.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(theme::RADIUS_SM))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::hover())))
                                    .child(txt(
                                        lang.tr("right.files.commentCancel"),
                                        12.0,
                                        theme::text(),
                                    ))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.files.comment_open = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("files-comment-add")
                                    .debug_selector(|| String::from("files-comment-add"))
                                    .px(px(12.0))
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(theme::RADIUS_SM))
                                    .cursor_pointer()
                                    .bg(rgb(if can_send {
                                        theme::accent()
                                    } else {
                                        theme::border()
                                    }))
                                    .child(txt(
                                        lang.tr("right.files.commentAction"),
                                        12.0,
                                        theme::page(),
                                    ))
                                    .on_click(cx.listener(move |this, _event, window, cx| {
                                        // Comment SENDS the review: the note is
                                        // pinned and the whole set goes to the model
                                        // as a prompt, queued when a turn is already
                                        // running - what the composer's own send
                                        // does, so nothing is sent twice or lost.
                                        this.pin_note(window, cx);
                                        this.send_review_to_chat(&send_rel, window, cx);
                                        this.send(window, cx);
                                        this.files.comment_open = false;
                                        this.files.notes_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }
    /// The drawer's toggle: a labelled control in the panel's tab strip.
    /// The notes drawer: what the reader pinned to the open file, plus the
    /// field that pins another.
    fn files_notes(&self, rel: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let notes = self.files.notes.on(rel);
        div()
            .debug_selector(|| String::from("files-notes"))
            .flex_none()
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .pt(px(6.0))
            .pb(px(6.0))
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .flex()
            .flex_col()
            .gap(px(6.0))
            // The pin field is always there, so adding a note is one click and
            // one keystroke away rather than a mode the reader has to enter -
            // except while the review card is up, because the card hosts the
            // SAME `note_text` entity. A frame that draws one entity twice
            // duplicates its accessibility node, which gpui-pre panics on in a
            // debug build: that was the crash a press on the + hit, since
            // raising the card also raises this drawer. One draft, one field.
            .when(!self.files.comment_open, |drawer| {
                drawer.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .debug_selector(|| String::from("files-note-field"))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    Textarea::new(&self.files.note_text)
                                        .appearance(false)
                                        .h(px(56.0)),
                                ),
                        )
                        .child(
                            div()
                                .id("files-note-add")
                                .debug_selector(|| String::from("files-note-add"))
                                .h(px(28.0))
                                .px(px(10.0))
                                .rounded(px(theme::RADIUS_SM))
                                .border_1()
                                .border_color(rgb(theme::border()))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(theme::hover())))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .child(icon(IconName::Plus, 13.0, theme::text()))
                                .child(
                                    txt(
                                        lang.tr("right.files.noteAdd"),
                                        theme::CHIP_SIZE,
                                        theme::text(),
                                    )
                                    .font_weight(FontWeight::MEDIUM),
                                )
                                .on_click(cx.listener(|this, _event, window, cx| {
                                    this.pin_note(window, cx);
                                })),
                        )
                        // The review goes to the model: the notes a reader wrote are a
                        // prompt they should not have to retype. Only offered when
                        // there is something to send.
                        .when(!notes.is_empty(), |row| {
                            let send_rel = rel.to_path_buf();
                            row.child(
                                div()
                                    .id("files-note-send")
                                    .debug_selector(|| String::from("files-note-send"))
                                    .h(px(28.0))
                                    .px(px(10.0))
                                    .rounded(px(theme::RADIUS_SM))
                                    .bg(rgb(theme::accent()))
                                    .cursor_pointer()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(icon(IconName::ArrowUp, 13.0, theme::page()))
                                    .child(
                                        txt(
                                            lang.tr("right.files.noteSend"),
                                            theme::CHIP_SIZE,
                                            theme::page(),
                                        )
                                        .font_weight(FontWeight::MEDIUM),
                                    )
                                    .on_click(cx.listener(move |this, _event, window, cx| {
                                        this.send_review_to_chat(&send_rel, window, cx);
                                    })),
                            )
                        }),
                )
            })
            .children(notes.iter().enumerate().map(|(index, note)| {
                let anchor = note.anchor();
                let text = note.text.clone();
                let del_rel = rel.to_path_buf();
                let del_anchor = anchor.clone();
                div()
                    .debug_selector(|| format!("files-note-{anchor}"))
                    .id(SharedString::from(format!("files-note-row-{index}")))
                    .w_full()
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(theme::RADIUS_SM))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .hover(|s| s.bg(rgb(theme::hover())))
                    .child(
                        txt(anchor, theme::CHIP_SIZE, theme::muted())
                            .flex_none()
                            .w(px(44.0)),
                    )
                    .child(
                        txt(text, theme::SIDEBAR_SIZE, theme::text())
                            .truncate()
                            .flex_1()
                            .min_w_0(),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("files-note-del-{index}")))
                            .debug_selector(move || format!("files-note-del-{del_anchor}"))
                            .px(px(4.0))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::hover())))
                            .child(icon(IconName::Trash, 12.0, theme::faint()))
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.delete_note(&del_rel, index, cx);
                            })),
                    )
            }))
    }

    /// The editor over the active tab: the file's own header, then the body.
    /// Absent when no tab is open, which leaves the tree the full width.
    ///
    /// The surface is the editor and nothing else - the tab chip already names
    /// the file, the dirty dot rides the chip, and cmd-s saves - so the bar
    /// that used to repeat the path and carry a save button is gone, and the
    /// action handler below is the whole save affordance.
    fn files_editor(&self, root: &Path, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.state.lang;
        let Some(open) = self.files.active_tab() else {
            return div()
                .debug_selector(|| String::from("files-editor"))
                .flex_1()
                .min_h_0()
                .w_full();
        };
        let rel = open.rel.clone();
        let save_rel = rel.clone();
        let shell = cx.entity();
        div()
            .debug_selector(|| String::from("files-editor"))
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            // cmd-s, dispatched from the focused editor, bubbles to here.
            .on_action(move |_event: &SaveFile, _window, cx| {
                shell.update(cx, |this, cx| {
                    this.save_file(&save_rel, cx);
                });
            })
            // The notes drawer, over the top of the document when open.
            .when(self.files.notes_open, |editor| {
                editor.child(self.files_notes(&rel, cx))
            })
            .child(match (&open.editor, open.kind) {
                (Some(editor), _) => div()
                    .debug_selector(|| String::from("files-editor-body"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .debug_selector(|| String::from("files-editor-frame"))
                            .id("files-editor-frame")
                            .size_full()
                            .flex_1()
                            .min_h_0()
                            .px(px(theme::RIGHT_PAD))
                            .py(px(6.0))
                            .border_1()
                            .border_color(rgb(theme::border_soft()))
                            // The kit editor draws its own bordered surface and
                            // sizes it to the content, so a SHORT file was a short
                            // box with the rest of the pane empty. Stretching it is
                            // what makes the pane read as a document.
                            .child(
                                div()
                                    .debug_selector(|| String::from("files-editor-fill"))
                                    // The frame around this is a plain block, so a
                                    // flex child here had nothing to grow in and
                                    // collapsed to the editor's own content height
                                    // (58px for a three-line file). A full height
                                    // is the fact, not a flex share.
                                    .h_full()
                                    .min_h_0()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    // The parent every document-anchored overlay is
                                    // measured against: the kit answers text boxes in
                                    // WINDOW space, so the overlay container has to be
                                    // the editor box for `document_point` subtraction
                                    // to be the whole conversion.
                                    .relative()
                                    .child(Editor::new(editor).h_full().w_full())
                                    // Pinned notes, the + that follows a selection and
                                    // the review card come LAST so they paint over the
                                    // document: added before the editor they were drawn
                                    // UNDER it, and the editor swallowed their presses.
                                    .children(self.notes_inline(&rel, cx))
                                    .children(self.comment_plus(cx))
                                    .when(self.files.comment_open, |fill| {
                                        fill.child(self.comment_card(&rel, cx))
                                    }),
                            ),
                    ),
                (None, Kind::Image) => {
                    let mut pane = div()
                        .debug_selector(|| String::from("files-image"))
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden();
                    match files::resolve(root, &rel) {
                        Some(path) => {
                            pane = pane.child(img(path).max_w(relative(1.0)).max_h(relative(1.0)));
                        }
                        // The tab's path must resolve inside the root; if it no
                        // longer does, say so rather than handing the image pane
                        // an empty path it silently draws nothing for.
                        None => {
                            pane = pane.child(
                                txt(
                                    lang.tr("right.files.outside"),
                                    theme::CHIP_SIZE,
                                    theme::faint(),
                                )
                                .debug_selector(|| String::from("files-image-outside")),
                            );
                        }
                    }
                    pane
                }
                (None, _) => div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(theme::RIGHT_PAD * 2.0))
                    .child(icon(IconName::Lock, 20.0, theme::faint()))
                    .child(
                        txt(
                            lang.tr("right.files.opaque"),
                            theme::CHIP_SIZE,
                            theme::faint(),
                        )
                        .text_align(TextAlign::Center)
                        .line_height(px(18.0)),
                    ),
            })
    }
}

impl Shell {
    /// Fold or unfold the panel, the way the header's left control folds the
    /// sidebar. A fold keeps the pane, so reopening lands where the reader was.
    pub(crate) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        self.right.open = !self.right.open;
        cx.notify();
    }

    /// Reveal or hide the seam's bar.
    ///
    /// The pointer moving onto the strip turns it on, and the column's own move
    /// turns it off again - the same pair the content column's handles use.
    pub(crate) fn hover_right_seam(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.right_hover != on {
            self.right_hover = on;
            cx.notify();
        }
    }

    /// The right panel column width for this frame, resolved from the reader's
    /// dragged preference against the space the row has left, the same way the
    /// sidebar's is.
    pub(crate) fn right_panel_w(&self) -> f32 {
        // The panel shares the row with the sidebar and the chat, so the space
        // it may claim is the window less the sidebar - not the window. The
        // reference's own `rightPanel: cols.right` and `sidebar: cols.sidebar`
        // sum to the same quantity: the share is a share of the row.
        let taken = if self.sidebar_folded() {
            0.0
        } else {
            self.sidebar_w()
        };
        let available = (self.viewport_w - taken).max(theme::RIGHT_PANEL_MIN);
        if self.right.maximized {
            // Every pixel the sidebar is not using: the maximised panel is the
            // reader saying "this is the thing I am working in", and the header
            // still carries the control that gives the space back.
            return available;
        }
        theme::right_panel_width_at(available, self.right.user_w)
    }

    /// Open a pane from the Start screen or the tab strip.
    ///
    /// Files is the pane that has to work now; the other two take the tab and
    /// say they are not built, which is what tells a reader pressing Terminal
    /// that they did press it.
    pub(crate) fn open_right_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        self.right.open = true;
        self.right.tab = Some(tab);
        if tab == RightTab::Files && self.files.roots.is_empty() && self.files.tabs.is_empty() {
            self.refresh_files(cx);
        }
        if tab == RightTab::Terminal && self.right.terminal.is_none() {
            let cwd = self.state.workspace_root().unwrap_or_default();
            // The SAME spec the shell tool runs under, derived from the live
            // policy and the effective permission. The pane cannot be confined
            // by it today - heel cannot host a caller-owned PTY - so the
            // terminal records that answer and the status line says it.
            let spec = self.state.sandbox_spec("terminal");
            let off_by_preset = self.state.effective().sandbox.off_by_preset;
            // The note is passed BEFORE the spawn: it overrides BSH_SANDBOX to
            // "none" for the shell itself, not only the status line.
            let note = off_by_preset.then(|| self.state.lang.tr("right.terminal.sandboxOff"));
            self.right.terminal = Some(match note {
                Some(note) => crate::terminal::Terminal::new_with_sandbox_note(
                    cwd,
                    self.state.lang,
                    spec.as_deref(),
                    Some(note),
                ),
                None => crate::terminal::Terminal::new_with_sandbox(
                    cwd,
                    self.state.lang,
                    spec.as_deref(),
                ),
            });
        }
        // A shell that already printed its first output would otherwise wait for
        // the next tick, so take the copy the spawn produced right away.
        self.sync_terminal(cx);
        cx.notify();
    }

    /// The policy a browser navigation asks.
    ///
    /// It is the SAME compiled set the turn's tools ask and the audit pane
    /// reads - deliberately not a fresh compile: a denial recorded here has to
    /// land in the ring the reader can see, and a second PolicySet would carry
    /// a second ring. A shell with no compiled policy yet (no turn has been
    /// priced) compiles one through the same constructor the turn uses, so the
    /// rules can still not differ; the only thing that is missing in that case
    /// is a ring to record into.
    fn browser_policy(&self) -> std::sync::Arc<agent::policy::PolicySet> {
        match self.state.policy.clone() {
            Some(policy) => policy,
            None => {
                let eff = self.state.effective();
                crate::caps::policy_for_effective(&self.state.settings, &eff)
            }
        }
    }

    /// The gate one navigation asks, named for the audit row it produces.
    fn browser_gate(&self, tool: &'static str) -> std::sync::Arc<agent::policy::EgressGate> {
        let eff = self.state.effective();
        std::sync::Arc::new(agent::policy::EgressGate::new(
            self.browser_policy(),
            std::sync::Arc::new(agent::policy::SystemResolver),
            self.state.active_id.clone().unwrap_or_default(),
            eff.preset_id,
            tool,
        ))
    }

    /// Navigate from the address bar: the one control a reader presses to open
    /// a page.
    pub(crate) fn browser_submit(&mut self, cx: &mut Context<Self>) {
        let typed = self.browser_url.read(cx).value().to_string();
        let url = normalize_address(&typed);
        if url.is_empty() {
            cx.notify();
            return;
        }
        self.browser_go(url, cx);
    }

    /// Start a navigation, showing the loading state on this frame.
    pub(crate) fn browser_go(&mut self, url: String, cx: &mut Context<Self>) {
        let gate = self.browser_gate("browser_open");
        let handle = self.state.browser.clone();
        // Begin here, not on the worker: the press has to have an answer before
        // the request exists, or the pane looks inert for a frame or two.
        let seq = handle.mutate(|browser| browser.begin(&url));
        self.browser_refresh_pane();
        cx.notify();
        // The GPUI background executor, not the runner's tokio runtime: this is
        // work the window started, and it is the executor the test scheduler
        // can drive deterministically.
        let task = cx.background_executor().spawn({
            let handle = handle.clone();
            let gate = gate.clone();
            let url = url.clone();
            async move { crate::browser::load(&handle, &gate, &url, seq) }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.browser_refresh_pane();
                // A refused or failed load already carries its sentence in the
                // pane; the notice ring would only repeat it.
                if let Err(failure) = &outcome {
                    if failure.kind == "scheme" {
                        this.state.notice(failure.tool_message());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Go back, forward or reload through the pane's own controls.
    ///
    /// Back and forward restore a cached document, so they ask no policy
    /// question and make no request; reload is a new navigation and goes
    /// through the same gate a fresh open does.
    pub(crate) fn browser_history(&mut self, step: isize, cx: &mut Context<Self>) {
        let outcome = self.state.browser.mutate(|browser| {
            if step < 0 {
                browser.back()
            } else {
                browser.forward()
            }
        });
        self.browser_refresh_pane();
        if let Err(message) = outcome {
            self.state.notice(message);
        }
        cx.notify();
    }

    /// Reload the page on screen.
    pub(crate) fn browser_reload(&mut self, cx: &mut Context<Self>) {
        let url = self
            .state
            .browser
            .read(|browser| browser.document().map(|doc| doc.url.clone()));
        match url {
            Some(url) => self.browser_go(url, cx),
            None => cx.notify(),
        }
    }

    /// Fill the address bar from the browser when a navigation moved it.
    ///
    /// Only a real change retriggers it: set_value always notifies, and a
    /// per-frame write would both repaint the field forever and fight a reader
    /// who is typing. The guard is the URL that was last written, not the
    /// field's focus.
    pub(crate) fn browser_sync_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.browser_pane.as_ref() else {
            return;
        };
        let url = view.url.clone();
        if url.is_empty() || url == self.browser_shown_url {
            return;
        }
        self.browser_shown_url.clone_from(&url);
        self.browser_url
            .update(cx, |state, cx| state.set_value(url, window, cx));
    }

    /// Rebuild the pane's cached view when the browser moved.
    ///
    /// Called from render, so it only costs an atomic read on a frame that did
    /// not change the browser; the clone of a page's text happens once per
    /// change, not once per frame.
    pub(crate) fn browser_refresh_pane(&mut self) {
        let revision = self.state.browser.revision();
        if self.browser_pane.is_some() && revision == self.browser_rev {
            return;
        }
        let view = self.state.browser.read(|browser| browser.pane_view());
        self.browser_pane = Some(view);
        self.browser_rev = revision;
    }

    /// Re-read the workspace policy when the file on disk changed.
    ///
    /// Rides the terminal clock, which is one stat per tick and no work at all
    /// when the stamp is unchanged: a policy edit takes effect without a
    /// restart, and a broken edit keeps the last good policy instead of
    /// wedging the pane.
    pub(crate) fn refresh_terminal_policy(&mut self, cx: &mut Context<Self>) {
        let refreshed = match self.right.terminal.as_mut() {
            Some(terminal) => terminal.refresh_policy(),
            None => false,
        };
        if refreshed {
            cx.notify();
        }
    }

    /// Apply the pane's measured size and copy the terminal's screen.
    ///
    /// Both jobs ride the terminal clock rather than the paint phase: the PTY
    /// resize and the screen copy are state changes, and the window must not be
    /// mutated while it is painting. The pane's prepaint stores the cell grid
    /// it measured, so this is where a panel resize reaches the shell.
    pub(crate) fn sync_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(frame) = self.terminal_frame.get() {
            if let Some(terminal) = self.right.terminal.as_mut() {
                terminal.resize(frame.cols, frame.rows);
            }
        }
        // A shell that exited is noticed here rather than only by the reader:
        // the pane keeps a slave description open for a sandboxed shell, so the
        // master does not EOF on its own. The child is the authority.
        if let Some(terminal) = self.right.terminal.as_mut() {
            terminal.poll();
        }
        let Some(terminal) = self.right.terminal.as_ref() else {
            return;
        };
        let version = terminal.version();
        if version == self.terminal_version {
            return;
        }
        self.terminal_version = version;
        self.terminal_snapshot = std::rc::Rc::new(terminal.snapshot());
        cx.notify();
    }

    /// The panel resize strip's press: the same origin/base dance the sidebar
    /// strip does, so both handles behave identically under the hand.
    pub(crate) fn begin_right_panel_drag(&mut self, pointer_x: f32, _cx: &mut Context<Self>) {
        self.right.drag = Some(SideDrag {
            origin_x: pointer_x,
            base: self.right.user_w.unwrap_or_else(|| self.right_panel_w()),
            moved: false,
        });
    }

    /// Track the strip's move, clamped to the panel's own contract range.
    pub(crate) fn drag_right_panel_width(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.right.drag.as_mut() else {
            return;
        };
        let delta = pointer_x - drag.origin_x;
        if delta.abs() > 2.0 {
            drag.moved = true;
        }
        // The strip sits on the panel's left edge, so dragging left widens it.
        if !drag.moved {
            return;
        }
        // Resizing by hand is the reader asking for a width, so it also gives up
        // the maximised column rather than fighting it.
        self.right.maximized = false;
        let width = (drag.base - delta)
            .round()
            .clamp(theme::RIGHT_PANEL_MIN, theme::RIGHT_PANEL_MAX);
        let width = Some(width);
        // The width is read into the column's layout this frame, so a move must
        // repaint: the sidebar's own drag notifies for exactly this reason, and
        // without it the panel only followed the pointer when something else
        // happened to schedule a frame.
        if self.right.user_w != width {
            self.right.user_w = width;
            cx.notify();
        }
    }

    /// Release the strip: the width stands, unless it was a click.
    pub(crate) fn commit_right_panel_width(&mut self, _pointer_x: f32, cx: &mut Context<Self>) {
        // A press that never moved is a click on the seam, not a resize: it must
        // leave the width the reader dragged out alone. It used to clear the
        // preference, so touching the handle threw the width away - the
        // sidebar's own seam has always kept it.
        self.right.drag = None;
        cx.notify();
    }

    /// Drop the file pane's view of the workspace it was showing.
    ///
    /// Every path in the pane is relative, so a switch changes what each one
    /// MEANS: an open tab is a buffer whose rel would now resolve into another
    /// workspace's file, and saving it would write there. So the tabs go, the
    /// cached tree goes with them, and the pane re-lists whatever workspace is
    /// active now.
    pub(crate) fn refile_workspace(&mut self, cx: &mut Context<Self>) {
        self.files.tabs.clear();
        self.files.active = None;
        self.files.expanded.clear();
        self.files.children.clear();
        self.files.roots.clear();
        self.files.error = None;
        self.files.renaming = None;
        self.files.deleting = None;
        self.files.new_file_open = false;
        self.files.notes_open = false;
        self.files.last_blank_press = None;
        if self.right.tab == Some(RightTab::Files) {
            // This also reloads the notes, which travel with the workspace.
            self.refresh_files(cx);
            return;
        }
        self.files.notes = crate::notes::Notes::default();
        cx.notify();
    }

    /// Re-list the workspace root and every directory the reader expanded.
    ///
    /// A tree with N branches costs N walks, but only on an explicit refresh or
    /// after a mutation, never on a keystroke - the listings between those
    /// points come from `children`.
    pub(crate) fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            self.files.roots.clear();
            self.files.children.clear();
            self.files.expanded.clear();
            self.files.notes = crate::notes::Notes::default();
            self.files.error = None;
            cx.notify();
            return;
        };
        // Notes travel with the workspace, so a root change reloads them; the
        // file reads are a single small JSON, and this runs on an explicit
        // refresh or a mutation, never on a frame.
        self.files.notes = crate::notes::Notes::load(&root);
        match files::list(&root, self.files.hidden) {
            Ok(rows) => {
                self.files.roots = rows;
                self.files.error = None;
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        // Re-walk exactly the branches that are open, leaving the rest alone:
        // a collapsed branch's listing is stale but invisible, and paying for
        // it would be work nobody can see.
        let expanded = self.files.expanded.clone();
        for dir in expanded {
            match files::list(&root.join(&dir), self.files.hidden) {
                Ok(rows) => {
                    self.files.children.insert(dir, rows);
                }
                Err(error) => self.files.error = Some(io_msg(&error)),
            }
        }
        cx.notify();
    }

    /// Re-list the directories a mutation touched, and nothing else.
    fn refresh_branch(&mut self, dir: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        match files::list(&root.join(dir), self.files.hidden) {
            Ok(rows) => {
                self.files.children.insert(dir.to_path_buf(), rows);
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        // The root listing is what shows a file created at the top level, and
        // it is one small walk, so refresh it with the branch.
        match files::list(&root, self.files.hidden) {
            Ok(rows) => self.files.roots = rows,
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        cx.notify();
    }

    /// Expand a directory, listing it on first open, or collapse it.
    ///
    /// Collapsing keeps the cached listing: reopening a branch the reader just
    /// looked at should not cost a second walk.
    pub(crate) fn toggle_dir(&mut self, rel: &Path, cx: &mut Context<Self>) {
        if let Some(index) = self.files.expanded.iter().position(|p| p == rel) {
            self.files.expanded.remove(index);
            cx.notify();
            return;
        }
        let already_listed = self.files.children.contains_key(rel);
        if !already_listed {
            let Some(root) = self.state.workspace_root() else {
                return;
            };
            match files::list(&root.join(rel), self.files.hidden) {
                Ok(rows) => {
                    self.files.children.insert(rel.to_path_buf(), rows);
                }
                Err(error) => {
                    self.files.error = Some(io_msg(&error));
                    cx.notify();
                    return;
                }
            }
        }
        self.files.expanded.push(rel.to_path_buf());
        cx.notify();
    }

    /// Open a file in the editor area as a tab.
    ///
    /// A file already open is focused rather than reopened, so the strip never
    /// carries the same file twice and a buffer with unsaved changes is never
    /// thrown away by clicking its row again. The read is bounded and the
    /// format decides the body: a text file lands in a highlighted editor, an
    /// image lands in an image pane, and anything else says it cannot be shown
    /// rather than dropping bytes into a buffer.
    /// Send a file's review notes to the composer as a prompt.
    ///
    /// A note is already anchored to a line, so the prompt says which line and
    /// what the reader wants changed; the reader edits it before sending rather
    /// than the app sending on its own.
    pub(crate) fn send_review_to_chat(
        &mut self,
        rel: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let notes = self.files.notes.on(rel);
        if notes.is_empty() {
            return;
        }
        let mut prompt = format!("Review of {}:\n", rel.display());
        for note in notes {
            // `line` is already one-based (it comes from the editor's own
            // numbering), so it is printed as-is; a range prints as a range.
            match (note.line, note.to) {
                (Some(from), Some(to)) if to > from => {
                    prompt.push_str(&format!("- lines {from}-{to}: {}\n", note.text));
                }
                (Some(line), _) => {
                    prompt.push_str(&format!("- line {line}: {}\n", note.text));
                }
                (None, _) => {
                    prompt.push_str(&format!("- {}\n", note.text));
                }
            }
        }
        prompt.push('\n');
        self.composer
            .update(cx, |state, cx| state.set_value(prompt.clone(), window, cx));
        self.composer_value = prompt;
        self.files.notes_open = false;
        self.files.comment_open = false;
        cx.notify();
    }
    pub(crate) fn open_file(&mut self, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.files.tab_index(rel) {
            self.files.active = Some(index);
            // Reopening a tab shows its remarks if it has any; a file with
            // none keeps the surface clean until the reader opens the drawer.
            self.files.notes_open = self.files.notes.has_notes(rel);
            cx.notify();
            return;
        }
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(path) = files::resolve(&root, rel) else {
            self.files.error = Some(self.state.lang.tr("right.files.outside"));
            cx.notify();
            return;
        };
        // The name decides the language and the size decides the bound, so both
        // come from the resolved path rather than from the label on the row.
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = path.metadata().map(|meta| meta.len()).unwrap_or(0);
        let kind = files::kind_for(&name, size);
        let mut tab = FileTab {
            rel: rel.to_path_buf(),
            editor: None,
            kind,
            dirty: false,
        };
        if !matches!(kind, Kind::Text(_)) {
            self.files.tabs.push(tab);
            self.files.active = Some(self.files.tabs.len() - 1);
            self.files.notes_open = self.files.notes.has_notes(rel);
            self.files.error = None;
            cx.notify();
            return;
        }
        match files::read(&root, rel) {
            Ok(bytes) => {
                // A file with a text extension that is not really text falls
                // back to the opaque body, so the buffer never holds bytes the
                // editor would render as control characters.
                if !files::is_text(&bytes) {
                    tab.kind = Kind::Opaque;
                    self.files.tabs.push(tab);
                    self.files.active = Some(self.files.tabs.len() - 1);
                    self.files.notes_open = self.files.notes.has_notes(rel);
                    cx.notify();
                    return;
                }
                let Ok(text) = String::from_utf8(bytes) else {
                    tab.kind = Kind::Opaque;
                    self.files.tabs.push(tab);
                    self.files.active = Some(self.files.tabs.len() - 1);
                    self.files.notes_open = self.files.notes.has_notes(rel);
                    cx.notify();
                    return;
                };
                let language = files::language_for(&name);
                let editor = cx.new(|cx| {
                    let mut state = EditorState::new(window, cx).searchable(true);
                    if let Some(language) = language {
                        state = state.language(language);
                    }
                    state.set_value(text, window, cx);
                    state
                });
                let editor_id = editor.clone();
                let open_rel = rel.to_path_buf();
                cx.subscribe_in(&editor, window, move |this, _editor, event, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        // The dirty mark belongs to the tab holding this exact
                        // editor, so a second tab over another file is never
                        // marked by the first one's keystrokes.
                        if let Some(tab) = this.files.tabs.iter_mut().find(|tab| {
                            tab.rel == open_rel && tab.editor.as_ref() == Some(&editor_id)
                        }) {
                            if !tab.dirty {
                                tab.dirty = true;
                                cx.notify();
                            }
                        }
                    }
                })
                .detach();
                tab.editor = Some(editor);
                self.files.tabs.push(tab);
                self.files.active = Some(self.files.tabs.len() - 1);
                self.files.notes_open = self.files.notes.has_notes(rel);
                self.files.error = None;
                cx.notify();
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Focus a tab by index, which is what pressing its chip does.
    pub(crate) fn focus_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.files.tabs.len() {
            self.files.active = Some(index);
            cx.notify();
        }
    }

    /// Show the tree again, which is what pressing the Files chip does.
    pub(crate) fn focus_tree(&mut self, cx: &mut Context<Self>) {
        self.files.active = None;
        cx.notify();
    }

    /// Close a tab and leave the neighbour that was beside it active.
    ///
    /// Closing the last one empties the editor area, so the tree takes the
    /// panel back over rather than leaving a dead header behind.
    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.files.tabs.len() {
            return;
        }
        self.files.tabs.remove(index);
        match self.files.active {
            // Closing the tab on screen hands the panel back to the tree,
            // which is the surface the Files chip names.
            Some(open) if open == index => self.files.active = None,
            Some(open) if open > index => self.files.active = Some(open - 1),
            _ => {}
        }
        cx.notify();
    }

    /// Write the active tab's buffer back to the file it came from.
    pub(crate) fn save_file(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(index) = self.files.tab_index(rel) else {
            return;
        };
        let Some(editor) = self.files.tabs[index].editor.clone() else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        match files::write(&root, rel, &text) {
            Ok(()) => {
                // Only the saved tab clears, so a second tab over another file
                // keeps its own unsaved marker.
                self.files.tabs[index].dirty = false;
                self.files.error = None;
            }
            Err(error) => self.files.error = Some(io_msg(&error)),
        }
        cx.notify();
    }

    /// Pin a note to the document the panel is showing.
    ///
    /// The note anchors to the editor's own cursor line, so a reader can place
    /// a remark exactly where they are looking; a tab with no editor (an
    /// image, or a file that will not open) takes a whole-file note instead.
    pub(crate) fn pin_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .files
            .note_text
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        let Some(rel) = self.files.active_tab().map(|tab| tab.rel.clone()) else {
            return;
        };
        // The SELECTION is the anchor: a note about three lines says three lines,
        // the way the reference's card reports them.
        let (line, to) = self
            .files
            .active_tab()
            .and_then(|tab| tab.editor.clone())
            .map(|editor| {
                let state = editor.read(cx);
                let range = state.selected_range();
                let text = state.value();
                let from = crate::notes::line_at_offset(text.as_ref(), range.start);
                let end = crate::notes::line_at_offset(text.as_ref(), range.end);
                (Some(from), (end > from).then_some(end))
            })
            .unwrap_or((None, None));
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        self.files
            .notes
            .add(&rel, crate::notes::Note { line, to, text });
        if let Err(error) = self.files.notes.save(&root) {
            self.files.error = Some(io_msg(&error));
        }
        self.files.note_text.update(cx, |state, cx| {
            state.set_value(String::new(), window, cx);
        });
        self.files.notes_open = true;
        cx.notify();
    }

    /// Remove one note from the document, by its index in the drawer.
    pub(crate) fn delete_note(&mut self, rel: &Path, index: usize, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        self.files.notes.remove(rel, index);
        if let Err(error) = self.files.notes.save(&root) {
            self.files.error = Some(io_msg(&error));
        }
        cx.notify();
    }

    /// Create the file named in the New file field.
    ///
    /// A blank name is a no-op rather than an error, because the field is
    /// always on screen and a reader tabbing past it should not be told off.
    pub(crate) fn create_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let name = self
            .files
            .new_name
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if name.is_empty() {
            return;
        }
        match files::create(&root, Path::new(&name)) {
            Ok(_path) => {
                self.files.new_name.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
                // The file exists, so the field that asked for it retires.
                self.files.new_file_open = false;
                self.refresh_branch(Path::new(""), cx);
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Open the new-file field after a double-click on the tree's blank space.
    ///
    /// There is no permanent create row in the reference, and a modal dialog
    /// would block a reader who only meant to clear the selection. So the
    /// gesture simply reveals the inline field and hands it the keyboard: the
    /// reader types a name and presses the button (or Enter) to make the file,
    /// and a single click elsewhere dismisses it.
    pub(crate) fn blank_double_clicked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.renaming = None;
        self.files.new_file_open = true;
        let focus = self.files.new_name.clone();
        focus.update(cx, |state, cx| {
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// Arm a row's delete: the first press marks it, the second deletes.
    pub(crate) fn press_delete(&mut self, rel: &Path, cx: &mut Context<Self>) {
        if self.files.deleting.as_deref() == Some(rel) {
            self.delete_file(rel, cx);
            return;
        }
        self.files.deleting = Some(rel.to_path_buf());
        cx.notify();
    }

    /// Delete the row, closing any tab showing it so no buffer points at a
    /// file that is gone.
    pub(crate) fn delete_file(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        match files::delete(&root, rel) {
            Ok(()) => {
                self.files.deleting = None;
                // The remarks described a file that is gone, so they go with it
                // rather than resurfacing if a second file takes the name.
                self.files.notes.forget(rel);
                if let Err(error) = self.files.notes.save(&root) {
                    self.files.error = Some(io_msg(&error));
                }
                // Walk backwards so an index the removal shifts never skips a
                // tab over the same file.
                let mut index = self.files.tabs.len();
                while index > 0 {
                    index -= 1;
                    if self.files.tabs[index].rel == rel {
                        self.close_tab(index, cx);
                    }
                }
                let parent = rel.parent().unwrap_or(Path::new("")).to_path_buf();
                self.refresh_branch(&parent, cx);
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// Put a row into the rename field, prefilled with the name it has.
    pub(crate) fn begin_rename(&mut self, rel: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.files.renaming = Some(rel.to_path_buf());
        let name = rel
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        self.files.rename_name.update(cx, |state, cx| {
            state.set_value(name, window, cx);
            // The reader pressed the pencil, so they mean to type: focus the
            // field and select nothing, the way the create field opens.
            state.focus_handle(cx).focus(window, cx);
        });
        self.files.deleting = None;
        cx.notify();
    }

    /// Apply the rename, reporting a refusal rather than a silent no-op.
    pub(crate) fn rename_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let Some(from_rel) = self.files.renaming.clone() else {
            return;
        };
        let to = self
            .files
            .rename_name
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if to.is_empty() || Some(to.as_str()) == from_rel.file_name().and_then(|n| n.to_str()) {
            self.files.renaming = None;
            self.files.rename_name.update(cx, |state, cx| {
                state.set_value(String::new(), window, cx);
            });
            cx.notify();
            return;
        }
        let to_rel = from_rel.with_file_name(&to);
        match files::rename(&root, &from_rel, &to_rel) {
            Ok(_path) => {
                self.files.renaming = None;
                self.files.rename_name.update(cx, |state, cx| {
                    state.set_value(String::new(), window, cx);
                });
                // An open tab over the old name follows it, so the reader does
                // not lose their buffer to a rename they made in the tree.
                if let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.rel == from_rel) {
                    tab.rel = to_rel.clone();
                }
                // Notes are keyed by the path, so they move with the file.
                self.files.notes.rename(&from_rel, &to_rel);
                if let Err(error) = self.files.notes.save(&root) {
                    self.files.error = Some(io_msg(&error));
                }
                self.refresh_branch(from_rel.parent().unwrap_or(Path::new("")), cx);
                if to_rel.parent() != from_rel.parent() {
                    self.refresh_branch(to_rel.parent().unwrap_or(Path::new("")), cx);
                }
            }
            Err(error) => {
                self.files.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }
}

/// The glyph a file is drawn with from its name alone, which is what a tab
/// chip needs: a tab carries a label, not a listing row.
fn file_glyph_for_name(name: &str) -> IconName {
    if files::image_extension(name) {
        return IconName::Image;
    }
    if name.ends_with(".md") || name.ends_with(".markdown") {
        return IconName::BookOpen;
    }
    if name.ends_with(".lock") {
        return IconName::Lock;
    }
    match files::language_for(name) {
        Some(_) => IconName::FileCode,
        None => IconName::FileText,
    }
}

/// The glyph a row is drawn with, in the reference's own file-type language.
fn file_glyph(row: &files::Row) -> IconName {
    if row.dir {
        return IconName::Folder;
    }
    if files::image_extension(&row.name) {
        return IconName::Image;
    }
    let name = row.name.as_str();
    if name.ends_with(".md") || name.ends_with(".markdown") {
        return IconName::BookOpen;
    }
    if name.ends_with(".lock") {
        return IconName::Lock;
    }
    match files::language_for(name) {
        Some(_) => IconName::FileCode,
        None => IconName::FileText,
    }
}

/// The size a row shows: a directory has none, and a file's bytes are grouped
/// the way a file manager groups them.
fn size_label(size: u64, dir: bool) -> String {
    if dir {
        return String::new();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{size} {}", UNITS[unit]);
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// The comparison surface's own chrome and controls, kept beside the pane they
/// belong to rather than in the panel's general body.
impl Shell {
    /// Open a workspace file as a comparison.
    ///
    /// The pane is reached by OPENING A FILE, not by a separate "pick a file"
    /// flow: a `.diff`/`.patch` opens as the patch it is, a conflicted file
    /// opens as the merge it is, and any other file opens against its committed
    /// text so the pane never shows a blank surface for a file the reader asked
    /// about.
    ///
    /// The READ happens on the runner's runtime, not here: a large file plus the
    /// two git subprocesses behind a real comparison are exactly the work that
    /// would stall the window on a click. The pane is focused first so the press
    /// has an immediate answer, then the result lands.
    pub(crate) fn open_diff(&mut self, rel: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.state.workspace_root() else {
            self.diff_refused("right.files.noWorkspace", cx);
            return;
        };
        if files::resolve(&root, rel).is_none() {
            self.diff_refused("right.files.outside", cx);
            return;
        }
        // Answer the click now: the pane is where the result is going.
        self.right.tab = Some(RightTab::Diff);
        self.right.open = true;
        self.right.diff.error = None;
        cx.notify();
        let rel = rel.to_path_buf();
        // The GPUI background executor, not the runner's tokio runtime: this is
        // work the window started, and this executor is the one the test
        // scheduler can drive deterministically. A tokio runtime runs on threads
        // the harness refuses to see activity from.
        let loading = cx.background_executor().spawn({
            let root = root.clone();
            let rel = rel.clone();
            async move { diff_pane::read_for(&root, &rel) }
        });
        let label = rel.to_string_lossy().to_string();
        cx.spawn(async move |this, cx| {
            let loaded = loading.await;
            let _ = this.update(cx, |this, cx| match loaded {
                Ok(loaded) => match diff_pane::classify(
                    &label,
                    &loaded.bytes,
                    loaded.git.as_deref(),
                    loaded.untracked,
                ) {
                    Ok((parsed, source)) => {
                        this.present_comparison(&rel, parsed, source, cx);
                    }
                    Err(key) => this.diff_refused(key, cx),
                },
                Err(diff_pane::LoadFailure::Key(key)) => this.diff_refused(key, cx),
                Err(diff_pane::LoadFailure::Io(error)) => {
                    this.diff_refused_text(io_msg(&error), cx);
                }
            });
        })
        .detach();
    }

    /// Put a refusal on the comparison pane and focus it.
    fn diff_refused(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let text = self.state.lang.tr(key);
        self.diff_refused_text(text, cx);
    }

    /// The same refusal when the message is already built.
    fn diff_refused_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.right.diff.error = Some(text);
        // A refusal replaces whatever the pane was showing, turn list included.
        self.right.diff.turn = None;
        self.right.tab = Some(RightTab::Diff);
        self.right.open = true;
        cx.notify();
    }

    /// Highlight and show a comparison parsed for a workspace file.
    ///
    /// The language table is the editor's own, so a patch of a `.tsx` is
    /// highlighted as one and the two surfaces cannot disagree.
    fn present_comparison(
        &mut self,
        rel: &Path,
        parsed: Vec<DiffFile>,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        let mut parsed = parsed;
        if let Some(language) = diff_pane::language_for(rel) {
            parsed = parsed
                .into_iter()
                .map(|file| file.with_language(language))
                .collect();
        }
        self.show_comparison(rel.to_path_buf(), parsed, source, cx);
    }

    /// Open a comparison that was built rather than read from a file.
    ///
    /// This is the CHAT side's entry point: a step that wrote a file knows the
    /// change it asked for, and `diff_pane::patch_for` turns that into a patch
    /// the kit parses. The pane is the same one a `.patch` file opens, so a
    /// request and a file are read the same way. The source is always
    /// read-only here, because a request is not a document that can be saved
    /// back over itself.
    pub(crate) fn open_diff_text(&mut self, label: &str, patch: &str, cx: &mut Context<Self>) {
        let files = match DiffFile::parse(patch) {
            Ok(files) if !files.is_empty() => files,
            _ => {
                self.right.diff.error = Some(self.state.lang.tr("diff.error"));
                self.right.diff.turn = None;
                self.right.tab = Some(RightTab::Diff);
                self.right.open = true;
                cx.notify();
                return;
            }
        };
        let path = PathBuf::from(label);
        let files = match diff_pane::language_for(&path) {
            Some(language) => files
                .into_iter()
                .map(|file| file.with_language(language))
                .collect(),
            None => files,
        };
        self.show_comparison(path, files, DiffSource::Patch, cx);
    }

    /// Put a parsed comparison on screen and focus the pane.
    ///
    /// Every arrival - a file, a conflict, a request - ends here, so the tab,
    /// the resolution subscription and the conflict bookkeeping cannot drift
    /// between the paths that open one.
    fn show_comparison(
        &mut self,
        path: PathBuf,
        parsed: Vec<DiffFile>,
        source: DiffSource,
        cx: &mut Context<Self>,
    ) {
        let conflicts: usize = parsed.iter().map(|file| file.conflicts().len()).sum();
        let mode = diff_pane::mode_of(&self.state.settings.global.general.diff_mode);
        let files = parsed.clone();
        let state = cx.new(|cx| {
            let mut state = DiffState::new(files, cx);
            state.set_mode(mode, cx);
            state
        });
        // The pane has to know when a conflict is resolved, because the save
        // control appears only once nothing is left unresolved. Reading it off
        // the state is what keeps a resolution made by the kit's own commands
        // indistinguishable from one made anywhere else.
        cx.subscribe(&state, |this, state, event: &DiffEvent, cx| {
            if matches!(event, DiffEvent::ConflictResolved(..)) {
                let state = state.read(cx);
                let outstanding: usize = state
                    .files()
                    .iter()
                    .map(|document| {
                        (0..document.conflicts().len())
                            .filter(|ix| state.conflict_resolution(document.path(), *ix).is_none())
                            .count()
                    })
                    .sum();
                this.right.diff.resolved = outstanding == 0;
                cx.notify();
            }
        })
        .detach();
        self.right.diff.state = Some(state);
        self.right.diff.path = Some(path);
        self.right.diff.source = Some(source);
        self.right.diff.error = None;
        // A file comparison and a turn list are two subjects for one surface;
        // opening a file takes the surface back.
        self.right.diff.turn = None;
        self.right.diff.conflicts = conflicts;
        // Nothing is resolved the moment a conflicted file opens; the flag is
        // recomputed on every resolution rather than assumed.
        self.right.diff.resolved = conflicts == 0;
        self.right.tab = Some(RightTab::Diff);
        self.right.open = true;
        cx.notify();
    }

    /// Set the comparison's view mode, remembering it for the next file.
    ///
    /// The mode is a preference, not a per-file fact: the reference keeps one
    /// choice for the reader, so opening the next patch shows the shape they
    /// last asked for.
    pub(crate) fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        if let Some(state) = self.right.diff.state.clone() {
            state.update(cx, |state, cx| state.set_mode(mode, cx));
        }
        self.right.diff.mode = mode;
        let key = diff_pane::mode_key(mode);
        if self.state.settings.global.general.diff_mode != key {
            self.state.settings.global.general.diff_mode = key.to_string();
            self.save_settings(cx);
        }
        cx.notify();
    }

    /// Flip between the two views, which is what the strip's control does.
    pub(crate) fn toggle_diff_mode(&mut self, cx: &mut Context<Self>) {
        let mode = match self.right.diff.mode {
            DiffMode::Split => DiffMode::Unified,
            _ => DiffMode::Split,
        };
        self.set_diff_mode(mode, cx);
    }

    /// Whether long lines wrap, remembering it like the mode.
    pub(crate) fn toggle_diff_wrap(&mut self, cx: &mut Context<Self>) {
        let wrap = !self.state.settings.global.general.diff_wrap;
        self.state.settings.global.general.diff_wrap = wrap;
        self.save_settings(cx);
        cx.notify();
    }

    /// Write the merged text back over the conflicted file.
    ///
    /// Every conflict must be resolved first: a partially merged file is not a
    /// merge, and writing one would leave markers on disk with the pane saying
    /// it was done.
    pub(crate) fn save_merged(&mut self, cx: &mut Context<Self>) {
        let (Some(state), Some(rel)) =
            (self.right.diff.state.clone(), self.right.diff.path.clone())
        else {
            return;
        };
        let Some(root) = self.state.workspace_root() else {
            return;
        };
        let label = rel.to_string_lossy().to_string();
        let Some(text) = state.read(cx).resolved_text(&label) else {
            self.right.diff.error = Some(self.state.lang.tr("diff.unresolved"));
            cx.notify();
            return;
        };
        match files::write(&root, &rel, &text) {
            Ok(()) => {
                // The editor may hold the same file with the markers still in
                // its buffer, so the tab is replaced rather than left stale.
                self.files.tabs.retain(|tab| tab.rel != rel);
                self.right.diff.resolved = false;
                self.right.diff.conflicts = 0;
                self.right.diff.source = None;
                self.right.diff.error = None;
                // Re-read it: the merged file has no conflicts left, so it comes
                // back as its own single column, which is the proof it landed.
                self.open_diff(&rel, cx);
            }
            Err(error) => {
                self.right.diff.error = Some(io_msg(&error));
                cx.notify();
            }
        }
    }

    /// The comparison surface.
    pub(crate) fn diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        // A turn-focused list is the pane's other subject. It is checked first
        // because the two are mutually exclusive and a turn has no kit
        // `DiffState` behind it - the list is built from the tool records.
        if self.right.diff.turn.is_some() {
            return self.turn_diff_pane(cx);
        }
        let lang = self.state.lang;
        let pane = div()
            .debug_selector(|| String::from("diff-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden();
        let Some(state) = self.right.diff.state.clone() else {
            // No comparison yet: name the three ways one arrives rather than
            // drawing an empty box.
            return pane
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .px(px(theme::RIGHT_PAD))
                        .child(icon(IconName::GitCompareArrows, 22.0, theme::faint()))
                        .child(txt(lang.tr("diff.empty"), theme::CHIP_SIZE, theme::text()))
                        // Name the two ways a comparison arrives. A pane that
                        // only says it is empty is the reason this surface read
                        // as unreachable: the reader had no way to know the tab
                        // needs a FILE, not a setting.
                        .child(
                            div()
                                .debug_selector(|| String::from("diff-empty-hint"))
                                .px(px(20.0))
                                .child(
                                    txt(
                                        lang.tr("diff.emptyHint"),
                                        theme::CHIP_SIZE,
                                        theme::faint(),
                                    )
                                    .line_height(px(17.0)),
                                ),
                        ),
                )
                .children(self.right.diff.error.clone().map(|error| {
                    txt(error, theme::CHIP_SIZE, theme::error())
                        .debug_selector(|| String::from("diff-error"))
                }))
                .into_any_element();
        };
        let wrap = self.state.settings.global.general.diff_wrap;
        let mode = self.right.diff.mode;
        let name = self
            .right
            .diff
            .path
            .as_ref()
            .map(|path| diff_pane::display_name(path))
            .unwrap_or_default();
        // The counts are the pane's own title detail: a file row already says
        // the path, so the header spends the space on what changed.
        let (added, deleted) = {
            let document = state.read(cx);
            document
                .files()
                .iter()
                .fold((0usize, 0usize), |(a, d), file| {
                    (a + file.additions(), d + file.deletions())
                })
        };
        // A merge can be written back, and only once nothing is left
        // unresolved: a partially merged file is not a merge. The predicate is
        // `DiffView`'s own so the control and the handler cannot disagree.
        let mergeable = self.right.diff.is_merge();
        let chip = |id: &'static str, label: String, on: bool| {
            let hover = label.clone();
            div()
                .debug_selector(move || id.to_string())
                .id(SharedString::from(id))
                .h(px(20.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .flex_none()
                .rounded(px(6.0))
                .cursor_pointer()
                .when(on, |cell| cell.bg(rgb(theme::hover())))
                .child(if on {
                    txt(label, 11.0, theme::text()).into_any_element()
                } else {
                    txt(hover.clone(), 11.0, theme::faint()).into_any_element()
                })
        };
        let header = div()
            .debug_selector(|| String::from("diff-toolbar"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(theme::RIGHT_PAD))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(icon(IconName::GitCompareArrows, 13.0, theme::faint()))
            .child(
                div()
                    .debug_selector(|| String::from("diff-title"))
                    .flex_1()
                    .min_w_0()
                    .child(txt(name, 12.0, theme::text()).truncate()),
            )
            .when(added > 0, |row| {
                row.child(txt(format!("+{added}"), 11.0, theme::success()))
            })
            .when(deleted > 0, |row| {
                row.child(txt(format!("\u{2212}{deleted}"), 11.0, theme::error()))
            })
            .child(
                chip(
                    "diff-mode",
                    match mode {
                        DiffMode::Split => self.state.lang.tr("diff.modeSplit"),
                        _ => self.state.lang.tr("diff.modeUnified"),
                    },
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.toggle_diff_mode(cx);
                })),
            )
            .child(
                chip("diff-wrap", self.state.lang.tr("diff.wrap"), wrap).on_click(cx.listener(
                    |this, _event, _window, cx| {
                        this.toggle_diff_wrap(cx);
                    },
                )),
            )
            .when(mergeable, |row| {
                row.child(
                    chip("diff-save", self.state.lang.tr("diff.save"), true).on_click(cx.listener(
                        |this, _event, _window, cx| {
                            this.save_merged(cx);
                        },
                    )),
                )
            })
            .when(self.right.diff.is_open(), |row| {
                row.child(
                    div()
                        .debug_selector(|| String::from("diff-close"))
                        .id("diff-close")
                        .h(px(20.0))
                        .w(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .hover(|cell| cell.bg(rgb(theme::hover())))
                        .child(icon(IconName::Close, 11.0, theme::faint()))
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.right.diff.clear();
                            this.right.diff.error = None;
                            cx.notify();
                        })),
                )
            });
        let mut body = div()
            .debug_selector(|| String::from("diff-body"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .px(px(theme::RIGHT_PAD))
            .py(px(6.0))
            .child(Diff::new(&state).soft_wrap(wrap).h_full().w_full());
        if let Some(error) = self.right.diff.error.clone() {
            body = body.child(txt(error, theme::CHIP_SIZE, theme::error()));
        }
        pane.child(header).child(body).into_any_element()
    }

    /// Open the Diff pane on one turn's changed files.
    ///
    /// The data is the turn's own tool records: the same grouping the closing
    /// card draws, plus the one fact the card hides - whether a file's old side
    /// was recorded. Nothing here reads the disk or a VCS, because a tool
    /// request is the only before-image bsh keeps.
    pub(crate) fn open_diff_turn(&mut self, message: usize, cx: &mut Context<Self>) {
        let Some(session) = self.state.active_session() else {
            return;
        };
        let Some(target) = session.messages.get(message) else {
            return;
        };
        // The tools are cloned out before the shell is written: the session is
        // borrowed from the shell, and the diff view is a field of it.
        let tools = target.tools.clone();
        // The turn's number, one-based: the number of prompts up to and
        // including the message the reader pressed. It names the turn in the
        // header and in the empty state, so a reader landing on a list knows
        // which one they are looking at.
        let ordinal = session
            .messages
            .iter()
            .take(message + 1)
            .filter(|line| line.role == Role::User && !line.steering)
            .count()
            .max(1);
        let files = diff_pane::turn_files(&tools);
        let commands = diff_pane::TurnView::ran_commands(&tools);
        let empty = files
            .is_empty()
            .then(|| diff_pane::TurnView::why_empty(&tools));
        // The file comparison and the turn list are exclusive subjects: the
        // turn takes the surface over, and its toolbar carries no mode or wrap
        // control because neither applies to a recorded hunk.
        self.right.diff.state = None;
        self.right.diff.path = None;
        self.right.diff.source = None;
        self.right.diff.conflicts = 0;
        self.right.diff.resolved = false;
        self.right.diff.turn = Some(diff_pane::TurnView {
            message,
            ordinal,
            files,
            open: std::collections::HashSet::new(),
            rows: std::collections::HashMap::new(),
            empty,
            commands,
        });
        self.right.diff.error = None;
        self.right.tab = Some(RightTab::Diff);
        self.right.open = true;
        cx.notify();
    }

    /// Open the turn's list with one changed file already expanded.
    ///
    /// A changed-file row names one file, so it lands on that file's comparison
    /// rather than on a list the reader then has to search.
    pub(crate) fn open_diff_turn_at(&mut self, message: usize, path: &str, cx: &mut Context<Self>) {
        self.open_diff_turn(message, cx);
        // The path is the card's own identity for a changed file; an index
        // would assume the two groupings never differ.
        let index = self
            .right
            .diff
            .turn
            .as_ref()
            .and_then(|turn| turn.files.iter().position(|file| file.path == path));
        if let Some(index) = index {
            self.expand_turn_file(index, cx);
        }
        cx.notify();
    }

    /// Open or close one file's rows in the turn list.
    pub(crate) fn toggle_turn_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let open = self
            .right
            .diff
            .turn
            .as_ref()
            .is_some_and(|turn| turn.open.contains(&index));
        if open {
            if let Some(turn) = self.right.diff.turn.as_mut() {
                turn.open.remove(&index);
            }
            cx.notify();
            return;
        }
        self.expand_turn_file(index, cx);
    }

    /// Resolve one file's rows and mark it open.
    ///
    /// The rows are resolved HERE rather than per frame because placing the
    /// hunk reads the file as it is now; the crate's own rule forbids that on
    /// the render path. A file with no recorded old side resolves to none, and
    /// the row shows its basis note instead.
    fn expand_turn_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self
            .right
            .diff
            .turn
            .as_ref()
            .and_then(|turn| turn.files.get(index))
            .cloned()
        else {
            return;
        };
        let current = self.current_text_of(&file.path);
        let rows = diff_pane::turn_rows(&file, current.as_deref());
        if let Some(turn) = self.right.diff.turn.as_mut() {
            if let Some(rows) = rows {
                turn.rows.insert(index, rows);
            }
            turn.open.insert(index);
        }
        cx.notify();
    }

    /// The turn-focused body: a header naming the turn, then one row per file
    /// it wrote, each expandable to the comparison the tool recorded.
    fn turn_diff_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let lang = self.state.lang;
        let turn = self
            .right
            .diff
            .turn
            .as_ref()
            .expect("the caller checked the turn view");
        let added: usize = turn.files.iter().map(|file| file.added).sum();
        let deleted: usize = turn.files.iter().map(|file| file.deleted).sum();
        let title = crate::i18n::tr_args(
            lang,
            "diff.turnTitle",
            &[("turn", turn.ordinal.to_string())],
        );
        let header = div()
            .debug_selector(|| String::from("diff-turn-toolbar"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(theme::RIGHT_PAD))
            .flex_none()
            .border_b_1()
            .border_color(rgb(theme::border_soft()))
            .child(icon(IconName::GitCompareArrows, 13.0, theme::faint()))
            .child(
                div()
                    .debug_selector(|| String::from("diff-turn-title"))
                    .flex_1()
                    .min_w_0()
                    .child(txt(title, 12.0, theme::text()).truncate()),
            )
            .when(added > 0, |row| {
                row.child(txt(format!("+{added}"), 11.0, theme::success()))
            })
            .when(deleted > 0, |row| {
                row.child(txt(format!("\u{2212}{deleted}"), 11.0, theme::error()))
            })
            .child(
                div()
                    .debug_selector(|| String::from("diff-turn-close"))
                    .id("diff-turn-close")
                    .h(px(20.0))
                    .w(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .hover(|cell| cell.bg(rgb(theme::hover())))
                    .child(icon(IconName::Close, 11.0, theme::faint()))
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.right.diff.clear();
                        this.right.diff.error = None;
                        cx.notify();
                    })),
            );
        // One scroller per turn: a reader who opens another turn's list lands
        // at its top rather than at the previous list's offset.
        let message = turn.message;
        let mut body = div()
            .debug_selector(|| String::from("diff-turn-body"))
            .id(SharedString::from(format!("diff-turn-body-{message}")))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_y_scroll();
        if turn.files.is_empty() {
            // An empty list has a reason, and the two reasons are different
            // facts: a turn that wrote nothing, versus one whose writes came
            // from a command nobody recorded file by file.
            let key = match turn.empty.unwrap_or(diff_pane::TurnEmpty::NoWrites) {
                diff_pane::TurnEmpty::CommandsOnly => "diff.turnCommands",
                diff_pane::TurnEmpty::NoWrites => "diff.turnEmpty",
            };
            let text = crate::i18n::tr_args(lang, key, &[("turn", turn.ordinal.to_string())]);
            body = body.child(
                div()
                    .debug_selector(|| String::from("diff-turn-empty"))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .px(px(theme::RIGHT_PAD))
                    .child(icon(IconName::GitCompareArrows, 22.0, theme::faint()))
                    .child(
                        txt(text, theme::CHIP_SIZE, theme::text()).text_align(TextAlign::Center),
                    ),
            );
        } else {
            for (index, file) in turn.files.iter().enumerate() {
                body = body.child(self.turn_file_row(index, file, cx));
            }
            if turn.commands {
                // The list knows the recorded writes. A command in the same
                // turn may have changed more, and saying so is the difference
                // between a summary and a false claim of completeness.
                body = body.child(
                    div()
                        .debug_selector(|| String::from("diff-turn-command-note"))
                        .px(px(theme::RIGHT_PAD))
                        .py(px(8.0))
                        .child(txt(lang.tr("diff.turnCommandNote"), 11.0, theme::faint())),
                );
            }
        }
        div()
            .debug_selector(|| String::from("diff-pane"))
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// One file in the turn list, with its expandable comparison.
    fn turn_file_row(
        &self,
        index: usize,
        file: &diff_pane::TurnFile,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.state.lang;
        let turn = self
            .right
            .diff
            .turn
            .as_ref()
            .expect("a file row belongs to a turn");
        let open = turn.open.contains(&index);
        let note = file.basis.note_key().map(|key| lang.tr(key));
        let name = crate::ui::transcript::base_name(&file.path);
        let row = div()
            .debug_selector(move || format!("diff-turn-file-{index}"))
            .id(SharedString::from(format!("diff-turn-file-{index}")))
            .w_full()
            .px(px(theme::RIGHT_PAD))
            .py(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .hover(|cell| cell.bg(rgb(theme::hover())))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_turn_file(index, cx);
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(txt(name, 12.0, theme::text()).font_family(theme::CODE_FONT)),
            )
            .when_some(note, |row, note| row.child(txt(note, 11.0, theme::warn())))
            .child(txt(format!("+{}", file.added), 11.0, theme::success()))
            .child(txt(
                format!("\u{2212}{}", file.deleted),
                11.0,
                theme::error(),
            ));
        let mut column = div().w_full().flex().flex_col().child(row);
        if open {
            if let Some(key) = file.basis.note_key() {
                // No before-image was recorded: say so instead of drawing the
                // requested additions as if they were a located hunk.
                column = column.child(
                    div()
                        .debug_selector(move || format!("diff-turn-note-{index}"))
                        .px(px(theme::RIGHT_PAD + 20.0))
                        .py(px(6.0))
                        .child(txt(lang.tr(key), 11.0, theme::warn())),
                );
            } else if let Some(rows) = turn.rows.get(&index) {
                let mut list = div().w_full().flex().flex_col();
                for (line_no, line) in rows.iter().take(diff_pane::TURN_MAX_ROWS).enumerate() {
                    list = list.child(Self::turn_diff_line(index, line_no, line));
                }
                if rows.len() > diff_pane::TURN_MAX_ROWS {
                    let left = rows.len() - diff_pane::TURN_MAX_ROWS;
                    list = list.child(
                        div()
                            .debug_selector(move || format!("diff-turn-more-{index}"))
                            .px(px(theme::RIGHT_PAD + 20.0))
                            .py(px(4.0))
                            .child(txt(
                                crate::i18n::tr_args(
                                    lang,
                                    "diff.turnMore",
                                    &[("count", left.to_string())],
                                ),
                                11.0,
                                theme::faint(),
                            )),
                    );
                }
                column = column.child(list);
            }
        }
        column.into_any_element()
    }

    /// One row of an expanded file: the side's line number, the `-` / `+`
    /// marker, and the line's text.
    fn turn_diff_line(index: usize, line_no: usize, line: &diff_pane::PreviewLine) -> AnyElement {
        let ink = match line.mark {
            '-' | '+' => theme::text(),
            _ => theme::muted(),
        };
        let fill = match line.mark {
            '-' => theme::diff_del(),
            '+' => theme::diff_add(),
            _ => theme::page(),
        };
        let marker = match line.mark {
            '-' => "\u{2212}",
            '+' => "+",
            _ => "",
        };
        let marker_ink = match line.mark {
            '-' => theme::error(),
            '+' => theme::success(),
            _ => theme::faint(),
        };
        let number = line
            .new
            .or(line.old)
            .map(|n| n.to_string())
            .unwrap_or_default();
        div()
            .debug_selector(move || format!("diff-turn-line-{index}-{line_no}"))
            .id(SharedString::from(format!(
                "diff-turn-line-{index}-{line_no}"
            )))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .h(px(18.0))
            .bg(rgb(fill))
            .child(
                txt(number, 11.0, theme::faint())
                    .w(px(34.0))
                    .pr(px(6.0))
                    .flex_none()
                    .line_height(px(18.0))
                    .text_align(TextAlign::Right),
            )
            .child(
                txt(marker, 11.0, marker_ink)
                    .w(px(14.0))
                    .flex_none()
                    .line_height(px(18.0))
                    .text_align(TextAlign::Center),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .px(px(6.0))
                    .child(
                        txt(line.text.clone(), 11.0, ink)
                            .font_family(theme::CODE_FONT)
                            .line_height(px(18.0)),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{FontStyle, FontWeight, Keystroke, Modifiers};

    use super::{anchor_line, grid_fonts, terminal_key_bytes};

    fn key(key: &str, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn enter_sends_a_carriage_return() {
        assert_eq!(terminal_key_bytes(&key("enter", None)), Some(vec![b'\r']));
    }

    #[test]
    fn printable_keys_send_their_utf8() {
        assert_eq!(
            terminal_key_bytes(&key("a", Some("a"))),
            Some(b"a".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(&key("s", Some("ß"))),
            Some("ß".as_bytes().to_vec())
        );
    }

    #[test]
    fn the_anchor_line_is_the_middle_of_the_part_in_view() {
        let text = "one\ntwo\nthree\nfour\nfive\nsix\n";
        // Lines two through four: "two" up to the start of "five".
        let range = text.find("two").unwrap()..text.find("five").unwrap();
        // Everything laid out: the middle of lines 2..=4.
        assert_eq!(anchor_line(text, &range, &(0..10)), Some(3));
        // The first line scrolled above the viewport; the middle is unchanged.
        assert_eq!(anchor_line(text, &range, &(1..10)), Some(3));
        // Only the tail is laid out: the middle of what is left is line 4.
        assert_eq!(anchor_line(text, &range, &(3..10)), Some(4));
        // The whole selection scrolled past: no line to ride.
        assert_eq!(anchor_line(text, &range, &(5..10)), None);
    }

    #[test]
    fn an_empty_selection_has_no_anchor() {
        assert_eq!(anchor_line("one\ntwo\n", &(0..0), &(0..10)), None);
        assert_eq!(anchor_line("one\ntwo\n", &(3..3), &(0..10)), None);
    }

    #[test]
    fn arrows_send_their_csi_sequences() {
        assert_eq!(
            terminal_key_bytes(&key("up", None)),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            terminal_key_bytes(&key("left", None)),
            Some(b"\x1b[D".to_vec())
        );
    }

    #[test]
    fn a_control_chord_sends_its_c0_byte() {
        let mut chord = key("c", Some("c"));
        chord.modifiers.control = true;
        assert_eq!(terminal_key_bytes(&chord), Some(vec![0x03]));
    }

    /// The painter picks a run's face with `bold | italic << 1`, so the table's
    /// order is part of the contract: a wrong index draws bold text upright or
    /// italic text roman, which no other assertion would catch.
    #[test]
    fn grid_fonts_index_by_bold_and_italic() {
        let fonts = grid_fonts();
        assert_eq!(fonts[0].weight, FontWeight::NORMAL);
        assert_eq!(fonts[0].style, FontStyle::Normal);
        assert_eq!(fonts[1].weight, FontWeight::BOLD);
        assert_eq!(fonts[1].style, FontStyle::Normal);
        assert_eq!(fonts[2].weight, FontWeight::NORMAL);
        assert_eq!(fonts[2].style, FontStyle::Italic);
        assert_eq!(fonts[3].weight, FontWeight::BOLD);
        assert_eq!(fonts[3].style, FontStyle::Italic);
        for font in fonts {
            assert_eq!(&*font.family, "monospace");
        }
    }

    #[test]
    fn cmd_chords_are_left_to_the_application() {
        let mut chord = key("p", Some("p"));
        chord.modifiers.platform = true;
        assert_eq!(terminal_key_bytes(&chord), None);
    }
}
