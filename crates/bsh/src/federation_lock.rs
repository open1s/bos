//! One writer per federation data directory.
//!
//! The peer store is a read-modify-write file: two writers that both load,
//! change, and save lose one of the changes. The runtime, the CLI, and the
//! settings pane therefore take an advisory lock on the directory's own lock
//! file around their mutation. A running federation node holds that lock for
//! its lifetime, so no other writer can silently diverge from its in-memory
//! store.
//!
//! A node's files are not necessarily in one directory: `identity_path` and
//! `peers_path` are configured separately, so a reader can keep the key beside
//! one workspace and the peer grants in another. `NodeLocks` therefore owns the
//! set of directories a node's files live in and takes all of them, because an
//! identity in one directory with a shared peer store is exactly the shape that
//! two nodes would otherwise both own.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

/// The lock a process holds while it owns a federation data directory.
pub(crate) struct DataDirLock {
    file: File,
}

impl DataDirLock {
    /// Take the directory's lock, or report who else holds it.
    pub(crate) fn acquire(directory: &Path) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|error| {
            format!(
                "the federation directory {} could not be created: {error}",
                directory.display()
            )
        })?;
        let path = directory.join("node.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "the federation lock {} could not be opened: {error}",
                    path.display()
                )
            })?;
        file.try_lock_exclusive().map_err(|_| {
            format!(
                "another BSH instance is using the federation data directory ({})",
                directory.display()
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Every directory one node's files live in, sorted and de-duplicated.
///
/// A bare file name has no parent; it lives beside the process's current
/// directory, which is what `.` names here. Sorting is load-bearing: a node
/// whose identity and peer store sit in different directories takes both, and
/// two nodes that share one of them must take their locks in the same order or
/// they can deadlock instead of one being refused.
fn lock_dirs(identity_path: &Path, peers_path: &Path) -> Vec<PathBuf> {
    let parent = |path: &Path| {
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let mut directories = BTreeSet::new();
    directories.insert(parent(identity_path));
    directories.insert(parent(peers_path));
    directories.into_iter().collect()
}

/// The locks a process holds while it owns a node's files.
///
/// Holds one `DataDirLock` per distinct directory, in sorted order, and
/// releases all of them together. A partial acquisition releases what it took,
/// so a refused node leaves nothing behind.
pub(crate) struct NodeLocks {
    /// Held only for its Drop: every directory is released together.
    #[allow(dead_code)]
    locks: Vec<DataDirLock>,
}

impl NodeLocks {
    /// Take every directory the two paths live in, or report who else holds one.
    pub(crate) fn acquire(identity_path: &Path, peers_path: &Path) -> Result<Self, String> {
        let mut locks = Vec::new();
        for directory in lock_dirs(identity_path, peers_path) {
            locks.push(DataDirLock::acquire(&directory)?);
        }
        Ok(Self { locks })
    }
}

#[cfg(test)]
mod tests {
    use super::{lock_dirs, DataDirLock, NodeLocks};
    use std::path::{Path, PathBuf};

    fn scratch(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "bsh-federation-lock-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn a_second_writer_is_refused_until_the_first_releases() {
        let root = scratch("one");
        let first = DataDirLock::acquire(&root).expect("the first writer");

        let second = DataDirLock::acquire(&root);
        assert!(second.is_err(), "a running node owns the directory");

        drop(first);
        let again = DataDirLock::acquire(&root).expect("the lock is reusable");
        assert!(root.join("node.lock").is_file());
        drop(again);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn different_directories_do_not_share_a_lock() {
        let left = scratch("left");
        let right = scratch("right");
        let _left = DataDirLock::acquire(&left).expect("left");
        let _right = DataDirLock::acquire(&right).expect("right");
        let _ = std::fs::remove_dir_all(&left);
        let _ = std::fs::remove_dir_all(&right);
    }

    #[test]
    fn a_node_locks_both_directories_its_files_live_in() {
        let keys = scratch("node-keys");
        let peers = scratch("node-peers");
        let first = NodeLocks::acquire(&keys.join("node.key"), &peers.join("peers.json"))
            .expect("the first node");

        let second = NodeLocks::acquire(&keys.join("node.key"), &peers.join("peers.json"));
        assert!(second.is_err(), "a running node owns both directories");
        assert!(keys.join("node.lock").is_file());
        assert!(peers.join("node.lock").is_file());

        drop(first);
        let again = NodeLocks::acquire(&keys.join("node.key"), &peers.join("peers.json"))
            .expect("the locks are reusable");
        drop(again);
        let _ = std::fs::remove_dir_all(&keys);
        let _ = std::fs::remove_dir_all(&peers);
    }

    #[test]
    fn a_shared_peer_store_is_refused_though_the_identities_differ() {
        let left = scratch("shared-left");
        let right = scratch("shared-right");
        let peers = scratch("shared-peers");
        let _first = NodeLocks::acquire(&left.join("node.key"), &peers.join("peers.json"))
            .expect("the first node");

        let second = NodeLocks::acquire(&right.join("node.key"), &peers.join("peers.json"));

        assert!(
            second.is_err(),
            "two identities still write one peer store, so the second node must be refused"
        );
        assert!(!right.join("node.lock").is_file(), "nothing was taken");
        let _ = std::fs::remove_dir_all(&left);
        let _ = std::fs::remove_dir_all(&right);
        let _ = std::fs::remove_dir_all(&peers);
    }

    #[test]
    fn one_directory_is_locked_once() {
        let root = scratch("one-dir");

        // A second exclusive lock on a fresh descriptor of the same file would
        // fail, so acquiring at all proves the pair collapsed to one directory.
        let held = NodeLocks::acquire(&root.join("node.key"), &root.join("peers.json"))
            .expect("one directory, one lock");
        assert!(NodeLocks::acquire(&root.join("node.key"), &root.join("peers.json")).is_err());

        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bare_file_name_lives_beside_the_current_directory() {
        assert_eq!(
            lock_dirs(Path::new("node.key"), Path::new("peers.json")),
            vec![PathBuf::from(".")]
        );

        let root = scratch("bare");
        assert_eq!(
            lock_dirs(
                &root.join("node.key"),
                &root.join("keys").join("peers.json")
            ),
            vec![root.clone(), root.join("keys")]
        );
    }
}
