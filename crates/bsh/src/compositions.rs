//! The workspace-level composition log.
//!
//! Every build records what it composed: which capabilities, at which versions,
//! from which provider, and whether a binding ran here or on a peer. The log is
//! diagnostic, so a write failure is logged and never fails the turn.
//!
//! It is append-only with a size bound: once the file passes the cap it is
//! rewritten with the newest records, so a long-lived workspace cannot grow it
//! without limit. Records carry identifiers and versions only; a snapshot has
//! no arguments, no results, and no secrets to leak.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use agent::capability::CompositionSnapshot;

/// The size at which the log is compacted.
pub(crate) const MAX_BYTES: u64 = 512 * 1024;

/// How many of the newest records survive a compaction.
pub(crate) const KEEP_LINES: usize = 200;

/// Append one record, compacting the log when it passes the default bound.
pub(crate) fn record(path: &Path, snapshot: &CompositionSnapshot) {
    if let Err(error) = record_bounded(path, snapshot, MAX_BYTES, KEEP_LINES) {
        log::warn!("composition record was not written: {error}");
    }
}

/// Append one record with an explicit bound, for tests and diagnostics.
pub(crate) fn record_bounded(
    path: &Path,
    snapshot: &CompositionSnapshot,
    max_bytes: u64,
    keep: usize,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(snapshot).map_err(io::Error::other)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    drop(file);
    if fs::metadata(path)?.len() > max_bytes {
        compact(path, keep)?;
    }
    Ok(())
}

/// The newest records, up to `limit`. A malformed line is skipped rather than
/// failing the read: a torn append must not hide the records around it.
///
/// The audit export is the reader this exists for.
pub(crate) fn load(path: &Path, limit: usize) -> Vec<CompositionSnapshot> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut records: Vec<CompositionSnapshot> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let start = records.len().saturating_sub(limit);
    records.split_off(start)
}

fn compact(path: &Path, keep: usize) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(keep);
    let mut body = String::new();
    for line in &lines[start..] {
        body.push_str(line);
        body.push('\n');
    }
    let temp = path.with_extension("jsonl.tmp");
    fs::write(&temp, body)?;
    fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::capability::composition::{CapabilityRequirement, CompositionSpec, LocalScope};
    use agent::capability::descriptor::CapabilityDescriptor;
    use agent::capability::id::{CapabilityKind, VersionSource, WorkspaceId};
    use agent::capability::{resolve, Catalogue};
    use semver::{Version, VersionReq};

    fn snapshot(index: usize) -> CompositionSnapshot {
        let mut catalogue = Catalogue::new();
        let id = format!("builtin/tool_{index}");
        let descriptor = CapabilityDescriptor::new(
            id.parse().expect("a valid id"),
            CapabilityKind::Tool,
            Version::new(1, 0, 0),
            VersionSource::Declared,
        );
        catalogue.insert(descriptor).expect("unique");
        let workspace = WorkspaceId::new("ws:test").expect("a valid workspace");
        let scope = LocalScope::admitting(workspace, &catalogue);
        let spec = CompositionSpec::new("compose:test", Version::new(1, 0, 0), "log test")
            .with_requirement(CapabilityRequirement::new(
                id.parse().expect("a valid id"),
                VersionReq::STAR,
            ));
        let resolved = resolve(&spec, &catalogue, &[], &scope).expect("resolved");
        CompositionSnapshot::from_resolved(&resolved, None, index as u64)
    }

    fn scratch(label: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("bsh-compositions-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a scratch root");
        root
    }

    #[test]
    fn records_append_and_load_in_order() {
        let root = scratch("order");
        let path = root.join("compositions.jsonl");
        record_bounded(&path, &snapshot(1), MAX_BYTES, KEEP_LINES).expect("first");
        record_bounded(&path, &snapshot(2), MAX_BYTES, KEEP_LINES).expect("second");

        let records = load(&path, 10);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].created_at_ms, 1);
        assert_eq!(records[1].created_at_ms, 2);
        let json = fs::read_to_string(&path).expect("readable");
        assert!(!json.contains("secret"), "{json}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_malformed_line_is_skipped() {
        let root = scratch("torn");
        let path = root.join("compositions.jsonl");
        record_bounded(&path, &snapshot(3), MAX_BYTES, KEEP_LINES).expect("record");
        let mut file = OpenOptions::new().append(true).open(&path).expect("open");
        file.write_all(b"{torn\n").expect("torn line");
        record_bounded(&path, &snapshot(4), MAX_BYTES, KEEP_LINES).expect("record");

        let records = load(&path, 10);

        assert_eq!(records.len(), 2, "the readable records survive");
        assert_eq!(records[0].created_at_ms, 3);
        assert_eq!(records[1].created_at_ms, 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_log_is_bounded_and_keeps_the_newest_records() {
        let root = scratch("bounded");
        let path = root.join("compositions.jsonl");
        for index in 0..6 {
            record_bounded(&path, &snapshot(index), 1, 2).expect("record");
        }

        let records = load(&path, 10);

        assert_eq!(records.len(), 2, "compaction keeps only the newest two");
        assert_eq!(records[0].created_at_ms, 4);
        assert_eq!(records[1].created_at_ms, 5);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_remote_binding_is_recorded_as_remote() {
        let root = scratch("remote");
        let path = root.join("compositions.jsonl");
        let local = Catalogue::new();
        let peer = agent::capability::PeerCatalogue::new(
            agent::capability::NodeId::new("node-b").expect("a node"),
            {
                let mut catalogue = Catalogue::new();
                catalogue
                    .insert(CapabilityDescriptor::new(
                        "builtin/build".parse().expect("an id"),
                        CapabilityKind::Tool,
                        Version::new(1, 0, 0),
                        VersionSource::Declared,
                    ))
                    .expect("unique");
                catalogue
            },
        );
        let workspace = WorkspaceId::new("ws:test").expect("a workspace");
        let scope = LocalScope::admitting(workspace, &local);
        let spec = CompositionSpec::new("compose:fed", Version::new(1, 0, 0), "remote")
            .with_peer(
                agent::capability::PeerRef::new(agent::capability::NodeId::new("node-b").unwrap())
                    .with_grant(agent::capability::GrantRef::new("grant:builder")),
            )
            .with_requirement(
                CapabilityRequirement::new(
                    "builtin/build".parse().expect("an id"),
                    VersionReq::STAR,
                )
                .with_remote(agent::capability::RemotePolicy::Allowed),
            );
        let resolved = resolve(&spec, &local, &[peer], &scope).expect("resolved");
        let snapshot = CompositionSnapshot::from_resolved(&resolved, None, 9);

        record_bounded(&path, &snapshot, MAX_BYTES, KEEP_LINES).expect("record");
        let records = load(&path, 1);

        assert_eq!(records.len(), 1);
        assert!(!records[0].is_local_only());
        assert_eq!(
            records[0].capabilities[0].origin,
            agent::capability::SnapshotOrigin::Remote {
                node: "node-b".to_string(),
                grant: "grant:builder".to_string(),
            }
        );
        let _ = fs::remove_dir_all(&root);
    }
}
