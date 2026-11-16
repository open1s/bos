//! Capability-based, read-only access to a registered workspace.
//!
//! The root directory is opened once and retained as a directory capability.
//! Every subsequent lookup is relative to that handle. Path components are
//! opened one at a time with symlink following disabled, including the final
//! file component. The embedding host is responsible for selecting the root
//! directory; after opening, this module does not use ambient paths again.

use std::{
    ffi::OsString,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

use cap_fs_ext::{
    ambient_authority, DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsMaybeDirExt,
    OpenOptionsSyncExt,
};
use cap_std::fs::{Dir, File, OpenOptions};

use crate::WorkspaceDescriptor;

const MAX_PATH_DEPTH: usize = 256;

/// Read-only filesystem capability for one workspace root.
pub struct WorkspaceFs {
    root: Dir,
    root_path: PathBuf,
}

impl WorkspaceFs {
    /// Open a directory capability for a previously validated workspace.
    ///
    /// The host must choose and authorize the workspace root. This operation
    /// uses ambient authority exactly once to acquire the root handle; all
    /// subsequent access is relative to that handle.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the root directory cannot be opened.
    pub fn open(workspace: &WorkspaceDescriptor) -> Result<Self, WorkspaceFsError> {
        let root_path = workspace.canonical_root().to_path_buf();
        let root = Dir::open_ambient_dir(&root_path, ambient_authority()).map_err(|source| {
            WorkspaceFsError::Io {
                path: root_path.clone(),
                source,
            }
        })?;

        Ok(Self { root, root_path })
    }

    /// Read a regular file beneath the workspace root, enforcing a byte limit.
    ///
    /// Absolute paths, parent traversal, current-directory components, and
    /// paths containing no normal components are rejected. Symlinks are not
    /// followed in any path component. Special files are rejected after a
    /// non-blocking open, before their contents are read.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid paths, filesystem failures, non-regular
    /// files, invalid limits, or files exceeding the supplied byte limit.
    pub fn read_file(
        &self,
        relative_path: impl AsRef<Path>,
        max_bytes: usize,
    ) -> Result<Vec<u8>, WorkspaceFsError> {
        let relative_path = relative_path.as_ref();
        let components = normal_components(relative_path)?;
        if components.len() > MAX_PATH_DEPTH {
            return Err(WorkspaceFsError::PathTooDeep {
                path: relative_path.to_path_buf(),
                max_depth: MAX_PATH_DEPTH,
            });
        }
        if max_bytes == usize::MAX {
            return Err(WorkspaceFsError::InvalidLimit);
        }

        let (file_name, parents) = components
            .split_last()
            .ok_or_else(|| WorkspaceFsError::InvalidRelativePath {
                path: relative_path.to_path_buf(),
            })?;

        let mut current: Option<Dir> = None;
        for component in parents {
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

        let parent = current.as_ref().unwrap_or(&self.root);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .maybe_dir(true)
            .nonblock(true);

        let file: File = parent
            .open_with(file_name, &options)
            .map_err(|source| WorkspaceFsError::Io {
                path: relative_path.to_path_buf(),
                source,
            })?;

        let metadata = file.metadata().map_err(|source| WorkspaceFsError::Io {
            path: relative_path.to_path_buf(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(WorkspaceFsError::NotRegularFile {
                path: relative_path.to_path_buf(),
            });
        }

        let max_bytes_u64 = max_bytes as u64;
        if metadata.len() > max_bytes_u64 {
            return Err(WorkspaceFsError::FileTooLarge {
                path: relative_path.to_path_buf(),
                max_bytes,
            });
        }

        let mut contents = Vec::with_capacity(metadata.len() as usize);
        file.take((max_bytes + 1) as u64)
            .read_to_end(&mut contents)
            .map_err(|source| WorkspaceFsError::Io {
                path: relative_path.to_path_buf(),
                source,
            })?;

        if contents.len() > max_bytes {
            return Err(WorkspaceFsError::FileTooLarge {
                path: relative_path.to_path_buf(),
                max_bytes,
            });
        }

        Ok(contents)
    }

    /// Return the canonical root path used to open this capability.
    ///
    /// This is informational only. File access is performed through the
    /// retained directory handle, not by reopening this path.
    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }
}

fn normal_components(path: &Path) -> Result<Vec<OsString>, WorkspaceFsError> {
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

    if components.is_empty() {
        return Err(WorkspaceFsError::InvalidRelativePath {
            path: path.to_path_buf(),
        });
    }

    Ok(components)
}

/// Errors returned by the workspace filesystem capability.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceFsError {
    /// A workspace-relative filesystem operation failed.
    #[error("workspace filesystem operation failed for {path}: {source}")]
    Io {
        /// Relative or root path associated with the operation.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },

    /// The requested path is not a non-empty, normal relative path.
    #[error("invalid workspace-relative path: {path}")]
    InvalidRelativePath {
        /// Rejected path.
        path: PathBuf,
    },

    /// The opened object is not a regular file.
    #[error("workspace path is not a regular file: {path}")]
    NotRegularFile {
        /// Rejected path.
        path: PathBuf,
    },

    /// The file exceeds the caller-supplied byte limit.
    #[error("workspace file exceeds the {max_bytes}-byte limit: {path}")]
    FileTooLarge {
        /// Rejected path.
        path: PathBuf,
        /// Maximum accepted size.
        max_bytes: usize,
    },

    /// The relative path exceeds the maximum component depth.
    #[error("workspace path exceeds the {max_depth}-component limit: {path}")]
    PathTooDeep {
        /// Rejected path.
        path: PathBuf,
        /// Maximum accepted component count.
        max_depth: usize,
    },

    /// The caller supplied an unrepresentable read limit.
    #[error("workspace file read limit is too large")]
    InvalidLimit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    fn workspace_fs(root: &Path) -> WorkspaceFs {
        let descriptor = WorkspaceDescriptor::new(crate::WorkspaceId::new(), root)
            .expect("create workspace descriptor");
        WorkspaceFs::open(&descriptor).expect("open workspace capability")
    }

    #[test]
    fn reads_regular_file_beneath_workspace() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("src")).expect("create src");
        fs::write(root.path().join("src/main.rs"), "fn main() {}").expect("write source");

        let workspace = workspace_fs(root.path());
        let contents = workspace
            .read_file("src/main.rs", 1024)
            .expect("read source");

        assert_eq!(contents, b"fn main() {}");
    }

    #[test]
    fn rejects_absolute_and_traversal_paths() {
        let root = tempdir().expect("create workspace");
        let workspace = workspace_fs(root.path());

        assert!(matches!(
            workspace.read_file("../outside.txt", 1024),
            Err(WorkspaceFsError::InvalidRelativePath { .. })
        ));
        assert!(matches!(
            workspace.read_file(root.path().join("outside.txt"), 1024),
            Err(WorkspaceFsError::InvalidRelativePath { .. })
        ));
        assert!(matches!(
            workspace.read_file(".", 1024),
            Err(WorkspaceFsError::InvalidRelativePath { .. })
        ));
    }

    #[test]
    fn enforces_file_size_limit() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("large.txt"), "0123456789").expect("write fixture");
        let workspace = workspace_fs(root.path());

        assert!(matches!(
            workspace.read_file("large.txt", 4),
            Err(WorkspaceFsError::FileTooLarge { max_bytes: 4, .. })
        ));
    }

    #[test]
    fn rejects_directory_as_file() {
        let root = tempdir().expect("create workspace");
        fs::create_dir(root.path().join("nested")).expect("create nested");
        let workspace = workspace_fs(root.path());

        assert!(matches!(
            workspace.read_file("nested", 1024),
            Err(WorkspaceFsError::NotRegularFile { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_final_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create workspace");
        let outside = tempdir().expect("create outside directory");
        fs::write(outside.path().join("secret.txt"), "secret").expect("write outside file");
        symlink(outside.path().join("secret.txt"), root.path().join("link.txt"))
            .expect("create symlink");

        let workspace = workspace_fs(root.path());
        assert!(workspace.read_file("link.txt", 1024).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_intermediate_symlink_to_outside_directory() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("create workspace");
        let outside = tempdir().expect("create outside directory");
        fs::write(outside.path().join("secret.txt"), "secret").expect("write outside file");
        symlink(outside.path(), root.path().join("escape")).expect("create directory symlink");

        let workspace = workspace_fs(root.path());
        assert!(workspace.read_file("escape/secret.txt", 1024).is_err());
    }

    #[test]
    fn empty_file_is_readable_with_zero_limit() {
        let root = tempdir().expect("create workspace");
        fs::write(root.path().join("empty.txt"), b"").expect("write empty file");
        let workspace = workspace_fs(root.path());

        assert_eq!(workspace.read_file("empty.txt", 0).expect("read empty"), b"");
    }
}
