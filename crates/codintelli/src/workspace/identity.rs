use std::{
    fmt,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use thiserror::Error;
use uuid::Uuid;

/// Opaque identity for one logical workspace checkout.
///
/// A separate ID must be assigned to each Git worktree, even when multiple
/// worktrees share the same Git repository. The ID is independent of the
/// workspace path and must be persisted by the embedding host if it needs to
/// remain stable across process restarts.
///
/// # Examples
///
/// ```
/// use codintelli::WorkspaceId;
///
/// let id = WorkspaceId::new();
/// let encoded = id.to_string();
/// let decoded: WorkspaceId = encoded.parse().expect("valid workspace ID");
/// assert_eq!(id, decoded);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceId(Uuid);

impl WorkspaceId {
    /// Generate a new, globally unique workspace identity.
    ///
    /// Persist the returned value in host-managed metadata if it must survive
    /// restarts. Generating a new ID for an existing checkout intentionally
    /// creates a new logical workspace identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the underlying UUID value.
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for WorkspaceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for WorkspaceId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// A workspace identity paired with its canonical directory root.
///
/// Construction resolves the supplied path through the filesystem and
/// rejects missing paths and non-directories. This descriptor is not itself
/// a sandbox: path checks followed by later file opens can be subject to
/// symlink and time-of-check/time-of-use races.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceDescriptor {
    id: WorkspaceId,
    canonical_root: PathBuf,
}

impl WorkspaceDescriptor {
    /// Create a descriptor after canonicalizing and validating a workspace
    /// directory.
    ///
    /// The supplied ID is caller-owned. The caller must ensure different
    /// worktrees receive different IDs and persist IDs when stable identity
    /// across restarts is required.
    ///
    /// # Errors
    ///
    /// Returns WorkspaceError::Canonicalize if the path cannot be
    /// canonicalized, or WorkspaceError::NotDirectory if it is not a directory.
    pub fn new(
        id: WorkspaceId,
        root: impl AsRef<Path>,
    ) -> Result<Self, WorkspaceError> {
        let supplied_root = root.as_ref();
        let canonical_root = fs::canonicalize(supplied_root).map_err(|source| {
            WorkspaceError::Canonicalize {
                path: supplied_root.to_path_buf(),
                source,
            }
        })?;

        let metadata = fs::metadata(&canonical_root).map_err(|source| {
            WorkspaceError::Canonicalize {
                path: canonical_root.clone(),
                source,
            }
        })?;

        if !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory {
                path: canonical_root,
            });
        }

        Ok(Self {
            id,
            canonical_root,
        })
    }

    /// Return this workspace's opaque identity.
    #[must_use]
    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    /// Return the canonical filesystem root resolved at construction time.
    #[must_use]
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Re-resolve the root and reject a missing or redirected directory.
    ///
    /// This detects a root whose path now resolves elsewhere, such as a path
    /// replaced by a symlink. It does not detect replacement by a different
    /// directory at the exact same canonical path. Filesystem object identity
    /// and race-resistant access belong to the later confinement layer.
    ///
    /// # Errors
    ///
    /// Returns WorkspaceError::Canonicalize if the root is unavailable,
    /// or WorkspaceError::RootChanged if it resolves to a different path.
    pub fn verify_root(&self) -> Result<(), WorkspaceError> {
        let current = fs::canonicalize(&self.canonical_root).map_err(|source| {
            WorkspaceError::Canonicalize {
                path: self.canonical_root.clone(),
                source,
            }
        })?;

        if current != self.canonical_root {
            return Err(WorkspaceError::RootChanged {
                expected: self.canonical_root.clone(),
                actual: current,
            });
        }

        let metadata = fs::metadata(&current).map_err(|source| {
            WorkspaceError::Canonicalize {
                path: current.clone(),
                source,
            }
        })?;

        if !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory { path: current });
        }

        Ok(())
    }
}

/// Errors raised while identifying or validating a workspace root.
#[derive(Debug, Error)]
pub enum WorkspaceError {
    /// The supplied workspace root could not be canonicalized or inspected.
    #[error("cannot resolve workspace root {path}: {source}")]
    Canonicalize {
        /// Path that could not be resolved.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// The resolved workspace root is not a directory.
    #[error("workspace root is not a directory: {path}")]
    NotDirectory {
        /// Resolved path that is not a directory.
        path: PathBuf,
    },

    /// The workspace root now resolves to a different canonical path.
    #[error("workspace root changed from {expected} to {actual}")]
    RootChanged {
        /// Canonical path recorded at construction.
        expected: PathBuf,
        /// Current canonical path.
        actual: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_id_round_trips_as_a_stable_string() {
        let id = WorkspaceId::new();
        let decoded: WorkspaceId = id.to_string().parse().expect("parse workspace ID");
        assert_eq!(id, decoded);
        assert_eq!(id.as_uuid(), decoded.as_uuid());
    }

    #[test]
    fn workspace_ids_are_independent() {
        assert_ne!(WorkspaceId::new(), WorkspaceId::new());
    }

    #[test]
    fn descriptor_stores_canonical_root() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let supplied = directory.path().join(".");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), supplied).expect("open workspace");
        assert_eq!(
            descriptor.canonical_root(),
            fs::canonicalize(directory.path()).expect("canonicalize temporary directory")
        );
        descriptor.verify_root().expect("root remains valid");
    }

    #[test]
    fn descriptor_rejects_missing_root() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let missing = directory.path().join("does-not-exist");
        let error = WorkspaceDescriptor::new(WorkspaceId::new(), &missing)
            .expect_err("missing workspace must fail");
        assert!(matches!(error, WorkspaceError::Canonicalize { .. }));
    }

    #[test]
    fn descriptor_rejects_file_as_root() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let file = directory.path().join("not-a-directory");
        fs::write(&file, "data").expect("create fixture file");
        let error = WorkspaceDescriptor::new(WorkspaceId::new(), &file)
            .expect_err("file cannot be a workspace root");
        assert!(matches!(error, WorkspaceError::NotDirectory { .. }));
    }

    #[test]
    fn verify_root_rejects_removed_workspace() {
        let directory = tempfile::tempdir().expect("create temporary directory");
        let root = directory.path().join("workspace");
        fs::create_dir(&root).expect("create workspace directory");
        let descriptor =
            WorkspaceDescriptor::new(WorkspaceId::new(), &root).expect("open workspace");
        fs::remove_dir(&root).expect("remove workspace directory");
        assert!(matches!(
            descriptor.verify_root(),
            Err(WorkspaceError::Canonicalize { .. })
        ));
    }
}
