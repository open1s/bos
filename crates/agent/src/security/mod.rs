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
    pub fn validate_path(&self, path: &str) -> Result<PathBuf, SecurityError> {
        let input_path = Path::new(path);

        if input_path.components().any(|c| c.as_os_str() == "..") {
            return Err(SecurityError::PathTraversal(format!(
                "Path contains '..' components: {}",
                path
            )));
        }

        if !self.allow_symlinks && input_path.is_symlink() {
            return Err(SecurityError::PathTraversal(format!(
                "Symlink detected: {}",
                path
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

        let normalized = Self::normalize_path(&full_path)?;
        let workspace_normalized = Self::normalize_path(&self.workspace_root)?;

        // Compare path components, not string prefixes: `/tmp/ws-evil` shares
        // the string prefix `/tmp/ws` but is not inside the workspace. Both
        // sides are canonical, so one components comparison settles it.
        if !normalized.starts_with(&workspace_normalized) {
            return Err(SecurityError::OutsideWorkspace(format!(
                "Path {} is outside workspace {}",
                path,
                self.workspace_root.display()
            )));
        }

        Ok(normalized)
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
        assert!(
            matches!(error, SecurityError::OutsideWorkspace(_)),
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
}
