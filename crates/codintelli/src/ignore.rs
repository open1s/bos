//! Configurable workspace exclusion rules for inventory scans.
//!
//! Rules are workspace-relative and evaluated in insertion order. The last
//! matching rule wins. This is intentionally a small path-policy API; it does
//! not claim compatibility with .gitignore syntax.

use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
};

use crate::fs::WorkspaceEntryKind;

const BUILTIN_IGNORED_DIRECTORY_NAMES: &[&str] = &[
    ".git", ".hg", ".svn", ".next", ".turbo", ".cache", ".venv",
    "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox",
    "target", "node_modules", "dist", "build",
];

/// Whether an ignore rule excludes or explicitly includes a matching path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoreAction {
    /// Exclude the matching path and its descendants where applicable.
    Exclude,
    /// Re-include the matching path and its descendants where applicable.
    Include,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IgnoreMatcher {
    PathPrefix(PathBuf),
    DirectoryName(OsString),
    Extension(OsString),
}

/// One ordered rule in an IgnorePolicy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoreRule {
    action: IgnoreAction,
    matcher: IgnoreMatcher,
}

impl IgnoreRule {
    /// Exclude a workspace-relative path and all descendants.
    pub fn exclude_path(path: impl AsRef<Path>) -> Result<Self, IgnorePolicyError> {
        Self::path_rule(path, IgnoreAction::Exclude)
    }

    /// Include a workspace-relative path and all descendants.
    pub fn include_path(path: impl AsRef<Path>) -> Result<Self, IgnorePolicyError> {
        Self::path_rule(path, IgnoreAction::Include)
    }

    /// Exclude directories with this component name at any workspace depth.
    pub fn exclude_directory(name: impl AsRef<OsStr>) -> Result<Self, IgnorePolicyError> {
        Self::directory_rule(name, IgnoreAction::Exclude)
    }

    /// Include directories with this component name at any workspace depth.
    pub fn include_directory(name: impl AsRef<OsStr>) -> Result<Self, IgnorePolicyError> {
        Self::directory_rule(name, IgnoreAction::Include)
    }

    /// Exclude regular files with this extension (without the leading dot).
    pub fn exclude_extension(extension: impl AsRef<OsStr>) -> Result<Self, IgnorePolicyError> {
        Self::extension_rule(extension, IgnoreAction::Exclude)
    }

    /// Include regular files with this extension (without the leading dot).
    pub fn include_extension(extension: impl AsRef<OsStr>) -> Result<Self, IgnorePolicyError> {
        Self::extension_rule(extension, IgnoreAction::Include)
    }

    /// Return this rule's action.
    #[must_use]
    pub fn action(&self) -> IgnoreAction {
        self.action
    }

    fn path_rule(
        path: impl AsRef<Path>,
        action: IgnoreAction,
    ) -> Result<Self, IgnorePolicyError> {
        let path = path.as_ref();
        let mut normalized = PathBuf::new();
        let mut count = 0usize;
        for component in path.components() {
            match component {
                Component::Normal(value) => {
                    normalized.push(value);
                    count += 1;
                }
                Component::CurDir
                | Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_) => {
                    return Err(IgnorePolicyError::InvalidPath(path.to_path_buf()));
                }
            }
        }
        if count == 0 {
            return Err(IgnorePolicyError::InvalidPath(path.to_path_buf()));
        }
        Ok(Self {
            action,
            matcher: IgnoreMatcher::PathPrefix(normalized),
        })
    }

    fn directory_rule(
        name: impl AsRef<OsStr>,
        action: IgnoreAction,
    ) -> Result<Self, IgnorePolicyError> {
        let name = name.as_ref();
        let candidate = Path::new(name);
        if name.is_empty()
            || candidate.components().count() != 1
            || !matches!(candidate.components().next(), Some(Component::Normal(_)))
        {
            return Err(IgnorePolicyError::InvalidName(name.to_os_string()));
        }
        Ok(Self {
            action,
            matcher: IgnoreMatcher::DirectoryName(name.to_os_string()),
        })
    }

    fn extension_rule(
        extension: impl AsRef<OsStr>,
        action: IgnoreAction,
    ) -> Result<Self, IgnorePolicyError> {
        let extension = extension.as_ref();
        if extension.is_empty()
            || extension.to_string_lossy().contains('/')
            || extension.to_string_lossy().contains('\\')
            || extension.to_string_lossy().contains('.')
        {
            return Err(IgnorePolicyError::InvalidName(extension.to_os_string()));
        }
        Ok(Self {
            action,
            matcher: IgnoreMatcher::Extension(extension.to_os_string()),
        })
    }

    fn matches(&self, path: &Path, kind: WorkspaceEntryKind) -> bool {
        match &self.matcher {
            IgnoreMatcher::PathPrefix(prefix) => path == prefix || path.starts_with(prefix),
            IgnoreMatcher::DirectoryName(name) => {
                let components = path.components().collect::<Vec<_>>();
                components.iter().enumerate().any(|(index, component)| {
                    let is_directory_component =
                        index + 1 < components.len() || kind == WorkspaceEntryKind::Directory;
                    is_directory_component
                        && matches!(component, Component::Normal(value) if value == name)
                })
            }
            IgnoreMatcher::Extension(extension) => {
                kind == WorkspaceEntryKind::File && path.extension() == Some(extension.as_os_str())
            }
        }
    }

    fn may_match_descendant(&self, directory: &Path) -> bool {
        match &self.matcher {
            IgnoreMatcher::PathPrefix(prefix) => {
                prefix != directory && prefix.starts_with(directory)
            }
            IgnoreMatcher::DirectoryName(_) | IgnoreMatcher::Extension(_) => true,
        }
    }
}

/// Ordered, configurable workspace exclusion policy.
///
/// The default policy excludes common VCS metadata and generated/dependency
/// directories. Add rules with IgnorePolicy::push; later matching rules
/// override earlier ones. Built-ins behave as the first exclusion rule, so a
/// custom include rule can explicitly re-include a path.
///
/// This policy is not a .gitignore parser. Git-ignore file discovery and its
/// nested precedence semantics are intentionally left to a future adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnorePolicy {
    include_builtins: bool,
    rules: Vec<IgnoreRule>,
}

impl Default for IgnorePolicy {
    fn default() -> Self {
        Self {
            include_builtins: true,
            rules: Vec::new(),
        }
    }
}

impl IgnorePolicy {
    /// Create a policy with the default built-in exclusions enabled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a policy without built-in exclusions.
    #[must_use]
    pub fn without_builtins() -> Self {
        Self {
            include_builtins: false,
            rules: Vec::new(),
        }
    }

    /// Append a rule. The last matching rule wins.
    pub fn push(&mut self, rule: IgnoreRule) {
        self.rules.push(rule);
    }

    /// Return the configured custom rules in evaluation order.
    #[must_use]
    pub fn rules(&self) -> &[IgnoreRule] {
        &self.rules
    }

    /// Whether a workspace-relative entry is excluded by this policy.
    #[must_use]
    pub fn is_ignored(&self, path: &Path, kind: WorkspaceEntryKind) -> bool {
        self.decision(path, kind).0
    }

    /// Whether an ignored directory must still be traversed for a later
    /// include rule that targets content beneath it.
    #[must_use]
    pub fn should_descend(&self, path: &Path) -> bool {
        let (ignored, last_matching_index) =
            self.decision(path, WorkspaceEntryKind::Directory);
        if !ignored {
            return true;
        }

        self.rules.iter().enumerate().any(|(index, rule)| {
            index + 1 > last_matching_index
                && rule.action == IgnoreAction::Include
                && rule.may_match_descendant(path)
        })
    }

    fn decision(&self, path: &Path, kind: WorkspaceEntryKind) -> (bool, usize) {
        let mut ignored = self.include_builtins && builtin_directory_match(path, kind);
        let mut last_matching_index = 0usize;

        for (index, rule) in self.rules.iter().enumerate() {
            if rule.matches(path, kind) {
                ignored = rule.action == IgnoreAction::Exclude;
                last_matching_index = index + 1;
            }
        }

        (ignored, last_matching_index)
    }
}

fn builtin_directory_match(path: &Path, kind: WorkspaceEntryKind) -> bool {
    let components = path.components().collect::<Vec<_>>();
    components.iter().enumerate().any(|(index, component)| {
        let is_directory_component =
            index + 1 < components.len() || kind == WorkspaceEntryKind::Directory;
        matches!(component, Component::Normal(value)
            if (is_directory_component || value == OsStr::new(".git"))
                && BUILTIN_IGNORED_DIRECTORY_NAMES
                    .iter()
                    .any(|name| value == OsStr::new(name)))
    })
}

/// Errors produced while constructing ignore rules.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum IgnorePolicyError {
    /// A rule path is empty, absolute, or contains non-normal components.
    #[error("ignore rule path must be a non-empty relative path: {0}")]
    InvalidPath(PathBuf),
    /// A directory name or extension is empty or contains path separators.
    #[error("invalid ignore rule name: {0:?}")]
    InvalidName(OsString),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_excludes_vcs_and_generated_directories() {
        let policy = IgnorePolicy::default();

        for path in [
            "src/.git/config",
            "target/debug/app",
            "node_modules/pkg/index.js",
            "web/.next/cache",
            "python/.venv/lib/site.py",
        ] {
            assert!(policy.is_ignored(Path::new(path), WorkspaceEntryKind::File), "{path}");
        }

        assert!(!policy.is_ignored(Path::new("src/main.rs"), WorkspaceEntryKind::File));
    }

    #[test]
    fn custom_path_rule_excludes_only_its_subtree() {
        let mut policy = IgnorePolicy::without_builtins();
        policy.push(IgnoreRule::exclude_path("fixtures/generated").unwrap());

        assert!(policy.is_ignored(
            Path::new("fixtures/generated/data.json"),
            WorkspaceEntryKind::File
        ));
        assert!(!policy.is_ignored(
            Path::new("fixtures/source.json"),
            WorkspaceEntryKind::File
        ));
    }

    #[test]
    fn directory_name_rule_matches_at_any_depth() {
        let mut policy = IgnorePolicy::without_builtins();
        policy.push(IgnoreRule::exclude_directory("cache").unwrap());

        assert!(policy.is_ignored(
            Path::new("src/cache/output.bin"),
            WorkspaceEntryKind::File
        ));
        assert!(policy.is_ignored(
            Path::new("src/cache"),
            WorkspaceEntryKind::Directory
        ));
    }

    #[test]
    fn extension_rule_only_matches_files() {
        let mut policy = IgnorePolicy::without_builtins();
        policy.push(IgnoreRule::exclude_extension("log").unwrap());

        assert!(policy.is_ignored(Path::new("logs/app.log"), WorkspaceEntryKind::File));
        assert!(!policy.is_ignored(Path::new("logs/app.rs"), WorkspaceEntryKind::File));
        assert!(!policy.is_ignored(Path::new("logs"), WorkspaceEntryKind::Directory));
    }

    #[test]
    fn last_matching_rule_wins_and_can_reinclude_a_path() {
        let mut policy = IgnorePolicy::default();
        policy.push(IgnoreRule::exclude_path("target").unwrap());
        policy.push(IgnoreRule::include_path("target/keep").unwrap());

        assert!(policy.is_ignored(Path::new("target/tmp/file.rs"), WorkspaceEntryKind::File));
        assert!(!policy.is_ignored(Path::new("target/keep/main.rs"), WorkspaceEntryKind::File));
    }

    #[test]
    fn ignored_parent_is_traversed_only_for_a_later_include_rule() {
        let mut policy = IgnorePolicy::default();
        policy.push(IgnoreRule::include_path("target/keep/main.rs").unwrap());

        assert!(policy.is_ignored(Path::new("target"), WorkspaceEntryKind::Directory));
        assert!(policy.should_descend(Path::new("target")));
        assert!(policy.is_ignored(Path::new("target/keep"), WorkspaceEntryKind::Directory));
        assert!(policy.should_descend(Path::new("target/keep")));
        assert!(!policy.is_ignored(
            Path::new("target/keep/main.rs"),
            WorkspaceEntryKind::File
        ));
    }

    #[test]
    fn a_later_exclusion_overrides_an_earlier_include() {
        let mut policy = IgnorePolicy::default();
        policy.push(IgnoreRule::include_path("target/keep/main.rs").unwrap());
        policy.push(IgnoreRule::exclude_path("target/keep").unwrap());

        assert!(policy.is_ignored(
            Path::new("target/keep/main.rs"),
            WorkspaceEntryKind::File
        ));
        assert!(!policy.should_descend(Path::new("target/keep")));
    }

    #[test]
    fn rejects_absolute_and_traversal_rule_paths() {
        assert!(IgnoreRule::exclude_path("/etc").is_err());
        assert!(IgnoreRule::exclude_path("../outside").is_err());
        assert!(IgnoreRule::exclude_path(".").is_err());
        assert!(IgnoreRule::exclude_directory("../cache").is_err());
        assert!(IgnoreRule::exclude_extension("rs/log").is_err());
    }

    #[test]
    fn builtins_can_be_disabled() {
        let policy = IgnorePolicy::without_builtins();
        assert!(!policy.is_ignored(Path::new("target"), WorkspaceEntryKind::Directory));
    }
    #[test]
    fn excludes_git_worktree_marker_file() {
        let policy = IgnorePolicy::default();
        assert!(policy.is_ignored(Path::new(".git"), WorkspaceEntryKind::File));
    }
}

