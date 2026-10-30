//! BOS Harness: the native desktop application.
//!
//! All of the work lives in the library target so the shell can be exercised
//! without a display; this binary is only the entry point.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The extension CLI is answered before any window exists. It is a
    // developer surface with its own help, so it owns every argument after
    // `ext`, including --help.
    if args.first().is_some_and(|arg| arg == "ext") {
        std::process::exit(bsh::extension_cli(&args[1..]));
    }
    // --help is answered here so it works without opening a window.
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "bsh - BOS Harness\n\nUSAGE: bsh [--settings[=SECTION]] [--browser=URL]\n       bsh ext <command> [options]\n\n  --settings  open the settings dialog on launch\n  SECTION     the rail's six cells - account | vault | general | models |\n              plugins | agent-presets - or a hosted view: plugin-inventory |\n              shell | agent-loop | tools | mcp | archived-sessions |\n              presentation | workspaces | context | automation (default: account)\n  --browser   open the Browser pane on URL (http or https)\n  ext         run the extension developer CLI; bsh ext help for the commands"
        );
        return;
    }
    bsh::run();
}
