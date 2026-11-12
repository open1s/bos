//! Untrusted path safety: one validator for every package-relative path.
//!
//! Extension manifests, registry indexes and market URLs all name paths that
//! an attacker controls. Before this module each call site joined the string
//! to a root and hoped (issues #305, #321, #322); Rust's join semantics let an
//! absolute path REPLACE the root, and .. / symlinks walk out of it. The
//! contract here is:
//!
//! 1. [validate_relative] rejects every rooted or traversing form BEFORE any
//!    filesystem access: absolute Unix paths, Windows drive paths (absolute
//!    and drive-relative), UNC and device namespaces, alternate data streams,
//!    parent traversal, control characters and Windows reserved names.
//! 2. [resolve_within] proves the resolved target stays inside a canonical
//!    root, including symlink escapes, and handles write targets that do not
//!    exist yet by canonicalizing the deepest existing ancestor.
//!
//! The same rules apply on every host: a package path is a Git-style relative
//! path, so a form that is only dangerous on Windows is rejected everywhere.

use std::path::{Component, Path, PathBuf};

/// Why a path was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// The path was empty.
    Empty,
    /// The path is absolute or rooted (Unix /, Windows drive, UNC, device).
    Rooted,
    /// The path contains a parent traversal component.
    Traversal,
    /// A component carries a character no supported filesystem accepts.
    InvalidChar(char),
    /// A component is a Windows reserved device name.
    ReservedName(String),
    /// A component ends with a dot or space (Windows strips these silently).
    TrailingDotOrSpace(String),
    /// The resolved target escapes the root.
    EscapesRoot,
    /// The root itself does not exist or cannot be canonicalized.
    BadRoot,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "path is empty"),
            Self::Rooted => write!(
                f,
                "path must be relative (absolute, drive, UNC and device paths are refused)"
            ),
            Self::Traversal => write!(f, "path must not traverse with .."),
            Self::InvalidChar(c) => write!(f, "path contains a forbidden character {c:?}"),
            Self::ReservedName(name) => {
                write!(f, "path component {name:?} is a reserved device name")
            }
            Self::TrailingDotOrSpace(name) => {
                write!(f, "path component {name:?} ends with a dot or space")
            }
            Self::EscapesRoot => write!(f, "resolved path escapes its root"),
            Self::BadRoot => write!(f, "root does not exist or cannot be resolved"),
        }
    }
}

impl std::error::Error for PathError {}

/// Windows reserved device names, case-insensitive, with or without an
/// extension (CON.txt is still CON).
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn is_reserved(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    RESERVED.iter().any(|name| stem.eq_ignore_ascii_case(name))
}

/// Validate an untrusted package-relative path and return its components.
///
/// Accepts / and \ as separators (Git writes /; Windows users write \),
/// normalizes away single-dot components, and refuses everything that could
/// root or traverse the path on ANY supported host.
pub fn validate_relative(input: &str) -> Result<Vec<String>, PathError> {
    if input.is_empty() {
        return Err(PathError::Empty);
    }
    // Rooted forms, before any component parsing.
    if input.starts_with('/') || input.starts_with('\\') {
        return Err(PathError::Rooted);
    }
    // A Windows drive prefix: "C:", "C:foo" (drive-relative) and "C:\...".
    let bytes = input.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(PathError::Rooted);
    }
    // Device namespaces and UNC in forward-slash form.
    if input.starts_with("//") || input.starts_with("\\.\\") || input.starts_with("\\?\\") {
        return Err(PathError::Rooted);
    }
    let mut components = Vec::new();
    for raw in input.split(['/', '\\']) {
        if raw.is_empty() || raw == "." {
            continue;
        }
        if raw == ".." {
            return Err(PathError::Traversal);
        }
        for c in raw.chars() {
            // Control characters, and the characters Windows forbids in a
            // component. ':' also blocks alternate data streams; '*' '?'
            // '"' '<' '>' '|' are invalid on Windows and never needed in a
            // package path.
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                return Err(PathError::InvalidChar(c));
            }
        }
        if is_reserved(raw) {
            return Err(PathError::ReservedName(raw.to_string()));
        }
        if raw.ends_with('.') || raw.ends_with(' ') {
            return Err(PathError::TrailingDotOrSpace(raw.to_string()));
        }
        components.push(raw.to_string());
    }
    if components.is_empty() {
        return Err(PathError::Empty);
    }
    Ok(components)
}

/// Resolve an untrusted relative path inside a root, proving containment.
///
/// Existing targets are canonicalized whole (which resolves symlinks) and
/// must stay under the canonical root. Non-existing write targets are
/// resolved against their deepest existing ancestor, which must itself stay
/// inside the root; the remaining components are already validated.
pub fn resolve_within(root: &Path, input: &str) -> Result<PathBuf, PathError> {
    let components = validate_relative(input)?;
    let canonical_root = root.canonicalize().map_err(|_| PathError::BadRoot)?;
    let mut candidate = root.to_path_buf();
    for component in &components {
        candidate.push(component);
    }
    if candidate.exists() {
        let canonical = candidate
            .canonicalize()
            .map_err(|_| PathError::EscapesRoot)?;
        if !canonical.starts_with(&canonical_root) {
            return Err(PathError::EscapesRoot);
        }
        return Ok(candidate);
    }
    // Walk up to the deepest existing ancestor and prove IT is contained.
    let mut ancestor = candidate.as_path();
    while !ancestor.exists() {
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => return Err(PathError::EscapesRoot),
        }
    }
    let canonical_ancestor = ancestor
        .canonicalize()
        .map_err(|_| PathError::EscapesRoot)?;
    if !canonical_ancestor.starts_with(&canonical_root) {
        return Err(PathError::EscapesRoot);
    }
    Ok(candidate)
}

/// Whether a candidate path is contained in a root, comparing canonical
/// forms so a symlinked root or candidate cannot fake the prefix.
pub fn is_contained(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(root), Ok(candidate)) => candidate.starts_with(root),
        _ => false,
    }
}

/// Whether a path component list is a safe relative path (convenience for
/// callers holding a Path rather than a string).
pub fn path_is_relative_safe(path: &Path) -> bool {
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(text) = part.to_str() else {
                    return false;
                };
                if validate_relative(text).is_err() {
                    return false;
                }
                saw_normal = true;
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    saw_normal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooted_forms_are_refused_on_every_host() {
        for input in [
            "/etc/passwd",
            r"C:\Windows\system32",
            "C:relative",
            "C:",
            r"\\server\share",
            "//server/share",
            r"\\.\PhysicalDrive0",
            r"\\?\C:\",
        ] {
            assert!(
                matches!(validate_relative(input), Err(PathError::Rooted)),
                "{input:?} must be refused as rooted"
            );
        }
    }

    #[test]
    fn traversal_and_invalid_forms_are_refused() {
        for input in [
            "..",
            "../outside",
            "a/../../outside",
            r"a/..\b",
            "",
            "file:stream",
            "a*b",
            "a?b",
            "CON",
            "con.txt",
            "NUL",
            "name.",
            "name ",
            "a\u{0}b",
        ] {
            assert!(
                validate_relative(input).is_err(),
                "{input:?} must be refused"
            );
        }
    }

    #[test]
    fn legitimate_nested_paths_normalize() {
        assert_eq!(
            validate_relative("packages/./hello/../hello").unwrap_err(),
            PathError::Traversal
        );
        assert_eq!(
            validate_relative("packages/hello").unwrap(),
            vec!["packages".to_string(), "hello".to_string()]
        );
        assert_eq!(
            validate_relative("packages\\hello").unwrap(),
            vec!["packages".to_string(), "hello".to_string()]
        );
        assert_eq!(
            validate_relative("./a/b/").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        // Unicode names are fine.
        assert_eq!(validate_relative("paket/你好").unwrap().len(), 2);
    }

    #[test]
    fn resolve_within_proves_containment() {
        let dir = tempfile_dir("contain");
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::write(root.join("inner/file.json"), "{}").unwrap();
        assert!(resolve_within(&root, "inner/file.json").is_ok());
        assert!(resolve_within(&root, "../outside").is_err());
        assert!(resolve_within(&root, "/etc/passwd").is_err());
        // A write target that does not exist yet resolves inside the root.
        assert!(resolve_within(&root, "inner/new/deep.json").is_ok());
        // A symlink out of the root is refused even though the text is relative.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path(), root.join("escape")).unwrap();
            assert_eq!(
                resolve_within(&root, "escape/anything"),
                Err(PathError::EscapesRoot)
            );
        }
    }

    #[test]
    fn sibling_prefix_confusion_is_refused() {
        let dir = tempfile_dir("sibling");
        let root = dir.path().join("root");
        let sibling = dir.path().join("root-evil");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("x"), "x").unwrap();
        assert!(!is_contained(&root, &sibling.join("x")));
        assert!(is_contained(&root, &root));
    }

    fn tempfile_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("hostkit-{label}-"))
            .tempdir()
            .expect("temp dir")
    }

    /// Cross-platform conformance guard (issue #313): the path validator
    /// refuses the same escape forms on every host.
    #[test]
    fn conformance_path_safety_on_every_host() {
        for input in ["/etc/passwd", "../escape", "C:\\windows", "a/../../b"] {
            assert!(
                crate::paths::validate_relative(input).is_err(),
                "{input:?} must be refused on every host"
            );
        }
        assert!(crate::paths::validate_relative("packages/hello").is_ok());
    }
}
