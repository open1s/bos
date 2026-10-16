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
            "bsh - BOS Harness\n\nUSAGE: bsh [--settings[=SECTION]]\n\n  --settings  open the settings dialog on launch\n  SECTION     the rail's five cells - account | general | models | plugins\n              | agent-presets - or a hosted view: plugin-inventory | shell\n              | agent-loop | tools | mcp | archived-sessions | presentation\n              | workspaces | context | automation (default: account)"
        );
        return;
    }
    bsh::run();
}
