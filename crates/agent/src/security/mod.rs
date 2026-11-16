//! Filesystem and command safety checks for agent tools.

/// Scoped "allow-always" approval rules, precedence, and audit ring.
pub mod rules;

pub use rules::{
    ArgMatch, ArgPredicate, AuditEntry, Decision, DecisionEffect, RuleEffect, RuleScope, RuleSet,
    ToolRule, DEFAULT_AUDIT_CAPACITY,
};

use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors raised by workspace validation.
#[derive(Debug, Error)]
pub enum SecurityError {
    /// The path contains `..` or an unexpected symlink.
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),

    /// The resolved path escapes the workspace root.
    #[error("Path outside workspace: {0}")]
    OutsideWorkspace(String),

    /// A command matched a destructive pattern.
    #[error("Destructive operation: {0}")]
    DestructiveOperation(String),

    /// A command matched an elevated-privilege pattern.
    #[error("Elevated privilege operation: {0}")]
    ElevatedPrivilege(String),

    /// The path could not be canonicalized.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Validates paths and commands against a workspace root.
pub struct WorkspaceValidator {
    workspace_root: PathBuf,
    allow_symlinks: bool,
    allow_absolute_paths: bool,
}

impl WorkspaceValidator {
    /// Create a validator rooted at `workspace_root`; symlinks and absolute paths are denied.
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            allow_symlinks: false,
            allow_absolute_paths: false,
        }
    }

    fn normalize_path(path: &Path) -> Result<PathBuf, SecurityError> {
        let canonical = fs::canonicalize(path).map_err(SecurityError::Io)?;
        Ok(canonical)
    }

    /// Resolve `path` inside the workspace, rejecting traversal and escapes.
    ///
    /// The contract (issue #308):
    /// - rooted and Windows-specific escape forms (drive-relative, UNC,
    ///   device namespace, alternate data streams) are refused before any
    ///   filesystem access, on every host;
    /// - with symlinks disallowed, EVERY existing component is checked, not
    ///   just the final one;
    /// - a write target that does not exist yet is validated against its
    ///   deepest existing ancestor, so new files are not refused merely
    ///   because canonicalize needs an existing target;
    /// - the final resolved target must stay inside the canonical root.
    ///
    /// Residual threat model: validation and the caller's later open are two
    /// operations, so a concurrent rename of an intermediate directory can
    /// still race (TOCTOU). Tools that need a hard guarantee must open
    /// relative to a directory handle with no-follow semantics; this
    /// validator is the policy gate, not the kernel boundary.
    pub fn validate_path(&self, path: &str) -> Result<PathBuf, SecurityError> {
        let input_path = Path::new(path);

        if input_path.components().any(|c| c.as_os_str() == "..") {
            return Err(SecurityError::PathTraversal(format!(
                "Path contains '..' components: {}",
                path
            )));
        }

        // Windows escape forms are refused on every host: a package or tool
        // path must be workspace-relative, and these forms root or redirect
        // it on Windows even though they look like ordinary names on Unix.
        if let Some(reason) = windows_escape_form(path) {
            return Err(SecurityError::OutsideWorkspace(format!(
                "{}: {}",
                reason, path
            )));
        }

        let expanded = if path.starts_with('~') {
            dirs::home_dir()
                .map(|h| h.join(path.strip_prefix("~").unwrap_or_default()))
                .unwrap_or_else(|| PathBuf::from(path))
        } else {
            PathBuf::from(path)
        };

        if !self.allow_absolute_paths && expanded.is_absolute() {
            return Err(SecurityError::OutsideWorkspace(format!(
                "Absolute path not allowed: {}",
                path
            )));
        }

        let full_path = if expanded.is_relative() {
            self.workspace_root.join(&expanded)
        } else {
            expanded
        };

        let workspace_normalized = Self::normalize_path(&self.workspace_root)?;

        // With symlinks disallowed, check every existing component of the
        // joined path, not only the input's final component.
        if !self.allow_symlinks {
            // Walk only the components BELOW the workspace root: the root's
            // own ancestors are the host's (macOS /var is a symlink to
            // /private/var) and are established by the caller, not the input.
            if let Ok(relative) = full_path.strip_prefix(&self.workspace_root) {
                let mut prefix = self.workspace_root.clone();
                for component in relative.components() {
                    prefix.push(component);
                    if prefix
                        .symlink_metadata()
                        .map(|m| m.file_type().is_symlink())
                        .unwrap_or(false)
                    {
                        return Err(SecurityError::PathTraversal(format!(
                            "Symlink detected in path component: {}",
                            prefix.display()
                        )));
                    }
                }
            }
        }

        if full_path.exists() {
            let normalized = Self::normalize_path(&full_path)?;
            if !normalized.starts_with(&workspace_normalized) {
                return Err(SecurityError::OutsideWorkspace(format!(
                    "Path {} is outside workspace {}",
                    path,
                    self.workspace_root.display()
                )));
            }
            return Ok(normalized);
        }

        // A non-existing write target: canonicalize the deepest existing
        // ancestor and prove IT is contained; the remaining components are
        // already traversal-free.
        let mut ancestor = full_path.as_path();
        while !ancestor.exists() {
            match ancestor.parent() {
                Some(parent) => ancestor = parent,
                None => {
                    return Err(SecurityError::OutsideWorkspace(format!(
                        "Path {} has no existing ancestor inside the workspace",
                        path
                    )));
                }
            }
        }
        let canonical_ancestor = Self::normalize_path(ancestor)?;
        if !canonical_ancestor.starts_with(&workspace_normalized) {
            return Err(SecurityError::OutsideWorkspace(format!(
                "Path {} is outside workspace {}",
                path,
                self.workspace_root.display()
            )));
        }
        Ok(full_path)
    }

    /// Whether `command` matches a known destructive pattern.
    pub fn is_destructive_command(command: &str) -> bool {
        let destructive_patterns = [
            "rm -rf",
            "rm -r",
            "rm -f",
            "del ",
            "format",
            "mkfs",
            "dd if=",
            "shred",
            "git push --force",
            "git push -f",
            "git reset --hard",
            "git clean -fd",
        ];

        let cmd_lower = command.to_lowercase();
        destructive_patterns.iter().any(|p| cmd_lower.contains(p))
    }

    /// Whether `command` matches a known privilege-escalation pattern.
    pub fn requires_elevated_privilege(command: &str) -> bool {
        let elevated_patterns = [
            "sudo",
            "su -",
            "chmod 777",
            "chown root",
            "kill -9",
            "killall",
            ":(){ :|:& };:",
        ];

        let cmd_lower = command.to_lowercase();
        elevated_patterns.iter().any(|p| cmd_lower.contains(p))
    }
}

/// A Windows path form that roots or redirects a path, refused on every host.
///
/// Returns a human-readable reason, or None for an ordinary relative path.
fn windows_escape_form(path: &str) -> Option<&'static str> {
    if path.starts_with("\\") || path.starts_with("//") {
        return Some("UNC or device namespace path");
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some("drive-prefixed path");
    }
    // An alternate data stream hides after a colon in any component.
    if path.split(['/', '\\']).any(|part| part.contains(':')) {
        return Some("alternate data stream syntax");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_traversal_detection() {
        let validator = WorkspaceValidator::new(PathBuf::from("/workspace"));

        let result = validator.validate_path("../etc/passwd");
        assert!(matches!(result, Err(SecurityError::PathTraversal(_))));
    }

    #[test]
    fn test_workspace_boundary() {
        let validator = WorkspaceValidator::new(PathBuf::from("/workspace/project"));

        let result = validator.validate_path("/etc/passwd");
        assert!(matches!(result, Err(SecurityError::OutsideWorkspace(_))));
    }

    /// A symlinked directory must not turn a same-prefix sibling into an
    /// "inside workspace" path.
    #[cfg(unix)]
    #[test]
    fn a_symlink_cannot_escape_through_a_shorter_workspace_prefix() {
        use std::os::unix::fs::symlink;

        let base =
            std::env::temp_dir().join(format!("bos-workspace-validator-{}", std::process::id()));
        let workspace = base.join("ws");
        let sibling = base.join("ws-evil");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("secret.txt"), "secret").unwrap();
        symlink(&sibling, workspace.join("link")).unwrap();

        let validator = WorkspaceValidator::new(workspace.clone());
        let error = validator
            .validate_path("link/secret.txt")
            .expect_err("a path resolving outside the workspace is refused");
        // Either refusal settles it: the intermediate-symlink gate fires
        // first (PathTraversal), the containment gate second (OutsideWorkspace).
        assert!(
            matches!(
                error,
                SecurityError::OutsideWorkspace(_) | SecurityError::PathTraversal(_)
            ),
            "{error:?}"
        );

        std::fs::write(workspace.join("inside.txt"), "ok").unwrap();
        assert!(validator.validate_path("inside.txt").is_ok());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn test_destructive_command_detection() {
        assert!(WorkspaceValidator::is_destructive_command("rm -rf /"));
        assert!(WorkspaceValidator::is_destructive_command(
            "git push --force"
        ));
        assert!(!WorkspaceValidator::is_destructive_command("ls -la"));
    }

    #[test]
    fn test_elevated_privilege_detection() {
        assert!(WorkspaceValidator::requires_elevated_privilege(
            "sudo rm -rf /"
        ));
        assert!(WorkspaceValidator::requires_elevated_privilege(
            "chmod 777 file"
        ));
        assert!(!WorkspaceValidator::requires_elevated_privilege("ls -la"));
    }

    /// An intermediate symlink is refused when symlinks are disallowed, even
    /// when it points inside the workspace: the old check only looked at the
    /// final component.
    #[cfg(unix)]
    #[test]
    fn an_intermediate_symlink_is_refused() {
        let base = std::env::temp_dir().join(format!("bos-val-mid-{}", std::process::id()));
        let workspace = base.join("ws");
        std::fs::create_dir_all(workspace.join("real")).unwrap();
        std::fs::write(workspace.join("real/f.txt"), "x").unwrap();
        std::os::unix::fs::symlink(workspace.join("real"), workspace.join("link")).unwrap();
        let validator = WorkspaceValidator::new(workspace.clone());
        let error = validator.validate_path("link/f.txt").unwrap_err();
        assert!(
            matches!(error, SecurityError::PathTraversal(_)),
            "{error:?}"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    /// A new write target validates against its deepest existing ancestor.
    #[test]
    fn a_non_existing_write_target_is_validated() {
        let base = std::env::temp_dir().join(format!("bos-val-new-{}", std::process::id()));
        let workspace = base.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        let validator = WorkspaceValidator::new(workspace.clone());
        let resolved = validator
            .validate_path("new/deep/file.txt")
            .expect("new file");
        assert!(resolved.starts_with(&workspace));
        // A traversal hidden behind a non-existing prefix is still refused.
        assert!(validator.validate_path("../outside.txt").is_err());
        std::fs::remove_dir_all(&base).ok();
    }

    /// Windows escape forms are refused on every host.
    #[test]
    fn windows_escape_forms_are_refused_everywhere() {
        let validator = WorkspaceValidator::new(PathBuf::from("/workspace"));
        for path in [
            "C:relative",
            "C:\\absolute",
            "\\\\server\\share",
            "//server/share",
            "\\\\.\\PhysicalDrive0",
            "file.txt:stream",
            "dir/file:ads",
        ] {
            assert!(
                validator.validate_path(path).is_err(),
                "{path:?} must be refused"
            );
        }
    }
}
