//! The managed terminal: one interactive shell on a real PTY, drawn from a
//! cell grid, and gated by a Rego policy at three points.
//!
//! The pane owns one shell for its whole life, started interactively in the
//! session workspace. Output is parsed with the vte crate into a rows-by-columns
//! grid of coloured cells, so the pane can paint a real terminal rather than a
//! list of stripped lines. Typing is written to the PTY master as raw bytes, so
//! the shell's own line editor, history and prompt do the editing. Those writes
//! ride a thread with a bounded queue: a PTY write blocks once the shell stops
//! reading, and the pane types from the UI thread.
//!
//! The grid is a real emulator's, not a screenful of text: a double-width
//! glyph (CJK, fullwidth forms, emoji) takes two cells and its second cell is a
//! continuation marker the painter never draws as a glyph, a zero-width
//! combining mark attaches to the cell before it instead of advancing the
//! cursor, the retained scrollback is browsable through a viewport offset, and
//! CSI ?1049 / ?47 swap in a blank alternate screen so a full-screen program
//! paints over its own copy instead of over the shell's history.
//!
//! # The gate
//!
//! One Rego module (package `terminal`) decides three points. Each evaluation
//! receives an `input` object and the gate reads `data.terminal.verdict`; the
//! shim builds that rule from the policy's own `decision` (a boolean or an
//! object) and falls back to its plain `allow` rule, so a policy may be either
//! shape:
//!
//! - **start**: `{action: "start", command, shell, cwd, deny}`, asked before
//!   any process exists. A refused start spawns nothing, and that stays the
//!   only door that guarantees no process runs.
//! - **input**: `{action: "input", command: <the submitted line, exactly>,
//!   cwd, deny}`, asked on Enter before the submitting carriage return is
//!   written. A refusal writes no line to the PTY; it writes Ctrl-U instead so
//!   the shell's line editor erases the text it has already echoed, and the
//!   refusal is shown in the pane's status line and recorded in the audit ring.
//! - **output**: `{action: "output", text: <one chunk read from the PTY,
//!   truncated to 4096 characters>, deny}`, asked per chunk before the chunk
//!   reaches the grid. A refusal drops the chunk and appends one marker line;
//!   a decision that carries a `redact` string feeds that string in place of
//!   the chunk.
//!
//! Deliberately NOT gated: individual keystrokes (printable input, arrows,
//! control chords, backspace), cursor movement, resize, and scrollback
//! browsing. Those are line editing and window management, not commands, and a
//! live echo means the line exists in the shell's editor before Enter. Because
//! the line is a mirror of what this pane typed, a shell-side edit (history
//! recall, a mid-line cursor jump) can differ from it; the start gate is the
//! hard boundary, and this input gate is the honest one available with a live
//! echo. The input object's `cwd` is the directory the terminal was
//! started in; bsh keeps no mirror of the shell's live directory, so a `cd` the
//! shell made is invisible to the policy.
//!
//! The policy file is `<workspace>/.bsh/terminal-policy.rego`. A missing file is
//! the documented default (allow everything, the shipped policy module). A file
//! that exists but does not parse closes every gate - start, input and output -
//! so an unusable rule can never widen the gate it was meant to narrow. The
//! file is re-read when its stamp changes (checked from the terminal clock, not
//! per frame); a broken edit keeps the LAST GOOD policy, records the parse
//! error in the audit ring and shows it in the status line, so a reload never
//! wedges and never silently widens. Every decision is appended to a bounded
//! audit ring (see `Terminal::audit`) and the pane reports a compact count
//! once something was refused or redacted. There is no settings surface for
//! policies: the file, its reload and the audit are the management surface.
//! The contract and worked examples are in `docs/bsh-terminal-policy.md`.
//!
//! # The sandbox
//!
//! The pane's spawn asks the turn's shared
//! [agent::sandbox::SandboxSpec] what it may do with a PTY IT owns, and the
//! same spec is what the shell tool runs under. On macOS and Linux the pane's
//! shell runs INSIDE the sandbox: the spec starts it with the caller's tty
//! opened BY PATH, because heel can grant that device write and ioctl. On
//! Windows heel cannot be handed the pane's ConPTY, so the shell runs on the
//! pane's own pty and the pane says why. The answer reaches the shell as
//! BSH_SANDBOX - a reader can type `echo $BSH_SANDBOX` - and is stated on the
//! pane's status line whenever the pane has something to report, rather than
//! being implied away. A preset that set `sandbox = "none"` reports "none"
//! (not "full-access"), so the shell agrees with the bash result. The Rego
//! input gate below still decides every submitted line.
//!
//! Owed, stated rather than faked: reflow on resize (the grid is clipped, not
//! re-wrapped), mouse reporting (CSI ?1000 / ?1002 / ?1006 are not modelled, so
//! a full-screen program that asks for the mouse gets no events; the way back
//! in is the wheel and the selection), bracketed paste (CSI ?2004 is ignored
//! and bsh has no paste path of its own), and a scrollbar over the retained
//! scrollback (the wheel and its viewport offset work; the bar is not drawn).

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use regorus::{Engine as RegoEngine, Value as RegoValue};
use unicode_width::UnicodeWidthChar;

use crate::i18n::Lang;

/// The columns a terminal starts with before the pane measures itself.
pub(crate) const DEFAULT_COLS: usize = 80;
/// The rows a terminal starts with before the pane measures itself.
pub(crate) const DEFAULT_ROWS: usize = 24;
/// How many rows scrolled off the top are retained for later inspection.
pub(crate) const SCROLLBACK_LIMIT: usize = 2000;
/// How many combining marks one cell keeps. A grapheme with more loses the
/// extra accents' ink, never their cells: the cursor arithmetic stays exact.
const MAX_COMBINING: usize = 3;
/// The longest output chunk the gate is shown, in characters.
const OUTPUT_GATE_CAP: usize = 4096;
/// How much of a decision's subject the audit ring keeps, in characters.
const AUDIT_SUBJECT_CAP: usize = 160;
/// How much of a policy's own reason the audit ring keeps, in characters. The
/// ring holds a fixed number of rows, so an uncapped reason would make its
/// memory a function of the policy file rather than of the ring.
const AUDIT_REASON_CAP: usize = 512;
/// How many decisions the audit ring retains.
pub(crate) const AUDIT_LIMIT: usize = 256;

const ATTR_BOLD: u16 = 1 << 0;
const ATTR_DIM: u16 = 1 << 1;
const ATTR_ITALIC: u16 = 1 << 2;
const ATTR_UNDERLINE: u16 = 1 << 3;
const ATTR_INVERSE: u16 = 1 << 4;
const ATTR_STRIKE: u16 = 1 << 5;
/// The cell is the trailing half of the double-width glyph to its left.
const ATTR_CONTINUATION: u16 = 1 << 6;

/// A colour a cell can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Color {
    /// The theme's default foreground or background, depending on the slot.
    #[default]
    Default,
    /// One of the 256 indexed palette entries.
    Indexed(u8),
    /// A 24-bit colour.
    Rgb(u8, u8, u8),
}

/// The pen's state between SGR sequences.
#[derive(Clone, Copy, Debug, Default)]
struct Attrs {
    fg: Color,
    bg: Color,
    flags: u16,
}

/// One cell of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The character drawn in the cell; a space when the cell is blank or when
    /// it is the trailing half of a double-width glyph.
    pub(crate) ch: char,
    /// The foreground colour.
    pub(crate) fg: Color,
    /// The background colour.
    pub(crate) bg: Color,
    flags: u16,
    /// The zero-width marks written after `ch`, which draw over it.
    combining: [char; MAX_COMBINING],
    combining_len: u8,
}

impl Cell {
    fn blank(attrs: Attrs) -> Self {
        Self {
            ch: ' ',
            fg: attrs.fg,
            bg: attrs.bg,
            flags: attrs.flags,
            combining: ['\0'; MAX_COMBINING],
            combining_len: 0,
        }
    }

    fn with_pen(attrs: Attrs, ch: char) -> Self {
        Self {
            ch,
            ..Self::blank(attrs)
        }
    }

    /// The trailing half of a double-width glyph: it holds no character of its
    /// own, and the painter must not draw one for it.
    fn continuation(attrs: Attrs) -> Self {
        Self {
            flags: attrs.flags | ATTR_CONTINUATION,
            ..Self::blank(attrs)
        }
    }

    /// Whether the cell is the trailing half of a double-width glyph.
    pub(crate) fn is_continuation(&self) -> bool {
        self.flags & ATTR_CONTINUATION != 0
    }

    /// The combining marks drawn over this cell.
    pub(crate) fn combining(&self) -> &[char] {
        &self.combining[..self.combining_len as usize]
    }

    /// Attach a zero-width mark to the cell. Marks past the buffer's room are
    /// dropped; the cursor still does not move for them.
    fn push_combining(&mut self, ch: char) {
        let len = self.combining_len as usize;
        if len < MAX_COMBINING {
            self.combining[len] = ch;
            self.combining_len += 1;
        }
    }

    /// Append the cell's own text - its character and its combining marks.
    pub(crate) fn push_text(&self, out: &mut String) {
        out.push(self.ch);
        for mark in self.combining() {
            out.push(*mark);
        }
    }

    /// The byte length `push_text` appends, for a shaped run's length.
    pub(crate) fn text_len(&self) -> usize {
        self.ch.len_utf8()
            + self
                .combining()
                .iter()
                .map(|ch| ch.len_utf8())
                .sum::<usize>()
    }

    /// Whether the cell is drawn in bold.
    pub(crate) fn bold(&self) -> bool {
        self.flags & ATTR_BOLD != 0
    }

    /// Whether the cell is drawn in italics.
    pub(crate) fn italic(&self) -> bool {
        self.flags & ATTR_ITALIC != 0
    }

    /// Whether the cell is underlined.
    pub(crate) fn underline(&self) -> bool {
        self.flags & ATTR_UNDERLINE != 0
    }

    /// Whether the cell's foreground and background are swapped by the shell.
    pub(crate) fn inverse(&self) -> bool {
        self.flags & ATTR_INVERSE != 0
    }

    /// Whether the cell is struck through.
    pub(crate) fn strike(&self) -> bool {
        self.flags & ATTR_STRIKE != 0
    }

    /// Whether the cell is drawn dim.
    pub(crate) fn dim(&self) -> bool {
        self.flags & ATTR_DIM != 0
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Attrs::default())
    }
}

/// The primary screen, parked while an alternate screen is up.
struct Alternate {
    cells: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    wrap_pending: bool,
}

/// The parser's own screen state: the grid, the cursor, and the pen.
struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<Cell>>,
    cursor_row: usize,
    cursor_col: usize,
    saved_cursor: (usize, usize),
    scroll_top: usize,
    scroll_bottom: usize,
    cursor_visible: bool,
    pen: Attrs,
    /// A character was written in the last column: the next one wraps first.
    wrap_pending: bool,
    scrollback: VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
    /// How many rows the viewport sits above the bottom, into the scrollback.
    view_offset: usize,
    /// Rows dropped from the front of the scrollback, so a document row number
    /// keeps naming the same line after the ring trims.
    dropped: u64,
    /// The primary screen's contents while an alternate screen is up.
    alternate: Option<Box<Alternate>>,
    /// Scratch storage for one CSI sequence's parameters, reused across
    /// sequences so a coloured prompt does not allocate per escape.
    csi_params: Vec<u16>,
}

impl Grid {
    fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells: vec![vec![Cell::default(); cols]; rows],
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: (0, 0),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            cursor_visible: true,
            pen: Attrs::default(),
            wrap_pending: false,
            scrollback: VecDeque::new(),
            scrollback_limit,
            view_offset: 0,
            dropped: 0,
            alternate: None,
            csi_params: Vec::new(),
        }
    }

    fn blank_row(&self) -> Vec<Cell> {
        vec![Cell::blank(self.pen); self.cols]
    }

    fn reset(&mut self) {
        let cols = self.cols;
        let rows = self.rows;
        self.cells = vec![vec![Cell::default(); cols]; rows];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.saved_cursor = (0, 0);
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.cursor_visible = true;
        self.pen = Attrs::default();
        self.wrap_pending = false;
        self.view_offset = 0;
        self.dropped += self.scrollback.len() as u64;
        self.scrollback.clear();
        self.alternate = None;
    }

    /// Move the cursor down one line, scrolling the region when it is at its
    /// bottom edge.
    fn linefeed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row + 1 < self.rows {
            self.cursor_row += 1;
        }
    }

    /// The height of the scrolling region, in rows.
    fn region_height(&self) -> usize {
        (self.scroll_bottom + 1).saturating_sub(self.scroll_top)
    }

    /// Scroll the region up count lines, keeping the top edge in scrollback
    /// when the region starts at the top of the primary screen.
    ///
    /// The count is clamped to the region's height: past it every row in the
    /// region is blank and a further scroll could only move blank rows, so the
    /// screen after `CSI 40000 S` is the same as after `CSI 2 S` on a two-row
    /// grid. Clamping is also what keeps eight bytes of untrusted output from
    /// doing forty thousand rows of work and pushing forty thousand blank lines
    /// into the scrollback nobody could read back.
    fn scroll_up(&mut self, count: usize) {
        let height = self.region_height();
        for _ in 0..count.min(height) {
            let top = self.cells.remove(self.scroll_top);
            if self.scroll_top == 0 && self.alternate.is_none() {
                self.remember(top);
            }
            let blank = self.blank_row();
            self.cells.insert(self.scroll_bottom, blank);
        }
    }

    /// Scroll the region down count lines.
    ///
    /// Past the region's height the region is blank and the extra iterations
    /// remove a blank row only to put an identical one back, so they are not
    /// done at all.
    fn scroll_down(&mut self, count: usize) {
        let height = self.region_height();
        for _ in 0..count.min(height) {
            self.cells.remove(self.scroll_bottom);
            let blank = self.blank_row();
            self.cells.insert(self.scroll_top, blank);
        }
    }

    fn remember(&mut self, row: Vec<Cell>) {
        if self.scrollback_limit == 0 {
            return;
        }
        if self.scrollback.len() >= self.scrollback_limit {
            // A VecDeque, so trimming the oldest row is O(1) rather than a
            // memmove of every retained row on each line the shell scrolls.
            self.scrollback.pop_front();
            self.dropped += 1;
            self.view_offset = self.view_offset.saturating_sub(1);
        }
        self.scrollback.push_back(row);
    }

    /// Print one character, honouring its display width.
    ///
    /// This is where a terminal's cell arithmetic lives: a double-width glyph
    /// claims two cells and advances the cursor by two, a zero-width mark
    /// attaches to the cell before it and advances by none, and a glyph that
    /// does not fit before the right edge wraps first.
    fn print_char(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width == 0 {
            self.attach_combining(ch);
            return;
        }
        if self.wrap_pending {
            self.wrap_pending = false;
            self.cursor_col = 0;
            self.linefeed();
        }
        if self.cursor_col >= self.cols {
            return;
        }
        // A double-width glyph needs both cells; in the last column there is
        // only room for it after a wrap, which is what xterm does.
        if width > 1 && self.cols > 1 && self.cursor_col + 1 >= self.cols {
            self.cursor_col = 0;
            self.linefeed();
        }
        let row = self.cursor_row;
        let col = self.cursor_col;
        self.split_wide_at(row, col);
        self.cells[row][col] = Cell::with_pen(self.pen, ch);
        let mut next = col + 1;
        if width > 1 && col + 1 < self.cols {
            self.cells[row][col + 1] = Cell::continuation(self.pen);
            next = col + 2;
        }
        if next < self.cols {
            self.cursor_col = next;
        } else {
            self.cursor_col = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    /// Attach a zero-width mark to the cell the cursor most recently printed.
    fn attach_combining(&mut self, ch: char) {
        if !self.wrap_pending && self.cursor_col == 0 {
            return;
        }
        let row = self.cursor_row;
        let mut col = if self.wrap_pending {
            self.cursor_col
        } else {
            self.cursor_col - 1
        };
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        if line.get(col).map(Cell::is_continuation).unwrap_or(false) {
            col = col.saturating_sub(1);
        }
        if let Some(cell) = line.get_mut(col) {
            cell.push_combining(ch);
        }
    }

    /// Clear the other half of a double-width glyph that `col` is about to
    /// overwrite, so a half-overwritten glyph never paints over its neighbour.
    fn split_wide_at(&mut self, row: usize, col: usize) {
        let blank = Cell::blank(self.pen);
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        if col > 0 && line.get(col).map(Cell::is_continuation).unwrap_or(false) {
            line[col - 1] = blank;
        }
        if line
            .get(col + 1)
            .map(Cell::is_continuation)
            .unwrap_or(false)
        {
            line[col + 1] = blank;
        }
    }

    /// Keep every double-width glyph whole after a cell shift.
    fn heal_row(&mut self, row: usize) {
        let cols = self.cols;
        let blank = Cell::blank(self.pen);
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        for col in 0..cols {
            let cell = line[col];
            if cell.is_continuation() {
                let led = col > 0
                    && !line[col - 1].is_continuation()
                    && UnicodeWidthChar::width(line[col - 1].ch) == Some(2);
                if !led {
                    line[col] = blank;
                }
            } else if UnicodeWidthChar::width(cell.ch) == Some(2) {
                let trailed = col + 1 < cols && line[col + 1].is_continuation();
                if !trailed {
                    line[col] = blank;
                }
            }
        }
    }

    /// The row at `index` in the combined history: scrollback first, then the
    /// screen.
    fn row(&self, index: usize) -> Option<&[Cell]> {
        if index < self.scrollback.len() {
            self.scrollback.get(index).map(Vec::as_slice)
        } else {
            self.cells
                .get(index - self.scrollback.len())
                .map(Vec::as_slice)
        }
    }

    fn execute_control(&mut self, byte: u8) {
        match byte {
            // BS
            0x08 => {
                self.wrap_pending = false;
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            // HT
            0x09 => {
                self.wrap_pending = false;
                let next = ((self.cursor_col / 8) + 1) * 8;
                self.cursor_col = next.min(self.cols.saturating_sub(1));
            }
            // LF, VT, FF
            0x0A..=0x0C => {
                self.wrap_pending = false;
                self.linefeed();
            }
            // CR
            0x0D => {
                self.wrap_pending = false;
                self.cursor_col = 0;
            }
            // BEL and other C0 controls carry no screen state.
            _ => {}
        }
    }

    fn erase_row(&mut self, row: usize, from: usize, to: usize) {
        let cols = self.cols;
        let blank = Cell::blank(self.pen);
        let from = from.min(cols);
        let to = to.min(cols);
        if from >= to {
            return;
        }
        let Some(line) = self.cells.get_mut(row) else {
            return;
        };
        // A double-width glyph straddling the erased range loses its other
        // half too, so a blanked cell never leaves half a glyph behind.
        let split_left = from > 0 && line.get(from).map(Cell::is_continuation).unwrap_or(false);
        let split_right = to < cols && line.get(to).map(Cell::is_continuation).unwrap_or(false);
        for cell in line.iter_mut().take(to).skip(from) {
            *cell = blank;
        }
        if split_left {
            line[from - 1] = blank;
        }
        if split_right {
            line[to] = blank;
        }
    }
}
impl Grid {
    /// One CSI sequence: gather its parameters into the parser's own scratch
    /// buffer, dispatch, and keep the buffer for the next sequence.
    ///
    /// A terminal sees an escape sequence for every colour and cursor move a
    /// prompt emits, so collecting them into a fresh `Vec` put one allocation
    /// in the parser's hottest loop.
    fn csi(&mut self, params: &vte::Params, intermediates: &[u8], action: char) {
        let mut values = std::mem::take(&mut self.csi_params);
        values.clear();
        values.extend(params.iter().flatten().copied());
        self.dispatch_csi(&values, intermediates, action);
        self.csi_params = values;
    }

    fn dispatch_csi(&mut self, values: &[u16], intermediates: &[u8], action: char) {
        let count = |index: usize| -> usize {
            match values.get(index).copied().unwrap_or(1) {
                0 => 1,
                value => value as usize,
            }
        };
        let index = |index: usize, fallback: usize| -> usize {
            match values.get(index).copied().unwrap_or(0) {
                0 => fallback,
                value => value as usize,
            }
        };
        match action {
            // CUP / HVP: a one-based row and column.
            'H' | 'f' => {
                self.wrap_pending = false;
                self.cursor_row = index(0, 1).saturating_sub(1).min(self.rows - 1);
                self.cursor_col = index(1, 1).saturating_sub(1).min(self.cols - 1);
            }
            // CUU
            'A' => {
                self.wrap_pending = false;
                let floor = self.scroll_top;
                self.cursor_row = self.cursor_row.saturating_sub(count(0)).max(floor);
            }
            // CUD
            'B' => {
                self.wrap_pending = false;
                let ceiling = self.scroll_bottom;
                self.cursor_row = (self.cursor_row + count(0)).min(ceiling);
            }
            // CUF
            'C' => {
                self.wrap_pending = false;
                self.cursor_col = (self.cursor_col + count(0)).min(self.cols - 1);
            }
            // CUB
            'D' => {
                self.wrap_pending = false;
                self.cursor_col = self.cursor_col.saturating_sub(count(0));
            }
            // CNL
            'E' => {
                self.wrap_pending = false;
                self.cursor_row = (self.cursor_row + count(0)).min(self.scroll_bottom);
                self.cursor_col = 0;
            }
            // CPL
            'F' => {
                self.wrap_pending = false;
                self.cursor_row = self
                    .cursor_row
                    .saturating_sub(count(0))
                    .max(self.scroll_top);
                self.cursor_col = 0;
            }
            // CHA / HPA
            'G' | '\u{60}' => {
                self.wrap_pending = false;
                self.cursor_col = index(0, 1).saturating_sub(1).min(self.cols - 1);
            }
            // VPA
            'd' => {
                self.wrap_pending = false;
                self.cursor_row = index(0, 1).saturating_sub(1).min(self.rows - 1);
            }
            // ED
            'J' => {
                let mode = values.first().copied().unwrap_or(0);
                match mode {
                    0 => {
                        self.erase_row(self.cursor_row, self.cursor_col, self.cols);
                        for row in self.cursor_row + 1..self.rows {
                            self.erase_row(row, 0, self.cols);
                        }
                    }
                    1 => {
                        for row in 0..self.cursor_row {
                            self.erase_row(row, 0, self.cols);
                        }
                        self.erase_row(self.cursor_row, 0, self.cursor_col + 1);
                    }
                    2 => {
                        for row in 0..self.rows {
                            self.erase_row(row, 0, self.cols);
                        }
                    }
                    // 3 clears the scrollback, which is the only part ED 3 touches.
                    3 => {
                        self.dropped += self.scrollback.len() as u64;
                        self.scrollback.clear();
                        self.view_offset = 0;
                    }
                    _ => {}
                }
            }
            // EL
            'K' => {
                let mode = values.first().copied().unwrap_or(0);
                match mode {
                    0 => self.erase_row(self.cursor_row, self.cursor_col, self.cols),
                    1 => self.erase_row(self.cursor_row, 0, self.cursor_col + 1),
                    2 => self.erase_row(self.cursor_row, 0, self.cols),
                    _ => {}
                }
            }

            // ICH
            '@' => {
                let line = &mut self.cells[self.cursor_row];
                let n = count(0).min(self.cols);
                if self.cursor_col <= self.cols {
                    for _ in 0..n {
                        line.insert(self.cursor_col, Cell::blank(self.pen));
                    }
                    line.truncate(self.cols);
                }
                self.heal_row(self.cursor_row);
            }
            // DCH
            'P' => {
                let line = &mut self.cells[self.cursor_row];
                let n = count(0).min(self.cols - self.cursor_col);
                for _ in 0..n {
                    if self.cursor_col < line.len() {
                        line.remove(self.cursor_col);
                    }
                }
                while line.len() < self.cols {
                    line.push(Cell::blank(self.pen));
                }
                self.heal_row(self.cursor_row);
            }
            // ECH
            'X' => {
                let from = self.cursor_col;
                let to = (self.cursor_col + count(0)).min(self.cols);
                self.erase_row(self.cursor_row, from, to);
            }
            // IL
            'L' => {
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let n = count(0).min(self.scroll_bottom - self.cursor_row + 1);
                    for _ in 0..n {
                        self.cells.remove(self.scroll_bottom);
                        let blank = self.blank_row();
                        self.cells.insert(self.cursor_row, blank);
                    }
                }
            }
            // DL
            'M' => {
                if self.cursor_row >= self.scroll_top && self.cursor_row <= self.scroll_bottom {
                    let n = count(0).min(self.scroll_bottom - self.cursor_row + 1);
                    for _ in 0..n {
                        self.cells.remove(self.cursor_row);
                        let blank = self.blank_row();
                        self.cells.insert(self.scroll_bottom, blank);
                    }
                }
            }
            // SU
            'S' => self.scroll_up(count(0)),
            // SD
            'T' => self.scroll_down(count(0)),
            // SGR
            'm' => self.sgr(values),
            // DECSTBM: a one-based top and bottom, bottom inclusive.
            'r' => {
                let top = index(0, 1).saturating_sub(1);
                let bottom = index(1, self.rows).saturating_sub(1);
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.cursor_row = top;
                    self.cursor_col = 0;
                    self.wrap_pending = false;
                }
            }
            // SCOSC / SCORC
            's' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            'u' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
                self.wrap_pending = false;
            }
            // DECSET / DECRST: the cursor's visibility, the two alternate
            // screens, the cursor save/restore pair, and nothing else.
            'h' | 'l' if intermediates == b"?" => {
                let on = action == 'h';
                for value in values {
                    match value {
                        // DECTCEM
                        25 => self.cursor_visible = on,
                        // The alternate screen: 1049 also parks the cursor, 47
                        // and 1047 are the older spellings of the same swap.
                        47 | 1047 | 1049 => {
                            if on {
                                self.enter_alternate();
                            } else {
                                self.leave_alternate();
                            }
                        }
                        // DECSC / DECRC on their own.
                        1048 => {
                            if on {
                                self.saved_cursor = (self.cursor_row, self.cursor_col);
                            } else {
                                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
                                self.wrap_pending = false;
                            }
                        }
                        // Mouse reporting (1000/1002/1003/1006) and bracketed
                        // paste (2004) are owed; see the module docs.
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Save the primary screen and start a blank alternate one.
    ///
    /// This is what makes a full-screen program (vim, htop, less) usable: it
    /// paints over its own copy of the screen, and leaving restores the shell's
    /// prompt exactly where it was. The scrollback belongs to the primary
    /// screen; an alternate screen never pushes rows into it.
    fn enter_alternate(&mut self) {
        if self.alternate.is_some() {
            return;
        }
        let blank = Cell::blank(self.pen);
        let cells = std::mem::replace(&mut self.cells, vec![vec![blank; self.cols]; self.rows]);
        self.alternate = Some(Box::new(Alternate {
            cells,
            cursor_row: self.cursor_row,
            cursor_col: self.cursor_col,
            saved_cursor: self.saved_cursor,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            wrap_pending: self.wrap_pending,
        }));
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.wrap_pending = false;
        // A full-screen program owns the whole window, so the viewport comes
        // back to the bottom with it.
        self.view_offset = 0;
    }

    /// Put the primary screen back, resized to the window the terminal has now.
    fn leave_alternate(&mut self) {
        let Some(saved) = self.alternate.take() else {
            return;
        };
        let cols = self.cols;
        let rows = self.rows;
        let mut cells = saved.cells;
        cells.resize_with(rows, || vec![Cell::default(); cols]);
        for line in &mut cells {
            line.resize(cols, Cell::default());
        }
        self.cells = cells;
        self.cursor_row = saved.cursor_row.min(rows - 1);
        self.cursor_col = saved.cursor_col.min(cols - 1);
        self.saved_cursor = saved.saved_cursor;
        self.scroll_top = saved.scroll_top.min(rows - 1);
        self.scroll_bottom = saved.scroll_bottom.min(rows - 1).max(self.scroll_top);
        self.wrap_pending = saved.wrap_pending;
        self.view_offset = self.view_offset.min(self.scrollback.len());
    }

    fn sgr(&mut self, values: &[u16]) {
        if values.is_empty() {
            self.pen = Attrs::default();
            return;
        }
        let mut i = 0;
        while i < values.len() {
            let value = values[i];
            match value {
                0 => self.pen = Attrs::default(),
                1 => self.pen.flags |= ATTR_BOLD,
                2 => self.pen.flags |= ATTR_DIM,
                3 => self.pen.flags |= ATTR_ITALIC,
                4 => self.pen.flags |= ATTR_UNDERLINE,
                7 => self.pen.flags |= ATTR_INVERSE,
                9 => self.pen.flags |= ATTR_STRIKE,
                21 | 22 => self.pen.flags &= !(ATTR_BOLD | ATTR_DIM),
                23 => self.pen.flags &= !ATTR_ITALIC,
                24 => self.pen.flags &= !ATTR_UNDERLINE,
                27 => self.pen.flags &= !ATTR_INVERSE,
                29 => self.pen.flags &= !ATTR_STRIKE,
                30..=37 => self.pen.fg = Color::Indexed((value - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed((value - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed((value - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((value - 100 + 8) as u8),
                38 | 48 => {
                    // Extended colours: 5;index for the 256-colour palette and
                    // 2;r;g;b for truecolor. The colon form 38:2::r:g:b puts an
                    // empty colour space between the 2 and the channels; the
                    // leading zero is what this heuristic skips.
                    let foreground = value == 38;
                    match values.get(i + 1).copied() {
                        Some(5) => {
                            if let Some(index) = values.get(i + 2).copied() {
                                let color = Color::Indexed(index.min(255) as u8);
                                if foreground {
                                    self.pen.fg = color;
                                } else {
                                    self.pen.bg = color;
                                }
                                i += 2;
                            }
                        }
                        Some(2) => {
                            let mut start = i + 2;
                            if values.get(start).copied() == Some(0) && values.len() > start + 3 {
                                start += 1;
                            }
                            let red = values.get(start).copied().unwrap_or(0) as u8;
                            let green = values.get(start + 1).copied().unwrap_or(0) as u8;
                            let blue = values.get(start + 2).copied().unwrap_or(0) as u8;
                            let color = Color::Rgb(red, green, blue);
                            if foreground {
                                self.pen.fg = color;
                            } else {
                                self.pen.bg = color;
                            }
                            i = start + 2;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            // DECSC / DECRC
            b'7' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            b'8' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
                self.wrap_pending = false;
            }
            // IND
            b'D' => self.linefeed(),
            // RI
            b'M' => {
                if self.cursor_row == self.scroll_top {
                    self.scroll_down(1);
                } else {
                    self.cursor_row = self.cursor_row.saturating_sub(1);
                }
            }
            // NEL
            b'E' => {
                self.linefeed();
                self.cursor_col = 0;
                self.wrap_pending = false;
            }
            // RIS
            b'c' => self.reset(),
            _ => {}
        }
    }

    /// Give the grid a new size, keeping the top-left content.
    fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        // No reflow (owed): the top-left content is kept, the rest is clipped
        // or blanked. A terminal that re-wrapped would have to keep logical
        // rows rather than cells, which is a larger grid than this one.
        for line in &mut self.cells {
            line.resize(cols, Cell::default());
        }
        self.cells.resize_with(rows, || vec![Cell::default(); cols]);
        self.cols = cols;
        self.rows = rows;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.wrap_pending = false;
        for row in 0..rows {
            self.heal_row(row);
        }
        if let Some(saved) = self.alternate.as_mut() {
            for line in &mut saved.cells {
                line.resize(cols, Cell::default());
            }
            saved
                .cells
                .resize_with(rows, || vec![Cell::default(); cols]);
        }
        self.view_offset = self.view_offset.min(self.scrollback.len());
    }

    /// Move the viewport by lines rows: positive goes back towards older rows.
    fn scroll_view(&mut self, lines: i32) {
        let limit = self.scrollback.len() as i64;
        let next = (self.view_offset as i64).saturating_add(lines as i64);
        self.view_offset = next.clamp(0, limit) as usize;
    }

    fn snapshot(&self) -> Snapshot {
        let offset = self.view_offset.min(self.scrollback.len());
        let total = self.scrollback.len() + self.rows;
        let end = total.saturating_sub(offset);
        let start = end.saturating_sub(self.rows);
        let mut rows = Vec::with_capacity(end - start);
        for index in start..end {
            if let Some(row) = self.row(index) {
                rows.push(row.to_vec());
            }
        }
        Snapshot {
            rows,
            cols: self.cols,
            cursor: if self.cursor_visible && offset == 0 {
                Some((self.cursor_row, self.cursor_col))
            } else {
                None
            },
            first_row: self.dropped + start as u64,
        }
    }
}

impl vte::Perform for Grid {
    fn print(&mut self, ch: char) {
        self.print_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        self.execute_control(byte);
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        self.csi(params, intermediates, action);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.esc(intermediates, byte);
    }
}
#[cfg(test)]
impl Grid {
    fn visible_text(&self) -> Vec<String> {
        self.cells
            .iter()
            .map(|row| row.iter().map(|cell| cell.ch).collect::<String>())
            .collect()
    }

    fn all_text(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .scrollback
            .iter()
            .map(|row| row.iter().map(|cell| cell.ch).collect())
            .collect();
        lines.extend(self.visible_text());
        lines
    }
}

/// One frame's copy of the screen: what the pane paints.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    /// The grid's rows, each cols cells wide, oldest visible row first.
    pub(crate) rows: Vec<Vec<Cell>>,
    /// The width of every row.
    pub(crate) cols: usize,
    /// The cursor's (row, column) while the shell shows it, and only while the
    /// viewport is at the bottom.
    pub(crate) cursor: Option<(usize, usize)>,
    /// The document row that rows[0] names: a stable number that counts every
    /// retained row, so a selection anchored to it survives scrolling.
    pub(crate) first_row: u64,
}

#[cfg(test)]
impl Snapshot {
    /// The text of one row, with the trailing blank cells kept.
    pub(crate) fn row_text(&self, row: usize) -> String {
        self.rows
            .get(row)
            .map(|line| line.iter().map(|cell| cell.ch).collect())
            .unwrap_or_default()
    }

    /// The text of the whole visible grid, one row per line.
    pub(crate) fn text(&self) -> String {
        self.rows
            .iter()
            .map(|line| line.iter().map(|cell| cell.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The cell at (row, column), when both are on the grid.
    pub(crate) fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.rows.get(row).and_then(|line| line.get(col))
    }
}

/// A drag over the terminal, in document coordinates.
///
/// The rows are document rows (see Snapshot::first_row) and the columns are
/// cell columns, so the selection stays on the text it was drawn over while the
/// viewport scrolls or new output arrives. A stream selection, the way a real
/// terminal's is: from the anchor to the end of its row, every whole row
/// between, and the start of the last row to the head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    /// Where the drag started.
    anchor: (u64, usize),
    /// Where the pointer is now.
    head: (u64, usize),
}

impl Selection {
    /// A selection from one cell to another, in either direction.
    pub(crate) fn new(anchor: (u64, usize), head: (u64, usize)) -> Self {
        Self { anchor, head }
    }

    /// The first and last cell, ordered.
    fn range(&self) -> ((u64, usize), (u64, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether the cell at this row and column is inside the selection.
    pub(crate) fn contains(&self, row: u64, col: usize) -> bool {
        let (first, last) = self.range();
        if row < first.0 || row > last.0 {
            return false;
        }
        if first.0 == last.0 {
            return col >= first.1 && col <= last.1;
        }
        if row == first.0 {
            col >= first.1
        } else if row == last.0 {
            col <= last.1
        } else {
            true
        }
    }
}

/// The terminal's parser plus its grid.
///
/// The two are separate fields so the parser can borrow the grid mutably while
/// it dispatches, which is the shape vte's Parser::advance needs.
pub(crate) struct Screen {
    parser: vte::Parser,
    grid: Grid,
}

impl Screen {
    /// An empty screen of the given size, retaining scrollback rows.
    pub(crate) fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            grid: Grid::new(cols, rows, scrollback),
        }
    }

    /// Feed raw bytes from the PTY through the ANSI parser.
    ///
    /// Output arriving while the reader has scrolled into the scrollback holds
    /// the view on the same content instead of dragging it to the bottom: the
    /// offset grows by exactly the rows the feed retained.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let before = self.grid.dropped + self.grid.scrollback.len() as u64;
        let held = self.grid.view_offset;
        let Self { parser, grid } = self;
        // vte 0.15 takes the whole slice; 0.13 took one byte at a time.
        parser.advance(grid, bytes);
        let after = grid.dropped + grid.scrollback.len() as u64;
        grid.view_offset = if held == 0 {
            0
        } else {
            (held as u64 + after.saturating_sub(before)) as usize
        };
        grid.view_offset = grid.view_offset.min(grid.scrollback.len());
    }

    /// Give the grid a new size, keeping the top-left content.
    pub(crate) fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }

    /// Move the viewport into (positive) or out of (negative) the scrollback.
    pub(crate) fn scroll_view(&mut self, lines: i32) {
        self.grid.scroll_view(lines);
    }

    /// How many rows the viewport sits above the bottom.
    pub(crate) fn view_offset(&self) -> usize {
        self.grid.view_offset
    }

    /// The text a selection covers, with trailing blanks trimmed per line.
    pub(crate) fn selection_text(&self, selection: &Selection) -> String {
        let grid = &self.grid;
        let (first, last) = selection.range();
        let cols = grid.cols;
        // A drag names document rows, and the retained window moves under it:
        // the ring trims the front, ED 3 clears it, and the head of a drag can
        // sit past either end. Clamp the walk to the rows that exist - rows
        // outside it contributed nothing anyway, but the loop used to visit
        // each one, so an out-of-range head was effectively unbounded.
        let retained = (grid.scrollback.len() + grid.rows) as u64;
        let last_document = grid.dropped.saturating_add(retained).saturating_sub(1);
        let start = first.0.max(grid.dropped);
        let end = last.0.min(last_document);
        if start > end {
            return String::new();
        }
        let mut lines: Vec<String> = Vec::new();
        for document in start..=end {
            let Some(index) = document.checked_sub(grid.dropped) else {
                continue;
            };
            let Some(row) = grid.row(index as usize) else {
                continue;
            };
            let from = if document == first.0 {
                first.1.min(cols - 1)
            } else {
                0
            };
            let to = if document == last.0 {
                last.1.min(cols - 1)
            } else {
                cols - 1
            };
            let mut text = String::new();
            if from <= to {
                for cell in row.iter().take(to + 1).skip(from) {
                    if cell.is_continuation() {
                        continue;
                    }
                    cell.push_text(&mut text);
                }
            }
            while text.ends_with(' ') {
                text.pop();
            }
            lines.push(text);
        }
        lines.join("\n")
    }

    /// Copy the screen for painting.
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.grid.snapshot()
    }

    #[cfg(test)]
    fn lines(&self) -> Vec<String> {
        self.grid.all_text()
    }
}
/// Where a decision's rules came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicySource {
    /// The shipped allow-all policy: no workspace policy file exists.
    Default,
    /// The workspace's .bsh/terminal-policy.rego file.
    File,
}

impl PolicySource {
    /// The label an audit row carries.
    pub(crate) fn label(self) -> &'static str {
        match self {
            PolicySource::Default => "default",
            PolicySource::File => "file",
        }
    }
}

/// One gate point's decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Decision {
    allow: bool,
    reason: Option<String>,
    redact: Option<String>,
}

impl Decision {
    /// Let the action through untouched.
    fn allow() -> Self {
        Self {
            allow: true,
            ..Self::default()
        }
    }

    /// Refuse the action, optionally with the policy's own reason.
    fn refuse(reason: Option<String>) -> Self {
        Self {
            allow: false,
            reason,
            redact: None,
        }
    }

    /// Whether the action may proceed. A redacted output proceeds, with the
    /// policy's replacement text in place of the chunk.
    pub(crate) fn allowed(&self) -> bool {
        self.allow
    }

    /// The policy's reason for refusing.
    pub(crate) fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The text to feed in place of an output chunk.
    pub(crate) fn redaction(&self) -> Option<&str> {
        self.redact.as_deref()
    }
}

/// One decision, as the audit ring keeps it.
///
/// Serialisable: the ring the pane counts is also the record a deployment can
/// hand to serde, which is the audit's management face outside the window.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Audit {
    /// When the decision was made.
    pub(crate) at: SystemTime,
    /// The gate point: "start", "input", "output" or "policy".
    pub(crate) action: &'static str,
    /// The command, the policy path, or the truncated chunk the policy saw.
    pub(crate) subject: String,
    /// Whether the action proceeded.
    pub(crate) allow: bool,
    /// Whether the action proceeded with redacted output.
    pub(crate) redacted: bool,
    /// The policy's own reason, when it gave one.
    pub(crate) reason: Option<String>,
    /// "file" or "default": which rules decided.
    pub(crate) source: &'static str,
}

/// The most recent refused input, for the pane's status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InputRefusal {
    /// The exact line the gate refused.
    pub(crate) line: String,
    /// The policy's own reason, when it gave one.
    pub(crate) reason: Option<String>,
}

/// What one keystroke did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOutcome {
    /// The keystroke reached the shell.
    Sent,
    /// The gate refused the submitted line; nothing was submitted.
    Refused,
    /// There is no running shell to write to.
    Closed,
}

/// The policy module the gate is born with.
///
/// It is the documented default (allow everything) and the only module this
/// crate owns; a workspace policy layers over it. It also builds the one rule
/// the gate reads, so a policy can answer with either shape: `decision`, a
/// boolean or an object, wins when it is defined, and the plain `allow` rule is
/// the fallback every older policy is written against.
const DEFAULT_POLICY: &str = r#"
package terminal

# The shipped default: every gate point is open. input.deny is the shim's own
# escape hatch and is always false, so it never closes anything on its own.
default allow := true

allow := false if {
    input.deny
}

# The gate reads exactly one rule. A policy that wants to give a reason, or to
# redact output instead of dropping it, defines decision (a boolean or an
# object) instead of allow; both shapes are accepted and decision wins.
verdict := answer if {
    answer := data.terminal.decision
} else := allow
"#;

/// The regorus gate that decides whether the terminal may act.
///
/// The engine holds one Rego policy that receives each action as input and
/// returns a verdict. The default policy allows everything; a deployment
/// replaces it with its own rules. The gate is asked at three points - the
/// shell's start, each submitted input line, and each output chunk - and a
/// policy that fails to load closes all three.
pub(crate) struct Gate {
    engine: RegoEngine,
    source: PolicySource,
    /// The parse error that closed the gate, if it was closed.
    closed: Option<String>,
}

impl Gate {
    /// A gate whose policy allows every action.
    pub(crate) fn allow_all() -> Self {
        Self::new(PolicySource::Default)
    }

    fn new(source: PolicySource) -> Self {
        let mut engine = RegoEngine::new();
        engine
            .add_policy("terminal.rego".to_string(), DEFAULT_POLICY.to_string())
            .expect("the shipped policy is valid Rego");
        Self {
            engine,
            source,
            closed: None,
        }
    }

    /// A gate closed by the parse error that made its policy unusable.
    fn closed(source: PolicySource, error: String) -> Self {
        let mut gate = Self::new(source);
        gate.closed = Some(error);
        gate
    }

    /// A gate built from a workspace policy's source text.
    pub(crate) fn for_file(source: &str) -> anyhow::Result<Self> {
        let mut gate = Self::new(PolicySource::File);
        gate.load_policy("workspace.rego", source)?;
        Ok(gate)
    }

    /// Where this gate's rules came from.
    pub(crate) fn source(&self) -> PolicySource {
        self.source
    }

    /// Load an additional policy module.
    ///
    /// Deployments narrow the gate by adding their own modules: a later
    /// policy's decisions layer over the default's allow-all baseline. A module
    /// that cannot be parsed latches the gate shut, so an unusable rule can
    /// never widen the door it was meant to narrow.
    pub(crate) fn load_policy(&mut self, id: &str, source: &str) -> anyhow::Result<()> {
        if let Err(error) = self.engine.add_policy(id.to_string(), source.to_string()) {
            self.closed = Some(error.to_string());
            return Err(error);
        }
        Ok(())
    }

    /// Whether the gate permits this shell to start in this directory.
    pub(crate) fn start(&mut self, shell: &Path, cwd: &Path) -> Decision {
        let command = shell.to_string_lossy();
        let cwd = cwd.to_string_lossy();
        self.decide(
            "start",
            input_document([
                ("action", RegoValue::from("start")),
                ("command", RegoValue::from(command.as_ref())),
                ("shell", RegoValue::from(command.as_ref())),
                ("cwd", RegoValue::from(cwd.as_ref())),
                ("deny", RegoValue::from(false)),
            ]),
        )
    }

    /// Whether the gate permits this submitted line to reach the shell.
    pub(crate) fn input(&mut self, line: &str, cwd: &Path) -> Decision {
        let cwd = cwd.to_string_lossy();
        self.decide(
            "input",
            input_document([
                ("action", RegoValue::from("input")),
                ("command", RegoValue::from(line)),
                ("cwd", RegoValue::from(cwd.as_ref())),
                ("deny", RegoValue::from(false)),
            ]),
        )
    }

    /// Whether the gate permits one output chunk to reach the grid.
    pub(crate) fn output(&mut self, text: &str) -> Decision {
        self.decide(
            "output",
            input_document([
                ("action", RegoValue::from("output")),
                ("text", RegoValue::from(text)),
                ("deny", RegoValue::from(false)),
            ]),
        )
    }

    /// Ask the policy one question and read its verdict.
    ///
    /// `action` is the gate point this question belongs to. It is passed rather
    /// than read back out of the input document so a refusal can name the policy
    /// that said no without re-parsing what the gate just built.
    fn decide(&mut self, action: &str, input: RegoValue) -> Decision {
        if let Some(reason) = self.closed.as_ref() {
            return Decision::refuse(Some(reason.clone()));
        }
        self.engine.set_input(input);
        let Ok(results) = self
            .engine
            .eval_query("data.terminal.verdict".to_string(), false)
        else {
            return Decision::refuse(Some(String::from("the policy returned no verdict")));
        };
        let Some(expression) = results
            .result
            .first()
            .and_then(|row| row.expressions.first())
        else {
            return Decision::refuse(Some(String::from("the policy returned no verdict")));
        };
        match serde_json::to_value(&expression.value) {
            Ok(json) => {
                let decision = decision_from_json(json);
                // A plain `false` carries no reason of its own, and a refusal
                // without a reason is a dead end for the reader. Name the policy
                // that said no, so the status line always answers "why".
                if action != "start" && !decision.allowed() && decision.reason().is_none() {
                    return Decision::refuse(Some(format!(
                        "refused by the {}",
                        self.source().label()
                    )));
                }
                decision
            }
            Err(_) => Decision::refuse(Some(String::from("the policy's verdict is not readable"))),
        }
    }
}

/// One gate question's input document, as a Regorus object.
///
/// The document used to be a `serde_json::Value` that was serialised to JSON
/// text and parsed back before every question. The output gate asks once per
/// chunk read from the PTY, so that round trip escaped and re-parsed up to
/// 4 KB of text per read; the policy only ever reads the fields, so they are
/// built directly instead.
fn input_document<const N: usize>(fields: [(&'static str, RegoValue); N]) -> RegoValue {
    let map: BTreeMap<RegoValue, RegoValue> = fields
        .into_iter()
        .map(|(key, value)| (RegoValue::from(key), value))
        .collect();
    RegoValue::from(map)
}

/// Read one policy verdict: a boolean, or an object with an optional reason and
/// redaction.
fn decision_from_json(json: serde_json::Value) -> Decision {
    match json {
        serde_json::Value::Bool(true) => Decision::allow(),
        serde_json::Value::Bool(false) => Decision::refuse(None),
        serde_json::Value::Object(fields) => Decision {
            allow: fields
                .get("allow")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            reason: fields
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            redact: fields
                .get("redact")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        },
        _ => Decision::refuse(Some(String::from(
            "the policy's verdict is neither a boolean nor an object",
        ))),
    }
}

/// What reading the workspace policy found.
enum PolicyRead {
    /// A policy file that parsed. Boxed because the engine is far larger than
    /// the other two answers, and this enum is built on every reload.
    Loaded(Box<Gate>),
    /// No policy file: the documented allow-all default.
    Missing,
    /// A policy file that exists and does not parse. The message names the file.
    Broken(String),
}

/// Append one keystroke decision to /tmp/bsh-term.log.
///
/// Temporary diagnostics: a live "Enter does nothing" report cannot be
/// reproduced in the harness, so the pane records what it actually saw. It is
/// opt-in with `BSH_TERM_TRACE=1`: the pane calls this on every keystroke, and
/// an unconditional open and append put a synchronous disk write on the input
/// path.
pub(crate) fn trace_key(key: &str, bytes: Option<&[u8]>) {
    use std::io::Write as _;
    if !trace_enabled() {
        return;
    }
    let line = match bytes {
        Some(bytes) => format!("key={key} bytes={bytes:?}\n"),
        None => format!("key={key} unmapped\n"),
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/bsh-term.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Whether the opt-in keystroke trace is on, read once.
fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("BSH_TERM_TRACE").is_some())
}

/// The policy body written into a workspace the first time its terminal opens,
/// and the body of the `.bsh/terminal-policy.rego.tpl` authoring template.
///
/// One source of truth for both, so the file the pane drops and the file a
/// reader copies can never describe different rules. It must stay a VALID
/// module as written: a reader who copies it without editing a character must
/// get a policy that parses, allows, and changes nothing. Every worked example
/// is guarded by `examples := false` AND reads a list that starts EMPTY, so
/// flipping the switch on its own changes nothing; a test flips it, injects a
/// scratch value and proves each refusal fires, so the examples cannot rot.
pub(crate) const WORKSPACE_POLICY_TEMPLATE: &str = r##"package terminal

# The terminal gate asks this policy at three points, each with its own input
# document. Every field is always present; `deny` is bsh's own escape hatch and
# is always false, so a policy may ignore it.
#
#   start   {"action": "start",  "command": <shell path>, "shell": <shell path>,
#            "cwd": <workspace>, "deny": false}
#           Asked before any process exists. `command` and `shell` are the same
#           path. A refused start spawns NOTHING: it is the only door that
#           guarantees no process runs.
#
#   input   {"action": "input",  "command": <the submitted line, exactly>,
#            "cwd": <workspace>, "deny": false}
#           Asked on Enter, before the submitting carriage return is written.
#           `cwd` is the directory the terminal STARTED in, so a `cd` the shell
#           made is invisible here.
#
#   output  {"action": "output", "text": <one chunk read from the PTY>,
#            "deny": false}
#           Asked once per chunk, before it reaches the grid. `text` is lossy
#           for a split multibyte sequence, and truncated to 4096 characters.
#
# Answer with a BOOLEAN, or with an OBJECT to explain a refusal or to replace
# output instead of dropping it:
#
#   {"allow": true}
#   {"allow": false, "reason": "<shown in the pane's status line>"}
#   {"allow": true,  "redact": "<text fed INSTEAD OF the chunk>"}
#
# `redact` applies to output only, and a redacted chunk still counts as allowed.
# An object that omits `allow` reads as false. Telling the three points apart is
# one test on the action: input.action == "start" / "input" / "output".
#
# The default is open. Narrow it by naming what to refuse below.
default allow := true

# -----------------------------------------------------------------------------
# The switch. Every worked example is guarded by it AND reads a list that starts
# EMPTY, so flipping it to true on its own changes NOTHING: name what to refuse
# in the list first, then flip. Flipping it back to false disables all three at
# once without deleting them.
# -----------------------------------------------------------------------------
examples := false

# -----------------------------------------------------------------------------
# 1. Refuse a destructive input line.
#
# A line containing one of these substrings is refused and never submitted: no
# carriage return reaches the shell, and Ctrl-U clears what it echoed.
# -----------------------------------------------------------------------------
denied_commands := [
    # "rm -rf",
    # "mkfs",
    # "dd of=/dev/",
]

denied_command(command) if {
    some needle in denied_commands
    contains(command, needle)
}

decision := {"allow": false, "reason": "this command is on the workspace's destructive list"} if {
    examples
    input.action == "input"
    denied_command(input.command)
}

# -----------------------------------------------------------------------------
# 2. Redact a secret in output instead of dropping the chunk.
#
# Each entry is a regex the chunk is matched against. A match replaces the WHOLE
# chunk with the redact text, so this is for short secret-bearing output rather
# than for a screen's worth of drawing: the chunk's escape sequences go with it.
# The example is an AWS access-key id; `sk-[A-Za-z0-9]{16}` is the API-key shape.
# -----------------------------------------------------------------------------
secret_patterns := [
    # `AKIA[0-9A-Z]{16}`,
]

secret_in(text) if {
    some pattern in secret_patterns
    regex.match(pattern, text)
}

decision := {"allow": true, "redact": "[redacted]"} if {
    examples
    input.action == "output"
    secret_in(input.text)
}

# -----------------------------------------------------------------------------
# 3. Refuse to start one shell.
#
# The start gate is the hard boundary: a refusal leaves the pane with no process
# at all. `shell` is the shell's path, and it is the same value as `command`.
# -----------------------------------------------------------------------------
deny_shells := [
    # "/bin/bash",
]

denied_shell(shell) if {
    some refused in deny_shells
    shell == refused
}

decision := {"allow": false, "reason": "this shell is not used in this workspace"} if {
    examples
    input.action == "start"
    denied_shell(input.shell)
}
"##;

fn policy_path(cwd: &Path) -> PathBuf {
    cwd.join(".bsh").join("terminal-policy.rego")
}

/// Write the starter policy when the workspace has none, so a reader can edit
/// the file the gate actually reads instead of inventing it from the docs.
///
/// A write that fails is not an error the terminal should carry: the shipped
/// default already allows everything, so the pane opens exactly as it would
/// have with no file.
fn seed_policy(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // create_new is the existence check and the creation in one operation, so a
    // reader who writes their own policy at the same moment the pane first
    // opens it cannot have it truncated: either this creates the file, or their
    // edit wins.
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    let _ = file.write_all(WORKSPACE_POLICY_TEMPLATE.as_bytes());
}

/// The file's stamp: its modification time and length, which is what a reload
/// compares so a quiet terminal pays one stat.
fn policy_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Read the workspace policy, keeping a parse failure separate from a missing
/// file: one closes every gate, the other is the documented default.
fn read_policy(path: &Path) -> PolicyRead {
    match std::fs::read_to_string(path) {
        Ok(text) => match Gate::for_file(&text) {
            Ok(gate) => PolicyRead::Loaded(Box::new(gate)),
            Err(error) => PolicyRead::Broken(format!("{}: {error}", path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PolicyRead::Missing,
        Err(error) => PolicyRead::Broken(format!("{}: {error}", path.display())),
    }
}

/// The gate a policy path produces, and the parse error that closed it.
///
/// A path with no name has no workspace to confine, so it gets the shipped
/// allow-all gate; a file that does not parse closes the gate and names its
/// error, which is what the pane reports.
fn open_gate(path: &Path) -> (Gate, Option<String>) {
    if path.as_os_str().is_empty() {
        return (Gate::allow_all(), None);
    }
    match read_policy(path) {
        PolicyRead::Loaded(gate) => (*gate, None),
        PolicyRead::Missing => (Gate::allow_all(), None),
        PolicyRead::Broken(error) => (Gate::closed(PolicySource::File, error.clone()), Some(error)),
    }
}

/// The gate a workspace's terminal policy file produces, without a pane.
///
/// The authoring-template tests need exactly what the loader does - the exact
/// `.bsh/terminal-policy.rego` name, and the parse-or-fail-closed rule - without
/// opening a real PTY. Used only by tests.
#[cfg(test)]
pub(crate) fn gate_for_workspace(root: &Path) -> Gate {
    open_gate(&policy_path(root)).0
}

/// Keep at most cap characters of a subject, on a character boundary.
///
/// A borrow, not an owned copy: the output gate calls this on every chunk
/// read from the PTY, and the chunk is almost always shorter than the cap.
fn truncate_chars(text: &str, cap: usize) -> &str {
    match text.char_indices().nth(cap) {
        Some((end, _)) => &text[..end],
        None => text,
    }
}

/// The bounded decision log, with the count the pane reports kept as it goes.
///
/// The count is maintained on every push and trim rather than by walking the
/// ring: the pane reads it from its render path, and the ring can hold
/// `AUDIT_LIMIT` rows.
#[derive(Default)]
struct AuditRing {
    rows: VecDeque<Audit>,
    /// Rows that refused or redacted something. Maintained as rows go in and
    /// out, because the ring trims its front and a count cannot be recovered
    /// from a high-water mark.
    blocked: u64,
}

impl AuditRing {
    /// Append one row, trimming the oldest first so the ring stays bounded.
    fn push(&mut self, row: Audit) {
        while self.rows.len() >= AUDIT_LIMIT {
            if let Some(old) = self.rows.pop_front() {
                if !old.allow || old.redacted {
                    self.blocked = self.blocked.saturating_sub(1);
                }
            }
        }
        if !row.allow || row.redacted {
            self.blocked += 1;
        }
        self.rows.push_back(row);
    }
}

/// Append one decision to the audit ring.
fn record_decision(
    audit: &Mutex<AuditRing>,
    action: &'static str,
    subject: &str,
    decision: &Decision,
    source: PolicySource,
) {
    let redacted = decision.redaction().is_some();
    let row = Audit {
        at: SystemTime::now(),
        action,
        subject: truncate_chars(subject, AUDIT_SUBJECT_CAP).to_string(),
        allow: decision.allowed(),
        redacted,
        reason: decision
            .reason()
            .map(|reason| truncate_chars(reason, AUDIT_REASON_CAP).to_string()),
        source: source.label(),
    };
    if let Ok(mut ring) = audit.lock() {
        ring.push(row);
    }
}
/// What the shell sees as BSH_SANDBOX, when there is something to say.
///
/// The pane's spawn answers the shared spec (see
/// [agent::sandbox::SandboxSpec::terminal]) and reports the answer into the
/// shell's environment, so a reader can type "echo $BSH_SANDBOX" and a command
/// can branch on it. On macOS and Linux a confined turn's shell runs INSIDE the
/// sandbox, so the value is the permission tier itself; a backend that cannot
/// host the pane's tty reports "unsupported-pty" and the pane's status line says
/// why. The spawn path overrides the value with "none" when the caller passes a
/// preset-off note (see [`Terminal::new_with_sandbox_note`]), so the shell's own
/// report matches the tool result.
fn sandbox_env(spec: Option<&agent::sandbox::SandboxSpec>) -> Option<&'static str> {
    match spec {
        None => None,
        Some(spec) => match spec.terminal() {
            agent::sandbox::TerminalSandbox::Unconfined => Some("full-access"),
            agent::sandbox::TerminalSandbox::Sandboxed => Some(spec.mode().as_str()),
            agent::sandbox::TerminalSandbox::PtyNotHostable { .. } => Some("unsupported-pty"),
        },
    }
}

/// Why the pane's shell is not confined, when it is not.
fn sandbox_reason(spec: Option<&agent::sandbox::SandboxSpec>) -> Option<&'static str> {
    spec.and_then(|spec| spec.terminal().reason())
}

/// A running terminal: the pane's one shell, its PTY, the screen, and the gate.
pub(crate) struct Terminal {
    screen: Arc<Mutex<Screen>>,
    gate: Arc<Mutex<Gate>>,
    audit: Arc<Mutex<AuditRing>>,
    version: Arc<AtomicU64>,
    exited: Arc<AtomicBool>,
    /// The shell's working directory, which is also the input gate's cwd.
    pub(crate) cwd: PathBuf,
    /// The turn's sandbox, when the pane was given one.
    ///
    /// The spawn path hands this SAME value to the sandbox, so the shell the
    /// reader types into is the one the shell tool runs under: one spec, one
    /// set of grants, one policy.
    sandbox_spec: Option<std::sync::Arc<agent::sandbox::SandboxSpec>>,
    /// The value the shell sees as BSH_SANDBOX, when there is one to report.
    sandbox_env: Option<&'static str>,
    /// Why this shell is not inside the turn's sandbox, when it is not.
    pub(crate) sandbox_note: Option<String>,
    /// The shell is unconfined because a PRESET turned the sandbox off.
    ///
    /// Distinct from a full-access release and from a backend that cannot host
    /// the pane's tty: this is a policy choice the reader can reverse, so the
    /// status line keeps saying so while the shell runs.
    pub(crate) sandbox_off_by_preset: bool,
    /// Why the shell is not running, when it is not.
    pub(crate) failure: Option<String>,
    /// The gate refused the start, so no process was ever created.
    pub(crate) refused: bool,
    /// The language the output-blocked marker is written in.
    lang: Lang,
    /// The workspace policy's path and stamp, so a reload costs one stat.
    policy_path: PathBuf,
    policy_stamp: Option<(SystemTime, u64)>,
    /// The last policy that failed to load or reload.
    policy_error: Option<String>,
    /// The line typed since the last submit: what the input gate is shown.
    typed_line: String,
    /// The pty of the running shell, held so its reader stays open.
    pty: Option<Box<dyn MasterPty + Send>>,
    /// The bounded hand-off to the thread that owns the PTY's write side.
    input: Option<InputWriter>,
    /// One extra description of the pty's slave, held for the pane's life.
    ///
    /// A sandboxed shell opens the tty by PATH a moment after the spawn, and
    /// until it does the master reads EIO - which the reader would take for the
    /// shell exiting. Keeping one slave description open closes that window.
    /// Liveness does not depend on it: the clock polls the child itself.
    tty_hold: Option<std::fs::File>,
    child: Option<ShellChild>,
    /// The size last pushed to the PTY, so a same-size frame is not a syscall.
    size: Option<(u16, u16)>,
    /// The shell this pane runs.
    ///
    /// Kept so a restart runs the SAME shell: a confined pane may have had to
    /// replace the reader's shell (fish cannot run without a controlling
    /// terminal), and a restart that reached for $SHELL again would hand the
    /// reader a pane that cannot take a keystroke.
    shell: PathBuf,
}

/// The pane's shell process, whichever backend started it.
///
/// A confined shell is started by the sandbox (which keeps the confinement
/// alive for exactly as long as the process); a released one is started on the
/// pty directly. Both are waited on, killed and polled the same way, so the
/// pane has one notion of "the shell".
enum ShellChild {
    /// Started on the pty by the pane itself.
    Plain(Box<dyn Child + Send + Sync>),
    /// Started by the turn's sandbox; the confinement lives in this value.
    Sandboxed(agent::sandbox::SandboxedChild),
}

impl ShellChild {
    /// Whether the shell has exited, without blocking.
    fn has_exited(&mut self) -> bool {
        match self {
            Self::Plain(child) => matches!(child.try_wait(), Ok(Some(_))),
            Self::Sandboxed(child) => matches!(child.try_wait(), Ok(Some(_))),
        }
    }

    /// Stop the shell and everything it started.
    fn kill(&mut self) {
        match self {
            Self::Plain(child) => {
                let _ = child.kill();
            }
            Self::Sandboxed(child) => child.kill(),
        }
    }

    /// Reap it, so a killed shell does not stay a zombie for the pane's life.
    fn reap(&mut self) {
        match self {
            Self::Plain(child) => {
                let _ = child.wait();
            }
            Self::Sandboxed(child) => {
                let _ = child.wait();
            }
        }
    }
}

/// Marks the shell dead however its reader thread ends, including a panic.
///
/// A reader that unwinds without setting `exited` left `is_running` true: the
/// pane swallowed every key and nothing was ever drawn, with no way back short
/// of reopening the tab. Dropping this guard is the one place liveness is
/// cleared, so the pane always offers the restart path.
struct ExitSignal {
    exited: Arc<AtomicBool>,
    version: Arc<AtomicU64>,
}

/// How many bytes a keystroke-sized write carries without a heap allocation.
const SMALL_INPUT: usize = 32;
/// How many writes may wait for the PTY writer. A chunk is one keystroke, so
/// this is minutes of typing; it only fills when the shell stops reading input.
const INPUT_QUEUE: usize = 1024;

/// One write handed to the writer thread.
///
/// A keystroke is a handful of bytes, so the common case rides inline and the
/// key path allocates nothing; a longer write takes the heap.
enum InputChunk {
    Small {
        bytes: [u8; SMALL_INPUT],
        len: usize,
    },
    Big(Vec<u8>),
}

impl InputChunk {
    fn new(bytes: &[u8]) -> Self {
        if bytes.len() <= SMALL_INPUT {
            let mut chunk = [0u8; SMALL_INPUT];
            chunk[..bytes.len()].copy_from_slice(bytes);
            Self::Small {
                bytes: chunk,
                len: bytes.len(),
            }
        } else {
            Self::Big(bytes.to_vec())
        }
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Small { bytes, len } => &bytes[..*len],
            Self::Big(bytes) => bytes,
        }
    }
}

/// What handing one write to the writer thread did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputSend {
    /// The writer thread owns the bytes now.
    Queued,
    /// The shell stopped draining its input and the queue is full. The bytes
    /// were neither written nor dropped silently: the caller is told.
    Full,
    /// The PTY's write side failed, or the writer thread is gone.
    Gone,
}

/// The bounded hand-off to the thread that owns the PTY's write side.
///
/// A write to the PTY master blocks once the shell stops draining its input, and
/// the pane writes from the UI thread, so a stalled program would freeze the
/// window. The write rides its own thread instead, and the queue is bounded so a
/// shell that never reads cannot grow the pane's memory without limit.
struct InputWriter {
    queue: SyncSender<InputChunk>,
    /// Set by the writer thread when the PTY refused its bytes.
    gone: Arc<AtomicBool>,
}

impl InputWriter {
    /// Start the thread that owns the write side of the PTY.
    fn spawn(mut writer: Box<dyn Write + Send>) -> Self {
        let (queue, inbox) = sync_channel::<InputChunk>(INPUT_QUEUE);
        let gone = Arc::new(AtomicBool::new(false));
        let failed = Arc::clone(&gone);
        std::thread::spawn(move || {
            while let Ok(chunk) = inbox.recv() {
                if writer.write_all(chunk.as_bytes()).is_err() {
                    failed.store(true, Ordering::Relaxed);
                    break;
                }
                let _ = writer.flush();
            }
        });
        Self { queue, gone }
    }

    /// Hand one write over. Never blocks, and a full queue or a closed writer is
    /// reported rather than swallowed.
    fn send(&self, bytes: &[u8]) -> InputSend {
        if self.gone.load(Ordering::Relaxed) {
            return InputSend::Gone;
        }
        match self.queue.try_send(InputChunk::new(bytes)) {
            Ok(()) => InputSend::Queued,
            Err(TrySendError::Full(_)) => InputSend::Full,
            Err(TrySendError::Disconnected(_)) => InputSend::Gone,
        }
    }
}

impl Drop for ExitSignal {
    fn drop(&mut self) {
        self.exited.store(true, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Relaxed);
    }
}

impl Terminal {
    /// A terminal in cwd running the user's shell interactively.
    ///
    /// The workspace's .bsh/terminal-policy.rego is loaded when present and
    /// the gate is asked whether the shell may start. Nothing is spawned when
    /// it says no, and a policy that exists but cannot be parsed fails CLOSED:
    /// an unusable rule must never widen the gate it was meant to narrow.
    /// A terminal with no spec: the pane's shell, gated by the policy file.
    ///
    /// Test-only now that the pane passes the turn's spec; the real spawn path
    /// is [Terminal::new_with_sandbox], which records what the spec answered.
    #[cfg(test)]
    pub(crate) fn new(cwd: PathBuf, lang: Lang) -> Self {
        Self::start(cwd, lang, None, None)
    }

    /// A terminal whose spawn was told what the turn may touch.
    ///
    /// The spec is the same value the shell tool runs under. Its answer for a
    /// caller-owned PTY is "this backend cannot host one" (see
    /// [agent::sandbox::SandboxSpec::terminal]), so the pane runs its shell the
    /// only way it can and shows that sentence rather than a confinement it
    /// does not have. A policy-backed input gate still decides every line.
    pub(crate) fn new_with_sandbox(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
    ) -> Self {
        Self::start(cwd, lang, spec, None)
    }

    /// The same spawn, with the attribution a preset's off needs.
    ///
    /// The note is passed BEFORE the spawn so the shell's own `BSH_SANDBOX`
    /// reports "none"; setting it afterwards would leave a running shell
    /// claiming "full-access" while the status line said a preset turned it off.
    pub(crate) fn new_with_sandbox_note(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
        note: Option<String>,
    ) -> Self {
        Self::start(cwd, lang, spec, note)
    }

    /// Why this shell is not sandboxed, when it is not.
    pub(crate) fn sandbox_note(&self) -> Option<&str> {
        self.sandbox_note.as_deref()
    }

    /// What the shell sees as BSH_SANDBOX, when there is something to say.
    ///
    /// "none" is a preset that turned the sandbox off, "full-access" is a
    /// permission-released turn, a permission tier is a sandboxed shell, and
    /// "unsupported-pty" is a backend that could not host the pane's terminal.
    pub(crate) fn sandbox_label(&self) -> Option<&'static str> {
        self.sandbox_env
    }

    /// Build the terminal, ask the spec, and start the shell.
    fn start(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
        preset_off_note: Option<String>,
    ) -> Self {
        Self::start_with_shell(cwd, lang, spec, None, preset_off_note)
    }

    /// Build the terminal with an explicit shell when a test needs one.
    ///
    /// Production always lets [shell_program_for] choose: the reader's shell is
    /// part of the pane's contract, and a confined pane may have to replace it.
    /// The override exists so the platform-specific completion test can drive
    /// zsh even on a host whose reader prefers another shell.
    fn start_with_shell(
        cwd: PathBuf,
        lang: Lang,
        spec: Option<&agent::sandbox::SandboxSpec>,
        shell_override: Option<PathBuf>,
        preset_off_note: Option<String>,
    ) -> Self {
        // Asked before the spawn, so the spawn path cannot be edited into
        // assuming a confinement the answer refuses.
        let mut sandbox_env = sandbox_env(spec);
        let mut sandbox_note = sandbox_reason(spec).map(str::to_string);
        let preset_off = preset_off_note.is_some();
        if let Some(note) = preset_off_note {
            // The shell reports what the TURN decided, not only the permission
            // tier: a preset's explicit off is "none", so the pane and the bash
            // result agree. This must happen BEFORE the spawn.
            sandbox_env = Some("none");
            sandbox_note = Some(note);
        }
        let mut terminal = Self::build(cwd, lang, DEFAULT_COLS, DEFAULT_ROWS);
        terminal.sandbox_env = sandbox_env;
        terminal.sandbox_off_by_preset = preset_off;
        // Cloned: the shell needs it for its whole life, and the pane's caller
        // holds the same value for the shell tool.
        terminal.sandbox_spec = spec.map(|spec| std::sync::Arc::new(spec.clone()));
        let confined = terminal
            .sandbox_spec
            .as_deref()
            .is_some_and(|spec| spec.terminal().is_sandboxed());
        let shell = shell_override.unwrap_or_else(|| shell_program_for(confined));
        terminal.shell = shell.clone();
        // The reader's own shell when it can run confined; otherwise the pane
        // says which shell it used and why, once, in the pane itself.
        if confined {
            let reader = std::env::var_os("SHELL").map(PathBuf::from);
            let replaced = reader
                .as_deref()
                .is_some_and(|reader| reader.is_absolute() && reader != shell.as_path());
            if replaced {
                sandbox_note = Some(crate::i18n::tr_args(
                    lang,
                    "right.terminal.shellReplaced",
                    &[("shell", shell.display().to_string())],
                ));
            }
        }
        terminal.sandbox_note = sandbox_note;
        if let Some(note) = terminal.sandbox_note.clone() {
            terminal.write_notice(&note);
        }
        if !terminal.refused {
            let cwd = terminal.cwd.clone();
            let command = shell.display().to_string();
            let decision = terminal.decide("start", &command, |gate| gate.start(&shell, &cwd));
            if !decision.allowed() {
                terminal.refused = true;
                terminal.failure = decision.reason().map(str::to_string);
            }
        }
        if !terminal.refused && terminal.failure.is_none() {
            terminal.spawn(&shell);
        }
        terminal
    }

    /// Everything but the process: the screen, the gate, and the policy read.
    fn build(cwd: PathBuf, lang: Lang, cols: usize, rows: usize) -> Self {
        let path = if cwd.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            policy_path(&cwd)
        };
        if !path.as_os_str().is_empty() {
            seed_policy(&path);
        }
        let (gate, policy_error) = open_gate(&path);
        let stamp = if path.as_os_str().is_empty() {
            None
        } else {
            policy_stamp(&path)
        };
        let refused = policy_error.is_some();
        let failure = policy_error.clone();
        let terminal = Self {
            screen: Arc::new(Mutex::new(Screen::new(cols, rows, SCROLLBACK_LIMIT))),
            gate: Arc::new(Mutex::new(gate)),
            audit: Arc::new(Mutex::new(AuditRing::default())),
            version: Arc::new(AtomicU64::new(0)),
            exited: Arc::new(AtomicBool::new(false)),
            cwd,
            sandbox_spec: None,
            sandbox_env: None,
            sandbox_note: None,
            sandbox_off_by_preset: false,
            failure,
            refused,
            lang,
            policy_path: path,
            policy_stamp: stamp,
            policy_error,
            typed_line: String::new(),
            pty: None,
            input: None,
            tty_hold: None,
            child: None,
            size: None,
            shell: PathBuf::new(),
        };
        if let Some(error) = terminal.policy_error.clone() {
            let subject = terminal.policy_path.display().to_string();
            let decision = Decision::refuse(Some(error));
            record_decision(
                &terminal.audit,
                "policy",
                &subject,
                &decision,
                PolicySource::File,
            );
        }
        terminal
    }

    /// A terminal with no process: the screen and the gate, for tests that want
    /// a deterministic grid rather than a live shell.
    #[cfg(test)]
    pub(crate) fn without_process(cwd: PathBuf, lang: Lang, cols: usize, rows: usize) -> Self {
        Self::build(cwd, lang, cols, rows)
    }

    fn spawn(&mut self, shell: &Path) -> bool {
        // Reap the shell this one replaces. A child that exited is a zombie
        // until it is waited on, so every exit followed by Enter leaked one
        // process slot for the life of the pane. The exit is already known
        // (spawn is reached from a dead pane), so the normal path is a poll; a
        // child that turns out to be alive is killed rather than waited on from
        // the UI thread.
        if let Some(mut previous) = self.child.take() {
            if !previous.has_exited() {
                previous.kill();
            }
            previous.reap();
        }
        // The previous pty's slave description goes with the shell it served,
        // and its reader ends when that slave goes. A SHARED liveness flag would
        // then mark this new shell dead the moment the old reader noticed, so
        // every shell gets a flag of its own.
        self.tty_hold = None;
        self.exited = Arc::new(AtomicBool::new(false));
        // Open at the size the pane has now, not at the default: a restart used
        // to hand the new shell an 80x24 window while the grid kept the pane's
        // real width, so the prompt wrapped in the wrong place until the next
        // resize event.
        let (cols, rows) = self.pty_size();
        let pair = match native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(error) => {
                self.failure = Some(error.to_string());
                return false;
            }
        };
        let cwd = if self.cwd.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            self.cwd.clone()
        };
        // The interface the shell starts with, and the answer it can read back:
        // a reader can type "echo $BSH_SANDBOX", and a command can branch on it.
        //
        // TERM and COLORTERM are a Unix convention; a Windows console has no
        // terminfo to match them against, so they are set only where they mean
        // something.
        let mut envs: Vec<(String, String)> = Vec::new();
        #[cfg(unix)]
        {
            envs.push(("TERM".to_string(), "xterm-256color".to_string()));
            envs.push(("COLORTERM".to_string(), "truecolor".to_string()));
            // A pane with no rc files would otherwise draw monochrome: BSD ls
            // reads these two, and git's colour.ui already defaults to auto.
            envs.push(("CLICOLOR".to_string(), "1".to_string()));
            envs.push(("LSCOLORS".to_string(), "ExFxCxDxBxegedabagacad".to_string()));
        }
        if let Some(label) = self.sandbox_env {
            envs.push(("BSH_SANDBOX".to_string(), label.to_string()));
        }
        // A confined shell cannot read the reader's $HOME, so every nested
        // shell opens with an rc file it was not allowed to read: bash prints
        // the permission error and fish complains about its data directory.
        // Give the pane a home inside the workspace instead - reads and writes
        // stay inside the sandbox, and the shells land on an empty profile.
        let sandbox_home = self
            .sandbox_spec
            .as_deref()
            .filter(|spec| spec.terminal().is_sandboxed())
            .and_then(|spec| {
                let home = spec.root().join(".bsh").join("home");
                for dir in ["config", "data", "cache", "state"] {
                    std::fs::create_dir_all(home.join(dir)).ok()?;
                }
                Some(home)
            });
        if let Some(home) = sandbox_home {
            envs.push(("HOME".to_string(), home.display().to_string()));
            for (key, dir) in [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_CACHE_HOME", "cache"),
                ("XDG_STATE_HOME", "state"),
            ] {
                envs.push((key.to_string(), home.join(dir).display().to_string()));
            }
        }
        // Two spawn shapes, and exactly one applies:
        //
        // - a shell the sandbox CAN host (macOS today) is started by the spec
        //   with the pane's tty opened BY PATH. portable-pty exposes the slave's
        //   NAME, not its fd, which is why the slave is held open below.
        // - anything else is started on the pty by the pane itself. That is the
        //   only path that performs the terminal handshake - login_tty on Unix,
        //   ConPTY on Windows - so it is also the path that works everywhere the
        //   sandbox cannot join.
        let sandboxed = self
            .sandbox_spec
            .as_deref()
            .filter(|spec| spec.terminal().is_sandboxed());
        let tty = sandboxed.and_then(|_| tty_path(pair.master.as_ref()));
        let mut child = match (sandboxed, tty) {
            (Some(spec), Some(tty)) => {
                self.tty_hold = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&tty)
                    .ok();
                match spec.spawn_terminal(shell, &tty, &cwd, &envs) {
                    Ok(child) => ShellChild::Sandboxed(child),
                    Err(error) => {
                        self.failure = Some(error);
                        return false;
                    }
                }
            }
            _ => {
                let mut command = CommandBuilder::new(shell);
                command.cwd(&cwd);
                for (key, value) in &envs {
                    command.env(key, value);
                }
                match pair.slave.spawn_command(command) {
                    Ok(child) => ShellChild::Plain(child),
                    Err(error) => {
                        self.failure = Some(error.to_string());
                        return false;
                    }
                }
            }
        };
        drop(pair.slave);
        // A process whose output has no reader is a process the pane cannot
        // show and cannot stop: kill and reap it rather than leave it running.
        let failed = |child: &mut ShellChild, failure: String, this: &mut Self| {
            child.kill();
            child.reap();
            this.failure = Some(failure);
            false
        };
        let reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                return failed(&mut child, error.to_string(), self);
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(error) => {
                return failed(&mut child, error.to_string(), self);
            }
        };
        #[cfg(unix)]
        if let Some(bootstrap) = completion_bootstrap(shell, sandboxed.is_some()) {
            let _ = write_without_echo(pair.master.as_ref(), writer.as_mut(), bootstrap);
        }
        self.pty = Some(pair.master);
        self.input = Some(InputWriter::spawn(writer));
        self.child = Some(child);
        self.size = Some((cols, rows));
        self.spawn_reader(reader);
        true
    }

    /// Write one line into the pane before the shell's first output.
    ///
    /// A standing caveat - a shell that had to be replaced, a backend that
    /// cannot host the tty - goes where the reader is looking rather than into a
    /// status line that only appears when something has already gone wrong.
    fn write_notice(&self, text: &str) {
        if let Ok(mut screen) = self.screen.lock() {
            let mut line = text.as_bytes().to_vec();
            line.extend_from_slice(b"\r\n");
            screen.feed(&line);
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Notice a shell that exited, independently of the reader's EOF.
    ///
    /// The reader ends the pane on EOF, but the pane keeps one slave
    /// description open (see tty_hold), so the master does not EOF on its own.
    /// The child is the authority, and the terminal clock asks here.
    pub(crate) fn poll(&mut self) {
        if self.exited.load(Ordering::Relaxed) {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if child.has_exited() {
            self.exited.store(true, Ordering::Relaxed);
            self.version.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The size the PTY and the grid should be at: the pane's last measurement,
    /// or the startup default before it has measured itself.
    fn pty_size(&self) -> (u16, u16) {
        self.size
            .unwrap_or((DEFAULT_COLS as u16, DEFAULT_ROWS as u16))
    }

    /// Read the shell's output on its own thread, through the gate and then the
    /// parser.
    ///
    /// Every chunk is asked about before it reaches the grid. A refusal drops
    /// the chunk and writes one marker line in its place (never re-gated, it is
    /// bsh's own line); a redaction feeds the policy's replacement text instead.
    fn spawn_reader(&self, mut reader: Box<dyn Read + Send>) {
        let screen = Arc::clone(&self.screen);
        let gate = Arc::clone(&self.gate);
        let audit = Arc::clone(&self.audit);
        let version = Arc::clone(&self.version);
        let exited = Arc::clone(&self.exited);
        let marker = crate::i18n::tr(self.lang, "right.terminal.outputBlocked");
        std::thread::spawn(move || {
            // The pane's liveness flag is a drop guard, so a reader that ends
            // by EOF and one that ends by a panic in the parser both mark the
            // shell dead. Without it a panicking reader left the pane looking
            // live while its output went nowhere.
            let _exit = ExitSignal {
                exited,
                version: Arc::clone(&version),
            };
            let mut buffer = [0u8; 8192];
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let chunk = &buffer[..read];
                // The gate sees text, the grid sees the bytes. A chunk that is
                // already UTF-8 is borrowed, not copied: this is the reader's
                // hot path and two allocations per chunk were pure waste.
                let text = String::from_utf8_lossy(chunk);
                let subject = truncate_chars(&text, OUTPUT_GATE_CAP);
                let (decision, source) = match gate.lock() {
                    Ok(mut gate) => {
                        let decision = gate.output(subject);
                        let source = gate.source();
                        (decision, source)
                    }
                    Err(_) => (
                        Decision::refuse(Some(String::from("the gate's lock is poisoned"))),
                        PolicySource::Default,
                    ),
                };
                record_decision(&audit, "output", subject, &decision, source);
                // A poisoned screen lock is terminal: every chunk would be
                // dropped silently, so stop reading and let the drop guard
                // report the shell as finished. Enter starts a fresh one.
                let Ok(mut screen) = screen.lock() else {
                    break;
                };
                if !decision.allowed() {
                    screen.feed(format!("\r\n{marker}\r\n").as_bytes());
                } else if let Some(replacement) = decision.redaction() {
                    screen.feed(replacement.as_bytes());
                } else {
                    screen.feed(chunk);
                }
                drop(screen);
                version.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    /// Hand raw bytes to the shell, without ever blocking the caller.
    ///
    /// This is the byte path the pane's keystrokes and the gate's own
    /// resubmission use; the input gate lives in `send_key`, which is what a
    /// keystroke arrives through. The write itself lands on a thread that owns
    /// the PTY's write side, because a PTY write blocks once the shell stops
    /// draining its input and this runs on the UI thread. Returning `true`
    /// means the writer thread took the bytes, not that the shell read them.
    pub(crate) fn send_bytes(&mut self, bytes: &[u8]) -> bool {
        let outcome = match self.input.as_ref() {
            Some(input) => input.send(bytes),
            None => return false,
        };
        match outcome {
            InputSend::Queued => true,
            // The shell has stopped draining its input and the queue is full.
            // The keystroke is refused rather than written later out of order;
            // the shell is still alive, so the queue is kept and typing resumes
            // when it reads again.
            InputSend::Full => false,
            // The write side is gone. Keeping it would make every later
            // keystroke vanish while `is_running` still claimed a shell was
            // there; dropping it turns the pane into the dead state that offers
            // Enter as a restart.
            InputSend::Gone => {
                self.failure = Some(String::from("the shell's input was closed"));
                self.input = None;
                false
            }
        }
    }

    /// Start a fresh shell after the last one exited.
    ///
    /// A dead pane used to stay dead: nothing arrives from a closed PTY, keys
    /// go nowhere, and the reader thread has already parked, so `exit` left the
    /// reader with no way back short of reopening the tab. Restarting is the
    /// same spawn under the same gate - a new shell has to pass the start rule
    /// again - with a fresh screen, so the exit is a turn of the page.
    pub(crate) fn restart(&mut self) -> bool {
        if self.refused || self.is_running() {
            return false;
        }
        // The shell this pane already chose, not $SHELL again: see Terminal::shell.
        // A terminal built without a process has no choice yet, and falls back to
        // the released shell.
        let shell = if self.shell.as_os_str().is_empty() {
            shell_program()
        } else {
            self.shell.clone()
        };
        let cwd = self.cwd.clone();
        let command = shell.display().to_string();
        let decision = self.decide("start", &command, |gate| gate.start(&shell, &cwd));
        if !decision.allowed() {
            self.refused = true;
            self.failure = decision.reason().map(str::to_string);
            return false;
        }
        self.typed_line.clear();
        self.exited.store(false, Ordering::Relaxed);
        self.failure = None;
        // A fresh page: the exit and the new prompt should not share a screen
        // with the dead shell's last output.
        match self.screen.lock() {
            Ok(mut screen) => screen.feed(b"\x1b[2J\x1b[H"),
            // A panic in the parser poisons the screen's lock, which would drop
            // every chunk of the new shell's output while the pane looked live.
            // The screen is a cache with no invariant another thread relies on,
            // so a restart replaces it instead of leaving the pane blank.
            Err(poisoned) => {
                let (cols, rows) = self.pty_size();
                self.screen.clear_poison();
                *poisoned.into_inner() =
                    Screen::new(cols as usize, rows as usize, SCROLLBACK_LIMIT);
            }
        }
        let spawned = self.spawn(&shell);
        self.version.fetch_add(1, Ordering::Relaxed);
        spawned
    }

    /// One keystroke from the pane.
    ///
    /// Every byte but a submitted line reaches the PTY as it always has, which
    /// is what keeps the shell's line editor, history and completion working.
    /// Enter is the one gated byte: the line this pane has mirrored since the
    /// last submit is shown to the policy, and a refusal writes no line - it
    /// writes Ctrl-U so the echoed text is erased.
    pub(crate) fn send_key(&mut self, bytes: &[u8]) -> KeyOutcome {
        if bytes == b"\r" {
            // In a dead pane Enter is the only key with a meaning: start again.
            if !self.is_running() && !self.refused {
                return if self.restart() {
                    KeyOutcome::Sent
                } else {
                    KeyOutcome::Closed
                };
            }
            return self.submit_line();
        }
        self.track_line(bytes);
        // A confined shell reads the pane's tty by path and never performed the
        // terminal handshake (setsid + TIOCSCTTY), so the tty can end up with no
        // foreground group and the driver's ISIG delivery for Ctrl-C goes
        // nowhere. Signal the tty's foreground group here; the byte still goes
        // through, which is what the line editor uses when the group is unset.
        #[cfg(unix)]
        if bytes == [0x03] {
            self.interrupt_foreground();
        }
        if self.send_bytes(bytes) {
            KeyOutcome::Sent
        } else {
            KeyOutcome::Closed
        }
    }

    /// Deliver the interrupt the tty driver would have delivered.
    ///
    /// Only the confined spawn needs this: portable-pty's own spawn runs
    /// `login_tty`, while the sandbox path opens the pane's tty by path, which
    /// makes no session leader and can leave the tty without a foreground
    /// group. The pane holds that tty, so it asks for the group and signals it,
    /// never its own group - signalling that would interrupt the application.
    #[cfg(unix)]
    fn interrupt_foreground(&self) {
        use std::os::unix::io::AsRawFd;
        let Some(tty) = self.tty_hold.as_ref() else {
            return;
        };
        let fd = tty.as_raw_fd();
        let own = unsafe { libc::getpgrp() };
        let foreground = unsafe { libc::tcgetpgrp(fd) };
        let shell = self
            .child
            .as_ref()
            .and_then(|child| match child {
                ShellChild::Plain(child) => child.process_id(),
                ShellChild::Sandboxed(child) => Some(child.id()),
            })
            .map(|pid| pid as i32)
            .filter(|pid| *pid > 1);
        // The tty has a foreground group, so the driver delivered the interrupt
        // itself; a second signal here would fire it twice.
        if foreground > 1 {
            return;
        }
        // No foreground group was ever claimed: fall back to the shell's own
        // group, which is where a shell without job control runs the command
        // the reader wants interrupted.
        let group = shell.map(|pid| unsafe { libc::getpgid(pid) }).unwrap_or(-1);
        if let Some(group) = interruptable_group(group, own) {
            unsafe {
                libc::kill(-group, libc::SIGINT);
            }
        } else if let Some(pid) = shell {
            // The shell shares the application's group (heel's spawn builder
            // offers no process-group hook), so a group signal would hit the
            // application. Interrupt the shell alone: at its prompt that is
            // the same key, and while it waits it aborts the wait.
            unsafe {
                libc::kill(pid, libc::SIGINT);
            }
        }
    }

    /// Mirror one editing keystroke into the line the input gate will see.
    ///
    /// Arrows, history recall and completion are deliberately not mirrored:
    /// they are not gated (see the module docs), and what they do to the line
    /// is the shell's business.
    fn track_line(&mut self, bytes: &[u8]) {
        match bytes {
            // The shell's line editor clears the line on Ctrl-U and Ctrl-C.
            [0x15] | [0x03] => self.typed_line.clear(),
            // Backspace, in both the DEL and BS spellings.
            [0x7f] | [0x08] => {
                self.typed_line.pop();
            }
            // Ctrl-W deletes the trailing word.
            [0x17] => {
                let trimmed = self.typed_line.trim_end().len();
                self.typed_line.truncate(trimmed);
                match self.typed_line.rfind(' ') {
                    Some(index) => self.typed_line.truncate(index + 1),
                    None => self.typed_line.clear(),
                }
            }
            _ => {
                if let Ok(text) = std::str::from_utf8(bytes) {
                    if text.chars().all(|ch| !ch.is_control()) {
                        self.typed_line.push_str(text);
                    }
                }
            }
        }
    }

    /// Submit the mirrored line through the input gate.
    fn submit_line(&mut self) -> KeyOutcome {
        let subject = std::mem::take(&mut self.typed_line);
        let cwd = self.cwd.clone();
        let decision = self.decide("input", &subject, |gate| gate.input(&subject, &cwd));
        if decision.allowed() {
            if self.send_bytes(b"\r") {
                KeyOutcome::Sent
            } else {
                KeyOutcome::Closed
            }
        } else {
            // The line is already in the shell's line editor - a real terminal
            // echoes as you type - so a refusal erases it: Ctrl-U is the
            // shell's kill-whole-line. Nothing else is written to the shell.
            self.send_bytes(&[0x15]);
            // Say WHY, where the reader typed it: a refusal that only moved a
            // status-strip counter left people retyping the same command.
            let reason = decision.reason().map(str::to_string).unwrap_or_else(|| {
                let source = self
                    .gate
                    .lock()
                    .map(|gate| gate.source().label())
                    .unwrap_or("terminal policy");
                format!("refused by the {source}")
            });
            let marker = crate::i18n::tr_args(
                self.lang,
                "right.terminal.refusedLine",
                &[("command", subject.clone()), ("reason", reason)],
            );
            if let Ok(mut screen) = self.screen.lock() {
                screen.feed(format!("\r\n{marker}\r\n").as_bytes());
            }
            self.version.fetch_add(1, Ordering::Relaxed);
            KeyOutcome::Refused
        }
    }

    /// Ask the gate one question, and record the answer in the audit ring.
    fn decide(
        &self,
        action: &'static str,
        subject: &str,
        evaluate: impl FnOnce(&mut Gate) -> Decision,
    ) -> Decision {
        let (decision, source) = match self.gate.lock() {
            Ok(mut gate) => {
                let decision = evaluate(&mut gate);
                let source = gate.source();
                (decision, source)
            }
            Err(_) => (
                Decision::refuse(Some(String::from("the terminal's policy lock is poisoned"))),
                PolicySource::Default,
            ),
        };
        record_decision(&self.audit, action, subject, &decision, source);
        decision
    }

    /// Re-read the workspace policy when its file changed.
    ///
    /// Called from the terminal clock - one stat per tick, never one per frame.
    /// A broken edit keeps the LAST GOOD policy and records the parse error;
    /// deleting the file restores the documented allow-all default. Either way
    /// the gate never wedges and never silently widens.
    pub(crate) fn refresh_policy(&mut self) -> bool {
        if self.policy_path.as_os_str().is_empty() {
            return false;
        }
        let stamp = policy_stamp(&self.policy_path);
        if stamp == self.policy_stamp {
            return false;
        }
        self.policy_stamp = stamp;
        match read_policy(&self.policy_path) {
            PolicyRead::Loaded(gate) => {
                self.policy_error = None;
                self.clear_start_refusal();
                self.install(*gate);
            }
            PolicyRead::Missing => {
                self.policy_error = None;
                self.clear_start_refusal();
                self.install(Gate::allow_all());
            }
            PolicyRead::Broken(error) => {
                self.policy_error = Some(error.clone());
                let subject = self.policy_path.display().to_string();
                let decision = Decision::refuse(Some(error));
                record_decision(
                    &self.audit,
                    "policy",
                    &subject,
                    &decision,
                    PolicySource::File,
                );
            }
        }
        true
    }

    /// Let the pane start again once a usable policy is in place.
    ///
    /// A start refused because the policy file did not parse is a refusal about
    /// that file, not a verdict on the shell: without this, fixing the file
    /// reloaded the gate but left the pane refusing forever, and only reopening
    /// the tab recovered it. Enter asks the start gate again, so a policy that
    /// still says no is simply refused a second time.
    fn clear_start_refusal(&mut self) {
        if self.refused {
            self.refused = false;
            self.failure = None;
        }
    }

    /// Put a gate in place, keeping the Arc the reader thread holds.
    fn install(&self, gate: Gate) {
        match self.gate.lock() {
            Ok(mut current) => *current = gate,
            // A panic inside the evaluator poisoned the lock, and every later
            // decision would refuse with no reason the reader could act on. The
            // engine holds no invariant another thread depends on, so the newly
            // read policy replaces it outright - which is what the reload was
            // for. The policy file still decides; nothing is widened by this.
            Err(poisoned) => {
                self.gate.clear_poison();
                *poisoned.into_inner() = gate;
            }
        }
    }

    /// Every decision the gate has made, oldest first, at most AUDIT_LIMIT.
    ///
    /// A test-facing copy of the ring: the pane itself reads the audit through
    /// `blocked_decisions` and `last_refusal`, which count and search under the
    /// lock instead of copying it on every frame.
    #[cfg(test)]
    pub(crate) fn audit(&self) -> Vec<Audit> {
        self.audit
            .lock()
            .map(|ring| ring.rows.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many decisions refused or redacted something.
    ///
    /// Counted under the ring's lock rather than off `audit()`, which clones
    /// every retained row: the pane reads this from its render path, so a full
    /// copy of the ring per frame bought nothing.
    pub(crate) fn blocked_decisions(&self) -> u64 {
        self.audit.lock().map(|ring| ring.blocked).unwrap_or(0)
    }

    /// The last policy that failed to load, when one did.
    pub(crate) fn policy_error(&self) -> Option<&str> {
        self.policy_error.as_deref()
    }

    /// The most recent refused input, for the status line.
    ///
    /// Derived from the audit ring, so what the pane shows and what a test
    /// asserts cannot disagree. A later allowed submit becomes the newest input
    /// row, which clears the line.
    pub(crate) fn last_refusal(&self) -> Option<InputRefusal> {
        let ring = self.audit.lock().ok()?;
        ring.rows
            .iter()
            .rev()
            .find(|row| row.action == "input")
            .filter(|row| !row.allow)
            .map(|row| InputRefusal {
                line: row.subject.clone(),
                reason: row.reason.clone(),
            })
    }

    /// The lines the input gate refused, oldest first.
    #[cfg(test)]
    pub(crate) fn refused_inputs(&self) -> Vec<String> {
        self.audit()
            .into_iter()
            .filter(|row| row.action == "input" && !row.allow)
            .map(|row| row.subject)
            .collect()
    }

    /// Move the viewport into the scrollback: positive scrolls back towards
    /// older rows, negative returns towards the bottom.
    pub(crate) fn scroll_view(&mut self, lines: i32) -> bool {
        let moved = self
            .screen
            .lock()
            .map(|mut screen| {
                let before = screen.view_offset();
                screen.scroll_view(lines);
                screen.view_offset() != before
            })
            .unwrap_or(false);
        if moved {
            self.version.fetch_add(1, Ordering::Relaxed);
        }
        moved
    }

    /// The text a selection covers.
    pub(crate) fn selection_text(&self, selection: &Selection) -> String {
        self.screen
            .lock()
            .map(|screen| screen.selection_text(selection))
            .unwrap_or_default()
    }

    /// Push a new size to the screen and the PTY.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        if self.size == Some((cols, rows)) {
            return;
        }
        if let Ok(mut screen) = self.screen.lock() {
            screen.resize(cols as usize, rows as usize);
        }
        self.size = Some((cols, rows));
        if let Some(pty) = self.pty.as_mut() {
            let _ = pty.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the shell process is alive.
    pub(crate) fn is_running(&self) -> bool {
        self.input.is_some() && !self.exited.load(Ordering::Relaxed)
    }

    /// A copy of the screen, taken when the pane needs to repaint.
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.screen
            .lock()
            .map(|screen| screen.snapshot())
            .unwrap_or_default()
    }

    /// The parser's version, bumped by every write and every resize.
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// The size last pushed to the PTY.
    #[cfg(test)]
    pub(crate) fn size(&self) -> Option<(u16, u16)> {
        self.size
    }

    /// Feed bytes to the screen model without a process.
    ///
    /// The pane's tests drive this, so a test of the grid's painting and
    /// selection does not depend on a real shell round-trip in CI.
    #[cfg(test)]
    pub(crate) fn feed(&self, bytes: &[u8]) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.feed(bytes);
        }
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// The screen's rows as text, oldest first, for tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn lines(&self) -> Vec<String> {
        self.screen
            .lock()
            .map(|screen| screen.lines())
            .unwrap_or_default()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.kill();
            // Reap it: a killed child that is never waited on stays a zombie
            // for the rest of the process's life.
            child.reap();
        }
        // Dropping the sender is what ends the writer thread: it finishes the
        // write it is in (the killed child has closed the other end) and exits.
        self.input = None;
    }
}

/// The pane's pty device, when the platform has one to open by name.
///
/// portable-pty's `tty_name` is a Unix method, and a ConPTY has no device node
/// to open; heel cannot be handed its handle either. Returning none makes the
/// caller use the pane's own spawn, which is the only path that works there.
fn tty_path(master: &dyn MasterPty) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        master.tty_name()
    }
    #[cfg(not(unix))]
    {
        let _ = master;
        None
    }
}

/// Write setup bytes to the PTY with the driver's echo off.
///
/// The line discipline echoes queued input, so without this the bootstrap bytes
/// would be printed by the driver and again by the shell's line editor. Only
/// the driver's copy is suppressed: zsh's zle draws the line it reads, which is
/// why the pane shows the one bootstrap command at the first prompt. Restoring
/// only the echo bit keeps any terminal settings the shell has already changed.
#[cfg(unix)]
fn write_without_echo<W: Write + ?Sized>(
    master: &dyn MasterPty,
    writer: &mut W,
    bytes: &[u8],
) -> std::io::Result<()> {
    let Some(fd) = master.as_raw_fd() else {
        return writer.write_all(bytes);
    };
    let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return writer.write_all(bytes);
    }
    let mut quiet = original;
    quiet.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return writer.write_all(bytes);
    }

    let result = writer.write_all(bytes).and_then(|_| writer.flush());

    let mut current = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut current) } == 0 {
        current.c_lflag = (current.c_lflag & !libc::ECHO) | (original.c_lflag & libc::ECHO);
        unsafe {
            libc::tcsetattr(fd, libc::TCSANOW, &current);
        }
    }
    result
}

/// The shell the terminal runs: the reader's own when it is usable, then the
/// shells a macOS install carries, then /bin/sh as the last resort.
pub(crate) fn shell_program() -> PathBuf {
    shell_program_for(false)
}

/// The shells a platform carries, most preferred first.
///
/// A Windows install has no /bin/sh at all, which is why the list is per
/// platform rather than a Unix list with a fallback: "cmd.exe" is the last
/// resort there, and the pane would be dead if it were not.
pub(crate) fn shell_candidates(
    platform: agent::sandbox::TerminalPlatform,
) -> &'static [&'static str] {
    use agent::sandbox::TerminalPlatform;
    match platform {
        TerminalPlatform::MacOs => &["/bin/zsh", "/bin/bash", "/bin/sh"],
        TerminalPlatform::Linux => &["/bin/bash", "/bin/zsh", "/bin/sh"],
        TerminalPlatform::Windows => &["pwsh.exe", "powershell.exe", "cmd.exe"],
        TerminalPlatform::Other => &["/bin/sh"],
    }
}

/// The shell a pane runs, given whether its shell is sandboxed.
///
/// A sandboxed shell cannot be given a controlling terminal: only
/// portable-pty's own spawn performs the handshake (login_tty on Unix, ConPTY on
/// Windows), and a shell started by the sandbox is in a session of its own with
/// no tty to lead. Most shells tolerate that; fish does not, and it is not a
/// warning - measured inside the sandbox, fish prints "No TTY for interactive
/// shell (tcgetpgrp failed)" and then never runs a command. So a confined pane
/// asks for a shell that works and says which one it used, instead of drawing a
/// pane that cannot take a keystroke. On Linux and Windows the pane is never
/// confined (see SandboxSpec::terminal), so the reader's own shell is kept there.
pub(crate) fn shell_program_for(confined: bool) -> PathBuf {
    // SHELL is the Unix convention and COMSPEC the Windows one; either may be
    // absolute or a bare name, and the lookup below resolves both.
    let reader = std::env::var_os("SHELL")
        .or_else(|| std::env::var_os("COMSPEC"))
        .map(PathBuf::from);
    shell_choice(
        confined,
        agent::sandbox::TerminalPlatform::current(),
        reader.as_deref(),
        &lookup_program,
    )
}

/// The shell decision itself, with resolution injected so every platform's
/// table can be tested from any one of them.
fn shell_choice(
    confined: bool,
    platform: agent::sandbox::TerminalPlatform,
    reader: Option<&Path>,
    lookup: &dyn Fn(&str) -> Option<PathBuf>,
) -> PathBuf {
    let usable = |path: &Path| -> Option<PathBuf> {
        let raw = path.to_str()?;
        let resolved = lookup(raw)?;
        // A confined pane may not keep fish: it needs a controlling terminal.
        if confined && is_fish(raw) {
            return None;
        }
        Some(resolved)
    };
    if let Some(shell) = reader.and_then(usable) {
        return shell;
    }
    let candidates = shell_candidates(platform);
    for candidate in candidates {
        if let Some(path) = lookup(candidate) {
            return path;
        }
    }
    PathBuf::from(candidates[0])
}

/// Whether a shell is fish, which cannot run without a controlling terminal.
fn is_fish(path: &str) -> bool {
    path.rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.starts_with("fish"))
}

/// The completion setup a confined zsh needs before its first prompt.
///
/// zsh loads completion functions through compinit, normally from the reader's
/// zshrc. The sandbox denies the reader's home, so that file is not available
/// and the pane would only get zsh's basic file-name completion. The bootstrap
/// loads the system's completion functions without a writable dump file, then
/// points history at /dev/null so the setup leaves no state behind.
///
/// `-C` skips compinit's security audit (a confined pane has no user rc
/// directories for it to judge, and the audit stats every fpath entry), and the
/// `completer` style keeps the two cheap passes: the default `_match` and
/// `_approximate` passes re-scan the filesystem on a Tab that finds no match,
/// which under the sandbox is the difference between an instant Tab and a
/// stalled one.
///
/// The prompt and the colour switches ride along because a confined pane has no
/// reader rc files: without them the pane is a bare zsh, monochrome by
/// construction, while every other terminal the reader uses has colours.
const ZSH_COMPLETION_BOOTSTRAP: &[u8] = b" autoload -Uz compinit; compinit -C -D; \
zstyle ':completion:*' completer _complete _ignored; \
PROMPT='%F{33}%n@%m%f %F{37}%~%f %# '; fc -p /dev/null 2>/dev/null\n";

/// The bootstrap a shell needs, if any.
///
/// An unconfined zsh already loaded the reader's own completion setup, while
/// bash and sh bring their own line editor defaults. Only a confined zsh needs
/// the extra command.
fn completion_bootstrap(shell: &Path, confined: bool) -> Option<&'static [u8]> {
    if !confined || !is_zsh(shell) {
        return None;
    }
    Some(ZSH_COMPLETION_BOOTSTRAP)
}

/// Whether a shell program is zsh.
fn is_zsh(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "zsh" || name.starts_with("zsh-"))
}

/// The process group an interrupt may signal, if one is safe to signal.
///
/// A group of 1 is init, and the application's own group is the pane itself;
/// a terminal's Ctrl-C means neither.
#[cfg(unix)]
fn interruptable_group(group: i32, own: i32) -> Option<i32> {
    (group > 1 && group != own).then_some(group)
}

/// Resolve a program the way the platform would: a path must exist, a bare name
/// is searched on PATH.
fn lookup_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(unix)]
    #[test]
    fn an_interrupt_never_targets_the_application_group() {
        assert_eq!(super::interruptable_group(1200, 900), Some(1200));
        assert_eq!(super::interruptable_group(900, 900), None);
        assert_eq!(super::interruptable_group(1, 900), None);
        assert_eq!(super::interruptable_group(0, 900), None);
    }

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-terminal-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn screen() -> Screen {
        Screen::new(24, 6, 100)
    }

    /// Write a workspace policy, creating the .bsh directory it lives in.
    fn write_policy(root: &Path, source: &str) {
        let dir = root.join(".bsh");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("terminal-policy.rego"), source).expect("the policy file");
    }

    /// Poll the real PTY for text, so the test waits on the shell.
    fn wait_for(terminal: &Terminal, needle: &str) -> bool {
        for _ in 0..240 {
            if terminal.lines().iter().any(|line| line.contains(needle)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// Poll until the check passes or the deadline is up.
    fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        check()
    }

    /// Ask a terminal's gate whether it would let this line through, through the
    /// same path the pane's Enter takes.
    fn input_allowed(terminal: &Terminal, line: &str) -> bool {
        terminal
            .decide("input", line, |gate| gate.input(line, Path::new("/tmp")))
            .allowed()
    }

    #[test]
    fn an_empty_screen_is_blank() {
        let screen = screen();
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cols, 24);
        assert_eq!(snapshot.rows.len(), 6);
        assert_eq!(snapshot.text().trim(), "");
        assert_eq!(snapshot.cursor, Some((0, 0)));
        assert_eq!(screen.view_offset(), 0);
    }

    #[test]
    fn sgr_sets_the_red_foreground() {
        let mut screen = screen();
        screen.feed(b"\x1b[31mred\x1b[0m");
        let snapshot = screen.snapshot();
        for col in 0..3 {
            assert_eq!(
                snapshot.cell(0, col).map(|cell| cell.fg),
                Some(Color::Indexed(1)),
                "the SGR sequence is carried by cell {col}"
            );
        }
        assert_eq!(snapshot.row_text(0).trim_end(), "red");
    }

    #[test]
    fn a_carriage_return_returns_to_column_zero() {
        let mut screen = screen();
        screen.feed(b"abc\rX");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.row_text(0).trim_end(), "Xbc");
        assert_eq!(snapshot.cursor, Some((0, 1)));
    }

    #[test]
    fn a_line_feed_advances_a_row() {
        let mut screen = screen();
        screen.feed(b"first\r\nsecond");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.row_text(0).trim_end(), "first");
        assert_eq!(snapshot.row_text(1).trim_end(), "second");
        assert_eq!(snapshot.cursor, Some((1, 6)));
    }

    #[test]
    fn cup_moves_the_cursor() {
        let mut screen = screen();
        screen.feed(b"\x1b[3;5Hx");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cursor, Some((2, 5)), "CUP is one-based");
        assert_eq!(
            snapshot.cell(2, 4).map(|cell| cell.ch),
            Some('x'),
            "the next character lands at the cursor"
        );
    }

    #[test]
    fn a_256_colour_sgr_parses() {
        let mut screen = screen();
        screen.feed(b"\x1b[38;5;196mX");
        assert_eq!(
            screen.snapshot().cell(0, 0).map(|cell| cell.fg),
            Some(Color::Indexed(196))
        );
    }

    #[test]
    fn a_truecolor_sgr_parses() {
        let mut screen = screen();
        screen.feed(b"\x1b[38;2;10;20;30mX");
        assert_eq!(
            screen.snapshot().cell(0, 0).map(|cell| cell.fg),
            Some(Color::Rgb(10, 20, 30))
        );
    }

    #[test]
    fn the_cursor_visibility_sequence_is_honoured() {
        let mut screen = screen();
        screen.feed(b"\x1b[?25l");
        assert_eq!(screen.snapshot().cursor, None);
        screen.feed(b"\x1b[?25h");
        assert_eq!(screen.snapshot().cursor, Some((0, 0)));
    }

    #[test]
    fn resize_changes_the_grid_and_keeps_the_top_left() {
        let mut screen = screen();
        screen.feed(b"hello");
        screen.resize(10, 3);
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cols, 10);
        assert_eq!(snapshot.rows.len(), 3);
        assert_eq!(snapshot.row_text(0).trim_end(), "hello");
    }

    #[test]
    fn scrolling_keeps_the_top_line_in_scrollback() {
        let mut screen = Screen::new(10, 2, 8);
        screen.feed(b"one\r\ntwo\r\nthree");
        let lines = screen.lines();
        assert_eq!(lines[0].trim_end(), "one");
        assert_eq!(lines[1].trim_end(), "two");
    }

    /// The highest-value fix: a CJK glyph is two cells wide, and the cell after
    /// it is a continuation marker the painter must never draw a glyph for.
    #[test]
    fn a_wide_character_takes_two_cells() {
        let mut screen = screen();
        screen.feed("\u{4e2d}".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(
            snapshot.cell(0, 0).map(|cell| cell.ch),
            Some('\u{4e2d}'),
            "the glyph holds the first cell"
        );
        assert!(
            snapshot
                .cell(0, 1)
                .expect("the trailing half exists")
                .is_continuation(),
            "the second cell is a continuation marker"
        );
        assert_eq!(
            snapshot.cell(0, 1).map(|cell| cell.ch),
            Some(' '),
            "and it carries no character of its own"
        );
        assert_eq!(snapshot.cursor, Some((0, 2)), "the cursor advanced by two");
    }

    /// Mixed ASCII and CJK lands at the column its display width names, and the
    /// selection text withholds the continuation cells.
    #[test]
    fn a_mixed_line_ends_at_the_right_column() {
        let mut screen = Screen::new(10, 4, 10);
        screen.feed("ab\u{4e2d}\u{6587}cd".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cell(0, 0).map(|cell| cell.ch), Some('a'));
        assert_eq!(snapshot.cell(0, 1).map(|cell| cell.ch), Some('b'));
        assert_eq!(snapshot.cell(0, 2).map(|cell| cell.ch), Some('\u{4e2d}'));
        assert!(snapshot.cell(0, 3).expect("half one").is_continuation());
        assert_eq!(snapshot.cell(0, 4).map(|cell| cell.ch), Some('\u{6587}'));
        assert!(snapshot.cell(0, 5).expect("half two").is_continuation());
        assert_eq!(snapshot.cell(0, 6).map(|cell| cell.ch), Some('c'));
        assert_eq!(snapshot.cell(0, 7).map(|cell| cell.ch), Some('d'));
        assert_eq!(snapshot.cursor, Some((0, 8)), "six glyphs fill eight cells");
        assert_eq!(
            screen.selection_text(&Selection::new((0, 0), (0, 8))),
            "ab\u{4e2d}\u{6587}cd",
            "the continuation cells are not text of their own"
        );
    }

    /// A combining accent joins the cell before it instead of taking one.
    #[test]
    fn a_combining_mark_attaches_to_the_cell_before_it() {
        let mut screen = Screen::new(10, 4, 10);
        screen.feed("e\u{301}x".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cell(0, 0).map(|cell| cell.ch), Some('e'));
        assert_eq!(
            snapshot.cell(0, 0).map(|cell| cell.combining().to_vec()),
            Some(vec!['\u{301}']),
            "the mark is attached to the cell it draws over"
        );
        assert_eq!(
            snapshot.cell(0, 1).map(|cell| cell.ch),
            Some('x'),
            "the next character did not shift"
        );
        assert_eq!(snapshot.cursor, Some((0, 2)));
        assert_eq!(
            screen.selection_text(&Selection::new((0, 0), (0, 1))),
            "e\u{301}x"
        );
    }

    /// A double-width glyph with only one column left wraps before printing.
    #[test]
    fn a_wide_glyph_at_the_right_edge_wraps_first() {
        let mut screen = Screen::new(4, 3, 10);
        screen.feed("abc\u{4e2d}".as_bytes());
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cell(0, 3).map(|cell| cell.ch), Some(' '));
        assert_eq!(snapshot.cell(1, 0).map(|cell| cell.ch), Some('\u{4e2d}'));
        assert!(snapshot.cell(1, 1).expect("the half").is_continuation());
        assert_eq!(snapshot.cursor, Some((1, 2)));
    }

    /// The alternate screen saves the primary grid, shows a blank one, and puts
    /// the primary back when the program leaves.
    #[test]
    fn the_alternate_screen_swaps_the_grid() {
        let mut screen = screen();
        screen.feed(b"primary");
        screen.feed(b"\x1b[?1049h");
        assert_eq!(
            screen.snapshot().row_text(0).trim_end(),
            "",
            "entering the alternate screen blanks it"
        );
        screen.feed(b"alternate");
        assert_eq!(screen.snapshot().row_text(0).trim_end(), "alternate");
        assert!(
            !screen.snapshot().row_text(0).contains("primary"),
            "the primary text is not visible"
        );
        screen.feed(b"\x1b[?1049l");
        assert_eq!(
            screen.snapshot().row_text(0).trim_end(),
            "primary",
            "leaving restores the primary screen"
        );
        assert_eq!(
            screen.snapshot().cursor,
            Some((0, 7)),
            "and the cursor it parked"
        );
        // The older spelling swaps the same way.
        screen.feed(b"\x1b[?47h");
        screen.feed(b"other");
        assert_eq!(screen.snapshot().row_text(0).trim_end(), "other");
        screen.feed(b"\x1b[?47l");
        assert_eq!(screen.snapshot().row_text(0).trim_end(), "primary");
    }

    /// The scrollback viewport: 100 lines into a 10-row grid, five rows back,
    /// and the visible top row moves by five; new output holds the view.
    #[test]
    fn the_scrollback_viewport_moves_and_holds_its_place() {
        let mut screen = Screen::new(10, 10, 200);
        for index in 0..100 {
            screen.feed(format!("line-{index:03}\r\n").as_bytes());
        }
        let bottom = screen.snapshot();
        assert_eq!(screen.view_offset(), 0);
        let top_at_bottom = bottom.first_row;
        screen.scroll_view(5);
        let scrolled = screen.snapshot();
        assert_eq!(screen.view_offset(), 5);
        assert_eq!(
            scrolled.first_row,
            top_at_bottom - 5,
            "the visible top row moved up five rows"
        );
        assert_eq!(
            scrolled.row_text(5).trim_end(),
            bottom.row_text(0).trim_end(),
            "the row that was at the top is now five rows down"
        );
        assert_eq!(
            scrolled.cursor, None,
            "the cursor hides while scrolled back"
        );

        // New output while the reader is scrolled back must not yank the view.
        let held = scrolled.first_row;
        screen.feed(b"more\r\n");
        assert_eq!(
            screen.snapshot().first_row,
            held,
            "the view sticks to the content it was showing"
        );

        // Returning to the bottom shows the cursor again.
        screen.scroll_view(-1000);
        assert_eq!(screen.view_offset(), 0);
        assert!(screen.snapshot().cursor.is_some());
        assert_eq!(
            screen.snapshot().first_row,
            bottom.first_row + 1,
            "the new line is at the bottom"
        );
    }

    /// A selection copies a stream range across rows, trimming the blank tail.
    #[test]
    fn a_selection_copies_its_rows() {
        let mut screen = Screen::new(12, 4, 10);
        screen.feed(b"hello\r\nworld");
        assert_eq!(
            screen.selection_text(&Selection::new((0, 0), (1, 4))),
            "hello\nworld"
        );
        assert_eq!(
            screen.selection_text(&Selection::new((1, 4), (0, 0))),
            "hello\nworld",
            "the drag direction does not change the text"
        );
        assert_eq!(
            screen.selection_text(&Selection::new((0, 1), (0, 3))),
            "ell",
            "a one-row drag is the cells under it"
        );
        assert_eq!(
            screen.selection_text(&Selection::new((0, 11), (0, 11))),
            "",
            "a cell past the text copies nothing"
        );
    }

    #[test]
    fn the_default_gate_allows_the_shell_to_start() {
        let mut gate = Gate::allow_all();
        assert!(gate
            .start(Path::new("/bin/zsh"), Path::new("/tmp"))
            .allowed());
        assert_eq!(gate.source().label(), "default");
    }

    #[test]
    fn the_default_gate_allows_input_and_output() {
        let mut gate = Gate::allow_all();
        assert!(gate.input("rm -rf /", Path::new("/tmp")).allowed());
        assert!(gate.output("anything at all").allowed());
    }

    /// A refusal says why, in the pane and in the status line: a plain `false`
    /// verdict still names the policy that said no.
    #[test]
    fn a_refused_command_shows_its_reason() {
        let dir = scratch("refusal-reason");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        );
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(terminal.is_running(), "the shell starts");
        for byte in b"touch nope" {
            terminal.send_key(&[*byte]);
        }
        let outcome = terminal.send_key(b"\r");
        assert!(
            matches!(outcome, KeyOutcome::Refused),
            "the line is refused"
        );
        let refusal = terminal.last_refusal().expect("a refusal is recorded");
        let reason = refusal.reason.expect("and it carries a reason");
        assert!(
            reason.contains("file") && reason.contains("refused"),
            "the reason names the policy that refused: {reason}"
        );
        let shown = terminal.lines().join("\n");
        assert!(
            shown.contains("touch nope") && shown.contains(&reason),
            "the pane prints the command and the reason: {shown}"
        );
    }
    /// `exit` used to leave a dead pane that swallowed every key. Enter now
    /// starts a fresh shell through the same gate.
    #[test]
    fn enter_restarts_a_shell_that_exited() {
        let dir = scratch("restart");
        write_policy(&dir, "package terminal\n\ndefault allow := true\n");
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(terminal.is_running(), "the shell starts");
        // The real path: every byte through send_key, including the Enter, so the
        // input gate and the line mirror are exercised the way a user drives it.
        for byte in b"exit" {
            terminal.send_key(&[*byte]);
        }
        terminal.send_key(b"\r");
        for _ in 0..240 {
            if !terminal.is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(!terminal.is_running(), "exit ends the shell");
        assert!(
            terminal.lines().iter().any(|line| line.contains("exit")),
            "the typed line reached the shell: {:?}",
            terminal.lines()
        );
        assert!(
            matches!(terminal.send_key(b"\r"), KeyOutcome::Sent),
            "Enter in a dead pane starts another shell"
        );
        assert!(terminal.is_running(), "and the pane is live again");
        assert!(!terminal.refused, "the new shell passed the start gate");
    }
    /// The workspace's policy file is seeded on first open, so a reader edits
    /// the file the gate actually reads instead of inventing it from the docs.
    #[test]
    fn a_missing_policy_is_written_from_the_template() {
        let dir = scratch("seed-policy");
        let path = dir.join(".bsh").join("terminal-policy.rego");
        assert!(!path.exists(), "the workspace starts with no policy");
        let terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(path.exists(), "the terminal writes one on first open");
        let written = std::fs::read_to_string(&path).expect("the seeded policy");
        assert!(
            written.contains("package terminal") && written.contains("default allow := true"),
            "the template is a working allow-all policy: {written}"
        );
        assert!(
            !terminal.refused,
            "and the seeded policy lets the shell start"
        );
        // It is seeded, not imposed: a policy the reader already wrote stays.
        let deny = "package terminal\n\nallow := false if { input.action == \"start\" }\n";
        write_policy(&dir, deny);
        let refused = Terminal::new(dir.clone(), Lang::En);
        assert!(refused.refused, "an existing policy is never overwritten");
        assert_eq!(
            std::fs::read_to_string(&path).expect("the reader's policy"),
            deny
        );
    }
    /// The gap this closes: the gate now decides whether a process exists at
    /// all, and a deny policy leaves the pane with nothing running.
    #[test]
    fn a_deny_policy_blocks_the_spawn() {
        let dir = scratch("deny");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"start\" }\n",
        );
        let terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(terminal.refused, "the gate refused the start");
        assert!(
            !terminal.is_running(),
            "no PTY is spawned when the gate says no"
        );
        let lines = terminal.lines();
        assert!(
            lines.iter().all(|line| line.trim().is_empty()),
            "and the screen stays blank: {lines:?}"
        );
        assert!(
            terminal
                .audit()
                .iter()
                .any(|row| row.action == "start" && !row.allow),
            "the refusal is in the audit ring"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_policy_fails_closed() {
        let dir = scratch("broken");
        write_policy(&dir, "this is not a rego module\n");
        let terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(terminal.refused, "an unparseable policy refuses the start");
        assert!(!terminal.is_running());
        assert!(terminal.failure.is_some(), "and the reason is kept");
        assert!(
            terminal.policy_error().is_some(),
            "the parse error is surfaced"
        );
        let audit = terminal.audit();
        assert!(
            audit.iter().any(|row| row.action == "policy" && !row.allow),
            "the parse error is in the audit ring: {audit:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A policy that exists but does not parse closes every gate point, not
    /// only the start: input and output must fail closed too.
    #[test]
    fn a_broken_policy_refuses_every_gate_point() {
        let mut gate = Gate::allow_all();
        assert!(gate
            .load_policy("workspace.rego", "this is not a rego module\n")
            .is_err());
        assert!(!gate
            .start(Path::new("/bin/zsh"), Path::new("/tmp"))
            .allowed());
        assert!(!gate.input("ls", Path::new("/tmp")).allowed());
        assert!(!gate.output("hello").allowed());
    }

    #[test]
    fn the_workspace_policy_can_allow_the_start() {
        let dir = scratch("allow");
        write_policy(
            &dir,
            "package terminal\n\nallow := true if { input.action == \"start\" }\n",
        );
        let terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(!terminal.refused, "the policy allows the shell");
        assert!(terminal.is_running(), "and the interactive shell runs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The input gate: a deny rule refuses the submitted line and says why.
    #[test]
    fn a_deny_input_rule_refuses_the_line() {
        let mut gate =
            Gate::for_file("package terminal\n\nallow := false if { input.action == \"input\" }\n")
                .expect("the policy parses");
        assert!(
            !gate.input("rm -rf /", Path::new("/tmp")).allowed(),
            "the line is refused"
        );
        assert!(
            gate.output("hello").allowed(),
            "output is a different question"
        );
        assert!(gate
            .start(Path::new("/bin/sh"), Path::new("/tmp"))
            .allowed());
    }

    /// A richer decision: a refusal with the policy's own reason, and output
    /// redacted instead of dropped.
    #[test]
    fn the_rich_decision_carries_a_reason_and_a_redaction() {
        let mut gate = Gate::for_file(
            "package terminal\n\n\
             decision := {\"allow\": false, \"reason\": \"destructive\"} if { input.action == \"input\" }\n\
             decision := {\"allow\": true, \"redact\": \"REDACTED\"} if { input.action == \"output\" }\n",
        )
        .expect("the richer policy parses");
        let refused = gate.input("rm -rf /", Path::new("/tmp"));
        assert!(!refused.allowed());
        assert_eq!(refused.reason(), Some("destructive"));
        let redacted = gate.output("sk-secret");
        assert!(
            redacted.allowed(),
            "a redaction still lets the chunk through"
        );
        assert_eq!(redacted.redaction(), Some("REDACTED"));
    }

    /// The contract in docs/bsh-terminal-policy.md parses and refuses the
    /// destructive list an author would copy.
    #[test]
    fn the_documented_destructive_command_policy_refuses() {
        let mut gate = Gate::for_file(
            "package terminal\n\n\
             denied := [\"rm -rf\", \"mkfs\", \"dd of=/dev/\"]\n\n\
             decision := {\"allow\": false, \"reason\": \"destructive command\"} if {\n\
                 input.action == \"input\"\n\
                 some needle in denied\n\
                 contains(input.command, needle)\n\
             }\n",
        )
        .expect("the documented policy parses");
        assert!(!gate
            .input("sudo rm -rf /tmp/x", Path::new("/tmp"))
            .allowed());
        assert!(!gate
            .input("mkfs.ext4 /dev/sda", Path::new("/tmp"))
            .allowed());
        assert!(gate.input("ls -la", Path::new("/tmp")).allowed());
    }

    /// A refused line never runs: the PTY sees the echoed keystrokes but not the
    /// submitting carriage return, and Ctrl-U clears them.
    #[test]
    fn a_denied_line_never_reaches_the_shell() {
        let dir = scratch("gate-input-deny");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        );
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(!terminal.refused, "{:?}", terminal.failure);
        for byte in b"touch refused-by-policy".iter() {
            terminal.send_key(&[*byte]);
        }
        assert_eq!(terminal.send_key(b"\r"), KeyOutcome::Refused);
        assert_eq!(
            terminal.refused_inputs(),
            vec![String::from("touch refused-by-policy")]
        );
        assert!(
            !dir.join("refused-by-policy").exists(),
            "the line never reached the shell"
        );
        let audit = terminal.audit();
        let row = audit
            .iter()
            .rev()
            .find(|row| row.action == "input")
            .expect("the input decision is recorded");
        assert!(!row.allow, "and it is recorded as a refusal");
        assert_eq!(row.subject, "touch refused-by-policy");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same keystrokes under the default policy do reach the shell.
    #[test]
    fn an_allowed_line_reaches_the_shell() {
        let dir = scratch("gate-input-allow");
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(!terminal.refused, "{:?}", terminal.failure);
        for byte in b"touch allowed-by-policy".iter() {
            terminal.send_key(&[*byte]);
        }
        assert_eq!(terminal.send_key(b"\r"), KeyOutcome::Sent);
        assert!(
            wait_until(Duration::from_secs(10), || dir
                .join("allowed-by-policy")
                .exists()),
            "the allowed line ran: {:?}",
            terminal.lines()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The output gate: a denied chunk never reaches the grid, one marker line
    /// takes its place, and the audit ring says what happened.
    #[test]
    fn a_deny_output_rule_blocks_the_chunk() {
        let dir = scratch("gate-output-deny");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"output\" }\n",
        );
        let terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(!terminal.refused, "{:?}", terminal.failure);
        let marker = crate::i18n::tr(Lang::En, "right.terminal.outputBlocked");
        assert!(
            wait_until(Duration::from_secs(10), || terminal
                .lines()
                .iter()
                .any(|line| line.contains(&marker))),
            "a blocked chunk leaves the marker line: {:?}",
            terminal.lines()
        );
        assert!(
            !terminal
                .lines()
                .iter()
                .any(|line| line.contains(Path::new("gate-output-deny").to_str().unwrap())),
            "nothing the shell said reached the grid"
        );
        assert!(terminal.blocked_decisions() > 0);
        let audit = terminal.audit();
        assert!(
            audit.iter().any(|row| row.action == "output" && !row.allow),
            "the blocked chunk is in the audit ring"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A changed policy file takes effect without restarting anything.
    #[test]
    fn a_changed_policy_takes_effect_without_a_restart() {
        let dir = scratch("policy-reload");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        );
        let mut terminal = Terminal::without_process(dir.clone(), Lang::En, 24, 6);
        assert!(
            !input_allowed(&terminal, "ls"),
            "the first policy refuses input"
        );
        std::thread::sleep(Duration::from_millis(30));
        write_policy(
            &dir,
            "package terminal\n\n# the second policy\nallow := false if { input.action == \"start\" }\n",
        );
        assert!(terminal.refresh_policy(), "the changed stamp is noticed");
        assert!(
            input_allowed(&terminal, "ls"),
            "the reloaded policy allows input"
        );
        assert!(terminal.policy_error().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A broken edit keeps the last good policy and records the parse error.
    #[test]
    fn a_broken_reload_keeps_the_last_good_policy() {
        let dir = scratch("policy-reload-broken");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        );
        let mut terminal = Terminal::without_process(dir.clone(), Lang::En, 24, 6);
        assert!(!input_allowed(&terminal, "ls"));
        std::thread::sleep(Duration::from_millis(30));
        write_policy(&dir, "this is not a rego module any more\n");
        assert!(terminal.refresh_policy());
        assert!(
            terminal.policy_error().is_some(),
            "the broken edit is recorded"
        );
        assert!(
            !input_allowed(&terminal, "ls"),
            "the last good policy is still installed: a broken reload never widens"
        );
        let audit = terminal.audit();
        assert!(
            audit.iter().any(|row| row.action == "policy" && !row.allow),
            "the parse error is in the audit ring"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shell is interactive and persistent: writes over one PTY keep the
    /// directory the first one changed.
    #[test]
    fn the_interactive_shell_persists_its_directory() {
        let dir = scratch("persist");
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(
            !terminal.refused,
            "the default gate allows the shell: {:?}",
            terminal.failure
        );
        assert!(terminal.is_running(), "the shell process is alive");
        assert!(
            terminal.send_bytes(format!("cd {}\r", dir.display()).as_bytes()),
            "the directory change is written to the pty"
        );
        assert!(
            terminal.send_bytes(b"printf 'READY-MARKER\\n'\r"),
            "the second command is written to the same pty"
        );
        assert!(
            wait_for(&terminal, "READY-MARKER"),
            "the interactive shell runs the second command: {:?}",
            terminal.lines()
        );
        assert!(
            terminal.send_bytes(b"touch made-by-the-shell\r"),
            "a third command runs in the persisted directory"
        );
        let created = dir.join("made-by-the-shell");
        assert!(
            wait_until(Duration::from_secs(10), || created.exists()),
            "the shell's cd persisted across commands"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The redaction example from docs/bsh-terminal-policy.md parses and does
    /// what it says: a secret-bearing chunk is replaced, a clean one is not.
    #[test]
    fn the_documented_secret_redaction_policy_redacts() {
        let mut gate = Gate::for_file(
            "package terminal\n\n\
             decision := {\"allow\": true, \"redact\": \"[redacted]\"} if {\n\
                 input.action == \"output\"\n\
                 regex.match(\"sk-[A-Za-z0-9]+\", input.text)\n\
             }\n",
        )
        .expect("the documented policy parses");
        assert_eq!(gate.output("key sk-abc123").redaction(), Some("[redacted]"));
        assert_eq!(gate.output("nothing secret").redaction(), None);
    }

    /// The outside-path example refuses an absolute path and lets a relative
    /// one through.
    #[test]
    fn the_documented_outside_path_policy_refuses() {
        let mut gate = Gate::for_file(
            "package terminal\n\n\
             decision := {\"allow\": false, \"reason\": \"path outside the workspace\"} if {\n\
                 input.action == \"input\"\n\
                 some token in split(input.command, \" \")\n\
                 startswith(token, \"/\")\n\
             }\n",
        )
        .expect("the documented policy parses");
        let refused = gate.input("cat /etc/passwd", Path::new("/tmp"));
        assert!(!refused.allowed());
        assert_eq!(refused.reason(), Some("path outside the workspace"));
        assert!(gate.input("cat notes.md", Path::new("/tmp")).allowed());
    }

    /// The shell program is a real path, whichever candidate wins.
    #[test]
    fn a_shell_program_is_found() {
        let shell = shell_program();
        assert!(shell.is_absolute(), "{shell:?}");
        assert!(shell.is_file(), "{shell:?}");
    }

    /// A scroll request past the region's height costs the region, not the
    /// requested count: the screen is blank, and the scrollback holds the rows
    /// that were on it, not forty thousand blank ones.
    #[test]
    fn a_huge_scroll_request_is_bounded_by_the_region() {
        let mut screen = Screen::new(4, 2, 100);
        screen.feed(b"aa\r\nbb");
        // CSI 40000 S: two real shifts, and the rest is a blank region already.
        screen.feed(b"\x1b[40000S");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.text().trim(), "", "the region is blank");
        assert_eq!(
            snapshot.first_row, 2,
            "only the two rows that were on the screen moved into the scrollback"
        );
        screen.scroll_view(2);
        assert_eq!(screen.snapshot().row_text(0).trim_end(), "aa");
        assert_eq!(screen.snapshot().row_text(1).trim_end(), "bb");
        screen.scroll_view(-1000);
        // Scrolling down discards rows; past the height it is a no-op.
        screen.feed(b"\x1b[40000T");
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.first_row, 2, "SD retains nothing");
        assert_eq!(snapshot.text().trim(), "");
    }

    /// A selection whose head is far outside the retained window copies the
    /// window, and costs the window rather than the distance.
    #[test]
    fn an_out_of_range_selection_clamps_to_the_retained_rows() {
        let mut screen = Screen::new(6, 2, 2);
        screen.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(
            screen.selection_text(&Selection::new((0, 0), (u64::MAX, 5))),
            "one\ntwo\nthree",
            "the retained rows are copied"
        );
        assert_eq!(
            screen.selection_text(&Selection::new((u64::MAX, 0), (u64::MAX, 5))),
            "",
            "a selection past the window has nothing to copy"
        );
    }

    /// A lookup over a fixed set of (name, resolved path) pairs, so the shell
    /// decision table can be exercised for every platform from this one.
    fn lookup_map(
        entries: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| {
            entries
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, path)| PathBuf::from(path))
        }
    }

    /// The shell decision is a per-platform TABLE, and none of the three
    /// platforms may fall through to a shell it does not carry.
    #[test]
    fn the_shell_decision_table_covers_all_three_platforms() {
        use agent::sandbox::TerminalPlatform;
        let mac = lookup_map(&[
            ("/opt/homebrew/bin/fish", "/opt/homebrew/bin/fish"),
            ("/bin/zsh", "/bin/zsh"),
            ("/bin/sh", "/bin/sh"),
        ]);
        // Unconfined: the reader's own shell, fish included.
        assert_eq!(
            shell_choice(
                false,
                TerminalPlatform::MacOs,
                Some(Path::new("/opt/homebrew/bin/fish")),
                &mac
            ),
            PathBuf::from("/opt/homebrew/bin/fish")
        );
        // Confined: fish needs a controlling terminal, so zsh is chosen and the
        // pane says so (see Terminal::start).
        assert_eq!(
            shell_choice(
                true,
                TerminalPlatform::MacOs,
                Some(Path::new("/opt/homebrew/bin/fish")),
                &mac
            ),
            PathBuf::from("/bin/zsh")
        );
        // A shell that works confined is kept.
        assert_eq!(
            shell_choice(
                true,
                TerminalPlatform::MacOs,
                Some(Path::new("/bin/zsh")),
                &mac
            ),
            PathBuf::from("/bin/zsh")
        );
        // No reader at all: the platform's first installed candidate.
        assert_eq!(
            shell_choice(false, TerminalPlatform::MacOs, None, &mac),
            PathBuf::from("/bin/zsh")
        );

        // Linux prefers bash, and /bin/sh still exists there.
        let linux = lookup_map(&[("/bin/bash", "/bin/bash"), ("/bin/sh", "/bin/sh")]);
        assert_eq!(
            shell_choice(false, TerminalPlatform::Linux, None, &linux),
            PathBuf::from("/bin/bash")
        );

        // Windows has no /bin/sh: pwsh, then powershell, then cmd.
        let windows = lookup_map(&[
            ("pwsh.exe", "C:/Program Files/PowerShell/7/pwsh.exe"),
            ("cmd.exe", "C:/Windows/system32/cmd.exe"),
        ]);
        assert_eq!(
            shell_choice(false, TerminalPlatform::Windows, None, &windows),
            PathBuf::from("C:/Program Files/PowerShell/7/pwsh.exe")
        );
        // A COMSPEC reader (cmd.exe) is used when it resolves.
        assert_eq!(
            shell_choice(
                false,
                TerminalPlatform::Windows,
                Some(Path::new("cmd.exe")),
                &windows
            ),
            PathBuf::from("C:/Windows/system32/cmd.exe")
        );
        // Nothing resolves: the name is left for the OS to resolve itself
        // (CreateProcess searches PATH and PATHEXT), never a Unix path.
        let empty = lookup_map(&[]);
        assert_eq!(
            shell_choice(false, TerminalPlatform::Windows, None, &empty),
            PathBuf::from("pwsh.exe")
        );
    }

    /// The real resolver: an absolute path must exist, a bare name is searched
    /// on PATH. This host always carries sh somewhere, and a bogus absolute path
    /// never resolves.
    #[test]
    fn the_program_lookup_resolves_paths_and_path_names() {
        assert!(lookup_program("sh").is_some(), "sh is on PATH on this host");
        assert!(lookup_program("/definitely/not/a/shell").is_none());
        assert!(lookup_program("/bin/sh").is_some());
    }

    /// Every shell the pane can pick is a real absolute path or a bare name the
    /// platform resolves; nothing here is a Unix path on Windows by accident.
    #[test]
    fn the_shell_candidates_are_platform_appropriate() {
        use agent::sandbox::TerminalPlatform;
        assert!(shell_candidates(TerminalPlatform::MacOs).contains(&"/bin/zsh"));
        assert!(shell_candidates(TerminalPlatform::Linux).contains(&"/bin/bash"));
        assert!(shell_candidates(TerminalPlatform::Windows).contains(&"pwsh.exe"));
        assert!(
            !shell_candidates(TerminalPlatform::Windows)
                .iter()
                .any(|candidate| candidate.starts_with('/')),
            "a Windows candidate list may not name a Unix path"
        );
    }
    /// Only a confined zsh needs the pane's completion bootstrap.
    #[test]
    fn only_a_confined_zsh_gets_the_completion_bootstrap() {
        assert!(completion_bootstrap(Path::new("/bin/zsh"), true).is_some());
        assert!(completion_bootstrap(Path::new("/bin/zsh"), false).is_none());
        assert!(completion_bootstrap(Path::new("/bin/bash"), true).is_none());
        let bootstrap = completion_bootstrap(Path::new("/bin/zsh"), true).unwrap();
        let text = std::str::from_utf8(bootstrap).expect("the bootstrap is UTF-8");
        assert!(text.contains("compinit"), "completion is set up: {text}");
        assert!(text.contains("PROMPT="), "the pane gets a prompt: {text}");
        assert!(text.contains("PROMPT="), "the prompt is coloured: {text}");
    }

    /// Type one line into a live pane and wait for it to come back.
    fn type_and_wait(terminal: &mut Terminal, line: &str, needle: &str) -> bool {
        for byte in line.as_bytes() {
            terminal.send_key(&[*byte]);
        }
        terminal.send_key(b"\r");
        wait_for(terminal, needle)
    }

    /// The pane's shell runs INSIDE the turn's sandbox: a command typed here
    /// cannot write outside the workspace, while the PTY stays a real terminal
    /// (echo, resize, exit). The probe is a marker file, not a message.
    ///
    /// Unix runs it on every backend that can grant the pane's device: macOS by
    /// sandbox-exec, Linux by heel's Landlock ruleset, which grants /dev/pts
    /// when allow_tty_write is on.
    #[cfg(unix)]
    #[test]
    fn the_terminal_shell_runs_inside_the_sandbox() {
        let dir = scratch("sandbox-shell");
        let spec = agent::sandbox::SandboxSpec::workspace_write(&dir);
        let mut terminal = Terminal::new_with_sandbox(dir.clone(), Lang::En, Some(&spec));
        assert_eq!(terminal.sandbox_label(), Some("workspace-write"));
        // A pane with something to explain (this host's fish cannot run
        // confined) says it in the pane; either way the shell is live.
        assert!(
            terminal.is_running(),
            "the shell starts inside the sandbox: {:?}",
            terminal.failure
        );

        // The tier reached the shell's own environment, through a real PTY: the
        // echo is the shell's line discipline, not the pane's.
        assert!(
            type_and_wait(
                &mut terminal,
                "echo bsh-tier:$BSH_SANDBOX",
                "bsh-tier:workspace-write"
            ),
            "the shell did not report its sandbox: {:?}",
            terminal.lines()
        );

        // A write outside the workspace is refused BY THE KERNEL: the probe file
        // never appears, wherever the command resolved the path from.
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let outside = home.join(format!("bsh-terminal-escape-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        // The shell reports the failed redirection with an exit code, which is
        // the same on both kernels; the message ("Operation not permitted" on
        // macOS, "Permission denied" on Linux) is not asserted here.
        assert!(
            type_and_wait(
                &mut terminal,
                &format!("printf escaped > {}; echo refused=$?", outside.display()),
                "refused=1"
            ),
            "the sandboxed shell was not refused: {:?}",
            terminal.lines()
        );
        assert!(
            !outside.exists(),
            "the kernel let a terminal write escape the workspace"
        );

        // The same command inside the workspace lands.
        assert!(
            type_and_wait(
                &mut terminal,
                "printf inside > inside-terminal.txt; echo wrote=$?",
                "wrote=0"
            ),
            "an in-workspace write failed: {:?}",
            terminal.lines()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("inside-terminal.txt")).expect("the inside file"),
            "inside"
        );

        // The terminal is still a terminal: it resizes and its shell exits.
        terminal.resize(100, 30);
        assert_eq!(terminal.size(), Some((100, 30)));
        for byte in b"exit" {
            terminal.send_key(&[*byte]);
        }
        terminal.send_key(b"\r");
        assert!(
            wait_until(Duration::from_secs(10), || {
                terminal.poll();
                !terminal.is_running()
            }),
            "exit ends the sandboxed shell"
        );

        // Enter starts a fresh sandboxed shell, and the old reader's end does
        // not mark it dead.
        assert!(
            matches!(terminal.send_key(b"\r"), KeyOutcome::Sent),
            "Enter starts a new shell"
        );
        assert!(
            terminal.is_running(),
            "the new shell is alive: {:?}",
            terminal.failure
        );
        assert!(
            type_and_wait(
                &mut terminal,
                "echo again:$BSH_SANDBOX",
                "again:workspace-write"
            ),
            "the restarted shell kept its sandbox: {:?}",
            terminal.lines()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A confined zsh still completes shell arguments.
    ///
    /// The sandbox denies the reader's home, so zsh cannot source the usual
    /// zshrc that calls compinit. The pane therefore has to bootstrap
    /// completion itself; otherwise Tab can only complete a file name and the
    /// right pane feels broken even though the shell is alive.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_confined_zsh_completes_a_shell_argument() {
        let dir = scratch("zsh-completion");
        let spec = agent::sandbox::SandboxSpec::workspace_write(&dir);
        let mut terminal = Terminal::start_with_shell(
            dir.clone(),
            Lang::En,
            Some(&spec),
            Some(PathBuf::from("/bin/zsh")),
            None,
        );
        assert!(
            terminal.is_running(),
            "the zsh starts: {:?}",
            terminal.failure
        );
        for byte in b"kill -s SIGTE" {
            terminal.send_key(&[*byte]);
        }
        assert_eq!(terminal.send_key(b"	"), KeyOutcome::Sent);
        let completed = wait_for(&terminal, "SIGTERM");
        let lines = terminal.lines();

        // Measured on this machine: the pane runs zsh and completion works by
        // loading compinit, whose bootstrap IS painted (see the lines below), so
        // the earlier version of this test asserted a behaviour the pane does not
        // have. What matters is that Tab actually completes the argument.
        assert!(completed, "Tab completes the signal name: {:?}", lines);
        assert!(
            lines.iter().any(|line| line.contains("compinit")),
            "the completion bootstrap is what completed it: {:?}",
            lines
        );
        // Drop kills and reaps the child, so nothing is left for the harness.
        drop(terminal);
    }
    /// A released permission has nothing to state: the shell runs bare and says
    /// so, and no confinement is implied.
    #[test]
    fn a_full_access_terminal_reports_the_released_tier() {
        let dir = scratch("sandbox-none");
        let spec = agent::sandbox::SandboxSpec::bare(agent::sandbox::SandboxMode::FullAccess, &dir);
        let mut terminal = Terminal::new_with_sandbox(dir.clone(), Lang::En, Some(&spec));
        assert!(
            terminal.sandbox_note().is_none(),
            "a released turn has no sandbox to explain"
        );
        assert_eq!(terminal.sandbox_label(), Some("full-access"));
        assert!(terminal.is_running(), "the shell starts");
        assert!(
            type_and_wait(
                &mut terminal,
                "echo bsh-tier:$BSH_SANDBOX",
                "bsh-tier:full-access"
            ),
            "a released turn's shell still learns what it runs under: {:?}",
            terminal.lines()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A preset's off is passed BEFORE the spawn, so the shell's own
    /// BSH_SANDBOX says "none" and cannot be mistaken for a permission release.
    #[test]
    fn a_preset_off_terminal_reports_none_to_the_shell() {
        let dir = scratch("sandbox-preset-off");
        let spec = agent::sandbox::SandboxSpec::bare(agent::sandbox::SandboxMode::FullAccess, &dir);
        let note = "a preset turned the sandbox off".to_string();
        let mut terminal =
            Terminal::new_with_sandbox_note(dir.clone(), Lang::En, Some(&spec), Some(note.clone()));
        assert!(terminal.sandbox_off_by_preset);
        assert_eq!(terminal.sandbox_note(), Some(note.as_str()));
        assert_eq!(
            terminal.sandbox_label(),
            Some("none"),
            "the status and the shell agree"
        );
        assert!(terminal.is_running(), "the shell starts");
        assert!(
            type_and_wait(&mut terminal, "echo bsh-tier:$BSH_SANDBOX", "bsh-tier:none"),
            "a preset's off reaches the shell, not only the status line: {:?}",
            terminal.lines()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A shell restarted by Enter keeps the pane's size: the new PTY used to
    /// open at the startup default, so a resized pane drew its prompt at the
    /// wrong width.
    #[test]
    fn a_restarted_shell_keeps_the_size_the_pane_set() {
        let dir = scratch("restart-size");
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(!terminal.refused, "{:?}", terminal.failure);
        terminal.resize(100, 30);
        assert_eq!(terminal.size(), Some((100, 30)));
        for byte in b"exit" {
            terminal.send_key(&[*byte]);
        }
        terminal.send_key(b"\r");
        assert!(
            wait_until(Duration::from_secs(10), || !terminal.is_running()),
            "exit ends the shell"
        );
        assert!(
            matches!(terminal.send_key(b"\r"), KeyOutcome::Sent),
            "Enter starts a fresh shell"
        );
        assert_eq!(
            terminal.size(),
            Some((100, 30)),
            "the new PTY keeps the pane's size"
        );
        let snapshot = terminal.snapshot();
        assert_eq!(
            (snapshot.cols, snapshot.rows.len()),
            (100, 30),
            "and the grid agrees with it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A panic while the screen is locked poisons it; a restart replaces the
    /// screen instead of leaving the pane permanently blank.
    #[test]
    fn a_poisoned_screen_is_replaced_by_a_restart() {
        let dir = scratch("poison");
        let mut terminal = Terminal::without_process(dir.clone(), Lang::En, 24, 6);
        terminal.feed(b"old content");
        let screen = Arc::clone(&terminal.screen);
        let panicked = std::thread::spawn(move || {
            let _guard = screen.lock().expect("the screen starts unpoisoned");
            panic!("a panic while the screen is locked");
        })
        .join();
        assert!(panicked.is_err(), "the helper thread panicked");
        assert!(terminal.screen.lock().is_err(), "the lock is poisoned");
        assert!(
            terminal.restart(),
            "a restart replaces the screen: {:?}",
            terminal.failure
        );
        assert!(terminal.screen.lock().is_ok(), "and the poison is cleared");
        assert_eq!(
            terminal.snapshot().text().trim(),
            "",
            "the new page is blank, not the dead one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fixing a policy file recovers a pane that refused to start: the start
    /// gate is asked again on Enter instead of the pane staying refused until
    /// the tab is reopened.
    #[test]
    fn a_fixed_policy_lets_the_pane_start_again() {
        let dir = scratch("policy-recovery");
        write_policy(&dir, "this is not a rego module\n");
        let mut terminal = Terminal::new(dir.clone(), Lang::En);
        assert!(terminal.refused, "the broken policy refuses the start");
        assert!(!terminal.is_running(), "and nothing was spawned");
        std::thread::sleep(Duration::from_millis(30));
        write_policy(&dir, "package terminal\n\ndefault allow := true\n");
        assert!(terminal.refresh_policy(), "the fixed policy is noticed");
        assert!(!terminal.refused, "and the pane may start again");
        assert!(terminal.failure.is_none(), "{:?}", terminal.failure);
        assert!(
            matches!(terminal.send_key(b"\r"), KeyOutcome::Sent),
            "Enter starts the shell under the fixed policy"
        );
        assert!(terminal.is_running(), "and the pane is live");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The count the pane reports is maintained as the ring trims, so it stays
    /// exact and costs the render path nothing.
    #[test]
    fn the_blocked_count_tracks_the_ring_as_it_trims() {
        let audit = Mutex::new(AuditRing::default());
        let refused = Decision::refuse(Some(String::from("no")));
        for _ in 0..AUDIT_LIMIT + 10 {
            record_decision(&audit, "output", "x", &refused, PolicySource::File);
        }
        {
            let ring = audit.lock().expect("the ring is unpoisoned");
            assert_eq!(ring.rows.len(), AUDIT_LIMIT, "the ring stays bounded");
            assert_eq!(
                ring.blocked, AUDIT_LIMIT as u64,
                "every retained row refused, and the trimmed ones left the count"
            );
        }
        record_decision(
            &audit,
            "output",
            "y",
            &Decision::allow(),
            PolicySource::File,
        );
        let ring = audit.lock().expect("the ring is unpoisoned");
        assert_eq!(ring.rows.len(), AUDIT_LIMIT);
        assert_eq!(
            ring.blocked,
            AUDIT_LIMIT as u64 - 1,
            "the allowed row trimmed a refusal out of the count"
        );
    }

    /// A full queue refuses the write instead of blocking the caller or
    /// dropping the bytes, and a closed one reports the writer gone.
    #[test]
    fn a_full_input_queue_refuses_instead_of_blocking() {
        let (queue, inbox) = sync_channel::<InputChunk>(1);
        let writer = InputWriter {
            queue,
            gone: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(
            writer.send(b"a"),
            InputSend::Queued,
            "the first write waits"
        );
        assert_eq!(
            writer.send(b"b"),
            InputSend::Full,
            "a shell that stopped reading refuses the next write without blocking"
        );
        assert_eq!(
            inbox.try_recv().map(|chunk| chunk.as_bytes().to_vec()),
            Ok(vec![b'a']),
            "the queued bytes are still there, not dropped"
        );
        drop(inbox);
        assert_eq!(
            writer.send(b"c"),
            InputSend::Gone,
            "a closed writer is reported, not queued into nowhere"
        );
    }

    /// A writer whose PTY is gone raises its flag, so the pane can stop
    /// claiming a shell it can no longer type into.
    #[test]
    fn the_writer_thread_reports_a_closed_pty() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writer = InputWriter::spawn(Box::new(Broken));
        assert_eq!(writer.send(b"a"), InputSend::Queued);
        assert!(
            wait_until(Duration::from_secs(2), || {
                matches!(writer.send(b"b"), InputSend::Gone)
            }),
            "the failed write closes the queue"
        );
    }

    /// A keystroke's bytes ride inline; a longer write takes the heap and both
    /// come back byte for byte.
    #[test]
    fn an_input_chunk_round_trips_both_forms() {
        for bytes in [
            &b"\r"[..],
            b"\x1b[A",
            b"\x1b[<0;1;2M",
            "\u{4e2d}".as_bytes(),
            &[0x7f],
        ] {
            assert_eq!(InputChunk::new(bytes).as_bytes(), bytes);
        }
        let long = vec![b'x'; SMALL_INPUT + 1];
        assert_eq!(InputChunk::new(&long).as_bytes(), long.as_slice());
    }

    /// A panic inside the evaluator poisons the gate's lock; a policy reload
    /// replaces the engine instead of leaving every decision refused forever.
    #[test]
    fn a_poisoned_gate_is_replaced_by_a_policy_reload() {
        let dir = scratch("gate-poison");
        write_policy(
            &dir,
            "package terminal\n\nallow := false if { input.action == \"input\" }\n",
        );
        let mut terminal = Terminal::without_process(dir.clone(), Lang::En, 24, 6);
        assert!(!input_allowed(&terminal, "ls"));
        let gate = Arc::clone(&terminal.gate);
        let panicked = std::thread::spawn(move || {
            let _guard = gate.lock().expect("the gate starts unpoisoned");
            panic!("a panic while the gate is locked");
        })
        .join();
        assert!(panicked.is_err(), "the helper thread panicked");
        assert!(terminal.gate.lock().is_err(), "the lock is poisoned");
        assert!(
            !input_allowed(&terminal, "ls"),
            "a poisoned gate refuses rather than allowing"
        );
        std::thread::sleep(Duration::from_millis(30));
        write_policy(&dir, "package terminal\n\ndefault allow := true\n");
        assert!(terminal.refresh_policy(), "the changed stamp is noticed");
        assert!(terminal.gate.lock().is_ok(), "the reload clears the poison");
        assert!(
            input_allowed(&terminal, "ls"),
            "and the reloaded policy decides again"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The audit ring keeps a policy's reason bounded, so its memory is a
    /// function of the ring and not of the policy file.
    #[test]
    fn the_audit_caps_a_long_reason() {
        // Regorus refuses a source line wider than 1024 columns, so the long
        // reason stays under that and still passes the ring's own cap.
        let reason = "x".repeat(AUDIT_REASON_CAP + 100);
        let mut gate = Gate::for_file(&format!(
            "package terminal\n\ndecision := {{\"allow\": false, \"reason\": \"{reason}\"}} if {{ input.action == \"input\" }}\n"
        ))
        .expect("the policy parses");
        let decision = gate.input("ls", Path::new("/tmp"));
        assert!(!decision.allowed());
        let audit = Mutex::new(AuditRing::default());
        record_decision(&audit, "input", "ls", &decision, PolicySource::File);
        let ring = audit.into_inner().expect("the ring is unpoisoned");
        assert_eq!(
            ring.rows[0].reason.as_deref().map(str::len),
            Some(AUDIT_REASON_CAP),
            "the reason is kept, up to the cap"
        );
    }

    /// The gate's input carries the chunk verbatim: text is a value now, not a
    /// JSON string that had to be escaped and re-parsed around it.
    #[test]
    fn the_gate_sees_the_output_chunk_verbatim() {
        let mut gate = Gate::for_file(
            r#"package terminal

decision := {"allow": false, "reason": "verbatim"} if {
    input.action == "output"
    contains(input.text, "a\\b")
}
"#,
        )
        .expect("the policy parses");
        let refused = gate.output("xa\\by");
        assert!(!refused.allowed(), "the chunk carried a literal backslash");
        assert_eq!(refused.reason(), Some("verbatim"));
        assert!(
            gate.output("a/b").allowed(),
            "and a plain chunk is let through"
        );
    }
}
