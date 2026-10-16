//! Opening the active workspace in an external editor.
//!
//! The reference header carries Open in Zed next to More ways to open. Which
//! editor that is depends on the machine, not on this app: the list is read from
//! the applications directory and matched against the editors that take a folder
//! argument, so nothing is offered that is not installed.

/// Editors that open a folder from the command line, in the order offered.
///
/// Zed leads because the reference names it; the rest are the editors that accept
/// a path the same way.
const EDITORS: [&str; 5] = [
    "Zed",
    "Visual Studio Code",
    "Cursor",
    "Sublime Text",
    "Nova",
];

/// The applications directory this platform installs into.
#[cfg(target_os = "macos")]
const APPS_DIR: &str = "/Applications";
/// The applications directory this platform installs into.
#[cfg(not(target_os = "macos"))]
const APPS_DIR: &str = "/usr/share/applications";

/// The name an editor is installed under.
fn app_bundle(name: &str) -> String {
    format!("{name}.app")
}

/// The known editors present in a directory listing, in offer order.
pub(crate) fn editors_in(listing: &[String]) -> Vec<&'static str> {
    EDITORS
        .iter()
        .copied()
        .filter(|name| listing.iter().any(|entry| entry == &app_bundle(name)))
        .collect()
}

/// The known editors installed on this machine, in offer order.
pub(crate) fn installed_editors() -> Vec<&'static str> {
    let Ok(entries) = std::fs::read_dir(APPS_DIR) else {
        return Vec::new();
    };
    let listing: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    editors_in(&listing)
}

/// The command that opens a folder in an editor, or reveals it when there is none.
pub(crate) fn open_command(editor: Option<&str>, path: &str) -> std::process::Command {
    let mut command = std::process::Command::new(crate::settings::OPENER);
    #[cfg(target_os = "macos")]
    match editor {
        Some(name) => {
            command.arg("-a").arg(name);
        }
        None => {
            command.arg("-R");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = editor;
    command.arg(path);
    command
}

/// Open a folder in an editor, or reveal it in the file browser when there is none.
pub(crate) fn open_in(editor: Option<&str>, path: &str) -> std::io::Result<()> {
    open_command(editor, path).spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{editors_in, open_command};

    /// Only installed editors are offered, and in the order the reference implies.
    #[test]
    fn only_installed_editors_are_offered() {
        let listing: Vec<String> = [
            "Safari.app",
            "Visual Studio Code.app",
            "Zed.app",
            "Notes.app",
        ]
        .iter()
        .map(|name| name.to_string())
        .collect();
        assert_eq!(editors_in(&listing), vec!["Zed", "Visual Studio Code"]);
        assert!(
            editors_in(&["Safari.app".to_string()]).is_empty(),
            "an unlisted application is not an editor to open the workspace in"
        );
    }

    /// The command names the editor and keeps the folder as the last argument.
    #[test]
    fn the_open_command_names_the_editor_and_the_folder() {
        let command = open_command(Some("Zed"), "/tmp/ws");
        assert_eq!(command.get_program(), crate::settings::OPENER);
        #[cfg(target_os = "macos")]
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["-a", "Zed", "/tmp/ws"]
        );
    }

    /// With no known editor the folder is revealed rather than opened in one.
    #[test]
    fn the_fallback_reveals_the_folder() {
        let command = open_command(None, "/tmp/ws");
        assert_eq!(command.get_program(), crate::settings::OPENER);
        #[cfg(target_os = "macos")]
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec!["-R", "/tmp/ws"]
        );
    }
}
