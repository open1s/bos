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
mod export;
mod menu;
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
    let mut app =
        app::App::connect(data_dir, session, workspace).map_err(|error| error.to_string())?;
    let mut terminal = term::Terminal::new().map_err(|error| format!("terminal: {error}"))?;
    let outcome = event_loop(&mut app, &mut terminal);
    terminal.restore();
    outcome
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
                frame.render_widget(Paragraph::new(lines), area);
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
            Ok(Some(key)) => app.handle_key(key),
            Ok(None) => {}
            Err(error) => return Err(format!("terminal: {error}")),
        }
    }
}
