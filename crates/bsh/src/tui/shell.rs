//! The terminal pane of the TUI client: one shell on a pseudoconsole.
//!
//! The desktop pane draws Ghostty's native surface on macOS/Linux and a
//! ConPTY-backed software grid on Windows (issue #296). A terminal client has
//! no native view to hand a Metal surface to, so the pane here is the software
//! terminal on every platform: portable-pty owns the shell, the same
//! [`crate::terminal_grid::Screen`] the desktop's Windows pane uses parses its
//! output, and the frame paints the cells instead of a native layer.
//!
//! Confinement: a shell started here is NOT launched inside a turn's
//! AppContainer, exactly like the desktop pane - so the hint row reports an
//! unconfined terminal truthfully rather than implying the turn's sandbox.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::terminal_grid::{Cell, Screen};
use crate::tui::term::Key;

/// A live shell: its process, its input pipe and the screen the reader thread
/// keeps current.
pub(crate) struct Shell {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<Screen>>,
    /// Cleared by the reader thread when the pseudoconsole closes, so the pane
    /// can say the shell exited instead of showing a frozen grid.
    alive: Arc<AtomicBool>,
    /// The program the pane started, split into its words, for the hint row.
    command: String,
}

impl Shell {
    /// Start the user's shell on a new pseudoconsole of the given size.
    ///
    /// `cwd` is the workspace the Host serves, so the shell starts where the
    /// desktop window's terminal starts instead of wherever this client was
    /// launched from.
    pub(crate) fn spawn(rows: u16, cols: u16, cwd: Option<&Path>) -> Result<Self, String> {
        Self::spawn_program(&default_shell(), rows, cols, cwd)
    }

    /// Start one specific program, for tests and for the shell picker.
    fn spawn_program(
        program: &str,
        rows: u16,
        cols: u16,
        cwd: Option<&Path>,
    ) -> Result<Self, String> {
        let mut words = program.split_whitespace();
        let name = words.next().ok_or("empty shell command")?;
        let mut builder = CommandBuilder::new(name);
        for argument in words {
            builder.arg(argument);
        }
        // The parser understands the xterm escapes a modern shell emits; a
        // TERM of "dumb" would turn every prompt into plain text.
        builder.env("TERM", "xterm-256color");
        if let Some(dir) = cwd {
            builder.cwd(dir);
        }

        let pair = native_pty_system()
            .openpty(pty_size(rows, cols))
            .map_err(|error| format!("pty: {error}"))?;
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| format!("spawn {program}: {error}"))?;
        // The slave end belongs to the child; holding it here would keep the
        // master from ever seeing EOF when the shell exits.
        drop(pair.slave);
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("pty input: {error}"))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("pty output: {error}"))?;

        let screen = Arc::new(Mutex::new(Screen::new(rows, cols)));
        let alive = Arc::new(AtomicBool::new(true));
        let reader_screen = Arc::clone(&screen);
        let reader_alive = Arc::clone(&alive);
        // The reader owns the drain so a shell that prints without being
        // asked still paints: the frame only takes the screen's lock briefly.
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if let Ok(mut screen) = reader_screen.lock() {
                            screen.feed(&buffer[..read]);
                        }
                    }
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            master: pair.master,
            child,
            writer,
            screen,
            alive,
            command: program.to_string(),
        })
    }

    /// Whether the shell is still running.
    pub(crate) fn is_alive(&mut self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self.child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => self.alive.load(Ordering::SeqCst),
        }
    }

    /// Write already-encoded keystrokes to the shell.
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        // A shell that exited mid-write fails the write; the pane shows the
        // exit on its hint row, so the error is not worth a status line.
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Resize the pseudoconsole and the screen to the frame's cell grid.
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let (current_rows, current_cols) = self
            .screen
            .lock()
            .map(|screen| screen.size())
            .unwrap_or((0, 0));
        if (current_rows, current_cols) == (rows, cols) {
            return;
        }
        let _ = self.master.resize(pty_size(rows, cols));
        if let Ok(mut screen) = self.screen.lock() {
            screen.resize(rows, cols);
        }
    }

    /// Snapshot the visible grid for the frame.
    pub(crate) fn cells(&self) -> Vec<Vec<Cell>> {
        self.screen
            .lock()
            .map(|screen| screen.cells())
            .unwrap_or_default()
    }

    /// The hint row under the grid: what runs, how to leave, confinement.
    ///
    /// The state comes from [`Shell::is_alive`] rather than the reader thread's
    /// flag alone: a process that exited can be reaped before the pty reaches
    /// EOF, and the pane must say so on the frame that shows it rather than on
    /// the frame after the next read.
    pub(crate) fn hint(&mut self) -> String {
        let state = if self.is_alive() {
            "unconfined"
        } else {
            "shell exited · Ctrl-] closes"
        };
        format!("{} · Ctrl-] close · {state}", self.command)
    }

    /// Terminate the shell and release the pseudoconsole.
    pub(crate) fn kill(&mut self) {
        let _ = self.child.kill();
        self.alive.store(false, Ordering::SeqCst);
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // The pane is a document the reader opened; closing it must not leave
        // a shell behind in the workspace.
        self.kill();
    }
}

/// A pty size with no pixel dimensions (the pane measures in cells only).
fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The shell the pane starts: the reader's own on a Unix host, PowerShell with
/// a cmd fallback on Windows - the same picker the desktop's ConPTY pane makes
/// (issue #298).
#[cfg(not(windows))]
fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .map(|shell| shell.trim().to_string())
        .filter(|shell| !shell.is_empty() && Path::new(shell).is_file())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".to_string()
            } else {
                "/bin/sh".to_string()
            }
        })
}

#[cfg(windows)]
fn default_shell() -> String {
    for candidate in ["pwsh", "powershell"] {
        if which(candidate) {
            return candidate.to_string();
        }
    }
    "cmd".to_string()
}

#[cfg(windows)]
fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .any(|dir| dir.join(format!("{name}.exe")).is_file() || dir.join(name).is_file())
        })
        .unwrap_or(false)
}

/// Encode one decoded key as the bytes a shell expects on its terminal.
///
/// `None` is a key the terminal never sends (nothing is typed into a shell by
/// a page key that the application did not ask for); every binding the
/// transcript uses has a terminal equivalent here, so the shell sees the same
/// keystroke the reader pressed.
pub(crate) fn encode(key: &Key) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = match key {
        Key::Char(character) => character.to_string().into_bytes(),
        // Enter in a shell is a carriage return; a newline would only reach
        // the line editor as `^J`.
        Key::Enter | Key::Newline => b"\r".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        // Modifier arrows are the CSI 1;<mod> form: 5 is Ctrl, 3 is Alt. The
        // pane forwards Ctrl, which is what this client decodes a word step
        // from, so the shell and the composer agree on one spelling.
        Key::WordLeft => b"\x1b[1;5D".to_vec(),
        Key::WordRight => b"\x1b[1;5C".to_vec(),
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::ScrollUp => b"\x1b[A".to_vec(),
        Key::ScrollDown => b"\x1b[B".to_vec(),
        Key::Tab => b"\t".to_vec(),
        // A paste goes to the shell whole, with its newlines as carriage
        // returns: the pane is a terminal, and pasting a command into it is
        // what a reader expects to do.
        Key::Paste(text) => text.replace("\r\n", "\r").replace('\n', "\r").into_bytes(),
        // A click is the frame's gesture, not the shell's: the pane has no
        // pointer protocol, so there is nothing to forward.
        Key::Click { .. } => return None,
        // A control chord is its C0 byte: Ctrl-A is 0x01, Ctrl-] is 0x1d.
        Key::Ctrl(character) => {
            let character = character.to_ascii_uppercase();
            if character.is_ascii_alphabetic() {
                vec![(character as u8) - b'A' + 1]
            } else if character == ']' {
                vec![0x1d]
            } else if character == '\\' {
                vec![0x1c]
            } else {
                return None;
            }
        }
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn plain_keys_encode_to_their_terminal_bytes() {
        assert_eq!(encode(&Key::Char('a')), Some(b"a".to_vec()));
        assert_eq!(encode(&Key::Enter), Some(b"\r".to_vec()));
        assert_eq!(encode(&Key::Tab), Some(b"\t".to_vec()));
        assert_eq!(encode(&Key::Backspace), Some(b"\x7f".to_vec()));
        assert_eq!(encode(&Key::Left), Some(b"\x1b[D".to_vec()));
    }

    /// The line and word steps reach the shell as the sequences its own line
    /// editor reads, so Ctrl-Left inside a shell is one word there too.
    #[test]
    fn the_line_and_word_steps_encode_to_their_sequences() {
        assert_eq!(encode(&Key::Delete), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode(&Key::Home), Some(b"\x1b[H".to_vec()));
        assert_eq!(encode(&Key::End), Some(b"\x1b[F".to_vec()));
        assert_eq!(encode(&Key::WordLeft), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(encode(&Key::WordRight), Some(b"\x1b[1;5C".to_vec()));
    }

    #[test]
    fn a_control_chord_is_its_c0_byte() {
        assert_eq!(encode(&Key::Ctrl('c')), Some(vec![0x03]));
        assert_eq!(encode(&Key::Ctrl('a')), Some(vec![0x01]));
        // The pane's own close chord is the only control byte it reserves.
        assert_eq!(encode(&Key::Ctrl(']')), Some(vec![0x1d]));
    }

    #[test]
    fn an_unmapped_control_chord_encodes_to_nothing() {
        assert_eq!(encode(&Key::Ctrl('1')), None);
    }

    /// A real pseudoconsole: the shell runs, its output reaches the grid and
    /// the pane reports it alive (the terminal view's own end-to-end check).
    #[cfg(unix)]
    #[test]
    fn a_shell_writes_its_output_to_the_grid() {
        let mut shell = Shell::spawn_program("/bin/sh", 24, 80, None).expect("spawn the shell");
        shell.write(b"echo bsh-pty-ok\r");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut grid = String::new();
        while Instant::now() < deadline {
            grid = shell
                .cells()
                .iter()
                .map(|row| row.iter().map(|cell| cell.text.clone()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            if grid.lines().any(|line| line.trim() == "bsh-pty-ok") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(shell.is_alive(), "the shell is still running");
        shell.kill();
        assert!(
            grid.lines().any(|line| line.trim() == "bsh-pty-ok"),
            "the command output reached the grid; grid was {grid:?}"
        );
    }

    /// A shell that exits reports it: the pane must not present a dead grid as
    /// a live terminal.
    #[cfg(unix)]
    #[test]
    fn an_exited_shell_is_reported_dead() {
        let mut shell = Shell::spawn_program("/bin/sh", 24, 80, None).expect("spawn the shell");
        shell.write(b"exit\r");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && shell.is_alive() {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!shell.is_alive(), "the exited shell is reported dead");
    }
}
