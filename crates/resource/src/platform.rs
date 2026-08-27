//! Cross-platform OS abstraction seam.
//!
//! The resource layer is a high-level abstraction over OS resources that must
//! behave identically (or degrade gracefully) across macOS, Linux, and Windows.
//! Portable primitives come from `tokio` (`tokio::fs`, `tokio::process`,
//! `tokio::net`), so most platform differences are already absorbed. Anything
//! that genuinely differs by OS — process termination, signals, PID typing — is
//! confined here so the rest of the crate never needs `#[cfg(...)]` branching.

/// The host operating system the resource layer is running on.
///
/// Reported to agents via metadata so discovery can distinguish e.g. a file
/// resource hosted on Windows from one on Linux, without the caller having to
/// care about how each is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Os {
    Linux,
    Macos,
    Windows,
    /// Any other unix-like OS.
    Unix,
    /// Unrecognized platform.
    Unknown,
}

impl Os {
    /// Detect the host platform at runtime.
    pub fn host() -> Self {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(unix) {
            Os::Unix
        } else {
            Os::Unknown
        }
    }

    /// Human-readable name for display / metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Unix => "unix",
            Os::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for Os {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Terminal signal semantics for a process, normalized across platforms.
///
/// POSIX exposes a rich signal set (SIGTERM, SIGKILL, SIGINT, …); Windows only
/// meaningfully supports unconditional termination (`TerminateProcess`). Rather
/// than leak raw POSIX signal numbers through the abstraction, we describe
/// *intent* and map it to platform specifics at the single point of use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminate {
    /// Graceful: request the process to exit (SIGTERM on Unix, TerminateProcess
    /// on Windows).
    Graceful,
    /// Force: kill immediately (SIGKILL on Unix, TerminateProcess on Windows).
    Force,
    /// A raw POSIX signal number. On Unix this is honored; on Windows it is
    /// treated as `Force` since Windows has no weak/strong signal distinction.
    Unix(i32),
}

impl Terminate {
    /// The normalized signal number, for policy/telemetry or wire transport.
    /// Returns `0` for platform-neutral variants (there is no portable number).
    pub fn signal_number(&self) -> i32 {
        match self {
            Terminate::Graceful => 15, // SIGTERM, by convention.
            Terminate::Force => 9,     // SIGKILL, by convention.
            Terminate::Unix(n) => *n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_detects() {
        let os = Os::host();
        assert_ne!(os, Os::Unknown, "host platform should be recognized");
    }

    #[test]
    fn signal_numbers_are_stable() {
        assert_eq!(Terminate::Graceful.signal_number(), 15);
        assert_eq!(Terminate::Force.signal_number(), 9);
        assert_eq!(Terminate::Unix(2).signal_number(), 2);
    }
}