//! One writer per federation data directory.
//!
//! The peer store is a read-modify-write file: two writers that both load,
//! change, and save lose one of the changes. The runtime, the CLI, and the
//! settings pane therefore take an advisory lock on the directory's own lock
//! file around their mutation. A running federation node holds that lock for
//! its lifetime, so no other writer can silently diverge from its in-memory
//! store.

use std::fs::{self, File, OpenOptions};
use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::DataDirLock;

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
}
