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

/// Commit `bytes` to `path` through the staged pipeline.
///
/// Stages: create the parent directory, create a unique sibling temp file,
/// write, fsync, apply the privacy policy to the temp, then rename it over the
/// target. Each stage passes through `gate` first. On any failure the temp
/// file is removed and the previous target (if any) is untouched; the error
/// names the stage.
pub fn commit(
    path: &Path,
    bytes: &[u8],
    privacy: Privacy,
    gate: &dyn CommitGate,
) -> Result<(), DurableError> {
    let io_err = |stage: IoStage, path: &Path, source: io::Error| DurableError::Io {
        stage,
        path: path.display().to_string(),
        source,
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            io_err(
                IoStage::CreateDir,
                path,
                io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory"),
            )
        })?;
    gate.pass(IoStage::CreateDir, path)
        .map_err(|e| io_err(IoStage::CreateDir, path, e))?;
    fs::create_dir_all(parent).map_err(|e| io_err(IoStage::CreateDir, parent, e))?;

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "state".to_string());
    let tmp = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), DurableError> {
        gate.pass(IoStage::CreateTemp, &tmp)
            .map_err(|e| io_err(IoStage::CreateTemp, &tmp, e))?;
        let mut options = fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        if privacy == Privacy::OwnerOnly {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| io_err(IoStage::CreateTemp, &tmp, e))?;
        if privacy == Privacy::OwnerOnly {
            crate::fspriv::private_permissions(&tmp)
                .map_err(|e| io_err(IoStage::CreateTemp, &tmp, e))?;
        }
        gate.pass(IoStage::Write, &tmp)
            .map_err(|e| io_err(IoStage::Write, &tmp, e))?;
        file.write_all(bytes)
            .map_err(|e| io_err(IoStage::Write, &tmp, e))?;
        gate.pass(IoStage::Sync, &tmp)
            .map_err(|e| io_err(IoStage::Sync, &tmp, e))?;
        file.sync_all()
            .map_err(|e| io_err(IoStage::Sync, &tmp, e))?;
        drop(file);
        // Harden BEFORE the replacement: the rename swaps in the already
        // private temp, so a failure at any stage - hardening included -
        // leaves the previous target untouched.
        gate.pass(IoStage::Harden, &tmp)
            .map_err(|e| io_err(IoStage::Harden, &tmp, e))?;
        if privacy == Privacy::OwnerOnly {
            crate::fspriv::private_permissions(&tmp)
                .map_err(|e| io_err(IoStage::Harden, &tmp, e))?;
        }
        gate.pass(IoStage::Replace, path)
            .map_err(|e| io_err(IoStage::Replace, path, e))?;
        fs::rename(&tmp, path).map_err(|e| io_err(IoStage::Replace, path, e))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
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
}
