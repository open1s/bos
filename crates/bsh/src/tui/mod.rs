//! The terminal interface (RFC-015) as a client of the Runtime Host.
//!
//! The loop is intentionally dumb: pump committed events, paint a frame, wait
//! briefly for a key. All state lives on the Host, so a TUI that stalls, is
//! suspended or is killed loses nothing but its own view.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ratatui::widgets::Paragraph;

use crate::runtime::ids::SessionId;

mod app;
mod board;
mod editor;
mod export;
mod menu;
mod nav;
mod picker;
mod prefs;
mod shell;
mod skin;
mod term;

/// Attach to the Host that serves `data_dir` and run the interface until the
/// user quits. Returns `Err` only when the client cannot continue (no Host, a
/// closed connection or a terminal failure).
///
/// `workspace` is the folder this client offers in its `@` list: it is the
/// root the Host was handed, so the terminal lists the folder the window would
/// open instead of whichever directory the shell happened to inherit.
pub(crate) fn run(
    data_dir: &Path,
    session: Option<SessionId>,
    workspace: Option<PathBuf>,
) -> Result<(), String> {
    // The frame owns the screen, so nothing else may write to the terminal: a
    // child process or a background thread that prints to stderr lands on top
    // of the composer. Stderr moves to a log file before the Host starts, which
    // is before anything can inherit it.
    #[cfg(unix)]
    let mut log = StderrLog::capture(data_dir);
    let outcome = attach_and_run(data_dir, session, workspace);
    #[cfg(unix)]
    {
        // The terminal is the caller's again, so stderr is handed back before
        // the outcome is said out loud.
        if let Some(log) = log.as_mut() {
            log.restore();
        }
        match (&outcome, &log) {
            (Err(error), Some(log)) => Err(format!(
                "{error} (what the run printed on stderr is in {})",
                log.path.display()
            )),
            _ => outcome,
        }
    }
    #[cfg(not(unix))]
    outcome
}

/// Attach to the Host and run the interface: the part that needs the screen.
fn attach_and_run(
    data_dir: &Path,
    session: Option<SessionId>,
    workspace: Option<PathBuf>,
) -> Result<(), String> {
    let mut app =
        app::App::connect(data_dir, session, workspace).map_err(|error| error.to_string())?;
    let mut terminal = term::Terminal::new().map_err(|error| format!("terminal: {error}"))?;
    let outcome = event_loop(&mut app, &mut terminal);
    terminal.restore();
    outcome
}

/// This process's stderr, moved to a file while the interface runs.
///
/// The words a child process or a background thread prints would otherwise be
/// drawn over the frame - an MCP server's own log line lands on the composer -
/// so they are kept in a file the reader can go back to instead of in the cells
/// the frame owns. Windows keeps its own handle arithmetic and is left alone.
#[cfg(unix)]
struct StderrLog {
    path: PathBuf,
    /// The descriptor stderr pointed at before it was moved.
    saved: i32,
}

#[cfg(unix)]
impl StderrLog {
    /// Move stderr to `data_dir/tui.log`.
    ///
    /// `None` when that cannot be done, in which case stderr stays where it
    /// was: an unwritable log must not stop the interface, and it must not
    /// silence anything either.
    fn capture(data_dir: &Path) -> Option<Self> {
        use std::os::unix::io::AsRawFd;

        let path = data_dir.join("tui.log");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        // SAFETY: descriptor arithmetic on this process's own stderr. `dup`
        // hands back a descriptor this function owns, `dup2` points stderr at
        // the log, and `file` keeps the source descriptor open across the call.
        unsafe {
            let saved = libc::dup(libc::STDERR_FILENO);
            if saved < 0 {
                return None;
            }
            if libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) < 0 {
                libc::close(saved);
                return None;
            }
            Some(Self { path, saved })
        }
    }

    /// Put stderr back where the caller left it, once and only once.
    fn restore(&mut self) {
        if self.saved < 0 {
            return;
        }
        // SAFETY: `saved` is the descriptor `capture` opened and nothing else
        // has closed it; stderr is still this process's to point.
        unsafe {
            libc::dup2(self.saved, libc::STDERR_FILENO);
            libc::close(self.saved);
        }
        self.saved = -1;
    }
}

#[cfg(unix)]
impl Drop for StderrLog {
    fn drop(&mut self) {
        self.restore();
    }
}

fn event_loop(app: &mut app::App, terminal: &mut term::Terminal) -> Result<(), String> {
    loop {
        app.pump().map_err(|error| error.to_string())?;
        terminal
            .draw(|frame| {
                // The frame owns the size, so the client never polls a resize:
                // it lays out against the cells it is about to paint.
                let area = frame.area();
                // The shell pane paints styled cells instead of the text lines
                // every other view returns, so it takes the frame first.
                if let Some((lines, cursor)) =
                    app.shell_lines(area.width as usize, area.height as usize)
                {
                    frame.render_widget(Paragraph::new(lines), area);
                    if let Some((row, column)) = cursor {
                        frame.set_cursor_position((column as u16, row as u16));
                    }
                    return;
                }
                // The renderer answers with painted rows: what a row is about
                // was decided where the row was written, not here.
                let (lines, cursor) = app.render(area.width as usize, area.height as usize);
                frame.render_widget(Paragraph::new(lines).style(app.page_style()), area);
                if let Some((row, column)) = cursor {
                    frame.set_cursor_position((column as u16, row as u16));
                }
            })
            .map_err(|error| format!("terminal: {error}"))?;
        if app.is_quitting() {
            return Ok(());
        }
        // A short key timeout is what makes committed events visible while the
        // user is not typing; the Host, not this loop, drives the turn.
        match terminal.next_key(Duration::from_millis(60)) {
            Ok(Some(key)) => {
                app.handle_key(key);
                // `^X` and `/editor` ask for the reader's own editor, which
                // needs the whole screen: the loop hands the terminal back for
                // exactly as long as that program runs, then hands the answer
                // to the app, which keeps the prompt or replaces it. The app
                // cannot do this itself - the terminal is the loop's.
                if let Some(text) = app.take_edit_request() {
                    let outcome = terminal
                        .hand_over(|| editor::run(&text))
                        .map_err(|error| format!("terminal: {error}"))
                        .and_then(|edited| edited.map_err(|error| error.to_string()));
                    app.after_edit(outcome);
                }
            }
            Ok(None) => {}
            Err(error) => return Err(format!("terminal: {error}")),
        }
    }
}
