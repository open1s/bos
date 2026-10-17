//! Session persistence, one directory per workspace.
//!
//! One JSON file per session, written whole, at
//! `<root>/.bsh/sessions/<id>.json` - beside the workspace it belongs to, so a
//! checkout carries its own history and two machines do not share one session
//! list. A session whose workspace is unknown is written to the harness
//! directory instead, which is the only place left to put it.
//!
//! The directory is the database: a session that fails to parse is skipped
//! rather than aborting the load, so one damaged file cannot take the sidebar
//! down with it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::Session;

/// The directory a workspace keeps its sessions in.
pub(crate) fn sessions_dir(root: &Path) -> PathBuf {
    root.join(".bsh").join("sessions")
}

/// The loaded set of sessions plus where each one is written.
pub(crate) struct Store {
    /// Where a session goes when its workspace is unknown.
    quarantine: PathBuf,
    /// Workspace id to the directory that workspace keeps its sessions in.
    roots: BTreeMap<String, PathBuf>,
    /// Every session, newest first.
    pub(crate) sessions: Vec<Session>,
    /// The credential strings that must never reach a session file.
    ///
    /// A transcript is a record of what was said, and what was said sometimes
    /// QUOTES the configuration - a tool that printed the settings file puts the
    /// key in the conversation. The session document must not store a
    /// credential (the design keeps provider references only), so the live keys
    /// are scrubbed out of the serialized text at the one place a session is
    /// written. Only the exact configured values are replaced, never a pattern,
    /// so an ordinary sentence is not mangled by a guess at what a secret looks
    /// like.
    secrets: Vec<String>,
}

impl Store {
    /// Open the store over every registered workspace.
    ///
    /// The pairs are `(workspace id, workspace root)`: the id is how a session
    /// says where it lives, the root is where that turns into a path.
    pub(crate) fn open(workspaces: &[(String, String)]) -> Self {
        Self::open_over(workspaces, crate::settings::dir().join("sessions"))
    }

    /// Open a store over explicit workspace roots and an explicit quarantine.
    ///
    /// Tests point the quarantine at a scratch directory, so nothing under the
    /// user home is read or written.
    pub(crate) fn open_over(workspaces: &[(String, String)], quarantine: PathBuf) -> Self {
        let roots: BTreeMap<String, PathBuf> = workspaces
            .iter()
            .filter(|(id, root)| !id.trim().is_empty() && !root.trim().is_empty())
            .map(|(id, root)| (id.clone(), sessions_dir(Path::new(root))))
            .collect();
        let mut store = Self {
            quarantine,
            roots,
            sessions: Vec::new(),
            secrets: Vec::new(),
        };
        let dirs: Vec<PathBuf> = store.roots.values().cloned().collect();
        store.read(&dirs);
        store.sort();
        store
    }

    /// Open a store rooted at one explicit session directory.
    ///
    /// Test-only: it is how a test points the store at a scratch directory, so
    /// nothing under the user home is read or written. A real store is opened
    /// with `open` or `open_over`, because its roots come from the registry.
    /// The directory this store writes to when a workspace is unknown.
    pub(crate) fn root(&self) -> &Path {
        &self.quarantine
    }

    /// Whether this store was pointed away from the real sessions home.
    ///
    /// A test opens one, and the answer is what keeps a test's settings writes
    /// beside its own sessions rather than in the user's configuration.
    pub(crate) fn is_scratch(&self) -> bool {
        self.quarantine != crate::settings::dir().join("sessions")
    }

    /// Open a store over a scratch directory, for the tests: its roots come
    /// from the registry and its quarantine is a directory a test owns.
    #[cfg(test)]
    pub(crate) fn open_at(dir: PathBuf) -> Self {
        let mut store = Self {
            quarantine: dir,
            roots: BTreeMap::new(),
            sessions: Vec::new(),
            secrets: Vec::new(),
        };
        store.read(&[]);
        store.sort();
        store
    }

    /// Read every session that parses, from each directory in turn.
    ///
    /// One directory may be named twice - a workspace rooted at the harness
    /// directory would collide with the quarantine - so a path is read once.
    fn read(&mut self, dirs: &[PathBuf]) {
        let quarantine = self.quarantine.clone();
        let mut seen: BTreeSet<&Path> = BTreeSet::new();
        for dir in dirs.iter().chain(std::iter::once(&quarantine)) {
            if !seen.insert(dir.as_path()) {
                continue;
            }
            let _ = fs::create_dir_all(dir);
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Ok(session) = serde_json::from_str::<Session>(&text) {
                    self.sessions.push(session);
                }
            }
        }
        self.dedupe_by_id();
    }

    /// Where one session is written: beside its workspace, else the quarantine.
    fn dir_for(&self, session: &Session) -> &Path {
        self.roots
            .get(&session.workspace)
            .map_or(self.quarantine.as_path(), PathBuf::as_path)
    }

    /// Declare the credentials that must not be written into a session file.
    pub(crate) fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        self.secrets = secrets
            .into_iter()
            .map(|secret| secret.trim().to_string())
            .filter(|secret| secret.len() >= 6)
            .collect();
        self
    }

    /// Redact every configured credential from one serialized session.
    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }

    /// Keep one session per id.
    ///
    /// The same id can be on disk twice: a session written while its workspace
    /// was unknown landed in the quarantine, and a later save lands beside its
    /// workspace. Reading both would show the same conversation twice, so the
    /// newer copy wins and the older one is dropped from the list.
    fn dedupe_by_id(&mut self) {
        let mut newest: BTreeMap<String, usize> = BTreeMap::new();
        for (index, session) in self.sessions.iter().enumerate() {
            match newest.get(&session.id) {
                Some(&kept) if self.sessions[kept].updated_at >= session.updated_at => {}
                _ => {
                    newest.insert(session.id.clone(), index);
                }
            }
        }
        let mut kept: Vec<usize> = newest.into_values().collect();
        kept.sort_unstable();
        self.sessions = kept.into_iter().map(|i| self.sessions[i].clone()).collect();
    }

    /// Attach the workspace-less sessions to the workspace in use.
    ///
    /// A session whose workspace id the registry no longer names belongs to no
    /// row the sidebar draws, so it is loaded and then invisible - history the
    /// reader cannot reach. Re-homing it is the only thing that can happen to
    /// it: there is no other workspace to attribute it to. The old file is
    /// removed, because the next save writes the session under its workspace and
    /// leaving the quarantined copy behind would put the same id on disk twice.
    pub(crate) fn rehome_orphans(&mut self, active: &str) -> usize {
        if active.trim().is_empty() || !self.roots.contains_key(active) {
            return 0;
        }
        let orphans: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| !self.roots.contains_key(&session.workspace))
            .map(|session| session.id.clone())
            .collect();
        for id in &orphans {
            let Some(index) = self.sessions.iter().position(|s| &s.id == id) else {
                continue;
            };
            let stale = self
                .dir_for(&self.sessions[index])
                .join(format!("{id}.json"));
            self.sessions[index].workspace = active.to_string();
            let session = self.sessions[index].clone();
            let _ = fs::remove_file(&stale);
            let _ = self.save(&session);
        }
        if orphans.is_empty() {
            return 0;
        }
        self.sort();
        orphans.len()
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
        let dir = self.dir_for(session);
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", session.id));
        let text = self.redact(&serde_json::to_string_pretty(session)?);
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
            provider: None,
            model: None,
            agent_preset: None,
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

    /// A scratch directory no other test shares.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/bsh-settings-tests")
            .join(format!("store-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// One registered workspace, as an (id, root) pair.
    fn registry(root: &std::path::Path) -> Vec<(String, String)> {
        vec![("w1".to_string(), root.to_string_lossy().to_string())]
    }

    #[test]
    fn a_session_is_written_beside_the_workspace_it_belongs_to() {
        let base = scratch("roots");
        let root = base.join("proj");
        let store = super::Store::open_over(&registry(&root), base.join("quarantine"));

        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        store.save(&mine).expect("saves");
        assert!(
            super::sessions_dir(&root).join("a.json").exists(),
            "a session belongs beside its workspace, so a checkout carries its own \
             history instead of sharing one list across machines"
        );

        let mut orphan = row("b", 1, 1);
        orphan.workspace = "gone".to_string();
        store.save(&orphan).expect("saves");
        assert!(
            base.join("quarantine").join("b.json").exists(),
            "a session whose workspace is unknown goes to the harness directory"
        );
    }

    #[test]
    fn a_store_reopened_over_a_workspace_finds_its_sessions() {
        let base = scratch("reopen");
        let root = base.join("proj");
        let quarantine = base.join("quarantine");
        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        super::Store::open_over(&registry(&root), quarantine.clone())
            .save(&mine)
            .expect("saves");

        let reopened = super::Store::open_over(&registry(&root), quarantine);
        assert!(
            reopened.session("a").is_some(),
            "a session is found where it was written"
        );
    }

    #[test]
    fn a_workspace_rooted_at_the_quarantine_is_read_once() {
        let base = scratch("collide");
        // A workspace whose root IS the harness directory points its session
        // directory at the quarantine, so the same files are reached twice.
        let registry = [("w1".to_string(), base.to_string_lossy().to_string())];
        let store = super::Store::open_over(&registry, super::sessions_dir(&base));
        let mut mine = row("a", 1, 1);
        mine.workspace = "w1".to_string();
        store.save(&mine).expect("saves");

        let reopened = super::Store::open_over(&registry, super::sessions_dir(&base));
        assert_eq!(
            reopened.sessions.len(),
            1,
            "one file must yield one session, not one per directory that reaches it"
        );
    }
}

#[cfg(test)]
mod raw_tests {
    use super::*;

    /// A session whose workspace nobody registers is re-homed, not hidden.
    ///
    /// It loads, then the sidebar filter drops it from every row, so the history
    /// is on disk and unreachable. Re-homing is the only thing that can happen
    /// to it: there is no other workspace to attribute it to.
    #[test]
    fn an_orphaned_session_is_rehomed_to_the_workspace_in_use() {
        let base = std::env::temp_dir().join(format!("bsh-rehome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("the workspace root");
        let quarantine = base.join("quarantine");
        std::fs::create_dir_all(&quarantine).expect("the quarantine");

        let mut orphan = crate::model::Session::new("dead-workspace-id", "New chat");
        orphan.id = "orphan-1".to_string();
        let text = serde_json::to_string_pretty(&orphan).expect("serialize");
        std::fs::write(quarantine.join("orphan-1.json"), text).expect("the orphan file");

        let mut store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            quarantine.clone(),
        );
        assert_eq!(store.sessions.len(), 1, "the orphan is loaded");
        assert!(
            store.in_workspace("live").is_empty(),
            "and invisible: that is the defect this pins"
        );

        let moved = store.rehome_orphans("live");
        assert_eq!(moved, 1, "one session re-homed");
        assert_eq!(
            store.in_workspace("live").len(),
            1,
            "the history is reachable now"
        );
        assert!(
            !quarantine.join("orphan-1.json").exists(),
            "the quarantined copy is gone, so the id is not on disk twice"
        );
        let beside = crate::store::sessions_dir(&root).join("orphan-1.json");
        assert!(beside.exists(), "and it lives beside its workspace now");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A configured credential never reaches a session file.
    ///
    /// A transcript can quote the configuration - a tool that printed the
    /// settings file puts the key in the conversation - and a session document
    /// stores provider references, never a credential. Only the exact configured
    /// value is replaced, so ordinary text is not mangled.
    #[test]
    fn a_configured_credential_is_redacted_from_the_session_file() {
        let base = std::env::temp_dir().join(format!("bsh-redact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("workspace");
        std::fs::create_dir_all(&root).expect("the workspace root");

        let store = Store::open_over(
            &[("live".to_string(), root.to_string_lossy().to_string())],
            base.join("quarantine"),
        )
        .with_secrets(vec!["sk-live-secret-123".to_string()]);

        let mut session = crate::model::Session::new("live", "New chat");
        session.id = "quotes-the-config".to_string();
        session.messages.push(crate::model::ChatMessage::user(
            "api_key = \"sk-live-secret-123\"\nbase_url = \"http://127.0.0.1:11436/v1\"",
        ));
        store.save(&session).expect("the save");

        let path = crate::store::sessions_dir(&root).join("quotes-the-config.json");
        let written = std::fs::read_to_string(&path).expect("the session file");
        assert!(
            !written.contains("sk-live-secret-123"),
            "the credential is not on disk: {written}"
        );
        assert!(
            written.contains("***"),
            "the field survives, its value does not: {written}"
        );
        assert!(
            written.contains("http://127.0.0.1:11436/v1"),
            "and the rest of the quoted document is untouched"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
