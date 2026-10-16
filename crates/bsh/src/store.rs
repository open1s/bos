//! Session persistence under ~/.bos/bsh/sessions/.
//!
//! One JSON file per session, written whole. The directory is the database:
//! a session that fails to parse is skipped rather than aborting the load, so
//! one damaged file cannot take the sidebar down with it.

use std::fs;
use std::path::PathBuf;

use crate::model::Session;

/// The loaded set of sessions plus the directory they live in.
pub(crate) struct Store {
    dir: PathBuf,
    /// Every session, newest first.
    pub(crate) sessions: Vec<Session>,
}

impl Store {
    /// Open the store, reading every session that parses.
    pub(crate) fn open() -> Self {
        Self::open_at(crate::settings::dir().join("sessions"))
    }

    /// Open a store rooted at an explicit session directory. Tests point this at
    /// a scratch directory, so nothing under the user home is read or written.
    pub(crate) fn open_at(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        let mut sessions = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Ok(session) = serde_json::from_str::<Session>(&text) {
                    sessions.push(session);
                }
            }
        }
        let mut store = Self { dir, sessions };
        store.sort();
        store
    }

    /// Sort newest-first, pinned sessions ahead of the rest.
    pub(crate) fn sort(&mut self) {
        self.sessions.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
    }

    /// Look up a session by id.
    pub(crate) fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// Look up a session by id for mutation.
    pub(crate) fn session_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    /// Insert a session at the front.
    pub(crate) fn insert(&mut self, session: Session) {
        self.sessions.insert(0, session);
    }

    /// Write one session to disk.
    pub(crate) fn save(&self, session: &Session) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{}.json", session.id));
        let text = serde_json::to_string_pretty(session)?;
        fs::write(path, text)?;
        Ok(())
    }

    /// Sessions belonging to a workspace, newest first.
    pub(crate) fn in_workspace(&self, workspace: &str) -> Vec<&Session> {
        let mut rows: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.workspace == workspace && s.archived_at.is_none())
            .collect();
        rows.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        rows
    }

    /// Archived sessions, newest first.
    pub(crate) fn archived(&self) -> Vec<&Session> {
        let mut rows: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.archived_at.is_some())
            .collect();
        rows.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        rows
    }
}

/// How the session tree is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SessionSort {
    /// Newest activity first.
    #[default]
    Updated,
    /// Newest creation first.
    Created,
    /// Title, A to Z, case-insensitively.
    Name,
}

impl SessionSort {
    /// Stable name used to persist the choice, distinct from the locale key.
    pub(crate) fn persisted(&self) -> &'static str {
        match self {
            SessionSort::Updated => "updated",
            SessionSort::Created => "created",
            SessionSort::Name => "name",
        }
    }

    /// Read a persisted name, falling back to the default order.
    pub(crate) fn from_persisted(key: &str) -> Self {
        match key {
            "created" => SessionSort::Created,
            "name" => SessionSort::Name,
            _ => SessionSort::Updated,
        }
    }

    /// The next view option in the cycle.
    /// The locale key that names this view option.
    pub(crate) fn key(self) -> &'static str {
        match self {
            SessionSort::Updated => "side.sortUpdated",
            SessionSort::Created => "side.sortCreated",
            SessionSort::Name => "side.sortName",
        }
    }
}

/// Reorder session rows by the chosen view option; pinned rows stay first.
pub(crate) fn sort_rows(rows: &mut Vec<&Session>, sort: SessionSort) {
    rows.sort_by(|a, b| {
        b.pinned.cmp(&a.pinned).then_with(|| match sort {
            SessionSort::Updated => b.updated_at.cmp(&a.updated_at),
            SessionSort::Created => b.created_at.cmp(&a.created_at),
            SessionSort::Name => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::{sort_rows, SessionSort};
    use crate::model::Session;

    /// A session carrying only what the view options read.
    fn row(title: &str, created: u64, updated: u64) -> Session {
        Session {
            id: title.to_string(),
            title: title.to_string(),
            created_at: created,
            updated_at: updated,
            workspace: "w".to_string(),
            messages: Vec::new(),
            plan: Vec::new(),
            goal: None,
            pinned: false,
            archived_at: None,
            archived: Vec::new(),
        }
    }

    fn titles(rows: &[&Session]) -> Vec<String> {
        rows.iter().map(|session| session.title.clone()).collect()
    }

    #[test]
    fn the_view_options_order_sessions_three_ways() {
        let (beta, alpha, gamma) = (
            row("beta", 10, 30),
            row("alpha", 30, 20),
            row("gamma", 20, 10),
        );
        let mut rows = vec![&beta, &alpha, &gamma];
        sort_rows(&mut rows, SessionSort::Updated);
        assert_eq!(titles(&rows), ["beta", "alpha", "gamma"]);
        sort_rows(&mut rows, SessionSort::Created);
        assert_eq!(titles(&rows), ["alpha", "gamma", "beta"]);
        sort_rows(&mut rows, SessionSort::Name);
        assert_eq!(titles(&rows), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn pinned_sessions_stay_on_top_under_every_view_option() {
        let (beta, alpha, pinned) = (
            row("beta", 10, 30),
            row("alpha", 30, 20),
            Session {
                pinned: true,
                ..row("gamma", 20, 10)
            },
        );
        let mut rows = vec![&beta, &alpha, &pinned];
        sort_rows(&mut rows, SessionSort::Name);
        assert_eq!(titles(&rows), ["gamma", "alpha", "beta"]);
    }
}
