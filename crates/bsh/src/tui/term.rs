//! Terminal driver for the TUI client.
//!
//! The terminal interface owns no execution and no durable state, so its I/O
//! surface is deliberately small: [`ratatui`] owns the frame, the alternate
//! screen and raw mode, and crossterm - re-exported by ratatui, so the backend
//! and the events are always the same version - owns key input.

use std::io::{self, Write};
use std::time::Duration;

use base64::Engine as _;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::{DefaultTerminal, Frame};

/// One key the application understands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// A printable character, inserted at the input cursor.
    Char(char),
    /// Submit the input block.
    Enter,
    /// Insert a line break in the input block.
    Newline,
    /// Delete the character before the cursor.
    Backspace,
    /// Delete the character under the cursor.
    Delete,
    /// Clear the input line.
    Escape,
    /// Move the input cursor left.
    Left,
    /// Move the input cursor right.
    Right,
    /// Move the input cursor to the start of its line.
    Home,
    /// Move the input cursor past the end of its line.
    End,
    /// Move the input cursor back to the start of the previous word.
    WordLeft,
    /// Move the input cursor past the end of the next word.
    WordRight,
    /// Recall the previous prompt line.
    Up,
    /// Recall the next prompt line.
    Down,
    /// Scroll the transcript up by one page.
    PageUp,
    /// Scroll the transcript down by one page.
    PageDown,
    /// Scroll the transcript up one wheel notch.
    ScrollUp,
    /// Scroll the transcript down one wheel notch.
    ScrollDown,
    /// Toggle the open session's plan mode.
    Tab,
    /// A control chord: `Ctrl('c')` for `^C`.
    Ctrl(char),
    /// Text the terminal delivered as one paste, newlines and all.
    ///
    /// A paste is not a sequence of keystrokes: pressing Enter after every
    /// pasted line would send the first one and lose the rest, so the whole
    /// block lands in the composer and nothing is submitted.
    Paste(String),
    /// A left click at one cell, in the frame's own coordinates.
    ///
    /// The frame decides what is under the pointer - a row of the session
    /// rail - because only the client knows what it painted where.
    Click { row: usize, column: usize },
}

/// The most text one clipboard escape carries, in bytes.
///
/// OSC 52 inlines the payload in the sequence the terminal parses, so a
/// multi-megabyte selection is one enormous line to the emulator, and a
/// terminal that caps the payload drops the selection outright instead of
/// shortening it. Nothing a reader pastes elsewhere is worth either failure,
/// so the copy is cut here and the caller says that it was.
pub(crate) const COPY_LIMIT: usize = 100_000;

/// What one copy put on the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Copied {
    /// How many characters of the message went out.
    pub(crate) chars: usize,
    /// Whether the message was longer than one escape carries.
    pub(crate) cut: bool,
}

/// Put `text` on the reader's clipboard through the terminal.
///
/// OSC 52 is the only clipboard a terminal client can reach, and it is the
/// right one here: it asks the emulator in front of the reader - including
/// over SSH, in tmux and behind a screen - to set the selection, so it works
/// where a native clipboard API cannot. The sequence paints nothing, so the
/// frame the client is drawing is untouched.
pub(crate) fn copy(text: &str) -> io::Result<Copied> {
    let (kept, cut) = carried(text);
    let sequence = copy_sequence(kept);
    let mut out = io::stdout();
    out.write_all(sequence.as_bytes())?;
    out.flush()?;
    Ok(Copied {
        chars: kept.chars().count(),
        cut,
    })
}

/// The part of `text` one copy carries, and whether anything was left behind.
///
/// The cut lands on a character boundary, so the base64 payload decodes back
/// to text rather than to the escaped half of a multi-byte character.
fn carried(text: &str) -> (&str, bool) {
    let end = text
        .char_indices()
        .map(|(index, character)| index + character.len_utf8())
        .take_while(|end| *end <= COPY_LIMIT)
        .last()
        .unwrap_or(0);
    (&text[..end], end < text.len())
}

/// The OSC 52 sequence that makes `text` the clipboard's contents.
///
/// The payload is base64 because the protocol says so: control bytes, and the
/// BEL that terminates the sequence, may not appear inside it.
fn copy_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

/// The terminal in alternate-screen mode, restored on drop.
pub(crate) struct Terminal {
    inner: DefaultTerminal,
}

impl Terminal {
    /// Enter the alternate screen and raw mode.
    pub(crate) fn new() -> io::Result<Self> {
        // `try_init` enables raw mode, switches to the alternate screen and
        // installs a panic hook that restores both, so a panic in this client
        // cannot leave the user's shell without echo or a cursor.
        let inner = ratatui::try_init()?;
        // Mouse capture is this client's own addition: without it the wheel
        // never arrives as an event, and a long transcript could only be
        // scrolled a page at a time. It is undone in `restore`.
        execute!(io::stdout(), EnableMouseCapture)?;
        // Bracketed paste is the other: a terminal that is not asked sends a
        // pasted block as keys, so a pasted newline would submit the first
        // line of it. Being asked, it sends one `Paste` event instead, and
        // this client inserts the whole block without sending anything.
        execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(Self { inner })
    }

    /// Paint one frame. The renderer gets the terminal's own size, so no
    /// separate resize polling is needed.
    pub(crate) fn draw(&mut self, render: impl FnOnce(&mut Frame)) -> io::Result<()> {
        self.inner.draw(render)?;
        Ok(())
    }

    /// Wait at most `timeout` for one key; `None` means the timeout elapsed or
    /// the terminal was resized (both are "redraw, no input").
    pub(crate) fn next_key(&mut self, timeout: Duration) -> io::Result<Option<Key>> {
        loop {
            if !event::poll(timeout)? {
                return Ok(None);
            }
            match event::read()? {
                Event::Key(key) => {
                    if let Some(key) = decode(key) {
                        return Ok(Some(key));
                    }
                }
                // A resize invalidates the last frame: end the wait so the
                // loop repaints against the new size.
                Event::Resize(_, _) => return Ok(None),
                // The wheel scrolls, and a left click is a selection; a drag
                // or a moved pointer is not a key and is dropped.
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => return Ok(Some(Key::ScrollUp)),
                    MouseEventKind::ScrollDown => return Ok(Some(Key::ScrollDown)),
                    MouseEventKind::Down(MouseButton::Left) => {
                        return Ok(Some(Key::Click {
                            row: usize::from(mouse.row),
                            column: usize::from(mouse.column),
                        }));
                    }
                    _ => {}
                },
                // A bracketed paste arrives as one event carrying its own
                // newlines, which is exactly what the composer wants.
                Event::Paste(text) => return Ok(Some(Key::Paste(text))),
                _ => {}
            }
        }
    }

    /// Hand the terminal back for the length of `run`, then take it again.
    ///
    /// The reader's editor is a full-screen program of its own: it needs their
    /// screen, cooked input and the cursor the frame borrowed. So the frame is
    /// given back, `run` happens on the reader's own terminal, and the alternate
    /// screen is entered again afterwards. No state crosses this boundary on
    /// purpose - the client repaints from the Host on the next frame, which is
    /// the whole point of keeping none here.
    pub(crate) fn hand_over<T>(&mut self, run: impl FnOnce() -> T) -> io::Result<T> {
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::try_restore()?;
        let value = run();
        self.inner = ratatui::try_init()?;
        execute!(io::stdout(), EnableMouseCapture)?;
        execute!(io::stdout(), EnableBracketedPaste)?;
        Ok(value)
    }

    /// Leave the alternate screen and restore the terminal mode.
    pub(crate) fn restore(&mut self) {
        // Mouse capture and the paste bracket are released before the screen
        // is: `try_restore` undoes raw mode and the alternate screen but knows
        // nothing about either, and a shell left in capture mode would swallow
        // the reader's selection or print the bracket markers around a paste.
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = execute!(io::stdout(), DisableMouseCapture);
        let _ = ratatui::try_restore();
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Map one crossterm key event to the key the application understands.
/// `None` means "not a binding" (a function key, a chord with Alt, a release).
fn decode(key: KeyEvent) -> Option<Key> {
    // Windows reports separate press/release; only a press is input.
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        // A control chord keeps its character: the shell pane forwards them as
        // their C0 bytes, and its own close chord is Ctrl-], not a letter.
        KeyCode::Char(character) if control => Some(Key::Ctrl(character.to_ascii_lowercase())),
        // Alt chords belong to the user's terminal/shell, not to this client.
        KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::ALT) => None,
        KeyCode::Char(character) => Some(Key::Char(character)),
        // Shift-Enter is a line break, the desktop composer's own binding: a
        // prompt can carry several lines, and Enter alone still sends.
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => Some(Key::Newline),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Delete => Some(Key::Delete),
        KeyCode::Esc => Some(Key::Escape),
        // Alt/Ctrl plus an arrow is the word step every shell and editor binds,
        // so a composer that ignored the modifier would move one character when
        // the reader asked for one word.
        KeyCode::Left
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
        {
            Some(Key::WordLeft)
        }
        KeyCode::Right
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) =>
        {
            Some(Key::WordRight)
        }
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Home => Some(Key::Home),
        KeyCode::End => Some(Key::End),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        KeyCode::Tab => Some(Key::Tab),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_one_osc_sequence_around_the_encoded_text() {
        let sequence = copy_sequence("héllo\nsecond line");
        let payload = sequence
            .strip_prefix("\x1b]52;c;")
            .and_then(|rest| rest.strip_suffix('\x07'))
            .expect("one OSC 52 sequence, terminated by BEL");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .expect("the payload is base64");
        assert_eq!(
            String::from_utf8(decoded).expect("the payload decodes to text"),
            "héllo\nsecond line"
        );
        // A terminal splits its input on line breaks: the sequence must not
        // carry one, or the rest of the copy would arrive as a keystroke.
        assert!(!sequence.trim_end_matches('\x07').contains(['\n', '\r']));
    }

    #[test]
    fn a_message_past_the_limit_is_cut_on_a_character_boundary() {
        // Two bytes per character, so the limit lands in the middle of one:
        // the cut has to move back to the boundary rather than split it.
        let long = "é".repeat(COPY_LIMIT);
        let (kept, cut) = carried(&long);
        assert!(cut, "a message longer than one escape is cut");
        assert!(kept.len() <= COPY_LIMIT, "the copy stays inside the limit");
        assert_eq!(
            kept.chars().count(),
            COPY_LIMIT / 2,
            "the whole characters that fit are kept"
        );
        // Cutting on a boundary is what makes the payload decode back to text.
        assert_eq!(&long[..kept.len()], kept);
        assert!(carried(&long[..kept.len()]).eq(&(kept, false)));
    }

    #[test]
    fn a_short_message_is_carried_whole() {
        let (kept, cut) = carried("one line");
        assert_eq!(kept, "one line");
        assert!(!cut);
    }
}
