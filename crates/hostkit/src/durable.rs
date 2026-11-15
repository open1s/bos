//! The staged durable-commit kernel (issues #338, #339, #341, #342).
//!
//! One implementation of "write bytes so a crash or a failure cannot lie".
//! Every durable state transition in the workspace - memory snapshots, peer
//! rotations, knowledge publications - commits through [`commit`], which walks
//! a fixed stage pipeline and reports a typed [`DurableError`] naming the exact
//! stage that failed. A failure at any stage leaves the previous file intact.
//!
//! [`CommitGate`] is the interception seam: one hook per stage boundary.
//! Production passes [`NoFaults`]; tests inject failures at temp creation,
//! write, sync or replacement without changing the filesystem's own semantics.
//!
//! # Commit levels (issue #352)
//!
//! A caller states the durability it needs with [`CommitLevel`]. The stage
//! pipeline is always the same seven boundaries - `CreateDir`, `CreateTemp`,
//! `Write`, `Sync`, `Harden`, `Replace`, `SyncDir` - and the level decides
//! how far it runs. [`commit`] keeps its signature and promises
//! [`CommitLevel::Durable`], so every existing consumer (memory, rotation,
//! knowledge, private files) gains the parent-directory sync for free.
//!
//! [`commit_stream`] takes the payload as a writer callback instead of a byte
//! slice, so a Parquet writer can stream a large segment straight into the temp
//! file the kernel created instead of buffering it in memory.
//!
//! [`load`] is the fail-closed read: a missing file is [`Loaded::Missing`],
//! every other read failure is an error, never an empty fallback. Callers that
//! need to distinguish "genuinely empty" from "unreadable" finally can.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The boundary a durable operation failed at.
///
/// The stage is part of the error, so a caller can tell a retryable storage
/// failure from a permanent one and diagnostics can name the exact boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoStage {
    /// Reading the existing state.
    Read,
    /// Creating the parent directory.
    CreateDir,
    /// Creating the sibling temporary file.
    CreateTemp,
    /// Writing the payload into the temporary file.
    Write,
    /// Flushing the payload to stable storage.
    Sync,
    /// Replacing the target with the temporary file.
    Replace,
    /// Applying the privacy policy after replacement.
    Harden,
    /// Syncing the parent directory so the rename survives a power loss.
    SyncDir,
    /// Acquiring the cross-process coordination lock.
    Lock,
}

impl fmt::Display for IoStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            IoStage::Read => "read",
            IoStage::CreateDir => "create-dir",
            IoStage::CreateTemp => "create-temp",
            IoStage::Write => "write",
            IoStage::Sync => "sync",
            IoStage::Replace => "replace",
            IoStage::Harden => "harden",
            IoStage::SyncDir => "sync-dir",
            IoStage::Lock => "lock",
        };
        f.write_str(label)
    }
}

/// A typed durable-storage failure.
///
/// Every variant is actionable: the I/O variants name the stage and path, the
/// semantic variants say what the caller must do (repair, retry, back off).
#[derive(Debug, thiserror::Error)]
pub enum DurableError {
    /// An OS operation failed at a named stage.
    #[error("{stage} failed for {path}: {source}")]
    Io {
        /// The stage that failed.
        stage: IoStage,
        /// The path being operated on.
        path: String,
        /// The underlying OS error.
        source: io::Error,
    },
    /// The state could not be serialized; nothing was written.
    #[error("cannot serialize state for {path}: {detail}")]
    Serialize {
        /// The target path.
        path: String,
        /// Why serialization failed.
        detail: String,
    },
    /// The existing state is unreadable as data; it was preserved untouched.
    #[error("{path} is corrupt: {detail}")]
    Corrupt {
        /// The path holding the corrupt state.
        path: String,
        /// What made it unreadable.
        detail: String,
    },
    /// Another writer holds the coordination lock.
    #[error("{path} is locked by another writer")]
    Locked {
        /// The lock path.
        path: String,
    },
    /// The durable state changed under a prepared commit.
    #[error("{path} changed under the commit: {detail}")]
    Conflict {
        /// The target path.
        path: String,
        /// What conflicted.
        detail: String,
    },
    /// The storage is configured but not usable right now.
    #[error("storage unavailable: {reason}")]
    Unavailable {
        /// Why the storage cannot serve the request.
        reason: String,
    },
}

impl DurableError {
    /// Whether retrying the same operation later can succeed.
    ///
    /// I/O, lock and unavailable failures are transient; corruption,
    /// serialization and conflict failures need caller action first.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            DurableError::Io { .. }
                | DurableError::Locked { .. }
                | DurableError::Unavailable { .. }
        )
    }
}

/// The privacy policy a committed file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privacy {
    /// Owner-only on every host (Unix 0600, Windows protected DACL).
    OwnerOnly,
    /// Keep the platform default; the caller owns the policy.
    Inherit,
}

/// The durability a commit promises when it returns successfully.
///
/// The levels are ordered: [`CommitLevel::Durable`] implies everything the
/// weaker levels promise. A caller that does not say otherwise gets
/// [`CommitLevel::Durable`] from [`commit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommitLevel {
    /// The storage layer accepted the bytes into the staged temp file.
    ///
    /// Nothing is renamed: the target path still names the previous state, and
    /// the temp file is left beside it for a later commit or a sweep to
    /// reclaim. The failure rule is unchanged - an error removes the temp and
    /// leaves the previous target intact - but a success at this level does
    /// not make the bytes visible at the target path. It is the level for data
    /// whose loss is tolerable (a cache, a scratch artifact).
    Accepted,
    /// The bytes are in the target file and the rename completed.
    ///
    /// The temp file was written and synced (so its contents reached the OS),
    /// then renamed over the target. The new state survives a process crash.
    /// The directory entry the rename created is not explicitly flushed, so a
    /// power loss may still lose the rename and resurrect the previous name.
    Written,
    /// The file contents and the parent directory entry are both on stable storage.
    ///
    /// The temp file is synced, renamed over the target, and the parent
    /// directory is then synced, so the new name survives a power loss.
    ///
    /// On Unix that means opening the parent directory and calling
    /// `sync_all` on it. Windows cannot open a directory to sync it, so there
    /// the rename is visible but the directory metadata is not explicitly
    /// flushed and the promise is the weaker one [`CommitLevel::Written`]
    /// makes. The `SyncDir` stage runs on both hosts, so fault injection sees
    /// the same pipeline everywhere.
    Durable,
}

/// One interception hook per commit stage.
///
/// The default passes every stage. A test gate returns an error at the stage
/// it wants to fail; the commit then behaves exactly as a real OS failure at
/// that boundary would: temp removed, previous file intact, typed error out.
pub trait CommitGate: Send + Sync {
    /// Called before the named stage runs against `path`.
    fn pass(&self, stage: IoStage, path: &Path) -> io::Result<()> {
        let _ = (stage, path);
        Ok(())
    }
}

/// The production gate: every stage passes.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFaults;

impl CommitGate for NoFaults {}

/// Unique temp suffix so concurrent commits to one path never share a temp.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// How many temp names one commit tries before reporting the collision.
const TEMP_ATTEMPTS: u32 = 64;

/// Build the typed I/O error for one stage.
fn io_error(stage: IoStage, path: &Path, source: io::Error) -> DurableError {
    DurableError::Io {
        stage,
        path: path.display().to_string(),
        source,
    }
}

/// Create the unique sibling temp file with `create_new(true)`.
///
/// A name that already exists is never opened: the sequence is bumped and the
/// next name is tried, so a pre-existing file (a copied directory, a killed
/// process's debris) cannot be overwritten or silently reused.
fn create_temp(
    parent: &Path,
    name: &str,
    privacy: Privacy,
    gate: &dyn CommitGate,
) -> Result<(PathBuf, fs::File), DurableError> {
    let mut last = None;
    for _ in 0..TEMP_ATTEMPTS {
        let temporary = parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        gate.pass(IoStage::CreateTemp, &temporary)
            .map_err(|error| io_error(IoStage::CreateTemp, &temporary, error))?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        if privacy == Privacy::OwnerOnly {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => {
                if privacy == Privacy::OwnerOnly {
                    crate::fspriv::private_permissions(&temporary)
                        .map_err(|error| io_error(IoStage::CreateTemp, &temporary, error))?;
                }
                return Ok((temporary, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(io_error(IoStage::CreateTemp, &temporary, error)),
        }
    }
    let source =
        last.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "no free temp name"));
    Err(io_error(IoStage::CreateTemp, parent, source))
}

/// Sync the parent directory so the rename survives a power loss.
///
/// The gate stage runs before the host-specific part on every platform, so a
/// test can fail the directory sync and see the same typed stage everywhere.
/// Windows cannot open a directory to sync it; there the rename is visible but
/// the directory metadata is not explicitly flushed (see CommitLevel::Durable).
fn sync_directory(parent: &Path, gate: &dyn CommitGate) -> Result<(), DurableError> {
    gate.pass(IoStage::SyncDir, parent)
        .map_err(|error| io_error(IoStage::SyncDir, parent, error))?;
    #[cfg(unix)]
    {
        let directory =
            fs::File::open(parent).map_err(|error| io_error(IoStage::SyncDir, parent, error))?;
        directory
            .sync_all()
            .map_err(|error| io_error(IoStage::SyncDir, parent, error))?;
    }
    Ok(())
}

/// Commit `bytes` to `path` at CommitLevel::Durable.
///
/// Stages: create the parent directory, create a unique sibling temp file,
/// write the bytes, sync them, harden the temp, rename it over the target and
/// sync the parent directory. Each stage passes through `gate` first. On any
/// failure the temp file is removed and the previous target (if any) is
/// untouched; the error names the stage.
///
/// This is the historical signature. It now means the durable level, so every
/// existing consumer (memory snapshots, federation rotation, knowledge
/// publication, private files) gains the directory sync without a call-site
/// change.
pub fn commit(
    path: &Path,
    bytes: &[u8],
    privacy: Privacy,
    gate: &dyn CommitGate,
) -> Result<(), DurableError> {
    commit_stream(path, privacy, CommitLevel::Durable, gate, |file| {
        file.write_all(bytes)
    })
}

/// Stream bytes into `path` at an explicit commit level.
///
/// The caller's `write` closure receives the temp file the kernel just created
/// with `create_new(true)`, so a large payload (a Parquet segment) can be
/// written through it without an intermediate buffer. The remaining stages are
/// the same ones commit walks, and the level decides how far the pipeline runs:
///
/// Accepted stops after the write and the privacy step, leaving the temp.
/// Written adds the file sync and the rename.
/// Durable adds the parent-directory sync.
///
/// A failure at any stage removes the temp file and leaves the previous target
/// intact, with one exception that is inherent to the order: a failure at
/// SyncDir happens after the rename, so the new file is already installed and
/// the error means only that the directory entry is not confirmed durable.
pub fn commit_stream(
    path: &Path,
    privacy: Privacy,
    level: CommitLevel,
    gate: &dyn CommitGate,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> Result<(), DurableError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io_error(
                IoStage::CreateDir,
                path,
                io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory"),
            )
        })?;
    gate.pass(IoStage::CreateDir, path)
        .map_err(|error| io_error(IoStage::CreateDir, path, error))?;
    fs::create_dir_all(parent).map_err(|error| io_error(IoStage::CreateDir, parent, error))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state".to_string());
    let (temporary, file) = create_temp(parent, &name, privacy, gate)?;
    let result = run_stages(path, parent, &temporary, file, privacy, level, gate, write);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The stages after the temp file exists.
#[allow(clippy::too_many_arguments)]
fn run_stages(
    path: &Path,
    parent: &Path,
    temporary: &Path,
    mut file: fs::File,
    privacy: Privacy,
    level: CommitLevel,
    gate: &dyn CommitGate,
    write: impl FnOnce(&mut fs::File) -> io::Result<()>,
) -> Result<(), DurableError> {
    gate.pass(IoStage::Write, temporary)
        .map_err(|error| io_error(IoStage::Write, temporary, error))?;
    write(&mut file).map_err(|error| io_error(IoStage::Write, temporary, error))?;
    if level >= CommitLevel::Written {
        gate.pass(IoStage::Sync, temporary)
            .map_err(|error| io_error(IoStage::Sync, temporary, error))?;
        file.sync_all()
            .map_err(|error| io_error(IoStage::Sync, temporary, error))?;
    }
    // The handle has to be closed before the rename on Windows, where an open
    // file blocks the replacement.
    drop(file);
    // Harden BEFORE the replacement: the rename swaps in the already private
    // temp, so a failure at any stage - hardening included - leaves the
    // previous target untouched.
    gate.pass(IoStage::Harden, temporary)
        .map_err(|error| io_error(IoStage::Harden, temporary, error))?;
    if privacy == Privacy::OwnerOnly {
        crate::fspriv::private_permissions(temporary)
            .map_err(|error| io_error(IoStage::Harden, temporary, error))?;
    }
    if level == CommitLevel::Accepted {
        return Ok(());
    }
    gate.pass(IoStage::Replace, path)
        .map_err(|error| io_error(IoStage::Replace, path, error))?;
    fs::rename(temporary, path).map_err(|error| io_error(IoStage::Replace, path, error))?;
    if level == CommitLevel::Durable {
        sync_directory(parent, gate)?;
    }
    Ok(())
}

/// The fail-closed read outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// No file exists at the path: a genuinely empty store.
    Missing,
    /// The file exists and was read; parsing is the caller's next step.
    Present(Vec<u8>),
}

/// Read durable state, distinguishing missing from unreadable.
///
/// `NotFound` is [`Loaded::Missing`]; every other I/O failure is a typed error.
/// A caller must never substitute an empty store for an error: that is how a
/// malformed-but-writable file gets silently overwritten (issue #339).
pub fn load(path: &Path) -> Result<Loaded, DurableError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Loaded::Present(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Loaded::Missing),
        Err(source) => Err(DurableError::Io {
            stage: IoStage::Read,
            path: path.display().to_string(),
            source,
        }),
    }
}

/// The lock file that coordinates commits to `store_path`.
///
/// The lock lives BESIDE the store and is never deleted: removing a lock file
/// races with processes already locking it. The OS releases the lock when a
/// holder exits, so a crash cannot strand it.
#[must_use]
pub fn lock_path_for(store_path: &Path) -> PathBuf {
    let mut name = store_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gate that fails exactly one stage, recording what it saw.
    struct FaultAt {
        stage: IoStage,
        seen: std::sync::Mutex<Vec<IoStage>>,
    }

    impl CommitGate for FaultAt {
        fn pass(&self, stage: IoStage, _path: &Path) -> io::Result<()> {
            self.seen.lock().unwrap().push(stage);
            if stage == self.stage {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn commit_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        commit(&path, b"first", Privacy::Inherit, &NoFaults).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        commit(&path, b"second", Privacy::Inherit, &NoFaults).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn commit_creates_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep").join("nested").join("state.json");
        commit(&path, b"x", Privacy::Inherit, &NoFaults).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"x");
    }

    /// Every injected stage failure leaves the previous file intact, removes
    /// the temp, and reports the exact stage (issues #338/#339/#342).
    #[test]
    fn injected_failure_at_every_stage_preserves_the_previous_file() {
        for stage in [
            IoStage::CreateTemp,
            IoStage::Write,
            IoStage::Sync,
            IoStage::Replace,
            IoStage::Harden,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.json");
            commit(&path, b"original", Privacy::Inherit, &NoFaults).unwrap();
            let gate = FaultAt {
                stage,
                seen: std::sync::Mutex::new(Vec::new()),
            };
            let error = commit(&path, b"replacement", Privacy::OwnerOnly, &gate)
                .expect_err("the injected stage must fail");
            match &error {
                DurableError::Io { stage: failed, .. } => assert_eq!(*failed, stage),
                other => panic!("expected an Io error at {stage}, got {other:?}"),
            }
            assert_eq!(
                fs::read(&path).unwrap(),
                b"original",
                "stage {stage} must not touch the committed file"
            );
            let leftovers: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
                .collect();
            assert!(
                leftovers.is_empty(),
                "stage {stage} left a temp file behind"
            );
        }
    }

    #[test]
    fn a_failed_first_commit_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let gate = FaultAt {
            stage: IoStage::Write,
            seen: std::sync::Mutex::new(Vec::new()),
        };
        assert!(commit(&path, b"x", Privacy::Inherit, &gate).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn owner_only_commits_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        commit(&path, b"secret", Privacy::OwnerOnly, &NoFaults).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn load_distinguishes_missing_present_and_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        assert_eq!(load(&path).unwrap(), Loaded::Missing);
        fs::write(&path, b"bytes").unwrap();
        assert_eq!(load(&path).unwrap(), Loaded::Present(b"bytes".to_vec()));
        // A directory where a file is expected is a real read failure on every
        // host, not a missing store.
        let error = load(dir.path()).expect_err("a directory is not a store");
        assert!(matches!(
            error,
            DurableError::Io {
                stage: IoStage::Read,
                ..
            }
        ));
        assert!(error.is_retryable());
    }

    #[test]
    fn retryability_classes_are_distinct() {
        let locked = DurableError::Locked { path: "x".into() };
        let corrupt = DurableError::Corrupt {
            path: "x".into(),
            detail: "bad json".into(),
        };
        assert!(locked.is_retryable());
        assert!(!corrupt.is_retryable());
    }

    #[test]
    fn lock_path_sits_beside_the_store() {
        assert_eq!(
            lock_path_for(Path::new("/data/memory.jsonl")),
            PathBuf::from("/data/memory.jsonl.lock")
        );
    }

    /// A gate that records every stage and fails none.
    struct Record {
        seen: std::sync::Mutex<Vec<IoStage>>,
    }

    impl CommitGate for Record {
        fn pass(&self, stage: IoStage, _path: &Path) -> io::Result<()> {
            self.seen.lock().unwrap().push(stage);
            Ok(())
        }
    }

    /// The durable level walks the whole pipeline, directory sync last.
    #[test]
    fn a_durable_commit_walks_every_stage_and_syncs_the_directory_last() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let gate = Record {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        commit(&path, b"payload", Privacy::Inherit, &gate).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"payload");
        assert_eq!(
            *gate.seen.lock().unwrap(),
            vec![
                IoStage::CreateDir,
                IoStage::CreateTemp,
                IoStage::Write,
                IoStage::Sync,
                IoStage::Harden,
                IoStage::Replace,
                IoStage::SyncDir,
            ],
            "the durable pipeline ends with the directory sync"
        );
    }

    /// A SyncDir failure happens after the rename, so the new file is already
    /// installed; the error still names the stage and no temp is left.
    #[test]
    fn a_directory_sync_failure_is_reported_after_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        commit(&path, b"original", Privacy::Inherit, &NoFaults).unwrap();
        let gate = FaultAt {
            stage: IoStage::SyncDir,
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let error = commit(&path, b"replacement", Privacy::Inherit, &gate)
            .expect_err("the injected directory sync must fail");
        match &error {
            DurableError::Io { stage, .. } => assert_eq!(*stage, IoStage::SyncDir),
            other => panic!("expected a SyncDir error, got {other:?}"),
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            b"replacement",
            "the rename already happened before the directory sync"
        );
        assert_no_temp(dir.path());
    }

    /// Written replaces the target but promises no directory sync.
    #[test]
    fn the_written_level_replaces_the_target_without_a_directory_sync() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let gate = Record {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        commit_stream(
            &path,
            Privacy::Inherit,
            CommitLevel::Written,
            &gate,
            |file| file.write_all(b"written"),
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"written");
        assert_eq!(
            *gate.seen.lock().unwrap(),
            vec![
                IoStage::CreateDir,
                IoStage::CreateTemp,
                IoStage::Write,
                IoStage::Sync,
                IoStage::Harden,
                IoStage::Replace,
            ],
            "nothing syncs the directory at the written level"
        );
    }

    /// Accepted keeps the bytes in the staged temp and leaves the target alone.
    #[test]
    fn the_accepted_level_keeps_the_bytes_in_the_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        commit(&path, b"original", Privacy::Inherit, &NoFaults).unwrap();
        let gate = Record {
            seen: std::sync::Mutex::new(Vec::new()),
        };
        commit_stream(
            &path,
            Privacy::Inherit,
            CommitLevel::Accepted,
            &gate,
            |file| file.write_all(b"accepted"),
        )
        .unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"original",
            "the accepted level never renames over the target"
        );
        let stages = gate.seen.lock().unwrap().clone();
        assert!(
            !stages.contains(&IoStage::Replace) && !stages.contains(&IoStage::SyncDir),
            "accepted stops before the replacement: {stages:?}"
        );
        let temps: Vec<PathBuf> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.to_string_lossy().contains(".tmp-"))
            .collect();
        assert_eq!(temps.len(), 1, "the accepted bytes live in one temp file");
        assert_eq!(fs::read(&temps[0]).unwrap(), b"accepted");
    }

    /// The streaming entry point writes through the caller's chunks.
    #[test]
    fn commit_stream_writes_through_the_callers_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.bin");
        commit_stream(
            &path,
            Privacy::Inherit,
            CommitLevel::Durable,
            &NoFaults,
            |file| {
                file.write_all(b"first")?;
                file.write_all(b"-second")?;
                file.write_all(b"-third")
            },
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first-second-third");
        assert_no_temp(dir.path());
    }

    /// A writer that fails is a Write-stage failure: temp removed, target kept.
    #[test]
    fn a_writer_error_removes_the_temp_and_keeps_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.bin");
        commit(&path, b"original", Privacy::Inherit, &NoFaults).unwrap();
        let error = commit_stream(
            &path,
            Privacy::Inherit,
            CommitLevel::Durable,
            &NoFaults,
            |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("writer failed"))
            },
        )
        .expect_err("the writer failed");
        match &error {
            DurableError::Io { stage, .. } => assert_eq!(*stage, IoStage::Write),
            other => panic!("expected a Write error, got {other:?}"),
        }
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_no_temp(dir.path());
    }

    /// Every stage of commit_stream behaves like the batch path on failure.
    #[test]
    fn commit_stream_failure_at_every_stage_keeps_the_previous_file() {
        for stage in [
            IoStage::CreateTemp,
            IoStage::Write,
            IoStage::Sync,
            IoStage::Replace,
            IoStage::Harden,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.bin");
            commit(&path, b"original", Privacy::Inherit, &NoFaults).unwrap();
            let gate = FaultAt {
                stage,
                seen: std::sync::Mutex::new(Vec::new()),
            };
            let error = commit_stream(
                &path,
                Privacy::OwnerOnly,
                CommitLevel::Durable,
                &gate,
                |file| file.write_all(b"replacement"),
            )
            .expect_err("the injected stage must fail");
            match &error {
                DurableError::Io { stage: failed, .. } => assert_eq!(*failed, stage),
                other => panic!("expected an Io error at {stage}, got {other:?}"),
            }
            assert_eq!(fs::read(&path).unwrap(), b"original");
            assert_no_temp(dir.path());
        }
    }

    /// A temp name that already exists is skipped, never overwritten.
    #[test]
    fn a_preexisting_temp_name_is_skipped_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let base = TEMP_SEQ.load(Ordering::Relaxed);
        let mut sentinels = Vec::new();
        for offset in 0..64 {
            let candidate = dir.path().join(format!(
                ".{name}.tmp-{}-{}",
                std::process::id(),
                base + offset
            ));
            if !candidate.exists() {
                fs::write(&candidate, b"sentinel").unwrap();
                sentinels.push(candidate);
            }
        }
        commit(&path, b"fresh", Privacy::Inherit, &NoFaults)
            .expect("a pre-existing temp name must not stop the commit");
        assert_eq!(fs::read(&path).unwrap(), b"fresh");
        for sentinel in sentinels {
            assert_eq!(
                fs::read(&sentinel).unwrap(),
                b"sentinel",
                "{} was overwritten",
                sentinel.display()
            );
        }
    }

    /// No temp file of ours survives a finished or failed commit.
    fn assert_no_temp(dir: &Path) {
        let leftovers: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".tmp-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
