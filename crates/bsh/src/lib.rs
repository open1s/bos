//! BOS Harness - a native desktop harness for the BOS agent framework.
//!
//! The crate is split the way the work is: framework-free state and transport
//! (store, settings, model, runner) underneath a GPUI Kit shell. Nothing in the
//! state layer imports GPUI, so the same code can be driven by a test, a CLI or
//! the window.
#![recursion_limit = "256"]
#![warn(missing_docs)]

rust_i18n::i18n!("locales", fallback = "en");

pub(crate) mod assets;
pub(crate) mod caps;
mod editors;
pub(crate) mod files;
pub(crate) mod i18n;
pub(crate) mod model;
pub(crate) mod notes;
pub(crate) mod plugins;
pub(crate) mod runner;
pub(crate) mod settings;
pub(crate) mod state;
pub(crate) mod store;
pub(crate) mod terminal;
pub(crate) mod theme;
pub(crate) mod ui;

/// Launch the BOS Harness desktop window. Blocks until the window closes.
pub fn run() {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".bos")
        .join("bsh")
        .join("log");
    std::fs::create_dir_all(&dir).ok();
    logging::auto_init_tracing_in(dir, "bsh");
    ui::run(ui::Startup::from_args());
}
