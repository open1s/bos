//! A tamper-evident audit ledger.
//!
//! The diagnostic audit tail is bounded and best-effort; this ledger is the
//! authoritative record. Each line is a JSON entry whose hash covers the entry
//! and the previous entry's hash, so a modified, reordered, or removed line
//! breaks the chain and [AuditLedger::verify] names the first bad line.
//!
//! A ledger alone cannot prove the TAIL was not cut off (a shorter consistent
//! chain verifies). Pair it with an externally stored checkpoint:
//! [AuditLedger::checkpoint] names the latest hash, and
//! [AuditLedger::verify_against] refuses a chain whose last hash moved.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::audit::{FederationAudit, FederationEvent};

/// The ledger line schema.
pub const LEDGER_SCHEMA: u32 = 1;

/// One chained ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Line format version.
    pub schema: u32,
    /// 1-based position in the chain.
    pub seq: u64,
    /// The event's own timestamp, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// The recorded decision.
    pub event: FederationEvent,
    /// The previous entry's hash; empty for the first entry.
    pub prev_hash: String,
    /// The hash of this entry, covering everything above.
    pub hash: String,
}

/// What a verification found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerReport {
    /// Entries checked.
    pub entries: u64,
    /// The hash the first entry chains from (empty for the first segment).
    pub first_prev_hash: String,
    /// The last entry's hash.
    pub last_hash: String,
}

/// Why a ledger failed verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerError {
    /// The file could not be read.
    #[error("the ledger could not be read: {0}")]
    Io(String),
    /// A line is not an entry.
    #[error("line {line} is not a valid ledger entry")]
    Malformed {
        /// 1-based line number.
        line: u64,
    },
    /// The chain is broken at this line.
    #[error("line {line} breaks the chain: {reason}")]
    Broken {
        /// 1-based line number.
        line: u64,
        /// Why.
        reason: String,
    },
    /// The last hash does not match the external checkpoint.
    #[error("the ledger's last hash does not match the checkpoint")]
    CheckpointMismatch,
}

/// An append-only, hash-chained ledger.
pub struct AuditLedger {
    path: PathBuf,
    guard: Mutex<LedgerState>,
}

struct LedgerState {
    writer: File,
    seq: u64,
    last_hash: String,
}

impl AuditLedger {
    /// Open (or create) a ledger, recovering the chain tail.
    pub fn open(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (seq, last_hash) = match Self::verify(&path) {
            Ok(report) => {
                let head = if report.entries == 0 {
                    std::fs::read_to_string(path.with_extension("head"))
                        .ok()
                        .map(|head| head.trim().to_string())
                        .unwrap_or_default()
                } else {
                    report.last_hash
                };
                (report.entries, head)
            }
            Err(LedgerError::Io(_)) => (0, String::new()),
            Err(error) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.to_string(),
                ));
            }
        };
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let writer = options.open(&path)?;
        Ok(Self {
            path,
            guard: Mutex::new(LedgerState {
                writer,
                seq,
                last_hash,
            }),
        })
    }

    /// Append one event and return the entry written.
    pub fn append(&self, event: &FederationEvent) -> std::io::Result<LedgerEntry> {
        let mut state = self
            .guard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = LedgerEntry {
            schema: LEDGER_SCHEMA,
            seq: state.seq + 1,
            at_ms: event.at_ms,
            event: event.clone(),
            prev_hash: state.last_hash.clone(),
            hash: String::new(),
        };
        let hash = entry_hash(&entry);
        let entry = LedgerEntry {
            hash: hash.clone(),
            ..entry
        };
        let line = serde_json::to_string(&entry).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        })?;
        state.writer.write_all(line.as_bytes())?;
        state.writer.write_all(b"\n")?;
        state.writer.flush()?;
        state.seq = entry.seq;
        state.last_hash = hash;
        Ok(entry)
    }

    /// Rotate the ledger: archive the current file and start a fresh segment
    /// that chains from the archived one. The chain head is recorded in a
    /// sidecar so a restart can continue across the rotation.
    pub fn rotate(&self, suffix: &str) -> std::io::Result<PathBuf> {
        let mut state = self
            .guard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.writer.flush()?;
        let head = state.last_hash.clone();
        let mut archive = self.path.with_extension(format!("{suffix}.jsonl"));
        let mut counter = 0;
        while archive.exists() {
            counter += 1;
            archive = self
                .path
                .with_extension(format!("{suffix}.{counter}.jsonl"));
        }
        std::fs::rename(&self.path, &archive)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        state.writer = options.open(&self.path)?;
        state.seq = 0;
        // The first entry of the new segment chains from the archived tail.
        state.last_hash = head.clone();
        let head_path = self.path.with_extension("head");
        if let Err(error) = std::fs::write(&head_path, &head) {
            tracing::warn!("ledger head was not written: {error}");
        }
        Ok(archive)
    }

    /// Verify every segment in a directory and prove they form one chain.
    ///
    /// Segments are ordered by the hashes themselves (a segment chains from the
    /// previous tail), not by file name, so rotation naming cannot hide a gap.
    pub fn verify_dir(directory: &Path, prefix: &str) -> Result<LedgerReport, LedgerError> {
        let entries =
            std::fs::read_dir(directory).map_err(|error| LedgerError::Io(error.to_string()))?;
        let mut reports: Vec<LedgerReport> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| LedgerError::Io(error.to_string()))?;
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if !name.starts_with(prefix) || !name.ends_with(".jsonl") {
                continue;
            }
            let report = Self::verify(&path)?;
            if report.entries > 0 {
                reports.push(report);
            }
        }
        let mut total = 0u64;
        let mut first_prev_hash = String::new();
        let mut last_hash = String::new();
        let mut remaining = reports;
        let mut started = false;
        while !remaining.is_empty() {
            let index = if !started {
                remaining
                    .iter()
                    .position(|report| report.first_prev_hash.is_empty())
                    .unwrap_or(0)
            } else {
                match remaining
                    .iter()
                    .position(|report| report.first_prev_hash == last_hash)
                {
                    Some(index) => index,
                    None => {
                        return Err(LedgerError::Broken {
                            line: 1,
                            reason: "a ledger segment does not chain from the previous".to_string(),
                        });
                    }
                }
            };
            let report = remaining.remove(index);
            if !started {
                first_prev_hash = report.first_prev_hash.clone();
                started = true;
            }
            total += report.entries;
            last_hash = report.last_hash;
        }
        Ok(LedgerReport {
            entries: total,
            first_prev_hash,
            last_hash,
        })
    }

    /// Write the last hash to an external checkpoint file with 0600 mode.
    pub fn checkpoint_to(
        path: &Path,
        checkpoint_path: &Path,
    ) -> Result<Option<String>, LedgerError> {
        let hash = Self::checkpoint(path)?;
        if let Some(hash) = &hash {
            if let Some(parent) = checkpoint_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| LedgerError::Io(error.to_string()))?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options
                .open(checkpoint_path)
                .map_err(|error| LedgerError::Io(error.to_string()))?;
            file.write_all(hash.as_bytes())
                .map_err(|error| LedgerError::Io(error.to_string()))?;
        }
        Ok(hash)
    }

    /// Verify against a checkpoint file written by [checkpoint_to].
    pub fn verify_with_checkpoint_file(
        path: &Path,
        checkpoint_path: &Path,
    ) -> Result<LedgerReport, LedgerError> {
        let expected = std::fs::read_to_string(checkpoint_path)
            .map_err(|error| LedgerError::Io(error.to_string()))?;
        Self::verify_against(path, expected.trim())
    }

    /// The ledger's file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Verify the whole chain.
    pub fn verify(path: &Path) -> Result<LedgerReport, LedgerError> {
        let file = File::open(path).map_err(|error| LedgerError::Io(error.to_string()))?;
        let mut entries = 0u64;
        let mut first_prev_hash = String::new();
        let mut last_hash = String::new();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|error| LedgerError::Io(error.to_string()))?;
            let number = index as u64 + 1;
            if line.trim().is_empty() {
                continue;
            }
            let entry: LedgerEntry =
                serde_json::from_str(&line).map_err(|_| LedgerError::Malformed { line: number })?;
            if entry.schema != LEDGER_SCHEMA {
                return Err(LedgerError::Broken {
                    line: number,
                    reason: format!("unsupported schema {}", entry.schema),
                });
            }
            if entry.seq != entries + 1 {
                return Err(LedgerError::Broken {
                    line: number,
                    reason: format!("expected seq {}, found {}", entries + 1, entry.seq),
                });
            }
            if entries == 0 {
                // A rotated segment chains from the previous segment's last
                // hash; the first segment starts from the empty string.
                first_prev_hash = entry.prev_hash.clone();
            } else if entry.prev_hash != last_hash {
                return Err(LedgerError::Broken {
                    line: number,
                    reason: "prev_hash does not name the previous entry".to_string(),
                });
            }
            if entry_hash(&entry) != entry.hash {
                return Err(LedgerError::Broken {
                    line: number,
                    reason: "the entry was modified".to_string(),
                });
            }
            entries += 1;
            last_hash = entry.hash;
        }
        Ok(LedgerReport {
            entries,
            first_prev_hash,
            last_hash,
        })
    }

    /// The last hash on disk, for an external checkpoint.
    pub fn checkpoint(path: &Path) -> Result<Option<String>, LedgerError> {
        let report = Self::verify(path)?;
        Ok((report.entries > 0).then_some(report.last_hash))
    }

    /// Verify against an externally stored checkpoint (truncation detection).
    pub fn verify_against(
        path: &Path,
        expected_last_hash: &str,
    ) -> Result<LedgerReport, LedgerError> {
        let report = Self::verify(path)?;
        if report.last_hash != expected_last_hash {
            return Err(LedgerError::CheckpointMismatch);
        }
        Ok(report)
    }
}

impl FederationAudit for AuditLedger {
    fn record(&self, event: FederationEvent) {
        if let Err(error) = self.append(&event) {
            tracing::warn!("federation audit ledger append failed: {error}");
        }
    }
}

/// The hash an entry must carry: SHA-256 over its canonical JSON without the hash.
fn entry_hash(entry: &LedgerEntry) -> String {
    #[derive(Serialize)]
    struct Input<'a> {
        schema: u32,
        seq: u64,
        at_ms: u64,
        event: &'a FederationEvent,
        prev_hash: &'a str,
    }
    let input = Input {
        schema: entry.schema,
        seq: entry.seq,
        at_ms: entry.at_ms,
        event: &entry.event,
        prev_hash: &entry.prev_hash,
    };
    let bytes = serde_json::to_vec(&input).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Direction;

    fn event(index: u64) -> FederationEvent {
        FederationEvent::new(
            Direction::Inbound,
            "node-b",
            "invoke",
            Some("builtin/echo"),
            "ok",
        )
        .with_subject(format!("run-{index}"))
    }

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("federation-ledger-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir.join("ledger.jsonl")
    }

    #[test]
    fn a_chain_verifies_and_names_its_last_hash() {
        let path = scratch("ok");
        let ledger = AuditLedger::open(&path).expect("open");
        for index in 1..=3 {
            ledger.append(&event(index)).expect("append");
        }
        let report = AuditLedger::verify(&path).expect("verify");
        assert_eq!(report.entries, 3);
        assert_eq!(
            AuditLedger::checkpoint(&path).expect("checkpoint"),
            Some(report.last_hash.clone())
        );
        assert!(AuditLedger::verify_against(&path, &report.last_hash).is_ok());
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_modified_entry_breaks_the_chain() {
        let path = scratch("tamper");
        let ledger = AuditLedger::open(&path).expect("open");
        for index in 1..=3 {
            ledger.append(&event(index)).expect("append");
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        let mut entry: LedgerEntry = serde_json::from_str(&lines[1]).expect("parse");
        entry.event.reason = Some("rewritten".to_string());
        lines[1] = serde_json::to_string(&entry).expect("serialize");
        std::fs::write(&path, lines.join("\n") + "\n").expect("write");
        assert!(matches!(
            AuditLedger::verify(&path),
            Err(LedgerError::Broken { line: 2, .. })
        ));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_removed_line_breaks_the_sequence() {
        let path = scratch("remove");
        let ledger = AuditLedger::open(&path).expect("open");
        for index in 1..=3 {
            ledger.append(&event(index)).expect("append");
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).expect("write");
        assert!(matches!(
            AuditLedger::verify(&path),
            Err(LedgerError::Broken { line: 2, .. })
        ));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_truncated_chain_needs_the_checkpoint_to_notice() {
        let path = scratch("truncate");
        let ledger = AuditLedger::open(&path).expect("open");
        for index in 1..=3 {
            ledger.append(&event(index)).expect("append");
        }
        let checkpoint = AuditLedger::checkpoint(&path)
            .expect("checkpoint")
            .expect("hash");
        let text = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n", lines[0])).expect("write");
        assert!(
            AuditLedger::verify(&path).is_ok(),
            "a shorter chain is self-consistent"
        );
        assert!(matches!(
            AuditLedger::verify_against(&path, &checkpoint),
            Err(LedgerError::CheckpointMismatch)
        ));
        let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn a_rotated_chain_stays_verifiable_across_segments() {
        let root = scratch("rotate");
        let path = root.join("ledger.jsonl");
        let ledger = AuditLedger::open(&path).expect("open");
        ledger.append(&event(1)).expect("append");
        ledger.append(&event(2)).expect("append");
        let archive = ledger.rotate("seg").expect("rotated");
        assert!(archive.exists());
        ledger.append(&event(3)).expect("append");
        ledger.append(&event(4)).expect("append");

        let report = AuditLedger::verify_dir(&root, "ledger").expect("the chain verifies");
        assert_eq!(report.entries, 4);
        assert_eq!(report.first_prev_hash, "");
        assert!(report.last_hash.len() == 64);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restart_after_rotation_continues_the_chain() {
        let root = scratch("rotate-restart");
        let path = root.join("ledger.jsonl");
        {
            let ledger = AuditLedger::open(&path).expect("open");
            ledger.append(&event(1)).expect("append");
            ledger.rotate("seg").expect("rotated");
        }
        // No entry was written to the fresh segment, so the head sidecar keeps
        // the chain's tail for the next process.
        let ledger = AuditLedger::open(&path).expect("reopen");
        ledger.append(&event(2)).expect("append");
        let report = AuditLedger::verify_dir(&root, "ledger").expect("the chain verifies");
        assert_eq!(report.entries, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tampered_archived_segment_is_detected() {
        let root = scratch("rotate-tamper");
        let path = root.join("ledger.jsonl");
        let ledger = AuditLedger::open(&path).expect("open");
        ledger.append(&event(1)).expect("append");
        let archive = ledger.rotate("seg").expect("rotated");
        ledger.append(&event(2)).expect("append");
        let text = std::fs::read_to_string(&archive).expect("read");
        let mut entry: LedgerEntry =
            serde_json::from_str(text.lines().next().expect("line")).expect("parse");
        entry.event.reason = Some("rewritten".to_string());
        std::fs::write(
            &archive,
            serde_json::to_string(&entry).expect("serialize")
                + "
",
        )
        .expect("write");
        assert!(AuditLedger::verify_dir(&root, "ledger").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkpoint_file_detects_truncation() {
        let root = scratch("checkpoint");
        let path = root.join("ledger.jsonl");
        let checkpoint = root.join("ledger.checkpoint");
        let ledger = AuditLedger::open(&path).expect("open");
        ledger.append(&event(1)).expect("append");
        ledger.append(&event(2)).expect("append");
        let hash = AuditLedger::checkpoint_to(&path, &checkpoint)
            .expect("checkpoint")
            .expect("a hash");
        assert_eq!(
            std::fs::read_to_string(&checkpoint).expect("read").trim(),
            hash
        );
        assert!(AuditLedger::verify_with_checkpoint_file(&path, &checkpoint).is_ok());
        let text = std::fs::read_to_string(&path).expect("read");
        let first = text.lines().next().expect("line").to_string();
        std::fs::write(
            &path,
            first
                + "
",
        )
        .expect("truncate");
        assert!(matches!(
            AuditLedger::verify_with_checkpoint_file(&path, &checkpoint),
            Err(LedgerError::CheckpointMismatch)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
