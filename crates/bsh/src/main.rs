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

/// Take `--federation-profile NAME` / `--federation-profile=NAME` out of the
/// launch arguments.
///
/// The flag selects which federation node profile this process runs, so a
/// second instance on one machine is a second node instead of a refusal from
/// the first one's locked data directory.
fn split_profile_flag(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut rest = Vec::with_capacity(args.len());
    let mut profile = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(name) = arg.strip_prefix("--federation-profile=") {
            profile = Some(name.to_string());
        } else if arg == "--federation-profile" {
            profile = iter.next().cloned();
        } else {
            rest.push(arg.clone());
        }
    }
    (rest, profile)
}

fn main() {
    let (args, profile) = split_profile_flag(&std::env::args().skip(1).collect::<Vec<_>>());
    if let Some(name) = profile {
        if !bsh::set_federation_profile(&name) {
            eprintln!(
                "'{name}' is not a valid federation profile name (letters, digits, '-', '_', '.')"
            );
            std::process::exit(2);
        }
    }
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
    // Federation management is headless too: a node's identity and peer grants
    // are files, and managing them must not require a window.
    if args
        .first()
        .is_some_and(|arg| arg == "federation" || arg == "fed")
    {
        std::process::exit(bsh::federation_cli(&args[1..]));
    }
    // --help is answered here so it works without opening a window.
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "bsh - BOS Harness\n\nUSAGE: bsh [--settings[=SECTION]] [--browser=URL] [--federation-profile=NAME]\n       bsh ext <command> [options]\n       bsh federation [--profile NAME] <command> [options]\n\n  --settings  open the settings dialog on launch\n  SECTION     a rail cell - account | vault | general | models | web-search |\n              extensions | agent-presets - or a hosted view: tools |\n              plugin-inventory | shell | agent-loop | mcp | archived-sessions |\n              workspaces | context | plugins | automation | policy | extensions\n              (default: account)\n  --browser   open the Browser pane on URL (http or https)
  --federation-profile NAME
              run this instance's federation node under NAME, so several BSH
              instances on one machine are several nodes\n  ext         run the extension developer CLI; bsh ext help for the commands
  federation  manage this node's identity and peer grants; bsh federation help"
        );
        return;
    }
    bsh::run();
}
