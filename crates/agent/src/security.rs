use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("Path traversal detected: {0}")]
    PathTraversal(String),

    #[error("Path outside workspace: {0}")]
    OutsideWorkspace(String),

    #[error("Destructive operation: {0}")]
    DestructiveOperation(String),

    #[error("Elevated privilege operation: {0}")]
    ElevatedPrivilege(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct WorkspaceValidator {
    workspace_root: PathBuf,
    allow_symlinks: bool,
    allow_absolute_paths: bool,
}

impl WorkspaceValidator {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            workspace_root,
            allow_symlinks: false,
            allow_absolute_paths: false,
        }
    }

    pub fn with_symlinks(mut self, allow: bool) -> Self {
        self.allow_symlinks = allow;
        self
    }

    pub fn with_absolute_paths(mut self, allow: bool) -> Self {
        self.allow_absolute_paths = allow;
        self
    }

    fn normalize_path(path: &Path) -> Result<PathBuf, SecurityError> {
        let canonical = fs::canonicalize(path).map_err(|e| SecurityError::Io(e))?;
        Ok(canonical)
    }

    pub fn validate_path(&self, path: &str) -> Result<PathBuf, SecurityError> {
        let input_path = Path::new(path);

        if input_path.components().any(|c| c.as_os_str() == "..") {
            return Err(SecurityError::PathTraversal(format!(
                "Path contains '..' components: {}",
                path
            )));
        }

        if !self.allow_symlinks {
            if input_path.is_symlink() {
                return Err(SecurityError::PathTraversal(format!(
                    "Symlink detected: {}",
                    path
                )));
            }
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

        let normalized_str = normalized.to_string_lossy().to_lowercase();
        let workspace_str = workspace_normalized.to_string_lossy().to_lowercase();

        if !normalized_str.starts_with(&workspace_str) {
            return Err(SecurityError::OutsideWorkspace(format!(
                "Path {} is outside workspace {}",
                path,
                self.workspace_root.display()
            )));
        }

        Ok(normalized)
    }

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
