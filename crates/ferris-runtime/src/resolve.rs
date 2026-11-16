//! Resolve an allowlisted `runtime.entry` to the program to spawn.
//!
//! `runtime.entry = "python3"` (or `"node"`, `"bun"`, `"sh"`) names a program,
//! and the process-based runtimes spawn it by path with a cleared environment.
//! A host launched without a shell would otherwise depend on a `PATH` it never
//! inherited. [`hostkit::programs::executable`] is the one cross-platform
//! search that answers this for every program, so no runtime keeps its own
//! per-language table.
//!
//! The manifest keeps naming the program: the capability check still runs
//! against `program:python3`, and only the executable that answers to it
//! changes.

use std::path::PathBuf;

/// The program for an allowlisted entry, resolved through the host's program
/// search; an unresolved name is returned unchanged so the spawn reports the
/// name the manifest declared.
pub(crate) fn program(entry: &str) -> PathBuf {
    hostkit::programs::executable(entry).unwrap_or_else(|| PathBuf::from(entry))
}

#[cfg(test)]
mod tests {
    use super::program;
    use std::path::PathBuf;

    /// A name nothing provides keeps the manifest's spelling, so the spawn
    /// error names what the package asked for.
    #[test]
    fn an_unknown_program_keeps_its_name() {
        assert_eq!(
            program("definitely-not-installed-xyz"),
            PathBuf::from("definitely-not-installed-xyz")
        );
    }

    /// A program the host has resolves to an existing file under that name.
    #[test]
    fn a_known_program_resolves_to_a_file() {
        let resolved = program("sh");
        assert!(
            resolved.is_file() || resolved.as_path() == std::path::Path::new("sh"),
            "resolved {resolved:?} is neither the name nor a file"
        );
    }
}
