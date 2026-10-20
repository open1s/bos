//! The shell state: store, settings, active session and streaming bookkeeping.
//!
//! Framework-free: GPUI owns a value of this struct, but nothing here knows
//! that. Every mutation the shell makes goes through a method, so the render
//! pass only reads.

use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::Lang;
use crate::model::{
    ChatMessage, MessageMeta, MessageUsage, PlanItem, PlanStore, Session, ToolEvent,
};
use crate::runner::{AgentEvent, Runner};
use crate::settings::Settings;
use crate::store::Store;

/// Wall-clock milliseconds since the Unix epoch, for the trajectory timeline.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record when a turn's first visible output arrived, once.
fn note_first_output(message: &mut ChatMessage, at: u64) {
    message.timing_mut().first_output.get_or_insert(at);
}

/// Close a turn's measured span, when it has a start and no end yet.
fn settle_ms(message: &mut ChatMessage, at: u64) {
    let Some(start) = message.timing().and_then(|timing| timing.start) else {
        return;
    };
    if message.timing().and_then(|timing| timing.ms).is_some() {
        return;
    }
    message.timing_mut().ms = Some(at.saturating_sub(start));
}

/// Wall-clock seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Format a past instant as a UTC date and time.
pub(crate) fn stamp(at: u64) -> String {
    let days = (at / 86_400) as i64;
    let seconds = at % 86_400;
    // Howard Hinnant's civil-from-days, shifted to the 1970 epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60
    )
}

/// Format a past instant as a short age.
pub(crate) fn age(then: u64) -> String {
    let now = now();
    if then == 0 || then > now {
        return String::new();
    }
    let secs = now - then;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// How many trust decisions the audit ring keeps.
pub(crate) const TRUST_AUDIT_LIMIT: usize = 64;

/// One recorded trust decision about an installed extension's commands.
///
/// The record names the package, the directory it was trusted at, the exact
/// command line(s) the decision covers, when it was made and whether it was an
/// approval. Environment VALUES never enter it: a command is the executable and
/// its arguments, never the child's environment.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct TrustDecision {
    /// The settings id of the package.
    pub(crate) extension_id: String,
    /// The package directory at the time of the decision.
    pub(crate) path: String,
    /// The exact command line(s) the decision covers.
    pub(crate) commands: Vec<String>,
    /// Unix seconds the decision was made.
    pub(crate) at: u64,
    /// Whether the reader approved the commands.
    pub(crate) allow: bool,
}

/// A full-window surface layered over the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Overlay {
    /// The shell alone.
    #[default]
    None,
    /// The settings dialog.
    Settings,
}

/// The main column's tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    /// The conversation.
    #[default]
    Chat,
    /// The plan and tool trajectory.
    Trajectory,
}

/// One message waiting for its session's running turn to finish.
///
/// The reference keeps these in the session's durable inbox over the Host. bsh
/// has no such service, so a queued message lives with the shell for as long as
/// the app runs: it is taken off the front when the turn it waited for ends and
/// becomes the next turn's own input.
#[derive(Clone)]
pub(crate) struct Queued {
    /// Identity, stable for as long as the row is queued.
    ///
    /// A row's own edit and remove name this rather than a position, so an
    /// action can never land on a neighbouring message.
    pub(crate) seq: u64,
    /// The text the reader submitted.
    pub(crate) text: String,
    /// Images submitted with it, sent as content parts on the next turn.
    pub(crate) attachments: Vec<Attachment>,
}

/// Everything the shell renders from.
pub(crate) struct AppState {
    /// Persisted sessions.
    pub(crate) store: Store,
    /// Persisted settings.
    pub(crate) settings: Settings,
    /// Interface language.
    pub(crate) lang: Lang,
    /// The selected session.
    pub(crate) active_id: Option<String>,
    /// Workspaces whose session list is folded.
    pub(crate) collapsed: BTreeSet<String>,
    /// Session id to the instant its stream started.
    pub(crate) streams: HashMap<String, u64>,
    /// Whether the session-name search field is showing.
    pub(crate) search_open: bool,
    /// The session-name filter; empty matches every session.
    pub(crate) search_query: String,
    /// The plugin-name filter on the plugin list pane.
    pub(crate) plugin_query: String,
    /// The model-id filter on the fetched-model picker.
    pub(crate) picker_query: String,
    /// Images attached to the next turn, sent as multimodal content.
    pub(crate) attachments: Vec<Attachment>,

    /// The live plan store of every session whose turn is still writing.
    ///
    /// The agent shares this store with its `update_plan` tool, so a running
    /// turn's plan is read from here and the finished copy is saved onto the
    /// session it belongs to.
    plans: HashMap<String, PlanStore>,
    /// Messages waiting for a running session's own turn to end, oldest first.
    ///
    /// The reference drains one per finished turn; bsh starts the next turn from
    /// the front of this queue, which is what keeps a submission made while the
    /// agent runs from opening a second stream over the same transcript.
    inbox: HashMap<String, Vec<Queued>>,
    /// Identity for the next queued row; see [`Queued::seq`].
    inbox_seq: u64,
    /// The streaming runtime.
    pub(crate) runner: Runner,
    /// The live MCP connections every agent build shares.
    ///
    /// A server is a process or an endpoint, so connecting is expensive and
    /// must survive across turns; the pool reconciles when the settings change.
    pub(crate) mcp: crate::mcp::McpPool,
    /// The bounded ring of trust decisions, oldest first.
    ///
    /// This crate has no extension-wide audit surface, so the trust gate keeps
    /// its own small record beside the state it guards. Env values never enter
    /// it: only the command line, the package and its path.
    trust_audit: std::collections::VecDeque<TrustDecision>,
    /// A transient line of feedback.
    pub(crate) notice: Option<String>,
    /// Whether the usage pill's dialog is open.
    pub(crate) stat_open: bool,
    /// Whether the context meter's panel is open.
    pub(crate) meter_open: bool,
    /// Serialized length of the tool definitions the last built agent carried.
    ///
    /// The meter's breakdown prices the request's tool schemas from exactly this
    /// array, so its figure follows the agent the next turn will run.
    pub(crate) tools_schema_chars: usize,
    /// The locale key of the standing provider-form refusal, if any.
    pub(crate) provider_refusal: Option<&'static str>,
    /// The name of the provider the form is editing, if any.
    pub(crate) editing_provider: Option<String>,
    /// The provider awaiting a delete confirmation, if any.
    pub(crate) removing_provider: Option<String>,
    /// The open overlay.
    pub(crate) overlay: Overlay,
    /// The active main-column tab.
    pub(crate) tab: Tab,
    /// Tool rows the reader has opened, keyed "message:tool".
    pub(crate) expanded_tools: BTreeSet<String>,
    /// Settings section selected in the dialog.
    pub(crate) settings_section: usize,
    /// The settings being edited; None when the dialog is closed.
    pub(crate) draft: Option<Settings>,
    /// The admin-key vault. It reopens at launch from the app-access sidecar
    /// when that file exists, so provider entries resolve without the admin
    /// passphrase while the Vault pane stays locked; a missing or stale sidecar
    /// leaves the app without access until the vault is unlocked once.
    pub(crate) vault: crate::vault::Vault,
    /// Whether the sidebar is folded away. This is the reference's stored
    /// layoutInfo.sidebar === 0 sentinel, which only governs a wide viewport.
    pub(crate) sidebar_collapsed: bool,
    /// The reference's narrowExpanded: a manual expand below the auto-collapse
    /// breakpoint, which is runtime state and is never stored.
    pub(crate) narrow_expanded: bool,
    /// When the streaming turn started, for the elapsed-time line.
    pub(crate) turn_started: Option<std::time::Instant>,
    /// The session tree view option.
    pub(crate) sort: crate::store::SessionSort,
}

impl AppState {
    /// Load the store and settings, guaranteeing an active session.
    pub(crate) fn new() -> Self {
        let settings = Settings::load();
        let mut store = Store::open(&session_roots(&settings)).with_secrets(settings.secrets());
        // A session whose workspace the registry no longer names is loaded and
        // then filtered out of every sidebar row, so it is history the reader
        // cannot reach. Attaching it to the workspace in use is the only thing
        // that can happen to it.
        store.rehome_orphans(&settings.global.general.active_workspace);
        Self::assemble(store, settings)
    }

    /// Build the state over an explicit store and the shipped defaults.
    ///
    /// Test-only: a scratch store is how a test keeps real session files out of
    /// the user home, and `Settings::default()` is how it keeps that machine's
    /// own configuration out of the assertions. Reading the real file here made
    /// a measured copy assertion depend on the developer's language and context
    /// budget - a green suite on one machine and a red one on the next.
    /// `new` reads the settings first, because it needs the workspace registry
    /// to know where the store is rooted, so it cannot come through here
    /// without parsing the file twice.
    #[cfg(test)]
    pub(crate) fn with_store(store: Store) -> Self {
        Self::assemble(store, Settings::default())
    }

    /// Build the state from a store and the settings already in hand.
    fn assemble(store: Store, settings: Settings) -> Self {
        let lang = Lang::from_locale(&settings.global.general.lang);
        let mut state = Self {
            store,
            settings,
            lang,
            sort: crate::store::SessionSort::default(),

            active_id: None,
            collapsed: BTreeSet::new(),
            streams: HashMap::new(),
            plans: HashMap::new(),
            inbox: HashMap::new(),
            inbox_seq: 0,
            search_open: false,
            search_query: String::new(),
            plugin_query: String::new(),
            picker_query: String::new(),
            attachments: Vec::new(),
            runner: Runner::new(),
            mcp: crate::mcp::McpPool::default(),
            trust_audit: std::collections::VecDeque::new(),
            notice: None,
            stat_open: false,
            meter_open: false,
            tools_schema_chars: 0,
            provider_refusal: None,
            editing_provider: None,
            removing_provider: None,
            overlay: Overlay::None,
            tab: Tab::Chat,
            expanded_tools: BTreeSet::new(),
            settings_section: 0,
            draft: None,
            // The path is settled after the scratch redirect below, because a
            // scratch store is a scratch home.
            vault: crate::vault::Vault::disabled(),
            sidebar_collapsed: false,
            narrow_expanded: false,
            turn_started: None,
        };
        // The registry is a fact about the launch, so it is settled before
        // anything that depends on it reads it - a restored session is tagged
        // with a workspace id, and the panes that show a workspace resolve that
        // id through the registry.
        state.settings.ensure_workspace();
        // A scratch store is a scratch home: the settings a test moves are
        // written beside the sessions it makes, never over the user's real
        // configuration.
        if state.store.is_scratch() {
            state.settings.save_root = Some(state.store.root().to_path_buf());
            // A scratch home is a scratch WORKSPACE too. The first workspace a
            // test registers comes from the process's cwd - the crate directory
            // - and the workspace document follows the registry, so a save wrote
            // a `.bsh/settings/settings.toml` into the checkout a reader owns.
            // Both documents land in the scratch tree now.
            let scratch = state.store.root().to_string_lossy().to_string();
            for workspace in &mut state.settings.global.workspaces {
                workspace.root = scratch.clone();
            }
        }
        // The vault lives beside the document this state writes, which is the
        // scratch store for a test and the bsh data directory for the app. It
        // starts LOCKED, whether or not the file exists.
        state.vault = crate::vault::Vault::new(state.settings.vault_path());
        state.ensure_active();
        state
    }

    /// Guarantee a valid active session, creating one when the store is empty.
    pub(crate) fn ensure_active(&mut self) {
        let valid = self
            .active_id
            .as_ref()
            .map(|id| self.store.session(id).is_some())
            .unwrap_or(false);
        if !valid {
            self.active_id = self
                .store
                .sessions
                .iter()
                .find(|s| s.archived_at.is_none())
                .map(|s| s.id.clone());
        }
        if self.active_id.is_none() {
            self.new_session();
        }
        if let Some(id) = self.active_id.clone() {
            self.store.rehydrate(&id);
        }
    }

    /// The selected session.
    pub(crate) fn active_session(&self) -> Option<&Session> {
        self.active_id
            .as_ref()
            .and_then(|id| self.store.session(id))
    }

    /// What one session contributes to model resolution.
    ///
    /// A session carries its own (provider, nickname) pair, so two chats in the
    /// same workspace can run on different models. The pair is a reference into
    /// the global tree and never a copy of the endpoint. It is the ONLY model
    /// selection: a session that names none has no model and is refused at send
    /// rather than inheriting one from somewhere else.
    pub(crate) fn session_choice(&self, id: &str) -> crate::settings::resolve::SessionChoice {
        self.store
            .session(id)
            .map(Session::choice)
            .unwrap_or_default()
    }

    /// The values a build in the given session uses.
    ///
    /// The session's own choice, resolved against the global tree - one order,
    /// read by the chip, the menu, the send gate and the agent.
    pub(crate) fn effective_in(&self, id: &str) -> crate::settings::Effective {
        self.settings.effective_for(&self.session_choice(id))
    }

    /// The values a build for the session on screen uses.
    ///
    /// No session at all reads as no choice, which is also what a session that
    /// never chose gets: an unchosen model, never a silent fallback.
    pub(crate) fn effective(&self) -> crate::settings::Effective {
        match self.active_id.as_deref() {
            Some(id) => self.effective_in(id),
            None => self.settings.effective(),
        }
    }

    /// Move a legacy workspace model pair onto the session that is open.
    ///
    /// Settings written before the choice became session-scoped carried the
    /// pair under the workspace's `[general]`. That document is deserialize-
    /// only now: this reads the pair ONCE, gives it to the session on screen,
    /// saves that session, and clears the in-memory copy so no later session
    /// can pick it up. A session that already chose keeps its own choice.
    ///
    /// Returns whether a session took the pair.
    pub(crate) fn adopt_legacy_model(&mut self) -> bool {
        let pair = self.settings.workspace.general.legacy_pair();
        // Consumed either way: the pair describes the session that was open
        // when the file was written, and that decision is made once.
        self.settings.workspace.general.legacy_provider.clear();
        self.settings.workspace.general.legacy_model.clear();
        let (Some(pair), Some(id)) = (pair, self.active_id.clone()) else {
            return false;
        };
        let Some(session) = self.store.session_mut(&id) else {
            return false;
        };
        if session.provider.is_some() || session.model.is_some() {
            return false;
        }
        session.provider = Some(pair.provider);
        session.model = Some(pair.model);
        self.save_session(&id);
        true
    }

    /// Whether the reference draws its hero for the active conversation.
    ///
    /// Measured: the hero replaces the transcript while a session exists and
    /// has no turns, so the composer is centred and carries the hero's own
    /// placeholder.
    pub(crate) fn hero_phase(&self) -> bool {
        self.tab == Tab::Chat
            && self
                .active_session()
                .map(|session| session.messages.is_empty())
                .unwrap_or(false)
    }

    /// Create a chat in the active workspace and select it.
    pub(crate) fn new_session(&mut self) -> Option<String> {
        self.settings.ensure_workspace();
        // A session is tagged with the workspace's stable id, which is the key
        // the sidebar and the restore filter both read. Tagging it with the
        // root would make every new session invisible to both.
        let workspace = self
            .settings
            .active_workspace()
            .map(|record| record.id.clone())
            .unwrap_or_default();
        let session = Session::new(
            workspace.as_str(),
            crate::i18n::tr(self.lang, "row.newChat").as_str(),
        );
        let id = session.id.clone();
        let _ = self.store.save(&session);
        self.store.insert(session);
        self.active_id = Some(id.clone());
        Some(id)
    }

    /// Reopen the session the user was last in, else the newest one here.
    ///
    /// The remembered id is a machine preference, so it can name a chat in a
    /// workspace the reader has since left: the restore only accepts it while it
    /// still belongs to the active workspace. Otherwise the newest visible chat
    /// here wins. Several sessions may be running; this only decides which one
    /// the window shows at launch.
    pub(crate) fn restore_session(&mut self) {
        let remembered = self
            .settings
            .global
            .general
            .active_session
            .trim()
            .to_string();
        let archived: Vec<String> = self
            .store
            .archived()
            .into_iter()
            .map(|session| session.id.clone())
            .collect();
        let workspace = self
            .settings
            .active_workspace()
            .map(|record| record.id.clone())
            .unwrap_or_default();
        if !remembered.is_empty()
            && !archived.contains(&remembered)
            && self
                .store
                .session(&remembered)
                .is_some_and(|session| session.workspace == workspace)
        {
            self.select(&remembered);
            return;
        }
        let newest = self
            .store
            .in_workspace(&workspace)
            .into_iter()
            .filter(|session| !archived.contains(&session.id))
            .max_by_key(|session| session.updated_at)
            .map(|session| session.id.clone());
        if let Some(id) = newest {
            self.select(&id);
        }
    }

    /// Reconcile the MCP pool with the servers the settings name.
    ///
    /// Connecting is a process spawn or an HTTP handshake, so it happens when
    /// the configuration changes or at launch, never per frame. Servers whose
    /// configuration is unchanged keep their live connection.
    pub(crate) fn refresh_mcp(&mut self) {
        let servers = self.settings.mcp_servers();
        let extensions = crate::mcp::extension_servers(&self.settings);
        let failed = self.mcp.sync_all(self.runner.rt(), &servers, &extensions);
        if !failed.is_empty() {
            self.notice(crate::i18n::tr_args(
                self.lang,
                "settings.mcpConnectFailed",
                &[("names", failed.join(", "))],
            ));
        }
    }

    /// Show a session in the window.
    ///
    /// This is a view change, not a runtime one: another session's turn keeps
    /// streaming, and the remembered id is a machine preference, so only the
    /// global document is rewritten.
    pub(crate) fn select(&mut self, id: &str) {
        if self.store.session(id).is_some() {
            self.store.rehydrate(id);
            self.active_id = Some(id.to_string());
            self.tab = Tab::Chat;
            self.settings.global.general.active_session = id.to_string();
            let _ = self.settings.save_global();
        }
    }

    /// Move the active chat into another workspace.
    ///
    /// The reference's hero workspace picker reassigns the blank session it
    /// sits under; bsh keeps the same session and moves its owning workspace,
    /// because the chat the editor sits over already exists.
    pub(crate) fn set_workspace(&mut self, workspace: &str) {
        self.settings.global.general.active_workspace = workspace.to_string();
        if let Some(id) = self.active_id.clone() {
            if let Some(session) = self.store.session_mut(&id) {
                session.workspace = workspace.to_string();
            }
            self.save_session(&id);
        }
        let _ = self.settings.save();
        // The workspace document also names MCP servers, so the effective
        // list just changed.
        self.refresh_mcp();
    }

    /// Re-read the workspace registry into the store's root map.
    ///
    /// Called after the registry changes: adding a workspace, deleting one. The
    /// settings tree is the truth; the store's map only says where the files go.
    fn sync_store_roots(&mut self) {
        let pairs: Vec<(String, String)> = self
            .settings
            .global
            .workspaces
            .iter()
            .map(|workspace| (workspace.id.clone(), workspace.root.clone()))
            .collect();
        self.store.set_roots(&pairs);
    }

    /// Register a directory as a workspace, keeping the store in step.
    pub(crate) fn adopt_workspace(&mut self, name: &str, root: &str) -> String {
        let id = self.settings.note_workspace(name, root);
        self.sync_store_roots();
        id
    }

    /// Retitle a workspace.
    ///
    /// A rename is the display name alone: the id is the key every session is
    /// tagged with and the root stays where it is, so nothing moves and no
    /// transcript is touched.
    pub(crate) fn rename_workspace(&mut self, id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let Some(record) = self
            .settings
            .global
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
        else {
            return false;
        };
        if record.name == name {
            return true;
        }
        record.name = name.to_string();
        let _ = self.settings.save();
        true
    }

    /// Delete a workspace from the list.
    ///
    /// The registry entry goes, and its sessions are REHOMED onto the workspace
    /// that becomes active rather than deleted: a row leaving a list must not
    /// take a transcript with it. Deleting the last one leaves none, which the
    /// sidebar and the hero both handle as "no workspace".
    pub(crate) fn delete_workspace(&mut self, id: &str) -> bool {
        // Park the record first, for the same reason a chat is parked first: a
        // delete nobody can undo is a delete nobody dares to use.
        let Some(record) = self
            .settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
        else {
            return false;
        };
        if self
            .stash(
                crate::trash::Kind::Workspace,
                &record.name,
                &record.id,
                &record,
            )
            .is_none()
        {
            return false;
        }
        let before = self.settings.global.workspaces.len();
        self.settings
            .global
            .workspaces
            .retain(|workspace| workspace.id != id);
        if self.settings.global.workspaces.len() == before {
            return false;
        }
        let next = if self.settings.global.general.active_workspace == id {
            self.settings
                .global
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone())
                .unwrap_or_default()
        } else {
            self.settings.global.general.active_workspace.clone()
        };
        self.settings.global.general.active_workspace = next.clone();
        // The store's copy of the registry is a CACHE, so it is re-read here:
        // without it the deleted workspace's chats still read as owned and the
        // orphan sweep below moves nothing.
        self.sync_store_roots();
        if !next.is_empty() {
            // The deleted workspace's sessions are unregistered now, which is
            // exactly the orphan case the store already knows how to move.
            self.store.rehome_orphans(&next);
        }
        let _ = self.settings.save();
        self.refresh_mcp();
        true
    }

    /// Create a chat in the named workspace and select it.
    ///
    /// Different from [`AppState::set_workspace`]: that one MOVES the chat under
    /// the editor, because the hero's picker reassigns the blank session it
    /// sits under. This one leaves every existing chat where it is.
    pub(crate) fn new_session_in(&mut self, workspace: &str) -> Option<String> {
        self.settings.global.general.active_workspace = workspace.to_string();
        self.new_session()
    }

    /// Retitle a session.
    ///
    /// A rename is a title and nothing else: the transcript, the workspace and
    /// the model reference stay put, and `updated_at` does not move - renaming a
    /// conversation must not reorder the reader's list under their hand.
    pub(crate) fn rename_session(&mut self, id: &str, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(session) = self.store.session_mut(id) {
            session.title = title.to_string();
        }
        self.save_session(id);
        self.store.sort();
    }

    /// Delete a chat, parking it in the recycle bin first.
    ///
    /// The parking write comes BEFORE the removal: if the envelope cannot be
    /// written the delete does not happen, because losing the copy is worse than
    /// refusing the delete. The selection then moves to the next chat in the list,
    /// or to none when that was the last one.
    pub(crate) fn delete_session(&mut self, id: &str) -> bool {
        let Some(session) = self.store.session(id).cloned() else {
            return false;
        };
        if self
            .stash(
                crate::trash::Kind::Session,
                &session.title,
                &session.workspace,
                &session,
            )
            .is_none()
        {
            return false;
        }
        if !self.store.remove(id) {
            return false;
        }
        self.plans.remove(id);
        if self.active_id.as_deref() == Some(id) {
            let next = self
                .store
                .sessions
                .first()
                .map(|session| session.id.clone());
            self.active_id = next.clone();
            self.settings.global.general.active_session = next.unwrap_or_default();
            let _ = self.settings.save_global();
        }
        true
    }

    /// Write one item's envelope into the bin.
    fn stash<T: serde::Serialize>(
        &self,
        kind: crate::trash::Kind,
        name: &str,
        workspace: &str,
        payload: &T,
    ) -> Option<std::path::PathBuf> {
        let payload = serde_json::to_value(payload).ok()?;
        let workspace_name = self
            .settings
            .global
            .workspaces
            .iter()
            .find(|record| record.id == workspace)
            .map(|record| record.name.clone())
            .unwrap_or_else(|| workspace.to_string());
        // The id keys both the envelope and the log beside it, so a restore can
        // find the pages without trusting a path recorded earlier.
        let id = payload
            .get("id")
            .and_then(|id| id.as_str())
            .unwrap_or("item")
            .to_string();
        let envelope = crate::trash::Envelope {
            kind: kind.key().to_string(),
            deleted_at: now(),
            workspace: workspace.to_string(),
            workspace_name,
            name: name.to_string(),
            // A chat's transcript lives in Parquet pages beside its document, so
            // the park carries them: the bin holds the whole chat, and restoring
            // does not depend on a directory a purge or a moved workspace could
            // take away.
            log: match kind {
                crate::trash::Kind::Session => format!("log-{id}"),
                crate::trash::Kind::Workspace => String::new(),
            },
            payload,
        };
        let dir = self.settings.trash_dir();
        if kind == crate::trash::Kind::Session {
            let from = crate::history::History::dir_for(&self.store.dir_for_id(&id), &id);
            let _ = crate::trash::park_log(&dir, &from, &envelope);
        }
        crate::trash::stash(&dir, &envelope).ok()
    }

    /// Everything in the bin, newest first.
    pub(crate) fn trash(&self) -> Vec<crate::trash::Entry> {
        crate::trash::entries(&self.settings.trash_dir())
    }

    /// Put a parked item back.
    ///
    /// A chat whose workspace no longer exists lands in the workspace in use - the
    /// alternative is restoring it onto a root nobody has, which is the orphan case
    /// the store already refuses to draw.
    pub(crate) fn restore_trashed(&mut self, path: &std::path::Path) -> bool {
        let Some(entry) = crate::trash::entries(&self.settings.trash_dir())
            .into_iter()
            .find(|entry| entry.path == path)
        else {
            return false;
        };
        match entry.kind() {
            Some(crate::trash::Kind::Session) => {
                let Ok(mut session) =
                    serde_json::from_value::<crate::model::Session>(entry.envelope.payload.clone())
                else {
                    return false;
                };
                let known = self
                    .settings
                    .global
                    .workspaces
                    .iter()
                    .any(|record| record.id == session.workspace);
                if !known {
                    session.workspace = self.settings.global.general.active_workspace.clone();
                }
                let restored = session.id.clone();
                if self.store.save(&session).is_err() {
                    return false;
                }
                self.store.insert(session);
                // The park moved the Parquet pages into the bin, so a restore
                // moves them back before the window is read: a chat comes out of
                // the bin whole, whatever happened to its workspace meanwhile.
                if !entry.envelope.log.is_empty() {
                    let parked = crate::trash::log_dir(&self.settings.trash_dir(), &entry.envelope);
                    let into = crate::history::History::dir_for(
                        &self.store.dir_for_id(&restored),
                        &restored,
                    );
                    let _ =
                        std::fs::create_dir_all(into.parent().unwrap_or(std::path::Path::new(".")));
                    let _ = std::fs::remove_dir_all(&into);
                    let _ = std::fs::rename(&parked, &into);
                }
                self.store.rehydrate(&restored);
                self.store.sort();
            }
            Some(crate::trash::Kind::Workspace) => {
                let Ok(record) = serde_json::from_value::<crate::settings::global::WorkspaceRef>(
                    entry.envelope.payload.clone(),
                ) else {
                    return false;
                };
                if !self
                    .settings
                    .global
                    .workspaces
                    .iter()
                    .any(|existing| existing.id == record.id)
                {
                    self.settings.global.workspaces.push(record);
                }
                self.sync_store_roots();
                let _ = self.settings.save();
            }
            None => return false,
        }
        crate::trash::purge(path);
        true
    }

    /// Delete a parked item for good.
    /// Remove the bsh state folder inside a workspace root.
    ///
    /// Only that one directory: a purge removes bsh own state (annotations, the
    /// terminal policy, the trace index) and never the workspace files. An empty
    /// or unknown root removes nothing.
    fn remove_workspace_state(root: &std::path::Path) {
        if root.as_os_str().is_empty() {
            return;
        }
        let state = root.join(".bsh");
        if state.is_dir() {
            let _ = std::fs::remove_dir_all(state);
        }
    }

    pub(crate) fn purge_trashed(&mut self, path: &std::path::Path) -> bool {
        // A parked session's Parquet log outlives its document: the bin holds the
        // document, the pages never moved. Purging has to take them too, or every
        // deleted chat leaves its transcript behind on disk.
        if let Some(entry) = crate::trash::entries(&self.settings.trash_dir())
            .into_iter()
            .find(|entry| entry.path == path)
        {
            if entry.kind() == Some(crate::trash::Kind::Session) {
                if let Ok(session) =
                    serde_json::from_value::<crate::model::Session>(entry.envelope.payload.clone())
                {
                    // The log lives under the session's WORKSPACE directory, not
                    // the store root. The old path named `root/history/<id>`, a
                    // directory that never existed, so every purge left the
                    // transcript and the trajectory behind on disk.
                    let session_dir = self.store.dir_for_workspace(&session.workspace);
                    let dir = crate::history::History::dir_for(&session_dir, &session.id);
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
            // Removing a workspace also removes the state bsh wrote into it: its
            // `.bsh` folder carries the annotations, the terminal policy and the
            // trace index for that root. The folder itself is never touched - only
            // the one directory bsh owns inside it.
            if entry.kind() == Some(crate::trash::Kind::Workspace) {
                if let Ok(workspace) = serde_json::from_value::<crate::settings::global::WorkspaceRef>(
                    entry.envelope.payload.clone(),
                ) {
                    Self::remove_workspace_state(std::path::Path::new(&workspace.root));
                }
            }
        }
        crate::trash::purge(path)
    }
    /// Branch a session: a copy of its transcript as a new conversation.
    ///
    /// The reference forks at the current point, which is the whole transcript
    /// here - bsh keeps no per-message branch point yet. The copy is a NEW
    /// session (its own id, its own file), laid out as a sibling: pin and
    /// archive do not follow it, because those are decisions about the original.
    pub(crate) fn fork_session(&mut self, id: &str) -> Option<String> {
        let mut forked = self.store.session(id)?.clone();
        forked.id = uuid::Uuid::new_v4().to_string();
        forked.created_at = now();
        forked.updated_at = forked.created_at;
        forked.pinned = false;
        forked.archived_at = None;
        let forked_id = forked.id.clone();
        // The copy carries the whole log, not just the window in memory.
        self.store.copy_history(id, &forked_id);
        let _ = self.store.save(&forked);
        self.store.insert(forked);
        self.store.sort();
        Some(forked_id)
    }

    /// Whether a chat is in the archive.
    pub(crate) fn is_archived(&self, id: &str) -> bool {
        self.store
            .session(id)
            .map(|session| session.archived_at.is_some())
            .unwrap_or(false)
    }

    /// A session's title, for the rename field to start from.
    pub(crate) fn session_title(&self, id: &str) -> Option<String> {
        self.store.session(id).map(|session| session.title.clone())
    }
    /// Fold or unfold a workspace's session list.
    pub(crate) fn toggle_collapse(&mut self, workspace: &str) {
        if !self.collapsed.remove(workspace) {
            self.collapsed.insert(workspace.to_string());
        }
    }

    /// The active workspace's filesystem root: the root the panel browses and
    /// the root the agent's own tools are confined to.
    ///
    /// The reference asks its remote `fileReferences` for the same tree, so the
    /// two views agree by construction. None while no workspace is selected or
    /// its folder has gone; a panel over none says so rather than opening on a
    /// directory it cannot read.
    pub(crate) fn workspace_root(&self) -> Option<std::path::PathBuf> {
        let settings = &self.settings;
        settings
            .global
            .workspaces
            .iter()
            .find(|workspace| workspace.id == settings.global.general.active_workspace)
            .map(|workspace| std::path::PathBuf::from(&workspace.root))
            .filter(|root| root.is_dir())
    }

    /// Whether a session is currently streaming.
    pub(crate) fn is_streaming(&self, id: &str) -> bool {
        self.streams.contains_key(id)
    }

    /// Any session streaming right now.
    /// How long the active turn has been streaming, as the reference shows it.
    pub(crate) fn turn_elapsed(&self) -> Option<String> {
        let seconds = self.turn_started?.elapsed().as_secs();
        Some(if seconds < 60 {
            format!("{seconds}s")
        } else {
            format!("{}m {}s", seconds / 60, seconds % 60)
        })
    }

    pub(crate) fn any_streaming(&self) -> bool {
        !self.streams.is_empty()
    }

    /// Sessions streaming right now other than the one on screen, newest first,
    /// as (id, title). These are the reference's background jobs: turns that
    /// keep running while the reader is looking somewhere else.
    pub(crate) fn background_jobs(&self) -> Vec<(String, String)> {
        let active = self.active_id.clone().unwrap_or_default();
        let mut jobs: Vec<(String, String, u64)> = self
            .streams
            .iter()
            .filter(|(id, _)| **id != active)
            .filter_map(|(id, started)| {
                self.store
                    .session(id)
                    .map(|session| (id.clone(), session.title.clone(), *started))
            })
            .collect();
        jobs.sort_by_key(|job| std::cmp::Reverse(job.2));
        jobs.into_iter().map(|(id, title, _)| (id, title)).collect()
    }

    /// How long a background job has been running, in milliseconds.
    pub(crate) fn job_elapsed_ms(&self, id: &str) -> Option<u64> {
        self.streams
            .get(id)
            .map(|started| now().saturating_sub(*started))
    }

    /// A transient notice line.
    pub(crate) fn notice(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    /// The active transcript as plain text, for the copy control.
    pub(crate) fn transcript_text(&self) -> String {
        let Some(session) = self.active_session() else {
            return String::new();
        };
        session
            .messages
            .iter()
            .map(|message| message.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(
                "

",
            )
    }

    /// Stop the active session's stream.
    pub(crate) fn stop(&mut self, id: &str) {
        self.runner.stop(id);
        self.streams.remove(id);
        let stopped = now_ms();
        if let Some(session) = self.store.session_mut(id) {
            if let Some(message) = session.messages.last_mut() {
                if message.text.is_empty() && message.error.is_none() {
                    message.text = "…".to_string();
                }
                // A stop is terminal too: the turn's measured span ends here.
                settle_ms(message, stopped);
            }
            session.updated_at = now();
        }
        if let Some(session) = self.store.session(id) {
            let _ = self.store.save(session);
        }
    }

    /// Snapshot the history, append the user turn and the assistant placeholder.
    /// The last turn's prompt and completion tokens, when the run reported any.
    pub(crate) fn last_usage(&self) -> Option<(u64, u64)> {
        let session = self.active_session()?;
        let meta = session.messages.last()?.meta.as_ref()?;
        let usage = meta.usage.as_ref()?;
        Some((usage.prompt_tokens, usage.completion_tokens))
    }

    /// Output rate of the streaming turn, in tokens per second.
    ///
    /// Only the turn in flight has a meaningful rate, so an idle session
    /// reports none rather than a figure averaged over the past.
    pub(crate) fn tokens_per_second(&self) -> Option<u64> {
        self.turn_started?;
        let (_, completion) = self.last_usage()?;
        let seconds = self.turn_started?.elapsed().as_secs_f32();
        (seconds >= 1.0 && completion > 0).then(|| (completion as f32 / seconds) as u64)
    }

    /// The model the status bar names: the last turn's, else the configured one.
    ///
    /// A session that switched models reports the model the newest turn ran
    /// against; a session with no usage yet falls back to what the next turn
    /// will be sent to, so the field is never blank while a model is set.
    #[cfg(test)]
    pub(crate) fn status_model(&self) -> Option<String> {
        let from_turn = self
            .active_session()
            .and_then(|session| session.messages.last())
            .and_then(|message| message.meta.as_ref())
            .and_then(|meta| meta.usage.as_ref())
            .map(|usage| usage.model.clone())
            .filter(|model| !model.is_empty());
        from_turn.or_else(|| {
            let configured = self.effective().model;
            (!configured.is_empty()).then_some(configured)
        })
    }

    /// The last turn's prompt size and the context budget behind the meter.
    pub(crate) fn context_figures(&self) -> Option<(u64, u64)> {
        let budget = self.settings.workspace.agent.context_budget as u64;
        let (prompt, _) = self.last_usage()?;
        (budget > 0 && prompt > 0).then_some((prompt, budget))
    }

    /// The last turn's prompt tokens as a share of the context budget.
    ///
    /// The reference rounds this share (`Math.round`) and caps it at 100, so a
    /// 45.4% occupancy reads 45% in the ring and in its panel alike.
    pub(crate) fn context_percent(&self) -> Option<u64> {
        let (prompt, budget) = self.context_figures()?;
        context_share(prompt, budget)
    }

    /// The heuristic composition of the context the next turn would send.
    ///
    /// The reference's `contextBreakdown` prices its retained surface; bsh
    /// prices the same three buckets from the effective system prompt, the tool
    /// schemas of the last built agent and the transcript. `None` while nothing
    /// is priced, which is when the panel falls back to the reference's single
    /// total segment.
    pub(crate) fn context_breakdown(&self) -> Option<crate::model::ContextBreakdown> {
        let session = self.active_session()?;
        let breakdown = crate::model::ContextBreakdown::price(
            self.settings.effective().system_prompt.chars().count(),
            self.tools_schema_chars,
            &session.messages,
        );
        (!breakdown.is_empty()).then_some(breakdown)
    }

    /// The active session's token buckets, when any turn reported usage.
    pub(crate) fn stat_summary(&self) -> Option<crate::model::UsageSummary> {
        let summary = self.active_session()?.usage_summary();
        (!summary.is_empty()).then_some(summary)
    }

    /// Snapshot the active session's history and append the user turn.
    pub(crate) fn begin_turn(&mut self, prompt: &str) -> Option<(String, Vec<ChatMessage>, usize)> {
        let id = self.active_id.clone()?;
        let (transcript, budget) = self.begin_turn_in(&id, prompt)?;
        Some((id, transcript, budget))
    }

    /// Begin a turn in one session, whatever put the message there.
    ///
    /// The scheduler runs a task in the session it was created in and the queue
    /// drains into the session whose turn just ended, so neither is necessarily
    /// the one on screen. The events fold into that session either way, so a
    /// reader who opens it sees the turn. `None` when the session is gone, which
    /// is what tells the caller to switch the task off.
    pub(crate) fn begin_turn_in(
        &mut self,
        session_id: &str,
        prompt: &str,
    ) -> Option<(Vec<ChatMessage>, usize)> {
        let budget = self.settings.effective().context_budget;
        let id = session_id.to_string();
        // One clock for the turn: the trajectory places the user row at the
        // submission and the assistant row at the same instant, so the input
        // lane measures the wait for the reply and the model lane the reply.
        let opened = now_ms();
        let transcript;
        {
            let session = self.store.session_mut(&id)?;
            transcript = session.messages.clone();
            if session.messages.is_empty() {
                session.title = Session::title_from(prompt, self.lang);
            }
            session.messages.push(ChatMessage::user(prompt.to_string()));
            session.messages.push(ChatMessage::assistant());
            for message in session.messages.iter_mut().rev().take(2) {
                message.timing_mut().start = Some(opened);
            }
            session.updated_at = now();
        }
        self.streams.insert(id, now());
        self.turn_started = Some(std::time::Instant::now());
        Some((transcript, budget))
    }

    /// Attach the plan store a turn about to start writes through.
    ///
    /// The store is shared with the `update_plan` tool, so the panel can read
    /// a running turn's plan live instead of waiting for the turn to end.
    pub(crate) fn attach_plan(&mut self, id: &str, store: PlanStore) {
        self.plans.insert(id.to_string(), store);
    }

    /// The plan to render for one session.
    ///
    /// A running turn reads through its live store; otherwise the copy its
    /// last finished turn saved is used, which is what survives a restart.
    pub(crate) fn plan_items(&self, id: &str) -> Vec<PlanItem> {
        self.plans.get(id).map(PlanStore::items).unwrap_or_else(|| {
            self.store
                .session(id)
                .map(|session| session.plan.clone())
                .unwrap_or_default()
        })
    }

    /// Save a finished turn's plan onto its session and drop the live store.
    ///
    /// Also runs for a stream that ends without a terminal event (a stop),
    /// which is why it tolerates a store that is already gone.
    pub(crate) fn finish_plan(&mut self, id: &str) {
        let Some(store) = self.plans.remove(id) else {
            return;
        };
        let items = store.items();
        if let Some(session) = self.store.session_mut(id) {
            session.plan = items;
        }
        self.save_session(id);
    }

    /// Queue a message for a session whose turn is already running.
    ///
    /// Returns the row's identity, which is what its own edit and remove act on.
    pub(crate) fn enqueue(&mut self, id: &str, text: String, attachments: Vec<Attachment>) -> u64 {
        self.inbox_seq += 1;
        let seq = self.inbox_seq;
        self.inbox.entry(id.to_string()).or_default().push(Queued {
            seq,
            text,
            attachments,
        });
        seq
    }

    /// The messages one session is waiting to send, oldest first.
    pub(crate) fn inbox(&self, id: &str) -> &[Queued] {
        self.inbox.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Take the oldest queued message, which is the next turn's own.
    pub(crate) fn pop_inbox(&mut self, id: &str) -> Option<Queued> {
        let queue = self.inbox.get_mut(id)?;
        if queue.is_empty() {
            return None;
        }
        let queued = queue.remove(0);
        let empty = queue.is_empty();
        if empty {
            self.inbox.remove(id);
        }
        Some(queued)
    }

    /// Drop one queued message, which is a row's own remove.
    pub(crate) fn remove_queued(&mut self, id: &str, seq: u64) {
        let empty = match self.inbox.get_mut(id) {
            Some(queue) => {
                queue.retain(|queued| queued.seq != seq);
                queue.is_empty()
            }
            None => false,
        };
        if empty {
            self.inbox.remove(id);
        }
    }

    /// Replace one queued message's text, which is a row's own save.
    ///
    /// Only the text changes, which is also the reference's rule: a row that
    /// carries an image is not editable in place.
    pub(crate) fn edit_queued(&mut self, id: &str, seq: u64, text: String) {
        if let Some(queue) = self.inbox.get_mut(id) {
            if let Some(queued) = queue.iter_mut().find(|queued| queued.seq == seq) {
                queued.text = text;
            }
        }
    }

    /// Fold one runner event into the transcript.
    pub(crate) fn apply_event(&mut self, id: &str, event: AgentEvent) {
        if let AgentEvent::Compacted {
            before,
            after,
            dropped,
        } = &event
        {
            self.notice(format!(
                "Context compacted: {before} -> {after} tokens, {dropped} messages dropped"
            ));
        }
        let terminal = matches!(event, AgentEvent::Done | AgentEvent::Error(_));
        // One clock for the event: it stamps a call's start, the turn's first
        // output, and - on the terminal event - the turn's settled span.
        let at = now_ms();
        // Read before the session borrow: the usage that lands belongs to the
        // model the turn was sent to, which is what the status bar names. The
        // session is resolved by id, because a turn may finish in a session the
        // reader is not looking at.
        let model = self.effective_in(id).model;
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            match event {
                AgentEvent::Text(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.text.push_str(&text);
                        note_first_output(message, at);
                    }
                }
                AgentEvent::Reasoning(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.reasoning.push_str(&text);
                        note_first_output(message, at);
                    }
                }
                AgentEvent::ToolCall { name, args } => {
                    if let Some(message) = session.messages.last_mut() {
                        message.tools.push(ToolEvent {
                            name,
                            args,
                            output: None,
                            ms: None,
                            at: Some(at),
                        });
                        note_first_output(message, at);
                    }
                }
                AgentEvent::ToolResult { name, output, ms } => {
                    if let Some(message) = session.messages.last_mut() {
                        if let Some(tool) = message
                            .tools
                            .iter_mut()
                            .rev()
                            .find(|tool| tool.name == name && tool.output.is_none())
                        {
                            tool.output = Some(output);
                            tool.ms = Some(ms);
                        }
                    }
                }
                AgentEvent::Usage {
                    prompt,
                    completion,
                    cached,
                } => {
                    if let Some(message) = session.messages.last_mut() {
                        let meta = message.meta.get_or_insert_with(MessageMeta::default);
                        let usage = meta.usage.get_or_insert_with(MessageUsage::default);
                        usage.prompt_tokens += prompt;
                        usage.completion_tokens += completion;
                        usage.cached_tokens += cached;
                        usage.model = model.clone();
                    }
                }
                AgentEvent::Error(error) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.error = Some(error);
                    }
                }
                AgentEvent::Compacted { .. } | AgentEvent::Done => {}
            }
            if terminal {
                session.updated_at = now();
                if let Some(message) = session.messages.last_mut() {
                    settle_ms(message, at);
                }
            }
        }
        if terminal {
            self.streams.remove(id);
            self.turn_started = None;
            if let Some(session) = self.store.session(id) {
                let _ = self.store.save(session);
            }
        }
    }

    /// Persist one session.
    pub(crate) fn save_session(&self, id: &str) {
        if let Some(session) = self.store.session(id) {
            let _ = self.store.save(session);
        }
    }

    /// Return an archived session to the sidebar.
    pub(crate) fn unarchive_session(&mut self, id: &str) {
        if let Some(session) = self.store.session_mut(id) {
            session.archived_at = None;
            session.updated_at = now();
        }
        self.save_session(id);
        self.store.sort();
        self.active_id = Some(id.to_string());
    }

    /// Archive a session so it leaves the sidebar without being destroyed.
    pub(crate) fn archive_session(&mut self, id: &str) {
        self.runner.stop(id);
        self.streams.remove(id);
        if let Some(session) = self.store.session_mut(id) {
            session.archived_at = Some(now());
            session.updated_at = now();
        }
        self.save_session(id);
        if self.active_id.as_deref() == Some(id) {
            self.active_id = None;
            self.ensure_active();
        }
    }

    /// Pin or unpin a session.
    pub(crate) fn toggle_pin(&mut self, id: &str) {
        if let Some(session) = self.store.session_mut(id) {
            session.pinned = !session.pinned;
        }
        self.store.sort();
        self.save_session(id);
    }

    /// Record a good/bad verdict on one assistant message.
    pub(crate) fn set_feedback(&mut self, id: &str, index: usize, value: &str) {
        if let Some(session) = self.store.session_mut(id) {
            if let Some(message) = session.messages.get_mut(index) {
                let meta = message.meta.get_or_insert_with(MessageMeta::default);
                meta.feedback = if meta.feedback.as_deref() == Some(value) {
                    None
                } else {
                    Some(value.to_string())
                };
            }
        }
        self.save_session(id);
    }

    /// Delete one message and everything after it.
    pub(crate) fn truncate_from(&mut self, id: &str, index: usize) {
        if let Some(session) = self.store.session_mut(id) {
            session.messages.truncate(index);
            session.updated_at = now();
        }
        self.save_session(id);
    }

    /// Toggle one tool row open or closed.
    pub(crate) fn toggle_tool(&mut self, message: usize, tool: usize) {
        let key = format!("{message}:{tool}");
        if !self.expanded_tools.remove(&key) {
            self.expanded_tools.insert(key);
        }
    }

    /// Whether a tool row is open.
    /// The window of older chat rows waiting to be read, if the log has one.
    ///
    /// Building this is index-only; the rows are decoded by the shell on the
    /// background executor, so the render path never touches the files.
    pub(crate) fn begin_older_read(&self) -> Option<crate::store::OlderRead> {
        let id = self.active_id.as_deref()?;
        self.store.older_read(id, crate::store::WINDOW)
    }

    /// Apply one decoded window above the loaded chat and bound the window.
    ///
    /// Returns how many rows arrived; zero means the page was stale or empty.
    pub(crate) fn finish_older_read(
        &mut self,
        page: crate::store::OlderPage,
        keep_windows: usize,
    ) -> usize {
        let id = page.session.clone();
        let added = self.store.apply_older(page);
        if added > 0 {
            self.store
                .trim_front(&id, crate::store::WINDOW * keep_windows);
        }
        added
    }

    /// The trajectory page waiting to be read, if the log has one.
    pub(crate) fn begin_trace_page(
        &self,
        id: &str,
        page: usize,
    ) -> Option<crate::store::TraceRead> {
        self.store.trace_read(id, page)
    }

    pub(crate) fn tool_open(&self, message: usize, tool: usize) -> bool {
        self.expanded_tools.contains(&format!("{message}:{tool}"))
    }

    /// Open the usage pill's dialog, or close it when it is already open.
    pub(crate) fn toggle_stat(&mut self) {
        self.stat_open = !self.stat_open;
    }

    /// Open the context meter's panel, or close it when it is already open.
    pub(crate) fn toggle_meter(&mut self) {
        self.meter_open = !self.meter_open;
    }

    /// Record the serialized length of the tool schemas a built agent carries.
    pub(crate) fn set_tools_schema(&mut self, chars: usize) {
        self.tools_schema_chars = chars;
    }

    /// Open the settings dialog on a copy of the saved settings.
    ///
    /// Nothing the dialog edits reaches the agent until it is committed, which
    /// is what Cancel undoes: the copy is simply dropped.
    pub(crate) fn open_settings(&mut self) {
        self.draft = Some(self.settings.clone());
        self.settings_section = 0;
        self.overlay = Overlay::Settings;
    }

    /// Discard the draft and close the dialog.
    pub(crate) fn cancel_settings(&mut self) {
        self.draft = None;
        self.overlay = Overlay::None;
    }

    /// Keep the draft, persist it, and close the dialog.
    pub(crate) fn commit_settings(&mut self) -> std::io::Result<()> {
        if let Some(draft) = self.draft.take() {
            self.settings = draft;
        }
        self.settings.ensure_workspace();
        match self.settings.save() {
            Ok(()) => {
                self.overlay = Overlay::None;
                // The MCP pane is config, and config is only real once the
                // pool reflects it: reconcile now rather than at the next turn.
                self.refresh_mcp();
                Ok(())
            }
            Err(error) => {
                // A refused write keeps the dialog open, with the draft the
                // reader had, because the refusal is something they can fix in
                // it - closing the pane over a notice would take the only place
                // the fix can be made away with it.
                self.draft = Some(self.settings.clone());
                Err(error)
            }
        }
    }

    /// The settings being edited, falling back to the saved ones.
    pub(crate) fn draft(&self) -> &Settings {
        self.draft.as_ref().unwrap_or(&self.settings)
    }

    /// The settings being edited, creating the draft on first use.
    pub(crate) fn draft_mut(&mut self) -> &mut Settings {
        if self.draft.is_none() {
            self.draft = Some(self.settings.clone());
        }
        match self.draft.as_mut() {
            Some(draft) => draft,
            None => unreachable!("the draft was just created"),
        }
    }

    /// Switch one tool on or off in the draft.
    pub(crate) fn toggle_tool_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .tools
            .iter()
            .position(|tool| tool == name)
        {
            Some(index) => {
                draft.workspace.disabled.tools.remove(index);
            }
            None => draft.workspace.disabled.tools.push(name.to_string()),
        }
    }

    /// Make one model the ACTIVE SESSION's choice.
    ///
    /// Session-scoped on purpose: the pair is stored on the session document,
    /// so a model chosen in one chat leaves every sibling alone and leaves
    /// `settings.workspace` untouched. A new session is not given a copy of
    /// this choice: it starts with no model and asks for one.
    ///
    /// A (provider, nickname) pair is stored whole or not at all. A half-made
    /// pair would resolve to nothing, so a blank half is refused here rather
    /// than written and reported later as an unknown name.
    pub(crate) fn select_model_from(&mut self, provider: &str, nickname: &str) {
        let (provider, nickname) = (provider.trim(), nickname.trim());
        if provider.is_empty() || nickname.is_empty() {
            return;
        }
        let Some(id) = self.active_id.clone() else {
            return;
        };
        if let Some(session) = self.store.session_mut(&id) {
            session.provider = Some(provider.to_string());
            session.model = Some(nickname.to_string());
        }
        // Saved like any other session mutation. `updated_at` deliberately does
        // not move: choosing a model is metadata, and a choice must not reorder
        // the reader's list under their hand.
        self.save_session(&id);
    }

    /// Choose the active session's model by nickname under the provider it
    /// already names, or the first provider that carries the nickname.
    ///
    /// The composer always passes the pair explicitly, so this is a test
    /// convenience: it looks in the saved tree AND the draft the settings pane
    /// edits, because a pane test seeds a provider in the draft.
    #[cfg(test)]
    pub(crate) fn select_model(&mut self, nickname: &str) {
        let chosen = self
            .effective()
            .model_ref
            .map(|reference| reference.provider);
        let find =
            |providers: &std::collections::BTreeMap<String, crate::settings::global::Provider>| {
                providers
                    .iter()
                    .find(|(_, provider)| provider.models.contains_key(nickname))
                    .map(|(id, _)| id.clone())
            };
        let provider = chosen
            .or_else(|| find(&self.settings.global.providers))
            .or_else(|| find(&self.draft().global.providers));
        if let Some(provider) = provider {
            self.select_model_from(&provider, nickname);
        }
    }

    /// Set the reasoning effort the composer's menu offers.
    ///
    /// `None` hands the choice back to the model's own declared effort; the
    /// literal `off` asks for NO effort on the wire (the reference's "Off turns
    /// reasoning off and needs no value"), which `caps::build_agent` maps to an
    /// absent field rather than the string.
    pub(crate) fn set_reasoning_effort(&mut self, level: Option<&str>) {
        let value = level
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(str::to_string);
        self.settings.workspace.agent.reasoning_effort = value.clone();
        if let Some(draft) = self.draft.as_mut() {
            draft.workspace.agent.reasoning_effort = value;
        }
        let _ = self.settings.save();
    }
    /// Save a provider into the global tree, or say why it cannot be saved.
    ///
    /// A refused call changes nothing, so pressing create with an id the tree
    /// already holds no longer quietly rewrites the provider behind it.
    pub(crate) fn add_custom_provider(
        &mut self,
        id: &str,
        name: &str,
        base_url: &str,
        models: &[crate::settings::global::ModelEntry],
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        if id.trim().is_empty() {
            return Err(crate::settings::provider::ProviderRefusal::NameEmpty);
        }
        if self.draft_mut().global.providers.contains_key(id) {
            return Err(crate::settings::provider::ProviderRefusal::IdTaken);
        }
        if base_url.trim().is_empty() {
            return Err(crate::settings::provider::ProviderRefusal::NeedsBaseUrl);
        }
        if let Some(refusal) = crate::settings::provider::base_url_refusal(base_url) {
            return Err(refusal);
        }
        if models.is_empty() {
            return Err(crate::settings::provider::ProviderRefusal::NeedsModel);
        }
        self.draft_mut()
            .global
            .add_custom_provider(id, name, base_url, models);
        Ok(())
    }

    /// Add a shipped family provider, taking its own endpoint and protocol.
    ///
    /// The catalogue route of the add card lands here: an id the tree already
    /// holds is refused, because the id is the registry key the engine
    /// registers the vendor under and two of them cannot share it.
    pub(crate) fn add_shipped_provider(
        &mut self,
        id: &str,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        if self.draft_mut().global.providers.contains_key(id) {
            return Err(crate::settings::provider::ProviderRefusal::IdTaken);
        }
        let Some((_, base_url, _)) = crate::settings::global::built_in(id) else {
            return Err(crate::settings::provider::ProviderRefusal::NeedsModel);
        };
        self.draft_mut().global.add_provider(id, "", base_url, &[]);
        Ok(())
    }

    /// Switch one input type on one model leaf of a provider.
    pub(crate) fn toggle_model_input(&mut self, provider: &str, key: &str, kind: &str) {
        self.draft_mut().global.set_input_type(provider, key, kind);
        self.provider_refusal = None;
    }
    /// Edit the connection and the model leaves of a provider already in the
    /// tree.
    pub(crate) fn set_provider_connection(&mut self, id: &str, base_url: &str) {
        self.draft_mut().global.set_connection(id, base_url);
    }

    /// Name the vault entry a provider reads its API key from.
    ///
    /// This writes the DRAFT like every other provider field, so the choice is
    /// committed by the dialog's own Save and dropped by its Cancel.
    pub(crate) fn set_provider_api_key_vault(&mut self, id: &str, entry: Option<&str>) {
        self.draft_mut().global.set_api_key_vault(id, entry);
    }

    /// Rename a provider. The id is the wire prefix and never changes; the
    /// display name is the pane's to edit.
    pub(crate) fn set_provider_name(&mut self, id: &str, name: &str) {
        self.draft_mut().global.set_provider_name(id, name);
    }

    /// Add one model leaf under a provider, or say why it is refused.
    pub(crate) fn add_model(
        &mut self,
        provider: &str,
        nickname: &str,
        display: &str,
        model: &str,
        api_mode: &str,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        self.draft_mut()
            .global
            .add_model(provider, nickname, display, model, api_mode)
    }

    /// Replace one model leaf in place.
    pub(crate) fn update_model(
        &mut self,
        provider: &str,
        nickname: &str,
        entry: &crate::settings::global::ModelEntry,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        self.draft_mut()
            .global
            .update_model(provider, nickname, entry)
    }

    /// Drop one model leaf; the connection is left alone.
    pub(crate) fn remove_model(&mut self, provider: &str, nickname: &str) -> bool {
        self.draft_mut().global.remove_model(provider, nickname)
    }

    /// Load a provider into the form and put the pane into edit mode.
    pub(crate) fn begin_provider_edit(&mut self, id: &str) {
        self.editing_provider = Some(id.to_string());
        self.provider_refusal = None;
        self.removing_provider = None;
    }

    /// Drop a provider from the global tree.
    ///
    /// Sessions that chose a model under it keep the reference and report it as
    /// unavailable at send, rather than being quietly repointed at another
    /// provider: the reader who chose it is the one who can correct it.
    pub(crate) fn remove_provider(&mut self, name: &str) {
        self.draft_mut().global.remove_provider(name);
        self.removing_provider = None;
    }

    /// Ask for confirmation before dropping a provider.
    pub(crate) fn begin_provider_remove(&mut self, name: &str) {
        self.removing_provider = Some(name.to_string());
    }

    /// Dismiss a pending delete confirmation.
    pub(crate) fn cancel_provider_remove(&mut self) {
        self.removing_provider = None;
    }

    /// Turn concurrent tool dispatch for one assistant turn on or off.
    pub(crate) fn set_parallel_tool_calls(&mut self, parallel: bool) {
        self.draft_mut().workspace.agent.parallel_tool_calls = parallel;
    }

    /// Make one preset the default a session takes when it names none.
    pub(crate) fn select_preset(&mut self, name: &str) {
        self.draft_mut().global.default_preset = name.to_string();
    }

    /// Switch one plugin on or off in the draft.
    pub(crate) fn toggle_plugin_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .extensions
            .iter()
            .position(|plugin| plugin == name)
        {
            Some(index) => {
                draft.workspace.disabled.extensions.remove(index);
            }
            None => draft.workspace.disabled.extensions.push(name.to_string()),
        }
    }

    /// The trust decisions recorded this run, oldest first.
    ///
    /// The pane shows the state, not the log; this is the test-facing copy of
    /// the ring. Bounded by [`TRUST_AUDIT_LIMIT`].
    #[cfg(test)]
    pub(crate) fn trust_audit(&self) -> Vec<TrustDecision> {
        self.trust_audit.iter().cloned().collect()
    }

    /// Append one trust decision, trimming the oldest so the ring stays bounded.
    fn record_trust(
        &mut self,
        allow: bool,
        id: &str,
        path: &str,
        launches: &[crate::mcp::McpLaunch],
    ) {
        while self.trust_audit.len() >= TRUST_AUDIT_LIMIT {
            self.trust_audit.pop_front();
        }
        self.trust_audit.push_back(TrustDecision {
            extension_id: id.to_string(),
            path: path.to_string(),
            commands: launches
                .iter()
                .map(|launch| launch.command.clone())
                .collect(),
            at: now(),
            allow,
        });
    }

    /// The declared MCP launches of one installed package, with its stored path.
    ///
    /// `None` when the package is not installed, has no path, or declares no
    /// MCP server - the three cases where a trust decision has nothing to
    /// approve. Env values are absent from the launches by construction.
    fn declared_launches(&self, id: &str) -> Option<(String, Vec<crate::mcp::McpLaunch>)> {
        let record = self
            .draft()
            .global
            .extensions
            .iter()
            .find(|record| record.id == id)?;
        let path = record
            .path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())?
            .to_string();
        let root = record.path_root()?;
        let launches = crate::mcp::declared_servers(&root, &record.id)
            .iter()
            .map(crate::mcp::ExtensionMcp::launch)
            .collect::<Vec<_>>();
        (!launches.is_empty()).then_some((path, launches))
    }

    /// Approve one installed package's declared commands, enable it and persist.
    ///
    /// This is the handler the pane's Trust and enable button calls: the store
    /// marks the package trusted AT THE PATH it is installed at, the workspace
    /// disabled list drops it, and the decision is written immediately - a
    /// package's commands are concrete state on disk, not a preference waiting
    /// for the dialog's Save.
    ///
    /// The audit row is appended BEFORE the pool reconciles, so the approval is
    /// recorded even if the child then fails to start. If the record cannot be
    /// saved, the in-memory flag is reverted and no pool reconcile runs: a
    /// decision that did not reach disk must not start a process for this
    /// session either.
    pub(crate) fn trust_extension(&mut self, id: &str) -> bool {
        let Some((path, launches)) = self.declared_launches(id) else {
            return false;
        };
        let was_disabled = self
            .draft()
            .workspace
            .disabled
            .extensions
            .iter()
            .any(|disabled| disabled == id);
        {
            let draft = self.draft_mut();
            let Some(record) = draft
                .global
                .extensions
                .iter_mut()
                .find(|record| record.id == id)
            else {
                return false;
            };
            record.trusted = true;
            record.trusted_path = Some(path.clone());
            draft
                .workspace
                .disabled
                .extensions
                .retain(|disabled| disabled != id);
        }
        self.record_trust(true, id, &path, &launches);
        if let Err(error) = self.persist_extension_state() {
            log::warn!("trust decision for extension '{id}' was not saved: {error}");
            let draft = self.draft_mut();
            if let Some(record) = draft
                .global
                .extensions
                .iter_mut()
                .find(|record| record.id == id)
            {
                record.trusted = false;
                record.trusted_path = None;
            }
            if was_disabled
                && !draft
                    .workspace
                    .disabled
                    .extensions
                    .iter()
                    .any(|disabled| disabled == id)
            {
                draft.workspace.disabled.extensions.push(id.to_string());
            }
            return false;
        }
        true
    }

    /// Record that the reader DECLINED one package's declared commands.
    ///
    /// Nothing is written to the setting: the package stays untrusted, which is
    /// already the state. The audit ring keeps the refusal, because a gate that
    /// only records approvals cannot answer "was this package ever offered?".
    pub(crate) fn decline_trust(&mut self, id: &str) -> bool {
        let Some((path, launches)) = self.declared_launches(id) else {
            return false;
        };
        self.record_trust(false, id, &path, &launches);
        true
    }

    /// Register one market plugin in the draft extension inventory.
    ///
    /// Ownership: the MARKET row is what installs, so the record carries the
    /// metadata the row already read (name, display name, description, version,
    /// standard, source label, install target) plus the package directory and
    /// whether that directory is BOS's managed copy. The path is authoritative:
    /// the pane re-reads the manifest from it, so a stale cached field can never
    /// make a package loadable or not.
    ///
    /// Installing is a draft write, like every other pane edit, but the caller
    /// persists it immediately through `persist_extension_state` - an installed
    /// package is concrete state already on disk, not a preference waiting for
    /// Save. A package whose id is already installed is left alone, so a
    /// repeated press cannot create a duplicate the settings validation would
    /// reject on Save.
    pub(crate) fn install_extension(
        &mut self,
        plugin: &crate::market::MarketPlugin,
        path: &std::path::Path,
        managed: bool,
        install_target: &str,
    ) -> bool {
        let draft = self.draft_mut();
        if draft
            .global
            .extensions
            .iter()
            .any(|extension| extension.id == plugin.id)
        {
            return false;
        }
        draft
            .global
            .extensions
            .push(crate::settings::global::InstalledExtension {
                id: plugin.id.clone(),
                // The manifest's package name is the id for both standards;
                // the display name is the market row's own title.
                name: plugin.id.clone(),
                display_name: plugin.name.clone(),
                description: plugin.description.clone(),
                version: plugin.version.clone(),
                standard: crate::settings::global::InstalledStandard::from(plugin.standard),
                source_label: plugin.source_label.clone(),
                install_target: install_target.to_string(),
                path: Some(path.to_string_lossy().to_string()),
                managed,
                installed_at: crate::settings::unix_now(),
                // A fresh package starts untrusted: its declared commands (MCP
                // servers) do not run until the reader approves the directory.
                trusted: false,
                trusted_path: None,
            });
        draft
            .workspace
            .disabled
            .extensions
            .retain(|disabled| disabled != &plugin.id);
        true
    }

    /// Add one remote market index URL to the draft.
    ///
    /// The URL is validated and normalized first; a duplicate or malformed URL
    /// is refused with a sentence the pane can show. Save remains the commit.
    pub(crate) fn add_market_index(&mut self, raw: &str) -> Result<String, String> {
        let url = crate::market::validate_index_url(raw)?;
        let draft = self.draft_mut();
        if draft
            .global
            .market_indexes
            .iter()
            .any(|existing| existing == &url)
        {
            return Err("that market index is already configured".to_string());
        }
        draft.global.market_indexes.push(url.clone());
        Ok(url)
    }

    /// Remove one remote market index URL from the draft.
    pub(crate) fn remove_market_index(&mut self, url: &str) -> bool {
        let draft = self.draft_mut();
        let before = draft.global.market_indexes.len();
        draft
            .global
            .market_indexes
            .retain(|existing| existing != url);
        draft.global.market_indexes.len() != before
    }

    /// Remove one installed market plugin from the draft inventory.
    ///
    /// Only an entry that carries a package path can be uninstalled: the
    /// built-in bundles have no directory to remove, so a stray call cannot
    /// hide one from the pane. The workspace's disabled entry goes with it.
    ///
    /// Ownership: a MANAGED package (one the market downloaded into
    /// `~/.bos/bsh/extensions`) has its directory deleted; a local package
    /// registered in place is the reader's own directory and is never deleted.
    /// `remove_managed` also refuses a path outside the managed root, so a
    /// hand-edited record cannot turn `managed = true` into an arbitrary
    /// directory delete.
    pub(crate) fn uninstall_extension(&mut self, id: &str) -> bool {
        self.uninstall_extension_in(&crate::settings::dir().join("extensions"), id)
    }

    /// Uninstall against an explicit managed root, for tests and for a
    /// sandboxed home. The production path supplies `~/.bos/bsh/extensions`.
    pub(crate) fn uninstall_extension_in(
        &mut self,
        managed_root: &std::path::Path,
        id: &str,
    ) -> bool {
        let draft = self.draft_mut();
        let Some(index) = draft
            .global
            .extensions
            .iter()
            .position(|extension| extension.id == id && extension.path.is_some())
        else {
            return false;
        };
        let record = draft.global.extensions.remove(index);
        draft
            .workspace
            .disabled
            .extensions
            .retain(|disabled| disabled != id);
        if record.managed {
            if let Some(raw) = record.path {
                let path = crate::caps::expand_path(&raw);
                crate::market::remove_managed(managed_root, &path);
            }
        }
        true
    }

    /// Commit the extension inventory and its per-workspace disabled list
    /// without committing unrelated draft settings.
    ///
    /// Installing, uninstalling, enabling and disabling are concrete package
    /// actions, not preferences the reader expects to have to Save: the files
    /// are already on disk, and a restart must find the same inventory. Only
    /// these two fields are copied out of the draft; a provider or theme edit
    /// still waits for the dialog's Save.
    pub(crate) fn persist_extension_state(&mut self) -> std::io::Result<()> {
        let mut next = self.settings.clone();
        next.global.extensions = self.draft().global.extensions.clone();
        next.workspace.disabled.extensions = self.draft().workspace.disabled.extensions.clone();
        next.save()?;
        self.settings = next;
        // The pool is configuration-derived: a newly enabled MCP plugin must
        // connect now, and a disabled one must drop its child before the next
        // turn. Centralized here so every persistence caller gets the same pool.
        self.refresh_mcp();
        Ok(())
    }

    /// Switch one skill on or off in the draft.
    pub(crate) fn toggle_skill_off(&mut self, name: &str) {
        let draft = self.draft_mut();
        match draft
            .workspace
            .disabled
            .skills
            .iter()
            .position(|skill| skill == name)
        {
            Some(index) => {
                draft.workspace.disabled.skills.remove(index);
            }
            None => draft.workspace.disabled.skills.push(name.to_string()),
        }
    }
}

/// The reference's display-ready cache-hit share, as text.
///
/// Its `formatCacheHitPercent` rounds to whole percent with ties up, but never
/// lets a partial hit read as a full one: when rounding would reach 100 it
/// widens the precision until the miss shows, so 199 of 200 reads `99.5`. No
/// prompt input at all reads as unknown rather than zero.
pub(crate) fn cache_hit_percent(cache_read: u64, prompt: u64) -> Option<String> {
    if prompt == 0 {
        return None;
    }
    let missed = prompt.saturating_sub(cache_read);
    if missed == 0 {
        return Some("100".to_string());
    }
    let units = (cache_read as u128 * 200 + prompt as u128) / (prompt as u128 * 2);
    if units < 100 {
        return Some(units.to_string());
    }
    // A partial hit whose whole percent rounds to 100: walk the precision out
    // until the remaining miss shows, the way the reference does.
    let mut places = 1usize;
    let mut gap = missed as u128 * 200;
    let tens = (prompt / 10) as u128;
    while gap <= tens {
        gap *= 10;
        places += 1;
    }
    let ones = (prompt % 10) as u128;
    let mut loss = 5u64;
    for candidate in 1..5u64 {
        let factor = candidate as u128 * 2 + 1;
        if gap <= factor * tens + factor * ones / 10 {
            loss = candidate;
            break;
        }
    }
    Some(format!("99.{}{}", "9".repeat(places - 1), 10 - loss))
}

/// A token count with the reference's own digit grouping: `17,734`.
pub(crate) fn exact_tokens(tokens: u64) -> String {
    let digits = tokens.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Compact token counts the way the reference's own `formatTokens` reads them:
/// `947`, `12.3K`, `518K`, `1.2M`.
///
/// Under a thousand it prints the count; above that it scales to K or M, where
/// a scaled value of 100 or more rounds to a whole number and a smaller one
/// keeps a single decimal. The reference localizes the two suffix keys, but its
/// own doc comment pins the shipped values to K and M.
pub(crate) fn human_tokens(tokens: u64) -> String {
    fn scaled(value: u64, scale: u64) -> String {
        let whole = value / scale;
        let remainder = value % scale;
        if whole >= 100 {
            return (whole + u64::from(remainder * 2 >= scale)).to_string();
        }
        let tenths = whole * 10 + (remainder * 10 + scale / 2) / scale;
        if tenths.is_multiple_of(10) {
            (tenths / 10).to_string()
        } else {
            format!("{}.{}", tenths / 10, tenths % 10)
        }
    }
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{}K", scaled(tokens, 1_000))
    } else {
        format!("{}M", scaled(tokens, 1_000_000))
    }
}

/// The reference's own rounding of an occupancy share, capped at 100.
///
/// Its `contextPressure` computes `Math.min(100, Math.round(used / window *
/// 100))`, so a 45.4% occupancy reads 45% in the ring and in the panel alike. A
/// missing budget or an unmeasured prompt reads as unknown rather than zero.
pub(crate) fn context_share(prompt: u64, budget: u64) -> Option<u64> {
    (budget > 0 && prompt > 0)
        .then(|| ((prompt as f64 / budget as f64) * 100.0).round().min(100.0) as u64)
}

/// Whether a session title matches a case-insensitive substring query.
///
/// An empty or whitespace-only query matches everything, so the filter stays
/// inert while the search field is empty.
pub(crate) fn matches(query: &str, title: &str) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty() || title.to_lowercase().contains(&needle)
}

/// One image attached to the next user turn.
///
/// bsh stores the payload already base64-encoded, because that is the form
/// both providers want and the file is read once, at attach time.
#[derive(Clone)]
pub(crate) struct Attachment {
    /// The file name, shown on the composer chip and in the transcript.
    pub(crate) name: String,
    /// The MIME type declared to the provider.
    pub(crate) mime: String,
    /// The base64 payload.
    pub(crate) base64: String,
}

impl Attachment {
    /// Read an image file into an attachment.
    ///
    /// Returns None for anything that is not one of the image types bsh sends,
    /// so the caller can fall back to mentioning the path as text.
    pub(crate) fn from_path(path: &std::path::Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        let mime = match extension.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => return None,
        };
        let bytes = std::fs::read(path).ok()?;
        Some(Self {
            name: path.file_name()?.to_string_lossy().to_string(),
            mime: mime.to_string(),
            base64: base64_encode(&bytes),
        })
    }
}

/// Encode bytes as standard base64 with padding.
///
/// Hand-rolled rather than pulled from a crate: bsh has one call site and the
/// workspace ships no base64 dependency, the same trade the UTC stamp made.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Build the content of one user turn from its text and its attachments.
///
/// A turn with no attachment stays plain text. One with attachments becomes the
/// multimodal form both vendors already map onto their own image blocks.
pub(crate) fn turn_input(text: &str, attachments: &[Attachment]) -> react::llm::Content {
    if attachments.is_empty() {
        return react::llm::Content::Text(text.to_string());
    }
    let mut parts = vec![react::llm::ContentPart::Text {
        text: text.to_string(),
    }];
    for attachment in attachments {
        if let react::llm::Content::Parts(mut image) =
            react::llm::Content::binary(attachment.mime.clone(), attachment.base64.clone())
        {
            parts.append(&mut image);
        }
    }
    react::llm::Content::Parts(parts)
}

/// The text a turn shows in the transcript.
///
/// gpui-kit has no image view here, so an attachment is named rather than drawn;
/// this is bsh own presentation and is recorded as such.
pub(crate) fn display_text(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let names: Vec<String> = attachments
        .iter()
        .map(|a| format!("[image: {}]", a.name))
        .collect();
    format!("{}{}{}", text, "\n", names.join(" "))
}

/// The `(workspace id, root)` pairs the session store scatters sessions across.
///
/// The store is rooted per workspace, so a session file lives beside the
/// workspace it belongs to; the registry is what turns an id into a path.
fn session_roots(settings: &Settings) -> Vec<(String, String)> {
    settings
        .global
        .workspaces
        .iter()
        .map(|workspace| (workspace.id.clone(), workspace.root.clone()))
        .collect()
}
#[cfg(test)]
mod tests {
    fn image_fixture() -> super::Attachment {
        super::Attachment {
            name: "shot.png".to_string(),
            mime: "image/png".to_string(),
            base64: "Zg==".to_string(),
        }
    }

    /// The occupancy share rounds like the reference's own `Math.round` and
    /// caps at 100.
    #[test]
    fn the_context_share_rounds_like_the_reference() {
        assert_eq!(super::context_share(454, 1_000), Some(45));
        assert_eq!(super::context_share(455, 1_000), Some(46));
        assert_eq!(super::context_share(999, 1_000), Some(100));
        // A prompt past its budget still reads full, never 101%.
        assert_eq!(super::context_share(50_000, 1_000), Some(100));
        assert_eq!(super::context_share(0, 1_000), None);
        assert_eq!(super::context_share(10, 0), None);
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(super::base64_encode(b""), "");
        assert_eq!(super::base64_encode(b"f"), "Zg==");
        assert_eq!(super::base64_encode(b"fo"), "Zm8=");
        assert_eq!(super::base64_encode(b"foo"), "Zm9v");
        assert_eq!(super::base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(super::base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(super::base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn only_image_extensions_become_attachments() {
        let dir = std::env::temp_dir();
        let note = dir.join("bsh-attach-note.txt");
        std::fs::write(&note, b"x").ok();
        assert!(super::Attachment::from_path(&note).is_none());
        assert!(super::Attachment::from_path(&dir.join("missing.png")).is_none());
    }

    #[test]
    fn a_turn_without_attachments_stays_plain_text() {
        assert_eq!(
            super::turn_input("hello", &[]),
            react::llm::Content::Text("hello".to_string())
        );
        assert_eq!(super::display_text("hello", &[]), "hello");
    }

    #[test]
    fn a_turn_with_an_attachment_becomes_multimodal() {
        let attachment = image_fixture();
        let content = super::turn_input("look", std::slice::from_ref(&attachment));
        let react::llm::Content::Parts(parts) = content else {
            panic!("an attached turn must be multimodal");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], react::llm::ContentPart::Text { text } if text == "look"));
        match &parts[1] {
            react::llm::ContentPart::Binary { binary } => {
                assert_eq!(binary.content_type, "image/png");
                assert_eq!(binary.url(), "data:image/png;base64,Zg==");
            }
            other => panic!("expected an image part, got {other:?}"),
        }
    }

    #[test]
    fn the_transcript_names_an_attachment() {
        let attachment = image_fixture();
        assert_eq!(
            super::display_text("look", std::slice::from_ref(&attachment)),
            "look\n[image: shot.png]"
        );
    }
    use super::{cache_hit_percent, exact_tokens, human_tokens, matches, stamp};

    #[test]
    fn an_empty_query_matches_every_title() {
        assert!(matches("", "anything"));
        assert!(matches("   ", "anything"));
    }

    #[test]
    fn the_match_ignores_case_and_trims_the_query() {
        assert!(matches("  FIX ", "fix the parser"));
        assert!(matches("parser", "Fix the Parser"));
        assert!(!matches("parser", "fix the lexer"));
    }

    /// The status bar names the model the newest usage ran against, and falls
    /// back to the configured model before any usage has landed.
    #[test]
    fn the_status_model_follows_the_usage_then_the_setting() {
        let mut state = super::AppState::with_store(crate::store::Store::open_at(
            "target/bsh-status-home".into(),
        ));
        // The tree owns the model: the workspace records which one it uses, and
        // the status bar reads the leaf the reference resolves to.
        let mut global = state.settings.global.clone();
        global.add_custom_provider(
            "local",
            "Local",
            "https://local.example/v1",
            &[
                crate::settings::global::ModelEntry::named("first-model"),
                crate::settings::global::ModelEntry::named("second-model"),
            ],
        );
        state.settings.global = global;
        state.select_model_from("local", "first-model");
        assert_eq!(
            state.status_model().as_deref(),
            Some("first-model"),
            "with no turn yet the session's chosen model is the honest answer"
        );

        let (id, _, _) = state.begin_turn("hello").expect("a turn");
        state.apply_event(
            &id,
            super::AgentEvent::Usage {
                prompt: 5,
                completion: 7,
                cached: 0,
            },
        );
        assert_eq!(state.status_model().as_deref(), Some("first-model"));

        state.select_model_from("local", "second-model");
        state.apply_event(
            &id,
            super::AgentEvent::Usage {
                prompt: 1,
                completion: 1,
                cached: 0,
            },
        );
        assert_eq!(
            state.status_model().as_deref(),
            Some("second-model"),
            "the model rides with the usage, not with the current setting"
        );
    }

    /// A session with no model anywhere names none, rather than an empty chip.
    #[test]
    fn the_status_model_is_absent_when_nothing_names_one() {
        let state = super::AppState::with_store(crate::store::Store::open_at(
            "target/bsh-status-empty".into(),
        ));
        // Nothing was chosen in this session, so there is no configured model.
        assert_eq!(state.status_model(), None);
    }

    /// A turn stamps both of its rows, notes the first output, and closes its
    /// span on the terminal event - which is what the trajectory reads.
    #[test]
    fn a_turn_records_its_measured_span() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("bsh-turn-timing");
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        let id = state.active_id.clone().expect("a session");
        state.begin_turn_in(&id, "hello").expect("a turn opens");

        // Back-date the turn so the measured span is a known, positive number
        // rather than whatever the two calls happened to land on.
        let opened = super::now_ms().saturating_sub(400);
        if let Some(session) = state.store.session_mut(&id) {
            for message in session.messages.iter_mut().rev().take(2) {
                message.timing_mut().start = Some(opened);
            }
        }
        state.apply_event(&id, super::AgentEvent::Text("hi".to_string()));
        state.apply_event(&id, super::AgentEvent::Done);

        let session = state.store.session(&id).expect("the session");
        let (user, assistant) = (&session.messages[0], &session.messages[1]);
        assert_eq!(
            user.timing().and_then(|timing| timing.start),
            Some(opened),
            "the submission is stamped on the user row"
        );
        assert_eq!(
            assistant.timing().and_then(|timing| timing.start),
            Some(opened),
            "and on the reply row, so both measure from the turn's opening"
        );
        let first = assistant
            .timing()
            .and_then(|timing| timing.first_output)
            .expect("the first output is noted");
        assert!(first >= opened, "it lands after the turn opened");
        let ms = assistant
            .timing()
            .and_then(|timing| timing.ms)
            .expect("the terminal event closes the span");
        assert!(ms >= 400, "the span covers the back-dated clock: {ms}");
        assert_eq!(
            user.timing().and_then(|timing| timing.ms),
            None,
            "the user row keeps no span of its own; the view derives its wait"
        );
    }

    #[test]
    fn token_counts_switch_units_at_a_thousand() {
        assert_eq!(human_tokens(0), "0");
        assert_eq!(human_tokens(947), "947");
        assert_eq!(human_tokens(12_345), "12.3K");
        assert_eq!(human_tokens(1_000), "1K");
        assert_eq!(human_tokens(517_500), "518K", "a scaled 100+ rounds");
        assert_eq!(human_tokens(69_200_000), "69.2M");
    }

    #[test]
    fn exact_counts_group_in_threes() {
        assert_eq!(exact_tokens(0), "0");
        assert_eq!(exact_tokens(947), "947");
        assert_eq!(exact_tokens(17_734), "17,734");
        assert_eq!(exact_tokens(1_234_567), "1,234,567");
    }

    #[test]
    fn the_cache_hit_never_rounds_a_partial_hit_to_full() {
        assert_eq!(cache_hit_percent(67, 100).as_deref(), Some("67"));
        assert_eq!(
            cache_hit_percent(2, 3).as_deref(),
            Some("67"),
            "ties round up"
        );
        assert_eq!(cache_hit_percent(199, 200).as_deref(), Some("99.5"));
        assert_eq!(cache_hit_percent(999, 1000).as_deref(), Some("99.9"));
        assert_eq!(cache_hit_percent(200, 200).as_deref(), Some("100"));
        assert_eq!(cache_hit_percent(0, 0), None);
        assert_eq!(cache_hit_percent(0, 100).as_deref(), Some("0"));
    }

    #[test]
    fn a_stamp_reads_as_a_utc_date_and_time() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(86_399), "1970-01-01 23:59");
        assert_eq!(stamp(86_400), "1970-01-02 00:00");
        assert_eq!(stamp(1_000_000_000), "2001-09-09 01:46");
        assert_eq!(stamp(1_752_000_000), "2025-07-08 18:40");
    }
    #[test]
    fn a_blank_first_prompt_keeps_the_localized_default_title() {
        let zh = crate::i18n::Lang::from_locale("zh");
        assert_eq!(
            crate::model::Session::title_from("", zh),
            crate::i18n::tr(zh, "row.newChat")
        );
        assert_eq!(crate::model::Session::title_from("hello", zh), "hello");
    }

    /// Register the provider the model tests choose from.
    fn seed_models(state: &mut super::AppState) {
        state.settings.global.add_custom_provider(
            "p",
            "P",
            "https://p.example/v1",
            &[
                crate::settings::global::ModelEntry::named("alpha"),
                crate::settings::global::ModelEntry::named("beta"),
                crate::settings::global::ModelEntry::named("gamma"),
            ],
        );
    }

    /// A state over a scratch directory that can be reopened, with the provider
    /// registered. No session carries a model until one chooses.
    fn state_with_models(label: &str) -> super::AppState {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-models-{label}"));
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        seed_models(&mut state);
        state
    }

    /// Two chats in ONE workspace choose different models, and the choice is a
    /// session document fact: reopening the store finds both.
    #[test]
    fn two_sessions_in_one_workspace_keep_their_own_models() {
        let mut state = state_with_models("two");
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        assert!(
            state.effective_in(&first).model_ref.is_none(),
            "a new session starts with no model"
        );
        assert!(
            state.effective_in(&second).model_ref.is_none(),
            "and so does a second one: the choice is not copied eagerly"
        );

        state.select(&first);
        state.select_model_from("p", "beta");
        state.select(&second);
        state.select_model_from("p", "gamma");

        // Switching sessions switches what the chip and the next turn see.
        let chip = |state: &super::AppState| {
            crate::ui::composer::model_label(&state.effective().model_name, crate::i18n::Lang::En)
        };
        assert_eq!(state.effective().model, "gamma");
        assert_eq!(
            chip(&state),
            "gamma",
            "the chip names the active session's model"
        );
        state.select(&first);
        assert_eq!(state.effective().model, "beta");
        assert_eq!(chip(&state), "beta", "and switching chats moves it");
        state.select(&second);
        assert_eq!(state.effective().model, "gamma");

        // The same two ids read back from disk.
        let dir = state.store.root().to_path_buf();
        let mut reopened = super::AppState::with_store(crate::store::Store::open_at(dir));
        seed_models(&mut reopened);
        assert_eq!(reopened.effective_in(&first).model, "beta");
        assert_eq!(reopened.effective_in(&second).model, "gamma");
    }

    /// Several chats in one workspace can be streaming at once; which one the
    /// window shows is a view choice, and it is the only thing recorded.
    #[test]
    fn sessions_run_concurrently_while_the_window_shows_one() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("bsh-concurrent-sessions");
        let _ = std::fs::remove_dir_all(&dir);
        let mut state = super::AppState::with_store(crate::store::Store::open_at(dir));
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        state.begin_turn_in(&first, "one").expect("turn one");
        state.begin_turn_in(&second, "two").expect("turn two");
        assert!(
            state.is_streaming(&first) && state.is_streaming(&second),
            "two turns run at the same time"
        );

        state.select(&first);

        assert!(
            state.is_streaming(&second),
            "showing a chat must not stop the turn running in another one"
        );
        assert_eq!(state.background_jobs().len(), 1, "the other one is a job");
        assert_eq!(
            state.settings.global.general.active_session, first,
            "the preference follows the view, not the stream"
        );
        assert!(
            state
                .settings
                .workspace
                .general
                .legacy_active_session
                .is_empty(),
            "the workspace document holds no session identity"
        );
        // The view preference is persisted, in the machine document only.
        let root = state.settings.save_root.clone().expect("a scratch root");
        let global = std::fs::read_to_string(root.join("settings.toml"))
            .expect("the global settings document");
        assert!(
            global.contains(&format!("active_session = \"{first}\"")),
            "the visible session is remembered across a relaunch: {global}"
        );
        assert!(
            !root.join(".bsh/settings/settings.toml").exists(),
            "selecting a chat does not write a workspace document at all"
        );
    }

    /// A chat that never chose has NO model: the chip falls back to its own
    /// "Select model" copy and the send gate refuses it.
    #[test]
    fn a_session_without_a_choice_has_no_model_and_is_refused() {
        let mut state = state_with_models("unchosen");
        let chosen = state.active_id.clone().expect("the chosen session");
        state.select_model_from("p", "beta");
        let unchosen = state.new_session().expect("the unchosen session");
        assert_eq!(state.effective_in(&chosen).model, "beta");
        assert!(
            state.effective_in(&unchosen).model_ref.is_none(),
            "the sibling never inherits its neighbour's model"
        );
        assert!(state.effective_in(&unchosen).model.is_empty());
        assert!(
            state.effective_in(&unchosen).model_name.is_empty(),
            "there is no name for the chip to show"
        );
        assert_eq!(
            crate::ui::composer::model_label(
                &state.effective_in(&unchosen).model_name,
                crate::i18n::Lang::En,
            ),
            crate::i18n::tr(crate::i18n::Lang::En, "composer.selectModel"),
            "so the chip shows the reference's own select-model copy"
        );
        assert_eq!(
            state.settings.refusal_for(&state.session_choice(&unchosen)),
            Some("composer.chooseModel"),
            "the send gate asks for a model before opening a turn"
        );
    }

    /// Choosing a model writes the ACTIVE session and nothing else.
    #[test]
    fn choosing_a_model_touches_only_its_session() {
        let mut state = state_with_models("isolated");
        let first = state.active_id.clone().expect("the first session");
        let second = state.new_session().expect("the second session");
        state.select(&first);
        let workspace_before = state.settings.workspace.clone();
        let sibling_before = state
            .store
            .session(&second)
            .map(|session| (session.provider.clone(), session.model.clone()));

        state.select_model_from("p", "beta");

        assert_eq!(
            state.settings.workspace, workspace_before,
            "the workspace document is untouched"
        );
        let sibling_after = state
            .store
            .session(&second)
            .map(|session| (session.provider.clone(), session.model.clone()));
        assert_eq!(
            sibling_before, sibling_after,
            "the sibling session is untouched"
        );
        let chosen = state.store.session(&first).expect("the chosen session");
        assert_eq!(chosen.provider.as_deref(), Some("p"));
        assert_eq!(chosen.model.as_deref(), Some("beta"));
    }

    /// A workspace file written before the model was session-scoped still names
    /// one. It is handed to the open session ONCE, and never to a later one.
    #[test]
    fn a_legacy_workspace_pair_seeds_the_open_session_once() {
        let mut state = state_with_models("legacy");
        let first = state.active_id.clone().expect("the open session");
        state.settings.workspace.general.legacy_provider = "p".to_string();
        state.settings.workspace.general.legacy_model = "beta".to_string();
        assert!(state.adopt_legacy_model(), "the old pair is adopted");
        assert_eq!(state.effective_in(&first).model, "beta");
        assert!(
            state.settings.workspace.general.legacy_pair().is_none(),
            "the pair is consumed, so it cannot be read twice"
        );

        let second = state.new_session().expect("a later session");
        assert!(
            !state.adopt_legacy_model(),
            "there is nothing left to adopt"
        );
        assert!(
            state.effective_in(&second).model_ref.is_none(),
            "a later session does not pick the retired pair up"
        );

        // A session that already chose is never overwritten.
        state.select(&first);
        state.settings.workspace.general.legacy_provider = "p".to_string();
        state.settings.workspace.general.legacy_model = "gamma".to_string();
        assert!(!state.adopt_legacy_model(), "an existing choice wins");
        assert_eq!(state.effective_in(&first).model, "beta");
    }

    /// Uninstall drops the inventory record in both cases, but deletes a
    /// directory ONLY for a managed copy. A local package is the user's own
    /// directory and is left exactly where it was.
    #[test]
    fn uninstall_deletes_only_a_managed_directory() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(format!("bsh-uninstall-{}", uuid::Uuid::new_v4()));
        let managed_root = base.join("managed");
        let managed = managed_root
            .join("plugin-a")
            .join("node_modules")
            .join("plugin-a");
        let local = base.join("local-plugin");
        std::fs::create_dir_all(&managed).expect("the managed fixture");
        std::fs::create_dir_all(&local).expect("the local fixture");

        let mut state =
            super::AppState::with_store(crate::store::Store::open_at(base.join("store")));
        let record = |id: &str, path: &std::path::Path, managed: bool| {
            crate::settings::global::InstalledExtension {
                id: id.to_string(),
                path: Some(path.to_string_lossy().to_string()),
                managed,
                ..Default::default()
            }
        };
        state.settings.global.extensions = vec![
            record("plugin-a", &managed, true),
            record("local-plugin", &local, false),
        ];
        state.settings.workspace.disabled.extensions =
            vec!["plugin-a".to_string(), "local-plugin".to_string()];

        assert!(state.uninstall_extension_in(&managed_root, "local-plugin"));
        assert!(
            local.is_dir(),
            "a local package is never deleted by an uninstall"
        );
        assert!(
            !state
                .draft()
                .workspace
                .disabled
                .extensions
                .contains(&"local-plugin".to_string()),
            "its disabled entry goes with the record"
        );

        assert!(state.uninstall_extension_in(&managed_root, "plugin-a"));
        assert!(
            !managed_root.join("plugin-a").exists(),
            "the managed directory is removed"
        );
        assert!(
            state.draft().global.extensions.is_empty(),
            "both records are gone"
        );
        assert!(
            !state
                .draft()
                .workspace
                .disabled
                .extensions
                .iter()
                .any(|id| id == "plugin-a"),
            "and both disabled entries with them"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
