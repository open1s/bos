//! The launcher's automatically maintained recent list.
//!
//! The Runtime records what the reader opens; the reader never has to mark a
//! favorite. One small JSON file beside the settings document survives a
//! restart, and the launcher renders it under a stable name. An entry is
//! identified by its kind and id, so opening the same panel or file twice moves
//! the existing row to the front instead of duplicating it.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Shorten a path under the reader's home to a leading tilde.
///
/// The launcher's recent rows show a path the reader can recognise; an
/// absolute home path is noise in a narrow panel. This is display only - the
/// row's id stays the workspace-relative path it opens.
pub(crate) fn display_path(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home = Path::new(&home).to_string_lossy();
    if path == home {
        return "~".to_string();
    }
    let prefix = format!("{home}/");
    match path.strip_prefix(&prefix) {
        Some(rest) => format!("~/{rest}"),
        None => path.to_string(),
    }
}

/// The most recent rows the file keeps.
pub(crate) const MAX_RECENT: usize = 30;

/// What a recent row opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecentKind {
    /// A workspace root.
    Workspace,
    /// A file in a workspace.
    Document,
    /// A right-panel surface.
    Panel,
}

/// One thing the reader opened, with the facts needed to open it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecentEntry {
    /// What the row opens.
    pub(crate) kind: RecentKind,
    /// Stable identity inside the kind: workspace id, relative path, panel slug.
    pub(crate) id: String,
    /// The line the launcher shows.
    pub(crate) label: String,
    /// The secondary line: a path, a root or a panel description.
    pub(crate) detail: String,
    /// The workspace a document belongs to, when it is not the current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace: Option<String>,
    /// When it was last opened, in milliseconds since the Unix epoch.
    pub(crate) opened_at_ms: u64,
}

/// Every recent row, newest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RecentStore {
    entries: Vec<RecentEntry>,
}

impl RecentStore {
    /// Read the file, or start empty when it is missing or unreadable.
    pub(crate) fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write the file atomically beside the settings document.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())
    }

    /// Record one opening, moving an existing row to the front.
    pub(crate) fn record(
        &mut self,
        kind: RecentKind,
        id: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
        workspace: Option<String>,
        opened_at_ms: u64,
    ) {
        let id = id.into();
        self.entries
            .retain(|entry| !(entry.kind == kind && entry.id == id));
        self.entries.insert(
            0,
            RecentEntry {
                kind,
                id,
                label: label.into(),
                detail: detail.into(),
                workspace,
                opened_at_ms,
            },
        );
        self.entries.truncate(MAX_RECENT);
    }

    /// Forget one row.
    pub(crate) fn remove(&mut self, kind: RecentKind, id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.kind == kind && entry.id == id));
        self.entries.len() != before
    }

    /// Every row, newest first.
    pub(crate) fn entries(&self) -> &[RecentEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::{RecentKind, RecentStore, MAX_RECENT};

    /// Re-opening a row moves it instead of copying it, and the cap holds.
    #[test]
    fn a_second_opening_moves_the_row_to_the_front() {
        let mut store = RecentStore::default();
        store.record(
            RecentKind::Panel,
            "files",
            "Files",
            "Open local files",
            None,
            1,
        );
        store.record(
            RecentKind::Panel,
            "terminal",
            "Terminal",
            "Open a terminal",
            None,
            2,
        );
        store.record(
            RecentKind::Panel,
            "files",
            "Files",
            "Open local files",
            None,
            3,
        );
        assert_eq!(store.entries().len(), 2);
        assert_eq!(store.entries()[0].id, "files");
        assert_eq!(store.entries()[1].id, "terminal");

        for index in 0..MAX_RECENT + 5 {
            store.record(
                RecentKind::Document,
                format!("file-{index}.md"),
                format!("file-{index}.md"),
                ".",
                None,
                index as u64,
            );
        }
        assert_eq!(store.entries().len(), MAX_RECENT);
    }

    /// The store round-trips through the file and a removal persists.
    #[test]
    fn the_file_round_trips_and_removal_persists() {
        let path = std::env::temp_dir().join(format!(
            "bsh-recent-{}-{}.json",
            std::process::id(),
            agent::capability::unix_ms()
        ));
        let mut store = RecentStore::default();
        store.record(
            RecentKind::Workspace,
            "w1",
            "bos",
            "/tmp/bos",
            Some("w1".to_string()),
            10,
        );
        store.save(&path).expect("save");
        let mut loaded = RecentStore::load(&path);
        assert_eq!(loaded.entries().len(), 1);
        assert_eq!(loaded.entries()[0].detail, "/tmp/bos");
        assert!(loaded.remove(RecentKind::Workspace, "w1"));
        loaded.save(&path).expect("save");
        assert!(RecentStore::load(&path).entries().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
