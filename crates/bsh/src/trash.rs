//! The recycle bin.
//!
//! A delete that cannot be undone is a delete nobody dares to use, so both
//! destructive actions - a chat and a workspace row - PARK what they removed here
//! instead of unlinking it. One JSON envelope per item, under the settings root so
//! a scratch configuration gets a scratch bin:
//!
//! ```text
//! ~/.bos/bsh/trash/session-<unix>-<id>.json
//! ~/.bos/bsh/trash/workspace-<unix>-<id>.json
//! ```
//!
//! The envelope carries the payload itself (the whole `Session`, or the
//! `WorkspaceRef`), not a pointer to the original file, so restoring cannot depend
//! on a workspace that may have moved or a file somebody edited in between.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What was deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A chat.
    Session,
    /// A workspace row.
    Workspace,
}

impl Kind {
    /// The word the file name and the envelope use.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Kind::Session => "session",
            Kind::Workspace => "workspace",
        }
    }

    /// Parse the label back; an unknown one is ignored rather than guessed.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "session" => Some(Kind::Session),
            "workspace" => Some(Kind::Workspace),
            _ => None,
        }
    }
}

/// One parked item, exactly as it sits on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Envelope {
    /// `session` or `workspace`.
    pub(crate) kind: String,
    /// Unix seconds the delete happened.
    pub(crate) deleted_at: u64,
    /// The workspace the item lived in (a session) or its own id (a workspace).
    pub(crate) workspace: String,
    /// The workspace's display name, so the row can say where the item came from
    /// even after that workspace is gone.
    pub(crate) workspace_name: String,
    /// The row's own label: a chat title or a workspace name.
    pub(crate) name: String,
    /// The directory a parked transcript log was moved into, relative to the bin.
    ///
    /// A session's messages live in Parquet pages beside its document, not inside
    /// it, so the envelope alone would restore a chat with no transcript. The
    /// park moves those pages next to the envelope and records the name here; an
    /// envelope without it (a workspace, or a chat saved before the log existed)
    /// simply has no pages to put back.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) log: String,
    /// The thing itself.
    pub(crate) payload: serde_json::Value,
}

/// A parked item and where its envelope lives.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The envelope file.
    pub(crate) path: PathBuf,
    /// What it holds.
    pub(crate) envelope: Envelope,
}

impl Entry {
    /// The kind, if the envelope names one this build knows.
    pub(crate) fn kind(&self) -> Option<Kind> {
        Kind::from_key(&self.envelope.kind)
    }
}

/// Park an item: write its envelope and return the path.
///
/// A name that cannot be written is an error the caller reports; the delete then
/// does not happen, because losing the copy is worse than refusing the delete.
pub(crate) fn stash(dir: &Path, envelope: &Envelope) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // The unix stamp keeps the list ordered newest-first by file name, and the
    // payload id keeps two deletes in the same second apart.
    let id = envelope
        .payload
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or("item");
    let path = dir.join(format!(
        "{}-{}-{}.json",
        envelope.kind, envelope.deleted_at, id
    ));
    let text = serde_json::to_string_pretty(envelope)?;
    fs::write(&path, text)?;
    Ok(path)
}

/// The directory a parked item's log lives in, inside the bin.
pub(crate) fn log_dir(dir: &Path, envelope: &Envelope) -> PathBuf {
    dir.join(&envelope.log)
}

/// Move a session's Parquet log into the bin, returning the name it took.
///
/// A copy would double every transcript on disk and a delete would leave the
/// pages orphaned where nothing can find them again, so the park MOVES them:
/// the bin then holds the chat entire, and restoring is a second move.
pub(crate) fn park_log(dir: &Path, from: &Path, envelope: &Envelope) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let into = log_dir(dir, envelope);
    let _ = fs::remove_dir_all(&into);
    fs::rename(from, &into)
}

/// Every parked item, newest first. An unreadable envelope is skipped, not fatal:
/// one bad file must not hide the rest of the bin.
pub(crate) fn entries(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Entry> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "json").unwrap_or(false))
        .filter_map(|path| {
            let text = fs::read_to_string(&path).ok()?;
            let envelope: Envelope = serde_json::from_str(&text).ok()?;
            Some(Entry { path, envelope })
        })
        .collect();
    found.sort_by_key(|entry| std::cmp::Reverse(entry.envelope.deleted_at));
    found
}

/// Delete an envelope for good.
pub(crate) fn purge(path: &Path) -> bool {
    fs::remove_file(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-trash-{label}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn envelope(kind: Kind, id: &str, name: &str, at: u64) -> Envelope {
        Envelope {
            kind: kind.key().to_string(),
            deleted_at: at,
            workspace: "ws".to_string(),
            workspace_name: "bos".to_string(),
            name: name.to_string(),
            log: String::new(),
            payload: serde_json::json!({ "id": id, "title": name }),
        }
    }

    /// A parked item comes back exactly as it went in, and the bin is ordered
    /// newest-first so the row a reader just deleted is at the top.
    #[test]
    fn a_parked_item_is_listed_and_restored_from_its_envelope() {
        let dir = scratch("roundtrip");
        let older = stash(&dir, &envelope(Kind::Session, "a", "older", 10)).expect("parked");
        let newer = stash(&dir, &envelope(Kind::Workspace, "b", "newer", 20)).expect("parked");

        let listed = entries(&dir);
        assert_eq!(listed.len(), 2, "both envelopes are listed");
        assert_eq!(listed[0].path, newer, "newest first");
        assert_eq!(listed[1].path, older);
        assert_eq!(listed[0].kind(), Some(Kind::Workspace));
        assert_eq!(listed[1].kind(), Some(Kind::Session));
        assert_eq!(listed[1].envelope.name, "older");
        assert_eq!(
            listed[1]
                .envelope
                .payload
                .get("title")
                .and_then(|t| t.as_str()),
            Some("older"),
            "the payload travels with the envelope, not as a pointer"
        );

        assert!(purge(&newer), "purging removes the file");
        assert_eq!(entries(&dir).len(), 1, "and the bin shrinks");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An envelope a future build cannot read is skipped, not fatal: one bad file
    /// must not hide the rest of the bin.
    #[test]
    fn an_unreadable_envelope_does_not_hide_the_others() {
        let dir = scratch("skip");
        stash(&dir, &envelope(Kind::Session, "a", "good", 10)).expect("parked");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("session-1-broken.json"), b"not json").expect("write");

        let listed = entries(&dir);
        assert_eq!(listed.len(), 1, "the readable one survives");
        assert_eq!(listed[0].envelope.name, "good");
        let _ = fs::remove_dir_all(&dir);
    }
}
