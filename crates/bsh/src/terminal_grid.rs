//! A software terminal screen for the Windows pane (issue #296).
//!
//! macOS and Linux draw Ghostty's own native surface; Windows has no Ghostty
//! render surface, so the pane parses the ConPTY byte stream with a VT parser
//! into a cell grid and draws that grid itself. This module is the grid half —
//! pure Rust and cross-platform, so its parsing is unit-tested on every host;
//! the ConPTY host that feeds it is Windows-only (`terminal_windows`).

/// One rendered cell: a character plus its foreground/background palette index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The cell's text (empty for a blank cell).
    pub(crate) text: String,
    /// Foreground colour as 0xRRGGBB.
    pub(crate) fg: u32,
    /// Background colour as 0xRRGGBB.
    pub(crate) bg: u32,
    /// Whether the cell is part of the cursor.
    pub(crate) cursor: bool,
}

/// A terminal screen: a rows x cols grid fed by a VT parser.
pub(crate) struct Screen {
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
}

/// vt100 sizes are non-zero; clamp a dimension to at least one row/column.
fn nz(value: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(value.max(1)).expect("clamped to >= 1")
}

impl Screen {
    /// A new screen of the given size.
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(nz(rows), nz(cols), 0),
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }

    /// Feed PTY output bytes into the parser.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Resize the grid (a pane resize).
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
    }

    /// The current grid size.
    pub(crate) fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Snapshot the visible grid as rows of cells, with the cursor marked.
    pub(crate) fn cells(&self) -> Vec<Vec<Cell>> {
        let screen = self.parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let mut rows = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            let mut cells = Vec::with_capacity(self.cols as usize);
            for col in 0..self.cols {
                let cell = screen.cell(row, col);
                let (text, fg, bg) = match cell {
                    Some(cell) => (
                        cell.contents().to_string(),
                        color_to_rgb(cell.fgcolor()),
                        color_to_rgb(cell.bgcolor()),
                    ),
                    None => (String::new(), DEFAULT_FG, DEFAULT_BG),
                };
                cells.push(Cell {
                    text,
                    fg,
                    bg,
                    cursor: row == cursor_row && col == cursor_col,
                });
            }
            rows.push(cells);
        }
        rows
    }
}

/// The default palette when a cell names no colour.
const DEFAULT_FG: u32 = 0xF0F0F0;
const DEFAULT_BG: u32 = 0x0A0A0A;

/// Map a vt100 colour to 0xRRGGBB.
fn color_to_rgb(color: vt100::Color) -> u32 {
    match color {
        vt100::Color::Default => DEFAULT_FG,
        vt100::Color::Idx(idx) => palette_rgb(idx),
        vt100::Color::Rgb(r, g, b) => ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
    }
}

/// The xterm 256-colour palette (the 16 base colours plus the 6x6x6 cube and
/// the grayscale ramp).
fn palette_rgb(idx: u8) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xCD0000, 0x00CD00, 0xCDCD00, 0x0000EE, 0xCD00CD, 0x00CDCD, 0xE5E5E5, 0x7F7F7F,
        0xFF0000, 0x00FF00, 0xFFFF00, 0x5C5CFF, 0xFF00FF, 0x00FFFF, 0xFFFFFF,
    ];
    match idx {
        0..=15 => BASE[idx as usize],
        16..=231 => {
            let n = idx - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            ((step(r) as u32) << 16) | ((step(g) as u32) << 8) | step(b) as u32
        }
        _ => {
            let gray = 8 + (idx - 232) * 10;
            ((gray as u32) << 16) | ((gray as u32) << 8) | gray as u32
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_lands_in_the_grid() {
        let mut screen = Screen::new(3, 20);
        screen.feed(b"hello");
        let cells = screen.cells();
        let row: String = cells[0].iter().map(|c| c.text.clone()).collect();
        assert!(row.starts_with("hello"), "row was {row:?}");
    }

    #[test]
    fn a_newline_moves_to_the_next_row() {
        let mut screen = Screen::new(3, 20);
        screen.feed(
            b"one
two",
        );
        let cells = screen.cells();
        let first: String = cells[0].iter().map(|c| c.text.clone()).collect();
        let second: String = cells[1].iter().map(|c| c.text.clone()).collect();
        assert!(first.starts_with("one"));
        assert!(second.starts_with("two"));
    }

    #[test]
    fn an_ansi_colour_is_parsed() {
        let mut screen = Screen::new(2, 20);
        // Red foreground, then text.
        screen.feed(b"[31mred");
        let cells = screen.cells();
        let red_cell = &cells[0][0];
        assert_eq!(red_cell.text, "r");
        assert_eq!(red_cell.fg, 0xCD0000);
    }

    #[test]
    fn resize_changes_the_grid() {
        let mut screen = Screen::new(3, 20);
        screen.resize(5, 40);
        assert_eq!(screen.size(), (5, 40));
        assert_eq!(screen.cells().len(), 5);
        assert_eq!(screen.cells()[0].len(), 40);
    }

    #[test]
    fn the_cursor_is_marked() {
        let mut screen = Screen::new(2, 20);
        screen.feed(b"ab");
        let cells = screen.cells();
        // The cursor sits just after the typed text.
        assert!(cells[0][2].cursor);
    }
}
