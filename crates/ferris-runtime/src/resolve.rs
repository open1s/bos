//! Resolve an allowlisted `runtime.entry` to the program to spawn.
//!
//! `runtime.entry = "python3"` names a program, and the process-based
//! runtimes spawn it by path with a cleared environment. A host launched
//! without a shell would otherwise depend on a `PATH` it never inherited, and
//! a macOS `/usr/bin/python3` stub would win over the interpreter the reader
//! installed with mise, asdf, pyenv or uv.
//!
//! The manifest keeps naming the program: the capability check still runs
//! against `program:python3`, and only the executable that answers to it
//! changes.

use std::path::PathBuf;

/// The program for an allowlisted entry, resolved when it is an interpreter a
/// shellless launch cannot rely on `PATH` to find.
pub(crate) fn program(entry: &str) -> PathBuf {
    match entry {
        "python" | "python3" => {
            hostkit::programs::python_interpreter().unwrap_or_else(|| PathBuf::from(entry))
        }
        _ => PathBuf::from(entry),
    }
}

#[cfg(test)]
mod tests {
    use super::program;
    use std::path::PathBuf;

    /// A name BSH has no resolution rule for is spawned exactly as the
    /// manifest wrote it.
    #[test]
    fn other_programs_are_passed_through() {
        assert_eq!(program("sh"), PathBuf::from("sh"));
        assert_eq!(program("node"), PathBuf::from("node"));
        assert_eq!(program("./server.py"), PathBuf::from("./server.py"));
    }
}
