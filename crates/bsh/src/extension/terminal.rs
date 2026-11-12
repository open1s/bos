//! Host terminal adapter; the contract lives in ferris-core.
//!
//! The command string is SHELL SYNTAX in the host's native shell: POSIX sh on
//! Unix, PowerShell on Windows (issue #298). The two dialects are not
//! interchangeable; an extension that needs portable execution should pass
//! argv to a process runtime instead. A raw command is never silently
//! reinterpreted across dialects.
pub(crate) use ferris_sdk::terminal::*;

use std::path::PathBuf;

/// Host adapter backed by the platform's native shell.
pub(crate) struct HostTerminal;

impl TerminalService for HostTerminal {
    fn run(&self, command: &str, cwd: Option<PathBuf>) -> Result<TerminalOutput, String> {
        let mut process = host_shell_command(command);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let output = process.output().map_err(|error| error.to_string())?;
        Ok(TerminalOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
        })
    }
}

/// The native shell command for one command string.
///
/// Unix: sh -c. Windows: PowerShell -Command, preferring pwsh (PowerShell 7+)
/// and falling back to Windows PowerShell, then cmd.exe /C as the last resort
/// so a clean install without PowerShell still runs a command.
fn host_shell_command(command: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        for program in ["pwsh.exe", "powershell.exe"] {
            if which(program).is_some() {
                let mut process = std::process::Command::new(program);
                process.arg("-NoProfile").arg("-Command").arg(command);
                return process;
            }
        }
        let mut process = std::process::Command::new("cmd.exe");
        process.arg("/C").arg(command);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = std::process::Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}

/// Whether a program is resolvable on PATH.
#[cfg(windows)]
fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_terminal_runs_a_native_command() {
        let command = if cfg!(windows) {
            "Write-Output hello"
        } else {
            "echo hello"
        };
        let output = HostTerminal
            .run(command, None)
            .expect("the host shell runs");
        assert_eq!(output.code, 0, "{output:?}");
        assert!(output.stdout.contains("hello"), "{output:?}");
    }

    #[test]
    fn a_non_zero_exit_is_reported() {
        let output = HostTerminal
            .run("exit 3", None)
            .expect("the host shell runs");
        assert_eq!(output.code, 3, "{output:?}");
    }

    #[test]
    fn a_cwd_with_spaces_is_honored() {
        let dir = tempfile::tempdir().expect("temp");
        let spaced = dir.path().join("a dir with spaces");
        std::fs::create_dir_all(&spaced).unwrap();
        let command = if cfg!(windows) {
            "Write-Output (Get-Location).Path"
        } else {
            "pwd"
        };
        let output = HostTerminal
            .run(command, Some(spaced.clone()))
            .expect("the host shell runs");
        assert!(output.stdout.contains("a dir with spaces"), "{output:?}");
    }
}
