//! Bounded, no-follow directory enumeration for a workspace capability.

use std::path::{Component, Path, PathBuf};

use cap_fs_ext::DirExt;
use cap_std::fs::Dir;

use super::{WorkspaceFs, WorkspaceFsError};

/// The kind of a filesystem entry returned by WorkspaceFs::read_dir.
///
/// Symbolic links and special files are intentionally omitted from results.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceEntryKind {
    /// A regular file.
    File,
    /// A directory. Opening it for traversal must still use no-follow semantics.
    Directory,
}

/// A safe, workspace-relative directory entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEntry {
    /// Path relative to the workspace root.
    pub relative_path: PathBuf,
    /// Entry kind determined without following symbolic links.
    pub kind: WorkspaceEntryKind,
}

impl WorkspaceFs {
    /// Enumerate one directory, returning at most max_entries regular files
    /// and directories. The empty path selects the workspace root.
    ///
    /// Symbolic links and non-regular special files are omitted. Results are
    /// sorted by their workspace-relative path for deterministic indexing.
    /// The method does not recurse; callers must apply their own depth and
    /// aggregate scan budgets and use no-follow APIs for each child directory.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid paths, inaccessible directories, or when
    /// the number of eligible entries exceeds max_entries.
    pub fn read_dir(
        &self,
        relative_path: impl AsRef<Path>,
        max_entries: usize,
    ) -> Result<Vec<WorkspaceEntry>, WorkspaceFsError> {
        let relative_path = relative_path.as_ref();
        let components = directory_components(relative_path)?;
        if components.len() > super::MAX_PATH_DEPTH {
            return Err(WorkspaceFsError::PathTooDeep {
                path: relative_path.to_path_buf(),
                max_depth: super::MAX_PATH_DEPTH,
            });
        }

        let mut current: Option<Dir> = None;
        for component in &components {
            let next = match current.as_ref() {
                Some(parent) => parent.open_dir_nofollow(component),
                None => self.root.open_dir_nofollow(component),
            }
            .map_err(|source| WorkspaceFsError::Io {
                path: relative_path.to_path_buf(),
                source,
            })?;
            current = Some(next);
        }

        let directory = current.as_ref().unwrap_or(&self.root);
        let entries = directory.entries().map_err(|source| WorkspaceFsError::Io {
            path: relative_path.to_path_buf(),
            source,
        })?;

        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| WorkspaceFsError::Io {
                path: relative_path.to_path_buf(),
                source,
            })?;
            let name = entry.file_name();

            // Query metadata relative to the already-open directory capability.
            // symlink_metadata does not follow a final symbolic link.
            let metadata = directory
                .symlink_metadata(&name)
                .map_err(|source| WorkspaceFsError::Io {
                    path: relative_path.join(&name),
                    source,
                })?;

            let kind = if metadata.is_file() {
                WorkspaceEntryKind::File
            } else if metadata.is_dir() {
                WorkspaceEntryKind::Directory
            } else {
                // Symlinks, sockets, pipes, devices, and other special entries
                // are not eligible for indexing.
                continue;
            };

            if result.len() == max_entries {
                return Err(WorkspaceFsError::EntryLimitExceeded {
                    path: relative_path.to_path_buf(),
                    max_entries,
                });
            }

            result.push(WorkspaceEntry {
                relative_path: relative_path.join(name),
                kind,
            });
        }

        result.sort_unstable_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(result)
    }
}

fn directory_components(path: &Path) -> Result<Vec<std::ffi::OsString>, WorkspaceFsError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(name.to_os_string()),
            Component::CurDir | Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(WorkspaceFsError::InvalidRelativePath {
                    path: path.to_path_buf(),
                });
            }
        }
    }
    Ok(components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn workspace_fs(root: &Path) -> WorkspaceFs {
        let descriptor = crate::WorkspaceDescriptor::new(crate::WorkspaceId::new(), root)
            .expect("create workspace descriptor");
        WorkspaceFs::open(&descriptor).expect("open workspace capability")
    }

    #[test]
    fn lists_root_entries_in_deterministic_order() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("z.rs"), "z").expect("write z");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::write(root.path().join("a.rs"), "a").expect("write a");

        let workspace = workspace_fs(root.path());
        let entries = workspace.read_dir("", 10).expect("list root");

        assert_eq!(
            entries.iter().map(|entry| entry.relative_path.as_path()).collect::<Vec<_>>(),
            vec![Path::new("a.rs"), Path::new("src"), Path::new("z.rs")]
        );
        assert_eq!(entries[0].kind, WorkspaceEntryKind::File);
        assert_eq!(entries[1].kind, WorkspaceEntryKind::Directory);
    }

    #[test]
    fn lists_nested_directory_without_reopening_ambient_paths() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::write(root.path().join("src/lib.rs"), "lib").expect("write lib");

        let workspace = workspace_fs(root.path());
        let entries = workspace.read_dir("src", 10).expect("list src");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("src/lib.rs"));
        assert_eq!(entries[0].kind, WorkspaceEntryKind::File);
    }

    #[test]
    fn enforces_entry_limit() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("a"), "a").expect("write a");
        fs::write(root.path().join("b"), "b").expect("write b");

        let workspace = workspace_fs(root.path());
        assert!(matches!(
            workspace.read_dir("", 1),
            Err(WorkspaceFsError::EntryLimitExceeded { max_entries: 1, .. })
        ));
    }

    #[test]
    fn rejects_traversal_and_symlinked_directory_paths() {
        let root = tempdir().expect("create workspace");
        let outside = tempdir().expect("create outside");
        fs::write(outside.path().join("secret"), "secret").expect("write secret");

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked"))
            .expect("create directory symlink");

        let workspace = workspace_fs(root.path());
        assert!(matches!(
            workspace.read_dir("../outside", 10),
            Err(WorkspaceFsError::InvalidRelativePath { .. })
        ));

        #[cfg(unix)]
        assert!(workspace.read_dir("linked", 10).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn omits_symbolic_link_entries() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("target.rs"), "source").expect("write target");
        symlink(root.path().join("target.rs"), root.path().join("link.rs"))
            .expect("create symlink");

        let workspace = workspace_fs(root.path());
        let entries = workspace.read_dir("", 10).expect("list root");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, Path::new("target.rs"));
    }

    #[test]
    fn zero_entry_budget_accepts_only_empty_directories() {
        let root = tempdir().expect("create workspace");
        let workspace = workspace_fs(root.path());
        assert!(workspace.read_dir("", 0).expect("empty root").is_empty());

        fs::write(root.path().join("one"), "1").expect("write one");
        assert!(matches!(
            workspace.read_dir("", 0),
            Err(WorkspaceFsError::EntryLimitExceeded { max_entries: 0, .. })
        ));
    }
}
