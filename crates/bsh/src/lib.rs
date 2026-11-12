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
pub(crate) mod command_invocation;
pub(crate) mod compositions;
pub(crate) mod context;
pub(crate) mod export;
pub(crate) mod extension;
pub(crate) mod federation;
pub(crate) mod federation_audit;
mod federation_knowledge;
mod federation_lock;
mod federation_node;
pub(crate) mod federation_tools;
pub(crate) mod files;
pub(crate) mod glob;
pub(crate) mod grep;
pub(crate) mod history;
pub(crate) mod i18n;
pub(crate) mod installed;
pub(crate) mod instructions;
pub(crate) mod intent;
pub(crate) mod market;
pub(crate) mod mcp;
pub(crate) mod model;
pub(crate) mod notes;
mod open_in_app;
pub(crate) mod plugins;
pub(crate) mod preset_catalogue;
pub(crate) mod prompts;
pub(crate) mod recent;
pub(crate) mod reference;
pub(crate) mod review;
pub(crate) mod runner;
pub(crate) mod schedule;
pub(crate) mod sed;
pub(crate) mod settings;
pub(crate) mod skill_invocation;
pub(crate) mod state;
pub(crate) mod store;
pub(crate) mod subagent;
pub(crate) mod task_tools;
pub(crate) mod terminal;
pub(crate) mod theme;
pub(crate) mod tool_metadata;
pub(crate) mod trash;
pub(crate) mod ui;
pub(crate) mod vault;
/// The local web-search tool: the model's own search client.
pub(crate) mod web;
pub(crate) mod workspace_ignore;
pub(crate) mod workspace_templates;

/// Run the extension developer CLI and return its exit code.
///
/// The entry point is public because the binary answers `bsh ext ...` before
/// it builds a window; everything behind it stays crate-private so the
/// platform's internal types are not a second public API.
pub fn extension_cli(args: &[String]) -> i32 {
    extension::cli(args)
}

/// Run the federation management CLI, returning its exit code.
///
/// This is the headless surface for a node's identity and peer grants: it works
/// before any window exists, and it is the same state a running node loads.
/// Select the federation node profile this process runs.
///
/// The launch flag (`--federation-profile=work`) calls this before the window
/// exists; two app instances on one machine share `settings.toml`, so a
/// per-process switch is what lets the second one run a node of its own.
/// Returns false for a name that cannot be a directory, so the caller can
/// refuse the launch instead of silently falling back to the default node.
pub fn set_federation_profile(name: &str) -> bool {
    if federation::valid_profile(name) {
        federation::set_profile(name);
        true
    } else {
        false
    }
}

/// The first bare argument, when it is not a command this build handles.
///
/// A misspelt subcommand used to fall through to the GUI launch: the app then
/// started a second instance with the DEFAULT federation profile, and the only
/// visible symptom was the data-directory lock warning. A typo is a command
/// error and must be reported as one.
pub fn unknown_launch_command(args: &[String]) -> Option<String> {
    let first = args.first()?;
    if first.starts_with('-') {
        return None;
    }
    if matches!(first.as_str(), "ext" | "federation" | "fed") {
        return None;
    }
    Some(first.clone())
}

#[cfg(test)]
mod launch_command_tests {
    use super::unknown_launch_command;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn known_commands_are_not_typos() {
        for value in ["ext", "federation", "fed"] {
            assert_eq!(unknown_launch_command(&args(&[value])), None, "{value}");
        }
    }

    #[test]
    fn flags_are_not_commands() {
        assert_eq!(unknown_launch_command(&args(&["--settings=tools"])), None);
        assert_eq!(unknown_launch_command(&args(&[])), None);
    }

    #[test]
    fn a_misspelt_command_is_reported() {
        assert_eq!(
            unknown_launch_command(&args(&["federaton", "--profile", "work"])),
            Some("federaton".to_string())
        );
    }
}

/// Run one `bsh federation` command, profile-aware.
///
/// `--profile NAME` (or `-p`) selects which node's files the command reads or
/// writes; without it the saved profile, then `default`. `profiles` lists every
/// node on the machine instead of acting on one.
pub fn federation_cli(args: &[String]) -> i32 {
    let mut out = std::io::stdout();
    // `--profile` (or `-p`) may lead the command, so it is taken out before the
    // command dispatch and every subcommand below is profile-scoped.
    let mut rest: Vec<String> = Vec::with_capacity(args.len());
    let mut requested: Vec<String> = Vec::new();
    let mut grants: Vec<String> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--profile" || arg == "-p" {
            match iter.next() {
                Some(name) => requested.push(name.clone()),
                None => {
                    eprintln!("--profile needs a name");
                    return 2;
                }
            }
        } else if let Some(name) = arg.strip_prefix("--profile=") {
            requested.push(name.to_string());
        } else if arg == "--grant" {
            match iter.next() {
                Some(name) => grants.push(name.clone()),
                None => {
                    eprintln!("--grant needs a capability id");
                    return 2;
                }
            }
        } else if let Some(name) = arg.strip_prefix("--grant=") {
            grants.push(name.to_string());
        } else {
            rest.push(arg.clone());
        }
    }
    for name in &requested {
        if !federation::valid_profile(name) {
            eprintln!(
                "'{name}' is not a valid federation profile name (letters, digits, '-', '_', '.')"
            );
            return 2;
        }
    }
    if let Some(name) = requested.first() {
        federation::set_profile(name);
    }
    let settings = settings::Settings::load();
    let profile = federation::resolve_profile(Some(&settings.global.federation.profile));
    let name = federation::display_name(settings.global.federation.node_name.as_deref(), &profile);
    let command = rest.first().map(String::as_str);
    if command == Some("serve") {
        // One process, one node per profile: repeating --profile runs several.
        let profiles = if requested.is_empty() {
            vec![profile.clone()]
        } else {
            requested.clone()
        };
        return federation::serve(&settings, &profiles);
    }
    if command == Some("pair") {
        let Some(other) = rest.get(1) else {
            eprintln!("pair needs the other profile's name: bsh federation pair <profile>");
            return 2;
        };
        return federation::pair(&federation::dir(), &profile, other, &grants, &mut out);
    }
    if !grants.is_empty() {
        eprintln!("--grant is only meaningful for the pair command");
        return 2;
    }
    if command == Some("profiles") {
        return federation::profiles(
            &federation::dir(),
            settings.global.federation.node_name.as_deref(),
            &mut out,
        );
    }
    federation::command(
        &rest,
        &federation::node_dir_for(&profile),
        Some(&name),
        &mut out,
    )
}

/// Launch the BOS Harness desktop window. Blocks until the window closes.
pub fn run() {
    let dir = settings::dir().join("log");
    std::fs::create_dir_all(&dir).ok();
    logging::auto_init_tracing_in(dir, "bsh");
    // A previous SIGKILL can leave Chromium behind; clean it before the next
    // window, not only when the browser pane is first opened.
    browser::reap_orphans();
    // The federation node starts off the window's path: with the node on by
    // default, opening a Zenoh session must not delay the first frame. A node
    // that cannot start is logged and the app runs without it.
    std::thread::Builder::new()
        .name("bsh-federation".to_string())
        .spawn(|| {
            crate::federation_node::start_from_settings(&crate::settings::Settings::load());
        })
        .ok();
    ui::run(ui::Startup::from_args());
    // The window closed, so no plugin process may outlive it. A bridged plugin is
    // a child this app started; leaving it running would be a leaked process the
    // reader never asked for and cannot see. The sessions are also reconciled on
    // every agent build and reaped by their own `Drop`, so this is the belt to
    // that pair of braces rather than the only guarantee.
    caps::stop_bridge_pool();
    crate::extension::shutdown();
}
