//! Native desktop chat GUI for BOS, built on [Tauri](https://tauri.app).
//!
//! The Rust side exposes IPC commands — session management, message
//! streaming, and settings — to a static HTML/CSS/JS frontend (`ui/`).
//! Streaming chunks are forwarded to the webview as `agent-event`
//! events; sessions persist under `~/.bos/gui/sessions/`.

#![warn(missing_docs)]

mod app;
mod approval;
mod caps;
mod compact;
mod init;
mod instructions;
mod mcp;
mod mentions;
mod runner;
mod session;
mod settings;
#[cfg(test)]
mod verify;

/// Launch the GUI (opens the Tauri window and blocks until quit).
pub fn run() -> anyhow::Result<()> {
    crate::app::run()
}
