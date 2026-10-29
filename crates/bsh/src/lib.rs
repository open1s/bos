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
pub(crate) mod audit;
pub(crate) mod browser;
pub(crate) mod caps;
pub(crate) mod context;
mod editors;
pub(crate) mod export;
pub(crate) mod files;
pub(crate) mod glob;
pub(crate) mod grep;
pub(crate) mod history;
pub(crate) mod i18n;
pub(crate) mod installed;
pub(crate) mod instructions;
pub(crate) mod market;
pub(crate) mod mcp;
pub(crate) mod model;
pub(crate) mod notes;
pub(crate) mod plugins;
pub(crate) mod preset_catalogue;
pub(crate) mod prompts;
pub(crate) mod reference;
pub(crate) mod runner;
pub(crate) mod settings;
pub(crate) mod skill_invocation;
pub(crate) mod state;
pub(crate) mod store;
pub(crate) mod subagent;
pub(crate) mod terminal;
pub(crate) mod theme;
pub(crate) mod trash;
pub(crate) mod ui;
pub(crate) mod vault;
/// The local web-search tool: the model's own search client.
pub(crate) mod web;
pub(crate) mod workspace_templates;

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
    // The window closed, so no plugin process may outlive it. A bridged plugin is
    // a child this app started; leaving it running would be a leaked process the
    // reader never asked for and cannot see. The sessions are also reconciled on
    // every agent build and reaped by their own `Drop`, so this is the belt to
    // that pair of braces rather than the only guarantee.
    caps::stop_bridge_pool();
}
