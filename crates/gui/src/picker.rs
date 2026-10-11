//! The platform's own folder dialog, asked for over a pipe.
//!
//! A workspace's root is chosen once and then trusted for a long time, so the
//! gesture that picks it should be the one the user already knows: the system's
//! folder panel. The Tauri dialog plugin cannot be used here — it is not in this
//! machine's offline crate cache — and its absence is not a reason to hand-roll a
//! weaker panel. Every desktop already ships a folder chooser that can be driven
//! from a command line, so this module builds that command line and reads the
//! answer back:
//!
//! | Platform | Program | The user's answer |
//! | --- | --- | --- |
//! | macOS | `osascript` (`choose folder`) | `POSIX path of` the chosen alias |
//! | Linux | `zenity --file-selection --directory`, else `kdialog --getexistingdirectory` | the path on stdout |
//! | Windows | PowerShell + `System.Windows.Forms.FolderBrowserDialog` | `SelectedPath` on stdout |
//!
//! Two rules are in this file rather than in the command that calls it, because
//! they are the parts that can be wrong without anyone noticing.
//!
//! **Cancelling is not failing.** Every panel has a way to be dismissed, and the
//! difference between "the user changed their mind" and "the picker is broken"
//! is the whole reason [`ChooseOutcome`] exists. macOS reports `-128` for a
//! cancelled `choose folder`, which the script catches and turns into an empty
//! answer; the other two exit non-zero with empty output. A cancel must leave the
//! field exactly as it was, and must never be written to the UI as an error.
//!
//! **A missing picker is also not failing.** If the platform has no program to
//! ask (a container, a headless session, a Linux box without zenity), the caller
//! gets [`ChooseOutcome::Unavailable`] with a reason it can show — and the
//! in-app folder browser stays reachable as the fallback, because a feature that
//! dies when `osascript` is absent is a feature that dies in CI.
//!
//! Everything that decides anything here is pure: [`command`] builds an argv,
//! [`outcome`] reads a finished process, and [`quote_applescript`] escapes. The
//! one impure function is [`choose`], which is the process spawn, and it is the
//! only thing no test in this crate can drive end to end — a modal panel needs a
//! human. What *is* tested is every decision around it, plus the refusal that
//! keeps the modal from ever being reached on a machine that has none.

use std::path::Path;

/// What the platform's folder panel had to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChooseOutcome {
    /// The user picked this folder. It is a path the OS produced, still to be
    /// resolved by the caller before it is trusted.
    Picked(String),
    /// The user dismissed the panel. Not an error, and not a change.
    Cancelled,
    /// This system has no folder panel to ask, with a reason to show.
    Unavailable(String),
}

/// The platform's folder dialog as an argv, or `None` when this build has none.
///
/// `start` is only used when it is a folder that exists: a panel opened at a
/// path the system cannot find is a panel that reports an error instead of
/// opening, and the user's gesture was "pick a folder", not "retype your path".
pub(crate) fn command(title: &str, start: Option<&str>) -> Option<(String, Vec<String>)> {
    let start = start.filter(|p| Path::new(p).is_dir());
    if cfg!(target_os = "macos") {
        // One `-e` per AppleScript statement: a single long line is harder to
        // escape and much harder to read when the panel misbehaves.
        let mut args: Vec<String> = vec!["-e".into(), "try".into()];
        let mut pick = String::from("set f to choose folder with prompt ");
        pick.push_str(&quote_applescript(title));
        if let Some(dir) = start {
            pick.push_str(" default location POSIX file ");
            pick.push_str(&quote_applescript(dir));
        }
        args.push("-e".into());
        args.push(pick);
        args.push("-e".into());
        args.push("return POSIX path of f".into());
        // The dismissal comes back as an AppleScript error whose number is -128.
        // Catching it here is what keeps "changed my mind" out of the error path.
        args.push("-e".into());
        args.push("on error number -128".into());
        args.push("-e".into());
        args.push("return \"\"".into());
        args.push("-e".into());
        args.push("end try".into());
        return Some(("osascript".into(), args));
    }
    if cfg!(target_os = "windows") {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = {}; \
             if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ $d.SelectedPath }}",
            quote_powershell(title)
        );
        return Some((
            "powershell".into(),
            vec![
                "-NoProfile".into(),
                "-STA".into(),
                "-Command".into(),
                script,
            ],
        ));
    }
    if cfg!(target_os = "linux") {
        // zenity is the one that is usually installed; kdialog is the KDE answer
        // and is tried only if zenity is absent, which `choose` decides by the
        // spawn failing rather than by guessing from PATH.
        let mut args: Vec<String> = vec![
            "--file-selection".into(),
            "--directory".into(),
            format!("--title={title}"),
        ];
        if let Some(dir) = start {
            args.push(format!("--filename={}/", dir.trim_end_matches('/')));
        }
        return Some(("zenity".into(), args));
    }
    None
}

/// The argv for the Linux fallback, tried when `zenity` is not installed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn linux_fallback(title: &str, start: Option<&str>) -> (String, Vec<String>) {
    let mut args: Vec<String> = vec!["--getexistingdirectory".into()];
    args.push(start.filter(|p| Path::new(p).is_dir()).unwrap_or("").into());
    args.push(format!("--title={title}"));
    ("kdialog".into(), args)
}

/// Read a finished dialog process as an outcome.
///
/// `status` is the exit code when the process ran to completion (`None` when it
/// was killed), and the two streams are its text. An empty answer with a clean
/// exit is a cancellation on macOS (whose script returns `""` for `-128`) and on
/// Windows (whose script prints nothing when the dialog is not `OK`); a non-zero
/// exit with nothing on either stream is a cancellation on Linux. Anything else
/// non-zero is a real failure and says so, with the program's own words.
pub(crate) fn outcome(
    program: &str,
    status: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> ChooseOutcome {
    let path = stdout.trim();
    if !path.is_empty() && status == Some(0) {
        return ChooseOutcome::Picked(path.to_string());
    }
    if status == Some(0) {
        return ChooseOutcome::Cancelled;
    }
    let said = {
        let err = stderr.trim();
        if !err.is_empty() {
            err
        } else {
            stdout.trim()
        }
    };
    // `osascript` reports a dismissal this way when the script did not catch it
    // (an older panel, a different locale): `-128` is "User canceled." and must
    // not be shown as a failure.
    if said.contains("-128") || said.contains("User canceled") || said.contains("User cancelled") {
        return ChooseOutcome::Cancelled;
    }
    // 125/126/127 are the shell's "could not run it"; 1 with no output is how
    // zenity and kdialog say "dismissed".
    let dismissed = status == Some(1) && said.is_empty();
    if dismissed {
        return ChooseOutcome::Cancelled;
    }
    if let Some(code) = status {
        if code == 127 || code == 126 || code == 125 {
            return ChooseOutcome::Unavailable(format!("{program} is not available here"));
        }
    }
    if said.is_empty() {
        return ChooseOutcome::Cancelled;
    }
    ChooseOutcome::Unavailable(format!("{program}: {}", first_line(said)))
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Escape a string for an AppleScript string literal.
///
/// Backslash first: escaping the quote and then the backslash would double the
/// escape and change the path. Control characters (a newline cannot appear inside
/// an AppleScript literal at all) are dropped rather than passed through.
pub(crate) fn quote_applescript(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Escape a string for a PowerShell single-quoted literal (doubling `'`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn quote_powershell(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        if ch == '\'' {
            out.push_str("''");
        } else if ch.is_control() {
            // A newline would end the statement; PowerShell has no literal escape
            // for it inside the quotes either.
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Ask the platform for a folder. `Ok(None)` is a dismissal, `Err` a reason.
pub(crate) fn choose(title: &str, start: Option<&str>) -> Result<Option<String>, String> {
    let Some((program, args)) = command(title, start) else {
        return Err(format!(
            "no folder dialog on this platform ({} {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    };
    match run(&program, &args) {
        Ok(outcome) => finish(outcome),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound && cfg!(target_os = "linux") => {
            let (fallback, args) = linux_fallback(title, start);
            match run(&fallback, &args) {
                Ok(outcome) => finish(outcome),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(
                    "no folder dialog here: neither zenity nor kdialog is installed".to_string(),
                ),
                Err(err) => Err(format!("kdialog: {err}")),
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("no folder dialog here: {program} is not installed"))
        }
        Err(err) => Err(format!("{program}: {err}")),
    }
}

fn finish(outcome: ChooseOutcome) -> Result<Option<String>, String> {
    match outcome {
        ChooseOutcome::Picked(path) => Ok(Some(path)),
        ChooseOutcome::Cancelled => Ok(None),
        ChooseOutcome::Unavailable(reason) => Err(reason),
    }
}

fn run(program: &str, args: &[String]) -> std::io::Result<ChooseOutcome> {
    let out = std::process::Command::new(program).args(args).output()?;
    Ok(outcome(
        program,
        out.status.code(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_applescript_literal_escapes_the_quote_and_the_backslash() {
        assert_eq!(quote_applescript("a\"b"), "\"a\\\"b\"");
        // The backslash is escaped first: `C:\tmp` must not double-escape the `t`.
        assert_eq!(quote_applescript("C:\\tmp"), "\"C:\\\\tmp\"");
        // A newline cannot live inside an AppleScript literal, so it is dropped
        // rather than turning the statement into two.
        assert_eq!(quote_applescript("a\nb"), "\"ab\"");
        assert_eq!(quote_applescript("plain"), "\"plain\"");
    }

    #[test]
    fn the_powershell_literal_doubles_the_quote() {
        assert_eq!(quote_powershell("it's"), "'it''s'");
        assert_eq!(quote_powershell("a\nb"), "'ab'");
        assert_eq!(quote_powershell("plain"), "'plain'");
    }

    #[test]
    fn the_command_is_a_folder_only_panel_and_a_start_that_exists() {
        let dir = std::env::temp_dir();
        let (program, args) = command("Pick", Some(&dir.to_string_lossy())).expect("a picker");
        assert!(
            program == "osascript" || program == "zenity" || program == "powershell",
            "{program}"
        );
        let line = args.join(" ");
        assert!(
            line.contains("choose folder") || line.contains("--directory"),
            "{line}"
        );
        // The title travels as one argument, never as part of a shell string.
        assert!(
            args.iter().any(|a| a.contains("Pick")),
            "the title reaches the panel: {args:?}"
        );
        // A start folder that does not exist is left out entirely.
        let (_, none) = command("Pick", Some("/no/such/place")).expect("a picker");
        assert!(
            !none.iter().any(|a| a.contains("/no/such/place")),
            "a missing start folder is not handed to the panel: {none:?}"
        );
    }

    #[test]
    fn a_dismissal_is_not_a_failure() {
        // macOS: the script catches -128 and returns an empty string cleanly.
        assert_eq!(
            outcome("osascript", Some(0), "\n", ""),
            ChooseOutcome::Cancelled
        );
        // macOS without the catch: the panel's own error.
        assert_eq!(
            outcome(
                "osascript",
                Some(1),
                "",
                "execution error: User canceled. (-128)"
            ),
            ChooseOutcome::Cancelled
        );
        // zenity/kdialog: exit 1 and nothing said.
        assert_eq!(outcome("zenity", Some(1), "", ""), ChooseOutcome::Cancelled);
        // Windows: the dialog was not OK, so the script printed nothing.
        assert_eq!(
            outcome("powershell", Some(0), "", ""),
            ChooseOutcome::Cancelled
        );
    }

    #[test]
    fn a_chosen_folder_comes_back_as_the_path_the_panel_gave() {
        assert_eq!(
            outcome("osascript", Some(0), "/Users/me/Projects/bos/\n", ""),
            ChooseOutcome::Picked("/Users/me/Projects/bos/".into())
        );
        assert_eq!(
            outcome("zenity", Some(0), "/home/me/bos\n", ""),
            ChooseOutcome::Picked("/home/me/bos".into())
        );
    }

    #[test]
    fn a_real_failure_says_what_the_program_said() {
        assert_eq!(
            outcome(
                "osascript",
                Some(1),
                "",
                "execution error: Not authorized (-1743)"
            ),
            ChooseOutcome::Unavailable("osascript: execution error: Not authorized (-1743)".into())
        );
        // The shell's "no such command": the caller turns this into the fallback.
        assert_eq!(
            outcome("zenity", Some(127), "", ""),
            ChooseOutcome::Unavailable("zenity is not available here".into())
        );
        // A path printed beside a non-zero exit is not a chosen folder.
        assert!(matches!(
            outcome("zenity", Some(2), "/tmp\n", "wobble"),
            ChooseOutcome::Unavailable(_)
        ));
    }

    /// The one thing no test here can do is click the panel; the one thing it can
    /// do is run *the same command line* with only the panel call swapped for a
    /// reference to a folder that exists. That leaves the shape of the script —
    /// the `-e` statements, the escaping, `POSIX path of` and the `-128` handler —
    /// under test against the real interpreter, so a typo in the wrapper cannot
    /// hide behind "a modal needs a human".
    #[test]
    #[cfg(target_os = "macos")]
    fn the_script_shape_runs_even_though_the_panel_needs_a_human() {
        let dir = std::env::temp_dir();
        let Some((program, mut args)) = command("Pick", None) else {
            return;
        };
        let mut swapped = false;
        for arg in args.iter_mut() {
            if arg.starts_with("set f to choose folder") {
                *arg = format!(
                    "set f to POSIX file {}",
                    quote_applescript(&dir.to_string_lossy())
                );
                swapped = true;
            }
        }
        assert!(swapped, "the panel call is the statement the swap replaces");
        // The dismissal handler is part of the shape: swapping the panel call
        // cannot fail, so the *presence* of the -128 catch is what this test can
        // hold — otherwise a wrong error number would be invisible until a user
        // dismissed the panel and saw a failure.
        assert!(
            args.iter().any(|a| a.contains("-128")),
            "the cancellation handler is in the command line: {args:?}"
        );
        let out = std::process::Command::new(&program)
            .args(&args)
            .output()
            .expect("osascript runs");
        assert!(
            out.status.success(),
            "the script shape must run: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let shown = String::from_utf8_lossy(&out.stdout);
        let want = std::fs::canonicalize(&dir).expect("temp dir resolves");
        assert_eq!(
            std::path::Path::new(shown.trim()).canonicalize().ok(),
            Some(want),
            "the script answers with the folder it was pointed at: {shown:?}"
        );
    }

    /// The same command line again, this time handed to `osacompile`, which
    /// validates the AppleScript — the `choose folder` verb, its parameter names,
    /// the `on error` handler — and writes a compiled script **without opening a
    /// panel**. The swap test above replaces that verb; this one is the only place
    /// the verb itself is checked to be AppleScript at all.
    #[test]
    #[cfg(target_os = "macos")]
    fn the_real_panel_statement_compiles_without_opening_one() {
        let Some((program, args)) = command("Pick a folder", Some("/tmp")) else {
            return;
        };
        if !std::path::Path::new("/usr/bin/osacompile").exists() {
            return;
        }
        assert_eq!(
            program, "osascript",
            "osacompile takes osascript's arguments"
        );
        let out_file = std::env::temp_dir().join("bos-picker-compiles.scpt");
        let _ = std::fs::remove_file(&out_file);
        let out = std::process::Command::new("/usr/bin/osacompile")
            .args(&args)
            .arg("-o")
            .arg(&out_file)
            .output()
            .expect("osacompile runs");
        assert!(
            out.status.success(),
            "the panel statement must compile: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let written = std::fs::metadata(&out_file).map(|m| m.len()).unwrap_or(0);
        assert!(
            written > 0,
            "a compiled script was written ({written} bytes)"
        );
        let _ = std::fs::remove_file(&out_file);
    }

    #[test]
    fn on_this_machine_an_absent_picker_is_reported_rather_than_hung_on() {
        // Whatever this machine is, `choose` either reaches a picker (which is
        // not driven: the panel needs a human) or says why it cannot. It must
        // never panic, and it must never return a path it did not receive.
        if command("Pick", None).is_none() {
            let err = choose("Pick", None).expect_err("no picker on this platform");
            assert!(err.contains("no folder dialog"), "{err}");
        }
    }
}
