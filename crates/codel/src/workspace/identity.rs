use std::{
    fmt, fs,
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
/// use codel::WorkspaceId;
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
        fmt::Display::fmt(&self.0, formatter)
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
    pub fn new(id: WorkspaceId, root: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let supplied_root = root.as_ref();
        let canonical_root =
            fs::canonicalize(supplied_root).map_err(|source| WorkspaceError::Canonicalize {
                path: supplied_root.to_path_buf(),
                source,
            })?;

        let metadata =
            fs::metadata(&canonical_root).map_err(|source| WorkspaceError::Canonicalize {
                path: canonical_root.clone(),
                source,
            })?;

        if !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory {
                path: canonical_root,
            });
        }

        Ok(Self { id, canonical_root })
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

        let metadata = fs::metadata(&current).map_err(|source| WorkspaceError::Canonicalize {
            path: current.clone(),
            source,
        })?;

        if !metadata.is_dir() {
            return Err(WorkspaceError::NotDirectory { path: current });
        }

        Ok(())
    }
}

/// In-memory registry that enforces a one-to-one mapping between workspace
/// IDs and canonical roots within one Engine instance.
///
/// The registry does not persist IDs. The embedding host must reload the same
/// ID for a checkout after restart. Different worktrees must be registered
/// with different IDs, even when they share a Git repository.
#[derive(Debug, Default)]
pub struct WorkspaceRegistry {
    by_id: std::collections::HashMap<WorkspaceId, WorkspaceDescriptor>,
    by_root: std::collections::HashMap<PathBuf, WorkspaceId>,
}

impl WorkspaceRegistry {
    /// Register a workspace, allowing an identical registration to be repeated.
    ///
    /// # Errors
    ///
    /// Returns DuplicateId if an ID is already associated with another root,
    /// or DuplicateRoot if a root is already associated with another ID.
    pub fn register(&mut self, descriptor: WorkspaceDescriptor) -> Result<(), WorkspaceError> {
        let id = descriptor.id();
        let root = descriptor.canonical_root().to_path_buf();

        if let Some(existing) = self.by_id.get(&id) {
            if existing.canonical_root() != root {
                return Err(WorkspaceError::DuplicateId {
                    id,
                    existing_root: existing.canonical_root().to_path_buf(),
                    new_root: root,
                });
            }
            return Ok(());
        }

        if let Some(existing_id) = self.by_root.get(&root) {
            if *existing_id != id {
                return Err(WorkspaceError::DuplicateRoot {
                    root,
                    existing_id: *existing_id,
                    new_id: id,
                });
            }
            return Ok(());
        }

        self.by_root.insert(root, id);
        self.by_id.insert(id, descriptor);
        Ok(())
    }

    /// Find a registered workspace by its opaque identity.
    #[must_use]
    pub fn get(&self, id: WorkspaceId) -> Option<&WorkspaceDescriptor> {
        self.by_id.get(&id)
    }

    /// Remove a workspace and return its descriptor, if registered.
    pub fn remove(&mut self, id: WorkspaceId) -> Option<WorkspaceDescriptor> {
        let descriptor = self.by_id.remove(&id)?;
        self.by_root.remove(descriptor.canonical_root());
        Some(descriptor)
    }

    /// Return the number of registered workspaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Return whether the registry contains no workspaces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
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

    /// A workspace ID is already associated with a different root.
    #[error("workspace ID {id} is already registered for {existing_root}, not {new_root}")]
    DuplicateId {
        /// Conflicting workspace ID.
        id: WorkspaceId,
        /// Root already associated with the ID.
        existing_root: PathBuf,
        /// Root being registered.
        new_root: PathBuf,
    },

    /// A canonical root is already associated with a different workspace ID.
    #[error("workspace root {root} is already registered as {existing_id}, not {new_id}")]
    DuplicateRoot {
        /// Canonical root already registered.
        root: PathBuf,
        /// ID already associated with the root.
        existing_id: WorkspaceId,
        /// ID being registered.
        new_id: WorkspaceId,
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
    fn registry_keeps_worktrees_with_separate_ids() {
        let first = tempfile::tempdir().expect("first worktree");
        let second = tempfile::tempdir().expect("second worktree");
        let first_id = WorkspaceId::new();
        let second_id = WorkspaceId::new();
        let mut registry = WorkspaceRegistry::default();

        registry
            .register(WorkspaceDescriptor::new(first_id, first.path()).expect("first descriptor"))
            .expect("register first worktree");
        registry
            .register(
                WorkspaceDescriptor::new(second_id, second.path()).expect("second descriptor"),
            )
            .expect("register second worktree");

        assert_eq!(registry.len(), 2);
        assert_ne!(
            registry
                .get(first_id)
                .expect("first registered")
                .canonical_root(),
            registry
                .get(second_id)
                .expect("second registered")
                .canonical_root()
        );
    }

    #[test]
    fn registry_rejects_one_id_for_two_roots() {
        let first = tempfile::tempdir().expect("first workspace");
        let second = tempfile::tempdir().expect("second workspace");
        let id = WorkspaceId::new();
        let mut registry = WorkspaceRegistry::default();
        registry
            .register(WorkspaceDescriptor::new(id, first.path()).expect("first descriptor"))
            .expect("register first workspace");

        let error = registry
            .register(WorkspaceDescriptor::new(id, second.path()).expect("second descriptor"))
            .expect_err("one ID cannot identify two roots");

        assert!(matches!(error, WorkspaceError::DuplicateId { .. }));
    }

    #[test]
    fn registry_rejects_two_ids_for_one_root() {
        let directory = tempfile::tempdir().expect("workspace");
        let mut registry = WorkspaceRegistry::default();
        registry
            .register(
                WorkspaceDescriptor::new(WorkspaceId::new(), directory.path())
                    .expect("first descriptor"),
            )
            .expect("register first identity");

        let error = registry
            .register(
                WorkspaceDescriptor::new(WorkspaceId::new(), directory.path())
                    .expect("second descriptor"),
            )
            .expect_err("one root cannot have two IDs");

        assert!(matches!(error, WorkspaceError::DuplicateRoot { .. }));
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
