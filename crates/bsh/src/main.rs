//! BOS Harness: the native desktop application.
//!
//! All of the work lives in the library target so the shell can be exercised
//! without a display; this binary is only the entry point.

fn main() {
    // --help is answered here so it works without opening a window.
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "bsh - BOS Harness\n\nUSAGE: bsh [--settings[=SECTION]] [--browser=URL]\n\n  --settings  open the settings dialog on launch\n  SECTION     the rail's six cells - account | vault | general | models |\n              plugins | agent-presets - or a hosted view: plugin-inventory |\n              shell | agent-loop | tools | mcp | archived-sessions |\n              presentation | workspaces | context | automation (default: account)\n  --browser   open the Browser pane on URL (http or https)"
        );
        return;
    }
    bsh::run();
}
