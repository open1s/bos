//! Host terminal adapter; the contract lives in ferris-core.
pub(crate) use ferris_sdk::terminal::*;

use std::path::PathBuf;

/// Host adapter backed by sh -c.
pub(crate) struct HostTerminal;

impl TerminalService for HostTerminal {
    fn run(&self, command: &str, cwd: Option<PathBuf>) -> Result<TerminalOutput, String> {
        let mut process = std::process::Command::new("sh");
        process.arg("-c").arg(command);
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
