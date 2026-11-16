//! The federation audit log.
//!
//! One JSON line per cross-node decision, written by the side that made it.
//! A line carries the peer, the operation, the capability, and the outcome —
//! never arguments, results, or secrets, because the event type has no field
//! for them. The log is bounded like the composition log: it is diagnostic, so
//! a write failure is logged and never fails a turn.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tokio::sync::broadcast;

use federation::{FederationAudit, FederationEvent};

/// The size at which the log is compacted.
pub(crate) const MAX_BYTES: u64 = 256 * 1024;

/// How many of the newest records survive a compaction.
pub(crate) const KEEP_LINES: usize = 200;

static EVENTS: OnceLock<broadcast::Sender<FederationEvent>> = OnceLock::new();

fn events() -> &'static broadcast::Sender<FederationEvent> {
    EVENTS.get_or_init(|| broadcast::channel(64).0)
}

/// Listen for records written by any sink in this process.
///
/// The shell keeps a receiver so the settings row can show a decision without
/// re-reading the log; a listener that falls behind drops the gap, which is
/// fine for a diagnostic tail.
pub(crate) fn subscribe() -> broadcast::Receiver<FederationEvent> {
    events().subscribe()
}

/// Keep at most five recent events, oldest dropped.
pub(crate) fn push_recent(recent: &mut Vec<FederationEvent>, event: FederationEvent) {
    recent.push(event);
    if recent.len() > 5 {
        recent.remove(0);
    }
}

/// An audit sink that appends to this process's own file.
pub(crate) struct FileAudit {
    path: PathBuf,
    guard: Mutex<()>,
}

impl FileAudit {
    /// Write beside `path`, creating the directory on first use.
    ///
    /// Each process appends to its own file (audit-<pid>.jsonl): two bsh
    /// instances sharing a data directory never write the same inode, so a
    /// compaction in one cannot swallow a line from the other. Readers merge
    /// the per-process files and the legacy single log.
    pub(crate) fn new(path: PathBuf) -> Self {
        let path = path.with_file_name(format!("audit-{}.jsonl", std::process::id()));
        Self {
            path,
            guard: Mutex::new(()),
        }
    }
}

/// The authoritative ledger plus the diagnostic tail.
///
/// The diagnostic file stays bounded and best-effort; the ledger is append-only
/// and hash-chained. A ledger failure is logged and never fails an operation.
pub(crate) struct DurableAudit {
    file: FileAudit,
    ledger: federation::ledger::AuditLedger,
}

impl DurableAudit {
    /// Build both sinks beside the configured audit path.
    pub(crate) fn new(path: PathBuf) -> io::Result<Self> {
        let ledger_path = path.with_file_name(format!("ledger-{}.jsonl", std::process::id()));
        let ledger = federation::ledger::AuditLedger::open(ledger_path)?;
        Ok(Self {
            file: FileAudit::new(path),
            ledger,
        })
    }

    /// The ledger file this sink writes.
    #[cfg(test)]
    pub(crate) fn ledger_path(&self) -> &Path {
        self.ledger.path()
    }
}

impl FederationAudit for DurableAudit {
    fn record(&self, event: FederationEvent) {
        self.file.record(event.clone());
        if let Err(error) = self.ledger.append(&event) {
            log::warn!("federation audit ledger append failed: {error}");
        }
    }
}

impl FederationAudit for FileAudit {
    fn record(&self, event: FederationEvent) {
        let _guard = self
            .guard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(error) = record_bounded(&self.path, &event, MAX_BYTES, KEEP_LINES) {
            log::warn!("federation audit record was not written: {error}");
            return;
        }
        let _ = events().send(event);
    }
}

/// Append one event with an explicit bound, for tests and diagnostics.
pub(crate) fn record_bounded(
    path: &Path,
    event: &FederationEvent,
    max_bytes: u64,
    keep: usize,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(event).map_err(io::Error::other)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")?;
    drop(file);
    if fs::metadata(path)?.len() <= max_bytes {
        return Ok(());
    }
    // Compaction keeps the newest lines, so a long-lived log stays bounded
    // without losing the events nearest to the problem.
    let text = fs::read_to_string(path)?;
    let mut lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() > keep {
        lines.drain(..lines.len() - keep);
    }
    let temporary = path.with_extension("jsonl.part");
    fs::write(&temporary, format!("{}\n", lines.join("\n")))?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(())
}

/// How old a process's audit file may be before another launch removes it.
pub(crate) const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// Remove per-process audit files older than [`MAX_AGE`].
///
/// The current process's file is never touched. Age is the only signal used:
/// a pid can be recycled, so asking the OS whether it is alive would keep an
/// unrelated process's file forever or delete a live one after a restart.
pub(crate) fn prune_stale(path: &Path, now: std::time::SystemTime) -> usize {
    let Some(directory) = path.parent() else {
        return 0;
    };
    let own = format!("audit-{}.jsonl", std::process::id());
    let Ok(entries) = fs::read_dir(directory) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == own || !name.starts_with("audit-") || !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        if now
            .duration_since(modified)
            .map(|age| age > MAX_AGE)
            .unwrap_or(false)
            && fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}
/// The newest records, up to `limit`; a malformed line is skipped.
///
/// Reads the legacy single log plus every audit-*.jsonl a process wrote beside
/// it, and orders the union by event time, so a shared data directory reads as
/// one story even though each process owns one file.
pub(crate) fn load(path: &Path, limit: usize) -> Vec<FederationEvent> {
    let mut files = vec![path.to_path_buf()];
    if let Some(directory) = path.parent() {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("audit-") && name.ends_with(".jsonl") {
                    files.push(entry.path());
                }
            }
        }
    }

    let mut events: Vec<FederationEvent> = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        events.extend(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str(line).ok()),
        );
    }
    events.sort_by_key(|event| event.at_ms);
    if events.len() > limit {
        events.drain(..events.len() - limit);
    }
    events
}

#[cfg(test)]
mod tests {
    use super::{load, push_recent, record_bounded, subscribe, DurableAudit, FileAudit};
    use federation::{Direction, FederationAudit, FederationEvent};

    fn event(peer: &str) -> FederationEvent {
        FederationEvent::new(
            Direction::Inbound,
            peer,
            "invoke",
            Some("builtin/echo"),
            "ok",
        )
    }

    #[test]
    fn the_tail_keeps_the_newest_five() {
        let mut recent = Vec::new();
        for index in 0..7 {
            push_recent(
                &mut recent,
                FederationEvent::new(
                    Direction::Outbound,
                    "peer",
                    "invoke",
                    None,
                    &index.to_string(),
                ),
            );
        }
        assert_eq!(recent.len(), 5);
        assert_eq!(recent.first().expect("oldest kept").outcome, "2");
        assert_eq!(recent.last().expect("newest").outcome, "6");
    }

    #[test]
    fn a_written_record_reaches_a_subscriber() {
        let mut receiver = subscribe();
        let root =
            std::env::temp_dir().join(format!("bsh-federation-broadcast-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let path = root.join("audit.jsonl");
        let event = FederationEvent::new(Direction::Inbound, "peer-bc", "resource", None, "ok")
            .with_subject("lease-bc");

        FileAudit::new(path).record(event);

        let mut found = false;
        for _ in 0..10 {
            match receiver.try_recv() {
                Ok(received) if received.subject.as_deref() == Some("lease-bc") => {
                    found = true;
                    assert_eq!(received.operation, "resource");
                    break;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
        assert!(found, "the record reached the subscriber");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_sink_writes_its_own_process_file() {
        let root =
            std::env::temp_dir().join(format!("bsh-federation-per-process-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let legacy = root.join("audit.jsonl");

        FileAudit::new(legacy.clone()).record(event("peer-own"));

        let own = root.join(format!("audit-{}.jsonl", std::process::id()));
        assert!(own.is_file(), "the process writes its own file");
        assert!(!legacy.exists(), "no two processes share an inode");
        assert_eq!(load(&legacy, 5).len(), 1, "the reader merges it back");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_files_from_several_processes_merge_in_time_order() {
        let root =
            std::env::temp_dir().join(format!("bsh-federation-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");

        let mut legacy = event("peer-legacy");
        legacy.at_ms = 100;
        let mut first = event("peer-first");
        first.at_ms = 200;
        let mut second = event("peer-second");
        second.at_ms = 150;

        record_bounded(&root.join("audit.jsonl"), &legacy, 64 * 1024, 10).expect("legacy");
        record_bounded(&root.join("audit-1.jsonl"), &first, 64 * 1024, 10).expect("first");
        record_bounded(&root.join("audit-2.jsonl"), &second, 64 * 1024, 10).expect("second");

        let events = load(&root.join("audit.jsonl"), 10);
        let peers: Vec<&str> = events.iter().map(|event| event.peer.as_str()).collect();
        assert_eq!(peers, ["peer-legacy", "peer-second", "peer-first"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_process_files_are_pruned_and_fresh_ones_kept() {
        let root =
            std::env::temp_dir().join(format!("bsh-federation-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let legacy = root.join("audit.jsonl");
        let old = root.join("audit-99999.jsonl");
        let fresh = root.join("audit-88888.jsonl");

        record_bounded(&old, &event("peer-old"), 64 * 1024, 10).expect("old");
        record_bounded(&fresh, &event("peer-fresh"), 64 * 1024, 10).expect("fresh");
        record_bounded(&legacy, &event("peer-legacy"), 64 * 1024, 10).expect("legacy");
        std::fs::File::options()
            .write(true)
            .open(&old)
            .expect("the old file")
            .set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600),
            )
            .expect("an old mtime");

        let removed = super::prune_stale(&legacy, std::time::SystemTime::now());

        assert_eq!(removed, 1);
        assert!(!old.exists(), "the stale process file is gone");
        assert!(fresh.exists(), "a recent process file stays");
        assert!(legacy.exists(), "the legacy log is never pruned");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn the_log_keeps_the_newest_records_within_its_bound() {
        let root =
            std::env::temp_dir().join(format!("bsh-federation-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch directory");
        let path = root.join("audit.jsonl");

        for index in 0..6 {
            record_bounded(&path, &event(&format!("peer-{index}")), 400, 3).expect("a record");
        }

        let events = load(&path, 10);
        assert!(
            events.len() <= 3,
            "the bound keeps the newest: {}",
            events.len()
        );
        assert_eq!(events.last().expect("a record").peer, "peer-5");

        let text = std::fs::read_to_string(&path).expect("the log");
        assert!(
            !text.contains("\"input\""),
            "no payload is recorded: {text}"
        );
        assert!(text.contains("\"outcome\":\"ok\""), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_durable_sink_writes_a_verifiable_ledger() {
        use federation::Direction;
        let root = std::env::temp_dir().join(format!("bsh-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        let path = root.join("audit.jsonl");
        let sink = DurableAudit::new(path.clone()).expect("sink");
        sink.record(FederationEvent::new(
            Direction::Inbound,
            "node-b",
            "invoke",
            Some("builtin/echo"),
            "ok",
        ));
        sink.record(FederationEvent::new(
            Direction::Outbound,
            "node-b",
            "delegate",
            None,
            "denied",
        ));
        let report = federation::ledger::AuditLedger::verify(sink.ledger_path())
            .expect("the chain verifies");
        assert_eq!(report.entries, 2);
        assert!(
            root.join(format!("audit-{}.jsonl", std::process::id()))
                .exists(),
            "the diagnostic tail is still written"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
