//! Storage identity and cross-process coordination (issue #341).
//!
//! Two handles to "the same file" must mean one coordinator, even when the
//! paths differ by a symlink, a relative prefix or a trailing alias.
//! [`canonical_identity`] resolves any path to that one key. [`FileLock`]
//! serializes commits across processes with an advisory lock on a sibling
//! lock file; within one process the memory manager adds one async gate per
//! identity on top.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::durable::{DurableError, IoStage};

/// Resolve `path` to the one key that names its storage identity.
///
/// An existing path is canonicalized (symlinks resolved). A path that does not
/// exist yet is resolved against its nearest existing ancestor, so a store and
/// its not-yet-created sibling agree on the directory identity. The result is
/// stable for aliases: `a/memory.jsonl` and `a/link/memory.jsonl` (where `link`
/// points at `a`) produce the same key.
#[must_use]
pub fn canonical_identity(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = absolute.as_path();
    loop {
        if let Ok(canonical) = fs::canonicalize(ancestor) {
            let mut identity = canonical;
            for part in tail.iter().rev() {
                identity.push(part);
            }
            return identity;
        }
        match ancestor.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                ancestor = ancestor.parent().unwrap_or(ancestor);
                if ancestor == Path::new("") {
                    break;
                }
            }
            None => break,
        }
    }
    absolute
}

/// An advisory cross-process write lock on a sibling lock file.
///
/// The lock is released when the guard drops or the holding process exits, so
/// a crash cannot strand it. The lock file itself is never deleted: removing
/// it would race with other processes already locking the same inode.
#[derive(Debug)]
pub struct FileLock {
    file: fs::File,
    path: PathBuf,
}

/// Whether an OS lock call failed because another holder owns the lock.
///
/// Unix reports `WouldBlock` (EWOULDBLOCK) or `PermissionDenied` (EACCES).
/// Windows reports `ERROR_LOCK_VIOLATION` (33), which std may surface as
/// `Other`/`Uncategorized` depending on the version, so the raw code is
/// checked too. Without this a Windows lock conflict would be a generic IO
/// error and callers could not tell "someone else holds it" from "I/O broke".
fn is_lock_conflict(error: &io::Error) -> bool {
    let conflict = matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::PermissionDenied
    );
    #[cfg(windows)]
    {
        conflict || error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        conflict
    }
}

/// How long a bounded acquire waits between two lock attempts.
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(5);

impl FileLock {
    /// Acquire the lock for `store_path`, waiting for any current holder.
    pub fn acquire(store_path: &Path) -> Result<Self, DurableError> {
        Self::open(store_path, true)
    }

    /// Acquire the lock or fail immediately with [`DurableError::Locked`].
    pub fn try_acquire(store_path: &Path) -> Result<Self, DurableError> {
        Self::open(store_path, false)
    }

    /// Acquire the lock, retrying for at most `budget` before giving up.
    ///
    /// This is the bounded form of a write-lock acquire: a caller that must not
    /// block a UI thread (a history append) waits a short while for a peer to
    /// finish and then gets the typed [`DurableError::Locked`] instead of
    /// hanging. [`FileLock::acquire`] keeps its blocking semantics, so a
    /// caller that wants to queue behind the holder still can.
    ///
    /// Only a lock conflict is retried; any other storage error (a directory
    /// that cannot be created, a lock file that cannot be opened) is returned
    /// immediately because waiting cannot fix it.
    pub fn acquire_bounded(
        store_path: &Path,
        budget: std::time::Duration,
    ) -> Result<Self, DurableError> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            match Self::try_acquire(store_path) {
                Ok(lock) => return Ok(lock),
                Err(error @ DurableError::Locked { .. }) => {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        return Err(error);
                    }
                    std::thread::sleep(LOCK_RETRY.min(deadline - now));
                }
                Err(other) => return Err(other),
            }
        }
    }

    fn open(store_path: &Path, blocking: bool) -> Result<Self, DurableError> {
        let lock_path = crate::durable::lock_path_for(store_path);
        let io_err = |source: io::Error| DurableError::Io {
            stage: IoStage::Lock,
            path: lock_path.display().to_string(),
            source,
        };
        if let Some(parent) = lock_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(io_err)?;
        let outcome = if blocking {
            file.lock_exclusive()
        } else {
            file.try_lock_exclusive()
        };
        match outcome {
            Ok(()) => Ok(Self {
                file,
                path: lock_path,
            }),
            Err(error) if !blocking && is_lock_conflict(&error) => Err(DurableError::Locked {
                path: lock_path.display().to_string(),
            }),
            Err(source) => Err(io_err(source)),
        }
    }

    /// The lock file this guard holds.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlink_aliases_share_one_identity() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let store = real.join("memory.jsonl");
        fs::write(&store, b"[]").unwrap();
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &link).unwrap();
        let via_link = link.join("memory.jsonl");
        assert_eq!(
            canonical_identity(&store),
            canonical_identity(&via_link),
            "a symlinked alias must resolve to the same storage identity"
        );
    }

    #[test]
    fn a_missing_file_resolves_against_its_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let identity = canonical_identity(&store);
        assert!(identity.is_absolute());
        assert_eq!(identity.file_name().unwrap(), "memory.jsonl");
        // Creating the file must not change the identity.
        fs::write(&store, b"[]").unwrap();
        assert_eq!(identity, canonical_identity(&store));
    }

    #[test]
    fn relative_and_absolute_paths_agree() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        fs::write(&store, b"[]").unwrap();
        let identity = canonical_identity(&store);
        let cwd = std::env::current_dir().unwrap();
        if let Ok(relative) = store.strip_prefix(&cwd) {
            assert_eq!(identity, canonical_identity(relative));
        }
    }

    #[test]
    fn a_second_holder_is_refused_while_the_lock_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let first = FileLock::try_acquire(&store).expect("first lock");
        let second = FileLock::try_acquire(&store);
        assert!(
            matches!(second, Err(DurableError::Locked { .. })),
            "a concurrent holder must get a detectable lock error"
        );
        drop(first);
        let third = FileLock::try_acquire(&store);
        assert!(third.is_ok(), "the lock is reusable after release");
    }

    #[test]
    fn the_lock_file_survives_release() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let lock = FileLock::acquire(&store).unwrap();
        let lock_path = lock.path().to_path_buf();
        drop(lock);
        assert!(
            lock_path.exists(),
            "deleting the lock file would race with other lockers"
        );
    }

    /// A bounded acquire gives up with the typed error instead of blocking.
    #[test]
    fn a_bounded_acquire_gives_up_with_the_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let held = FileLock::acquire(&store).unwrap();
        let started = std::time::Instant::now();
        let outcome = FileLock::acquire_bounded(&store, std::time::Duration::from_millis(40));
        let waited = started.elapsed();
        assert!(
            matches!(outcome, Err(DurableError::Locked { .. })),
            "a held lock must surface the typed Locked error"
        );
        assert!(
            waited < std::time::Duration::from_secs(2),
            "the bounded acquire must not block: waited {waited:?}"
        );
        drop(held);
    }

    /// A zero budget is a single attempt, not a blocking wait.
    #[test]
    fn a_zero_budget_is_one_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let held = FileLock::acquire(&store).unwrap();
        let outcome = FileLock::acquire_bounded(&store, std::time::Duration::ZERO);
        assert!(matches!(outcome, Err(DurableError::Locked { .. })));
        drop(held);
        assert!(FileLock::acquire_bounded(&store, std::time::Duration::ZERO).is_ok());
    }

    /// A bounded acquire wins once the holder releases inside the budget.
    #[test]
    fn a_bounded_acquire_waits_out_a_short_holder() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("memory.jsonl");
        let held = FileLock::acquire(&store).unwrap();
        let holder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            drop(held);
        });
        let lock = FileLock::acquire_bounded(&store, std::time::Duration::from_secs(5))
            .expect("the lock is free after the holder releases");
        drop(lock);
        holder.join().unwrap();
    }
}
