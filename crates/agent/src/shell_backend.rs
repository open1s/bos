//! The Bash execution backend: which POSIX shell runs a BashTool command.
//!
//! BashTool's contract is the Bash/POSIX command language, NOT "whatever
//! shell happens to exist" (issue #311). On Unix that is the system sh.
//! Windows has no system POSIX shell, so the backend resolves an explicitly
//! installed Bash runtime — Git for Windows Bash or WSL Bash — from native
//! install locations, never by assuming sh.exe is on PATH. When none is
//! installed the tool refuses with an actionable error instead of silently
//! reinterpreting the command as CMD or PowerShell.
//!
//! Confinement stays truthful: a WSL runtime cannot be wrapped by the
//! Windows AppContainer backend (the Linux side runs outside it), so a
//! confined turn refuses WSL with a specific diagnostic rather than
//! running unconfined.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// A resolved POSIX shell runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BashRuntime {
    /// The system sh (Unix) or an explicitly configured bash binary.
    Native(PathBuf),
    /// Git for Windows bash.exe: MSYS2 POSIX layer on the Windows kernel.
    GitBash(PathBuf),
    /// WSL bash: a Linux distribution behind wsl.exe.
    Wsl(PathBuf),
}

impl BashRuntime {
    /// The executable to spawn.
    pub fn program(&self) -> &Path {
        match self {
            Self::Native(path) | Self::GitBash(path) | Self::Wsl(path) => path,
        }
    }

    /// The arguments that run one command string in this runtime's dialect.
    pub fn command_args(&self, command: &str) -> Vec<OsString> {
        match self {
            Self::Native(_) | Self::GitBash(_) => {
                vec![OsString::from("-c"), OsString::from(command)]
            }
            // --cd maps the Windows working directory into the distribution;
            // -e runs bash without a login shell's profile side effects.
            Self::Wsl(_) => vec![
                OsString::from("--cd"),
                OsString::from("."),
                OsString::from("-e"),
                OsString::from("bash"),
                OsString::from("-c"),
                OsString::from(command),
            ],
        }
    }

    /// The working directory to spawn in. WSL keeps the Windows path: --cd
    /// translates it. Git Bash accepts a Windows cwd natively.
    pub fn spawn_cwd(&self, cwd: &Path) -> PathBuf {
        cwd.to_path_buf()
    }

    /// The dialect the tool description names to the model.
    pub fn dialect(&self) -> &'static str {
        match self {
            Self::Native(_) => "sh (POSIX)",
            Self::GitBash(_) => "Git Bash (POSIX on Windows)",
            Self::Wsl(_) => "WSL Bash (Linux)",
        }
    }

    /// Whether the platform sandbox backend can confine this runtime.
    ///
    /// WSL's Linux side runs outside any Windows AppContainer, so a confined
    /// turn must refuse it rather than claim an isolation it does not have.
    pub fn confinable(&self) -> bool {
        !matches!(self, Self::Wsl(_))
    }
}

/// Resolve the Bash runtime for this host, or refuse with an actionable error.
///
/// Precedence: the BSH_BASH_BIN override (a file that must exist), then the
/// platform's native resolution. The override is treated as a native bash.
pub fn resolve() -> Result<BashRuntime, String> {
    if let Some(explicit) = std::env::var_os("BSH_BASH_BIN") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(BashRuntime::Native(path));
        }
        return Err(format!(
            "BSH_BASH_BIN points at {}, which is not a file; unset it or point it at a Bash executable",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        Ok(BashRuntime::Native(PathBuf::from("sh")))
    }
    #[cfg(windows)]
    {
        resolve_windows()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("no POSIX shell backend is supported on this platform".to_string())
    }
}

/// Windows discovery: Git Bash install roots, then PATH, then WSL.
#[cfg(windows)]
fn resolve_windows() -> Result<BashRuntime, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Programs")),
    ]
    .into_iter()
    .flatten()
    {
        let root = PathBuf::from(root);
        candidates.push(root.join("Git").join("bin").join("bash.exe"));
        candidates.push(root.join("Git").join("usr").join("bin").join("bash.exe"));
    }
    // A bash.exe already on PATH (Git's cmd dir, MSYS2, Cygwin).
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("bash.exe"));
        }
    }
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(BashRuntime::GitBash(candidate.clone()));
        }
    }
    // WSL: the system32 launcher, or one on PATH.
    let mut wsl_candidates = Vec::new();
    if let Some(root) = std::env::var_os("SystemRoot") {
        wsl_candidates.push(PathBuf::from(root).join("System32").join("wsl.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            wsl_candidates.push(dir.join("wsl.exe"));
        }
    }
    for candidate in &wsl_candidates {
        if candidate.is_file() {
            return Ok(BashRuntime::Wsl(candidate.clone()));
        }
    }
    Err(
        "no Bash runtime found: install Git for Windows (bash.exe) or WSL, or set BSH_BASH_BIN \
         to a Bash executable. BashTool runs the POSIX command language and will not reinterpret \
         commands as CMD or PowerShell."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_resolves_the_system_shell() {
        #[cfg(unix)]
        {
            let runtime = resolve().expect("unix always has sh");
            assert_eq!(runtime.dialect(), "sh (POSIX)");
            assert!(runtime.confinable());
            assert_eq!(runtime.command_args("echo hi").len(), 2);
        }
    }

    #[test]
    fn wsl_is_not_confinable() {
        let runtime = BashRuntime::Wsl(PathBuf::from("wsl.exe"));
        assert!(!runtime.confinable());
        assert_eq!(runtime.dialect(), "WSL Bash (Linux)");
        let args = runtime.command_args("echo hi");
        assert!(args.iter().any(|a| a == "--cd"));
        assert!(args.iter().any(|a| a == "bash"));
    }

    #[test]
    fn a_missing_override_is_actionable() {
        // Serialized through a mutex-free env guard: the override is read
        // once; set, resolve, restore.
        let previous = std::env::var_os("BSH_BASH_BIN");
        std::env::set_var("BSH_BASH_BIN", "/nonexistent/bash-xyz");
        let error = resolve().unwrap_err();
        assert!(error.contains("BSH_BASH_BIN"), "{error}");
        match previous {
            Some(value) => std::env::set_var("BSH_BASH_BIN", value),
            None => std::env::remove_var("BSH_BASH_BIN"),
        }
    }

    #[test]
    fn git_bash_runs_a_posix_command() {
        let runtime = BashRuntime::GitBash(PathBuf::from("bash.exe"));
        assert!(runtime.confinable());
        let args = runtime.command_args("echo hi");
        assert_eq!(args[0], OsString::from("-c"));
        assert_eq!(args[1], OsString::from("echo hi"));
    }
}
