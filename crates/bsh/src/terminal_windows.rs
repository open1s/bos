//! The Windows terminal pane's ConPTY host (issue #296).
//!
//! macOS and Linux draw Ghostty's native surface; Windows has none, so the pane
//! spawns the shell attached to a Windows pseudoconsole (ConPTY), parses its
//! output into a software screen ([`crate::terminal_grid`]) and draws that grid.
//! This module owns the process and the reader thread; the view owns rendering
//! and input.
//!
//! Confinement: a ConPTY shell is NOT launched inside the turn's AppContainer,
//! so the pane reports an unconfined terminal truthfully (the same gate as the
//! interactive terminal, issue #323).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::terminal_grid::{Cell, Screen};

/// A live ConPTY-backed terminal: the shell process, its input pipe and the
/// software screen the reader thread keeps current.
pub(crate) struct ConPtyTerminal {
    process: conpty::Process,
    input: conpty::io::PipeWriter,
    screen: Arc<Mutex<Screen>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

/// Pick the Windows shell: PowerShell (pwsh, then powershell) with a cmd.exe
/// fallback, matching the host terminal service (issue #298).
pub(crate) fn default_shell() -> String {
    for candidate in ["pwsh", "powershell"] {
        if which(candidate) {
            return format!("{candidate} -NoLogo -NoProfile");
        }
    }
    "cmd".to_string()
}

/// Whether an executable resolves on PATH.
fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .any(|dir| dir.join(format!("{name}.exe")).is_file() || dir.join(name).is_file())
        })
        .unwrap_or(false)
}

impl ConPtyTerminal {
    /// Spawn the shell attached to a new pseudoconsole of the given size.
    pub(crate) fn spawn(command: &str, rows: u16, cols: u16) -> Result<Self, String> {
        let mut process = conpty::spawn(command).map_err(|e| format!("ConPTY spawn: {e}"))?;
        // ConPTY sizes are (columns, rows).
        let _ = process.resize(cols as i16, rows as i16);
        let input = process.input().map_err(|e| format!("ConPTY input: {e}"))?;
        let mut output = process
            .output()
            .map_err(|e| format!("ConPTY output: {e}"))?;
        let screen = Arc::new(Mutex::new(Screen::new(rows, cols)));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // The reader thread drains the pseudoconsole into the screen until the
        // shell exits; the view reads the screen on its own cadence.
        let reader_screen = Arc::clone(&screen);
        let reader_alive = Arc::clone(&alive);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match output.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut screen) = reader_screen.lock() {
                            screen.feed(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
            reader_alive.store(false, std::sync::atomic::Ordering::SeqCst);
        });

        Ok(Self {
            process,
            input,
            screen,
            alive,
        })
    }

    /// Whether the shell is still running.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::SeqCst) && self.process.is_alive()
    }

    /// Write keystrokes (already encoded as terminal input) to the shell.
    pub(crate) fn write_input(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.input
            .write_all(bytes)
            .map_err(|e| format!("ConPTY write: {e}"))?;
        self.input.flush().map_err(|e| format!("ConPTY flush: {e}"))
    }

    /// Resize the pseudoconsole and the screen to the pane's new cell grid.
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self.process.resize(cols as i16, rows as i16);
        if let Ok(mut screen) = self.screen.lock() {
            screen.resize(rows, cols);
        }
    }

    /// Snapshot the visible grid for rendering.
    pub(crate) fn cells(&self) -> Vec<Vec<Cell>> {
        self.screen
            .lock()
            .map(|screen| screen.cells())
            .unwrap_or_default()
    }

    /// The shell's process id, for diagnostics and tree cleanup.
    pub(crate) fn pid(&self) -> u32 {
        self.process.pid()
    }

    /// Terminate the shell and release the pseudoconsole.
    pub(crate) fn kill(&mut self) {
        let _ = self.process.exit(0);
        self.alive.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for ConPtyTerminal {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shell picker prefers PowerShell and falls back to cmd (issue #296).
    #[test]
    fn default_shell_is_a_windows_shell() {
        let shell = default_shell();
        assert!(
            shell.starts_with("pwsh") || shell.starts_with("powershell") || shell == "cmd",
            "unexpected shell {shell:?}"
        );
    }

    /// A real ConPTY spawn runs a command and its output reaches the screen
    /// grid (issue #296). Runs on the Windows CI lane.
    #[test]
    fn a_conpty_spawn_reaches_the_screen() {
        let mut terminal = ConPtyTerminal::spawn("echo bsh-conpty-ok", 24, 80).expect("spawn");
        // Give the shell a moment to run and the reader thread to drain it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            let text: String = terminal
                .cells()
                .iter()
                .map(|row| row.iter().map(|c| c.text.clone()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            if text.contains("bsh-conpty-ok") {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        terminal.kill();
        assert!(found, "the command output reached the screen grid");
    }
}
