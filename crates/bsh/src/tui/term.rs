//! Terminal driver for the TUI client.
//!
//! The terminal interface owns no execution and no durable state, so its I/O
//! surface is deliberately small: [`ratatui`] owns the frame, the alternate
//! screen and raw mode, and crossterm - re-exported by ratatui, so the backend
//! and the events are always the same version - owns key input.

use std::io;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::{DefaultTerminal, Frame};

/// One key the application understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    /// A printable character, inserted at the input cursor.
    Char(char),
    /// Submit the input block.
    Enter,
    /// Insert a line break in the input block.
    Newline,
    /// Delete the character before the cursor.
    Backspace,
    /// Clear the input line.
    Escape,
    /// Move the input cursor left.
    Left,
    /// Move the input cursor right.
    Right,
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
                // The wheel is the one mouse event this client binds; a click,
                // a drag or a moved pointer is not a key and is dropped.
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => return Ok(Some(Key::ScrollUp)),
                    MouseEventKind::ScrollDown => return Ok(Some(Key::ScrollDown)),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Leave the alternate screen and restore the terminal mode.
    pub(crate) fn restore(&mut self) {
        // Mouse capture is released before the screen is: `try_restore` undoes
        // raw mode and the alternate screen but knows nothing about it, and a
        // shell left in capture mode would swallow the reader's selection.
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
        KeyCode::Esc => Some(Key::Escape),
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        KeyCode::Tab => Some(Key::Tab),
        _ => None,
    }
}
