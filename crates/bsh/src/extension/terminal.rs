//! Typed terminal service exposed to extensions.

use std::any::Any;
use std::path::PathBuf;

/// Result of one terminal command.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TerminalOutput {
    /// Standard output.
    pub(crate) stdout: String,
    /// Standard error.
    pub(crate) stderr: String,
    /// Exit code, or -1 when the process was signalled.
    pub(crate) code: i32,
}

/// Request/response terminal capability.
pub(crate) trait TerminalService: Any + Send + Sync {
    /// Run one shell command in cwd.
    fn run(&self, command: &str, cwd: Option<PathBuf>) -> Result<TerminalOutput, String>;
}

/// Sized handle stored in the service registry.
#[derive(Clone)]
pub(crate) struct TerminalHandle(pub std::sync::Arc<dyn TerminalService>);

impl std::ops::Deref for TerminalHandle {
    type Target = dyn TerminalService;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

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
