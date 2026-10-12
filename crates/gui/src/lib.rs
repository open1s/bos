//! Native desktop chat GUI for BOS, built on [Tauri](https://tauri.app).
//!
//! The Rust side exposes IPC commands — session management, message
//! streaming, and settings — to a static HTML/CSS/JS frontend (`ui/`).
//! Streaming chunks are forwarded to the webview as `agent-event`
//! events; sessions persist under `~/.bos/gui/sessions/`.

#![warn(missing_docs)]

mod agi;
mod app;
mod approval;
mod caps;
mod compact;
mod context;
mod export;
mod filetree;
mod folders;
mod init;
mod instructions;
mod knowledge;
mod logx;
mod mcp;
mod mcp_policy;
mod mcp_view;
mod mentions;
mod picker;
mod pty;
mod replay;
mod runner;
mod runs;
mod schedule;
mod session;
mod settings;
mod skills;
#[cfg(test)]
mod verify;
mod webtools;
mod workspace_dir;

/// Launch the GUI (opens the Tauri window and blocks until quit).
pub fn run() -> anyhow::Result<()> {
    // The GUI's rotating log, in its own subtree beside the sessions it
    // already owns: the webview's decision trail lands here through `logx`.
    // Before any command can log. No home directory, no file — same
    // best-effort contract as the logging crate's own default.
    if let Some(home) = dirs::home_dir() {
        logging::auto_init_tracing_in(home.join(".bos").join("gui").join("log"), "gui");
    }
    crate::app::run()
}
