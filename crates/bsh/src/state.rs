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
        }
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
    }

    /// The selected session.
    pub(crate) fn active_session(&self) -> Option<&Session> {
        self.active_id
            .as_ref()
            .and_then(|id| self.store.session(id))
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

    /// Select a session by id.
    /// Reopen the session the user was last in, else the newest one here.
    pub(crate) fn restore_session(&mut self) {
        let remembered = self
            .settings
            .workspace
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
        if !remembered.is_empty()
            && !archived.contains(&remembered)
            && self.store.session(&remembered).is_some()
        {
            self.select(&remembered);
            return;
        }
        let workspace = self
            .settings
            .active_workspace()
            .map(|record| record.id.clone())
            .unwrap_or_default();
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

    pub(crate) fn select(&mut self, id: &str) {
        if self.store.session(id).is_some() {
            self.active_id = Some(id.to_string());
            self.tab = Tab::Chat;
            self.settings.workspace.general.active_session = id.to_string();
            let _ = self.settings.save();
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
    }

    /// Fold or unfold a workspace's session list.
    pub(crate) fn toggle_collapse(&mut self, workspace: &str) {
        if !self.collapsed.remove(workspace) {
            self.collapsed.insert(workspace.to_string());
        }
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
        if let Some(session) = self.store.session_mut(id) {
            if let Some(message) = session.messages.last_mut() {
                if message.text.is_empty() && message.error.is_none() {
                    message.text = "…".to_string();
                }
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
    pub(crate) fn status_model(&self) -> Option<String> {
        let from_turn = self
            .active_session()
            .and_then(|session| session.messages.last())
            .and_then(|message| message.meta.as_ref())
            .and_then(|meta| meta.usage.as_ref())
            .map(|usage| usage.model.clone())
            .filter(|model| !model.is_empty());
        from_turn.or_else(|| {
            let configured = self.settings.effective().model;
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
        let transcript;
        {
            let session = self.store.session_mut(&id)?;
            transcript = session.messages.clone();
            if session.messages.is_empty() {
                session.title = Session::title_from(prompt, self.lang);
            }
            session.messages.push(ChatMessage::user(prompt.to_string()));
            session.messages.push(ChatMessage::assistant());
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
        // Read before the session borrow: the usage that lands belongs to the
        // model the turn was sent to, which is what the status bar names.
        let model = self.settings.effective().model;
        {
            let Some(session) = self.store.session_mut(id) else {
                return;
            };
            match event {
                AgentEvent::Text(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.text.push_str(&text);
                    }
                }
                AgentEvent::Reasoning(text) => {
                    if let Some(message) = session.messages.last_mut() {
                        message.reasoning.push_str(&text);
                    }
                }
                AgentEvent::ToolCall { name, args } => {
                    if let Some(message) = session.messages.last_mut() {
                        message.tools.push(ToolEvent {
                            name,
                            args,
                            output: None,
                            ms: None,
                        });
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

    /// Make one provider the endpoint the agent is built against.
    ///
    /// The workspace records which of the global providers it uses; it never
    /// carries a second copy of the endpoint, so changing it here cannot drop
    /// the credential.
    pub(crate) fn select_provider(&mut self, name: &str) {
        // The two halves move together. The workspace file refuses a selection
        // that names a provider and no model, so writing the provider alone left
        // the document uncommittable: creating a fresh provider made the NEXT
        // save fail with "names only half a model", about a model the reader had
        // never been asked for.
        //
        // A provider with no models yet leaves no selection at all, which is a
        // state the file accepts and the composer already explains.
        let first = self
            .draft()
            .global
            .providers
            .get(name)
            .and_then(|provider| provider.models.keys().next().cloned());
        // A provider with no models cannot be selected at all: the pair must be
        // both or neither, so there is nothing to point the other half at. The
        // selection is left where it was rather than emptied, so creating a
        // provider does not silently drop the model the reader was using.
        if let Some(model) = first {
            let draft = self.draft_mut();
            draft.workspace.general.provider = name.to_string();
            draft.workspace.general.model = model;
        }
    }

    /// Make one model the workspace's active choice, by nickname.
    ///
    /// The reference's model menu writes the same reference the workspace
    /// resolves against: a provider id and the nickname it is filed under,
    /// never a bare model string, so a rename does not orphan the choice.
    pub(crate) fn select_model(&mut self, nickname: &str) {
        self.draft_mut().workspace.general.model = nickname.to_string();
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
        api_key: &str,
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
            .add_custom_provider(id, name, base_url, api_key, models);
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
        api_key: &str,
    ) -> Result<(), crate::settings::provider::ProviderRefusal> {
        if self.draft_mut().global.providers.contains_key(id) {
            return Err(crate::settings::provider::ProviderRefusal::IdTaken);
        }
        let Some((_, base_url, _)) = crate::settings::global::built_in(id) else {
            return Err(crate::settings::provider::ProviderRefusal::NeedsModel);
        };
        self.draft_mut()
            .global
            .add_provider(id, "", base_url, api_key, &[]);
        Ok(())
    }

    /// Switch one input type on one model leaf of a provider.
    pub(crate) fn toggle_model_input(&mut self, provider: &str, key: &str, kind: &str) {
        self.draft_mut().global.set_input_type(provider, key, kind);
        self.provider_refusal = None;
    }
    /// Edit the connection and the model leaves of a provider already in the
    /// tree.
    pub(crate) fn set_provider_connection(&mut self, id: &str, base_url: &str, api_key: &str) {
        self.draft_mut()
            .global
            .set_connection(id, base_url, api_key);
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

    /// Drop a provider from the global tree, clearing the selection when the
    /// workspace had chosen it.
    pub(crate) fn remove_provider(&mut self, name: &str) {
        let draft = self.draft_mut();
        if draft.global.remove_provider(name) && draft.workspace.general.provider == name {
            draft.workspace.general.provider.clear();
        }
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
            .extentions
            .iter()
            .position(|plugin| plugin == name)
        {
            Some(index) => {
                draft.workspace.disabled.extentions.remove(index);
            }
            None => draft.workspace.disabled.extentions.push(name.to_string()),
        }
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
            "key",
            &[
                crate::settings::global::ModelEntry::named("first-model"),
                crate::settings::global::ModelEntry::named("second-model"),
            ],
        );
        state.settings.global = global;
        state.settings.workspace.general.provider = "local".to_string();
        state.settings.workspace.general.model = "first-model".to_string();
        assert_eq!(
            state.status_model().as_deref(),
            Some("first-model"),
            "with no turn yet the configured model is the honest answer"
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

        state.settings.workspace.general.model = "second-model".to_string();
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
        let mut state = super::AppState::with_store(crate::store::Store::open_at(
            "target/bsh-status-empty".into(),
        ));
        state.settings.workspace.general.model = String::new();
        assert_eq!(state.status_model(), None);
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
}
