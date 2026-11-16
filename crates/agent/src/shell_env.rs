//! The environment a shell command inherits from the reader's login shell.
//!
//! An app launched from Finder starts with the system's minimal environment:
//! `PATH` has no Homebrew, mise or asdf, and none of the reader's profile has
//! run. Every command the tool starts would then disagree with the terminal the
//! reader is looking at - `cargo` exists in one and not the other.
//!
//! The login shell is asked once, for its whole environment, and that snapshot
//! is merged over the process's own for each command. Running the profile
//! inside the sandbox is not an option (reads of `$HOME` are denied by design),
//! so the profile's effect is what is inherited, not its code.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a profile may take before the command runs on the process
/// environment instead.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);

/// Keys that describe the capturing shell, not the command that inherits them.
const NOT_INHERITED: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_", "TERM", "PS1", "PROMPT"];

/// The reader's login-shell environment, captured once per process.
pub(crate) fn login_environment() -> &'static HashMap<String, String> {
    static CAPTURED: OnceLock<HashMap<String, String>> = OnceLock::new();
    CAPTURED.get_or_init(capture)
}

/// Merge the login shell's environment over the process's own.
///
/// On Windows variable names are case-insensitive: a snapshot PATH replaces
/// an inherited Path instead of creating a duplicate the child would resolve
/// ambiguously (issue #317).
pub(crate) fn inherit(base: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut envs: HashMap<String, String> = base.into_iter().collect();
    for (key, value) in login_environment() {
        if NOT_INHERITED
            .iter()
            .any(|skip| skip.eq_ignore_ascii_case(key))
        {
            continue;
        }
        if cfg!(windows) {
            envs.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        }
        envs.insert(key.clone(), value.clone());
    }
    envs.into_iter().collect()
}

/// Ask the reader's shell for its environment, bounded by `CAPTURE_TIMEOUT`.
fn capture() -> HashMap<String, String> {
    #[cfg(unix)]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let Ok(mut child) = Command::new(&shell)
            .args(["-l", "-i", "-c", "env -0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return HashMap::new();
        };
        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let mut bytes = Vec::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = std::io::Read::read_to_end(&mut stdout, &mut bytes);
                    }
                    return parse(&bytes);
                }
                Ok(None) => {}
                Err(_) => return HashMap::new(),
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return HashMap::new();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        // Windows has no login shell to ask. The deterministic equivalent is
        // the native user environment: the process environment overlaid with
        // the persisted HKCU\Environment values, so profile-installed
        // toolchains are visible to a desktop-launched BSH (issue #317).
        // hostkit reads the registry through native APIs, bounded, and falls
        // back to the process environment on failure — never an empty map
        // that would silently strip PATH.
        hostkit::env::user_environment()
            .iter()
            .map(|(key, value)| {
                // Upper-case the names so the Unix-shaped consumers of this
                // map (PATH lookups, the sandbox grant walk) find the Windows
                // variables, whose native casing is Path/SystemRoot/etc.
                (
                    key.to_string_lossy().to_ascii_uppercase(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }
}

/// Parse the NUL-separated `name=value` list `env -0` prints.
fn parse(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| {
            let text = String::from_utf8_lossy(chunk);
            let (key, value) = text.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_separated_entries_become_pairs() {
        let env = parse(b"PATH=/usr/bin:/bin\0LANG=zh_CN.UTF-8\0EMPTY=\0");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
    }

    #[test]
    fn entries_without_a_separator_are_ignored() {
        let env = parse(b"NOT-AN-ENTRY\0PATH=/bin\0");
        assert!(env.contains_key("PATH"));
        assert!(!env.contains_key("NOT-AN-ENTRY"));
    }

    #[test]
    fn the_process_environment_survives_the_merge() {
        let merged = inherit(vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("KEEP".to_string(), "yes".to_string()),
        ]);
        assert!(merged
            .iter()
            .any(|(key, value)| key == "KEEP" && value == "yes"));
    }

    #[test]
    fn shell_state_is_not_inherited() {
        assert!(NOT_INHERITED.contains(&"PWD"));
        assert!(NOT_INHERITED.contains(&"PS1"));
    }
}
