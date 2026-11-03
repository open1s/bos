//! Typed terminal service contract.

use std::any::Any;
use std::path::PathBuf;

/// Result of one terminal command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminalOutput {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Exit code, or -1 when the process was signalled.
    pub code: i32,
}

/// Request/response terminal capability.
pub trait TerminalService: Any + Send + Sync {
    /// Run one shell command in cwd.
    fn run(&self, command: &str, cwd: Option<PathBuf>) -> Result<TerminalOutput, String>;
}

/// Sized handle stored in the service registry.
#[derive(Clone)]
pub struct TerminalHandle(pub std::sync::Arc<dyn TerminalService>);

impl std::ops::Deref for TerminalHandle {
    type Target = dyn TerminalService;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}
