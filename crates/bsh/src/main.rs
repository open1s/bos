//! BOS Harness: the native desktop application.
//!
//! All of the work lives in the library target so the shell can be exercised
//! without a display; this binary is only the entry point.

/// Hide the console window Windows gives a console-subsystem binary when it
/// is launched as a GUI.
///
/// Explorer starts a console-subsystem process with a fresh console of its
/// own, which is the window users see behind BSH. Started from an existing
/// terminal, the console also has the shell attached, and hiding it would hide
/// the user's terminal - so only a one-process console is hidden. CLI
/// invocations (`ext`, `--help`) keep their console either way.
#[cfg(windows)]
fn hide_launch_console() {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(process_list: *mut u32, count: u32) -> u32;
        fn GetConsoleWindow() -> *mut c_void;
    }

    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    }

    const SW_HIDE: i32 = 0;

    // SAFETY: the buffer holds two entries and the API writes at most `count`;
    // a null window means there is no console, and ShowWindow accepts any HWND.
    unsafe {
        let mut processes = [0_u32; 2];
        let attached = GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32);
        if attached != 1 {
            return;
        }
        let window = GetConsoleWindow();
        if !window.is_null() {
            ShowWindow(window, SW_HIDE);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A GUI launch from Explorer should not leave a console window behind it.
    // A CLI invocation needs that console for its own output.
    #[cfg(windows)]
    {
        let cli = args.first().is_some_and(|arg| arg == "ext")
            || args.iter().any(|arg| arg == "--help" || arg == "-h");
        if !cli {
            hide_launch_console();
        }
    }
    // The extension CLI is answered before any window exists. It is a
    // developer surface with its own help, so it owns every argument after
    // `ext`, including --help.
    if args.first().is_some_and(|arg| arg == "ext") {
        std::process::exit(bsh::extension_cli(&args[1..]));
    }
    // --help is answered here so it works without opening a window.
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "bsh - BOS Harness\n\nUSAGE: bsh [--settings[=SECTION]] [--browser=URL]\n       bsh ext <command> [options]\n\n  --settings  open the settings dialog on launch\n  SECTION     a rail cell - account | vault | general | models | web-search |\n              extensions | agent-presets - or a hosted view: tools |\n              plugin-inventory | shell | agent-loop | mcp | archived-sessions |\n              workspaces | context | plugins | automation | policy | extensions\n              (default: account)\n  --browser   open the Browser pane on URL (http or https)\n  ext         run the extension developer CLI; bsh ext help for the commands"
        );
        return;
    }
    bsh::run();
}
