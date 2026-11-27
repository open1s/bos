//! The Host's session repository: the desktop's own session store.
//!
//! A session is one document plus an append-only Parquet transcript, written by
//! [crate::store::Store] and [crate::history] - the same two modules the
//! desktop's shell uses. The Host does not keep a second session format: a chat
//! a terminal creates is a chat the window lists, and a chat the window wrote is
//! one a terminal opens.
//!
//! The runtime's own identity types and its read model are mapped here, at the
//! two boundaries where a stored session becomes a [SessionProjection] and
//! where a projection from a Host that predates this repository is imported
//! once:
//!
//! ```text
//! model::Session  <->  contract::SessionProjection
//!       ^                        ^
//!       |                        |
//!  Store + history          the Host's journal
//! ```
//!
//! The journal stays the durable record of what a *runtime* did - executions,
//! approvals, tool facts, goals as commands. The session document is the
//! durable record of what a *chat* is. Facts the journal folds are written
//! through here as well, so a client of either interface reads one
//! conversation.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};

use crate::history::History;
use crate::model::{
    ChatMessage, CompactionRecord, Goal, MessageMeta, MessageTiming, MessageUsage, Role, Session,
    ToolEvent, TurnPart,
};
use crate::runtime::contract::{
    CompactionOutcomeView, GoalState, MessageRole, MessageView, PlanItemStatus, PlanItemView,
    SessionConfig, SessionGoalView, SessionProjection, ToolCallStatus, ToolCallView, TurnUsage,
};
use crate::runtime::ids::{MessageId, SessionId};
use crate::store::Store;

/// Why a session could not be written.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionRepoError {
    /// The store refused the document or its transcript.
    #[error("session store: {0}")]
    Store(String),
    /// The session named does not exist in the store.
    #[error("session {0} is not in the workspace store")]
    Missing(String),
}

/// The desktop's session store, seen through the runtime's workspace.
///
/// One repository serves one workspace. The store roots itself at
/// <workspace>/.bsh/sessions, which is where the window keeps the same
/// workspace's chats, so both interfaces read one directory.
pub(crate) struct SessionRepo {
    store: Store,
    /// The workspace id a new session is tagged with. The desktop's registry
    /// minted it; the Host only carries it, because a session tagged with a
    /// derived id would be invisible to the window's workspace filter.
    workspace: String,
    /// The workspace's display name, recorded on a bin entry.
    workspace_name: String,
    /// The recycle bin the desktop parks deleted chats in.
    trash_dir: PathBuf,
}

impl SessionRepo {
    /// Open the store for one workspace.
    ///
    /// The quarantine is where a session with no known workspace lands; a Host
    /// serves exactly one workspace, so it is used only for a row whose tag is
    /// empty.
    pub(crate) fn open(
        workspace: impl Into<String>,
        workspace_name: impl Into<String>,
        root: &Path,
        quarantine: PathBuf,
        trash_dir: PathBuf,
        secrets: Vec<String>,
    ) -> Self {
        let workspace = workspace.into();
        let store = Store::open_over(
            &[(workspace.clone(), root.display().to_string())],
            quarantine,
        )
        .with_secrets(secrets);
        Self {
            store,
            workspace,
            workspace_name: workspace_name.into(),
            trash_dir,
        }
    }

    /// The workspace id every session this repository creates is tagged with.
    pub(crate) fn workspace(&self) -> &str {
        &self.workspace
    }

    /// The bin the desktop parks chats in.
    pub(crate) fn trash_dir(&self) -> &Path {
        &self.trash_dir
    }

    /// Every session this workspace holds, newest first.
    ///
    /// Rows start metadata-only; [Self::hydrate] reads one's document and
    /// transcript window when the Host is about to open or mutate it.
    pub(crate) fn sessions(&self) -> Vec<&Session> {
        self.store
            .sessions
            .iter()
            .filter(|session| session.workspace == self.workspace)
            .collect()
    }

    /// One session, metadata-only unless it was hydrated.
    pub(crate) fn session(&self, id: &str) -> Option<&Session> {
        self.store.session(id)
    }

    /// One session with its document and transcript window loaded.
    pub(crate) fn hydrate(&mut self, id: &str) -> Option<&Session> {
        self.store.ensure_document(id);
        self.store.rehydrate(id);
        self.store.session(id)
    }

    /// Read a session's fields without keeping a borrow, for a command that
    /// has to append an event while it works.
    pub(crate) fn snapshot_of(&mut self, id: &str) -> Option<Session> {
        self.hydrate(id).cloned()
    }

    /// Adopt a session another writer produced (a journal imported once) and
    /// persist it.
    pub(crate) fn adopt(&mut self, session: Session) -> Result<(), SessionRepoError> {
        self.save(&session)?;
        self.store.insert(session);
        self.store.sort();
        Ok(())
    }

    /// Branch one session: a new chat opening with the source's whole log.
    ///
    /// The transcript pages are copied first, so a fork carries the rows ABOVE
    /// the loaded window too; the save then only has to write the document.
    pub(crate) fn fork(&mut self, id: &str) -> Option<Session> {
        let mut forked = self.snapshot_of(id)?;
        let source = forked.id.clone();
        forked.id = uuid::Uuid::new_v4().to_string();
        forked.created_at = crate::runtime::clock::now();
        forked.updated_at = forked.created_at;
        forked.pinned = false;
        forked.archived_at = None;
        self.store.copy_history(&source, &forked.id);
        let _ = self.store.save(&forked);
        self.store.insert(forked.clone());
        self.store.sort();
        Some(forked)
    }

    /// Put back a parked chat addressed by identity, as a client does.
    pub(crate) fn restore_by_id(&mut self, id: &str) -> Result<Session, SessionRepoError> {
        let path = self
            .trash()
            .into_iter()
            .find(|entry| {
                entry
                    .envelope
                    .payload
                    .get("id")
                    .and_then(|value| value.as_str())
                    == Some(id)
            })
            .map(|entry| entry.path)
            .ok_or_else(|| SessionRepoError::Missing(id.to_string()))?;
        self.restore(&path)
    }

    /// Delete a parked chat for good, addressed by identity.
    pub(crate) fn purge_by_id(&self, id: &str) -> bool {
        let path = self
            .trash()
            .into_iter()
            .find(|entry| {
                entry
                    .envelope
                    .payload
                    .get("id")
                    .and_then(|value| value.as_str())
                    == Some(id)
            })
            .map(|entry| entry.path);
        match path {
            Some(path) => self.purge(&path),
            None => false,
        }
    }

    /// Create a session document and persist it.
    ///
    /// The new chat is tagged with this repository's workspace, because that
    /// is the tag the window's list filters on.
    pub(crate) fn create(&mut self, title: &str, config: &SessionConfig) -> Session {
        let mut session = Session::new(&self.workspace, title);
        apply_config(&mut session, config);
        let _ = self.store.save(&session);
        self.store.insert(session.clone());
        self.store.sort();
        session
    }

    /// Persist one session as it now stands.
    pub(crate) fn save(&self, session: &Session) -> Result<(), SessionRepoError> {
        self.store
            .save(session)
            .map_err(|error| SessionRepoError::Store(error.to_string()))
    }

    /// Persist a session document without rewriting its transcript.
    pub(crate) fn save_document(&self, session: &Session) -> Result<(), SessionRepoError> {
        self.store
            .save_document(session)
            .map_err(|error| SessionRepoError::Store(error.to_string()))
    }

    /// Write the rows a running turn has not committed to the log yet.
    pub(crate) fn checkpoint(&self, session: &Session, base: usize) {
        if let Err(error) = self.store.checkpoint(session, base) {
            log::warn!(
                "session {}: the in-flight turn could not be checkpointed: {error}",
                session.id
            );
        }
    }

    /// Every parked chat in the workspace's bin, newest first.
    pub(crate) fn trash(&self) -> Vec<crate::trash::Entry> {
        crate::trash::entries(&self.trash_dir)
            .into_iter()
            .filter(|entry| {
                entry.envelope.kind == "session" && entry.envelope.workspace == self.workspace
            })
            .collect()
    }

    /// Park one session: move its document and transcript pages into the bin.
    ///
    /// This is the desktop's delete, not a visibility flag: a parked chat is
    /// out of the store, and its transcript travels with it so a restore
    /// returns the whole conversation.
    pub(crate) fn park(&mut self, id: &str) -> Result<(), SessionRepoError> {
        let Some(session) = self.snapshot_of(id) else {
            return Err(SessionRepoError::Missing(id.to_string()));
        };
        let dir = self.store.dir_for_id(id);
        let log_from = History::dir_for(&dir, id);
        crate::trash::park_session(&session, &self.workspace_name, &self.trash_dir, &log_from)
            .map_err(|error| SessionRepoError::Store(error.to_string()))?;
        self.store.remove(id);
        Ok(())
    }

    /// Put one parked chat back and return the restored session.
    pub(crate) fn restore(&mut self, path: &Path) -> Result<Session, SessionRepoError> {
        let Some(entry) = crate::trash::entries(&self.trash_dir)
            .into_iter()
            .find(|entry| entry.path == path)
        else {
            return Err(SessionRepoError::Missing(path.display().to_string()));
        };
        let mut session: Session = serde_json::from_value(entry.envelope.payload.clone())
            .map_err(|error| SessionRepoError::Store(error.to_string()))?;
        if session.workspace.trim().is_empty() {
            session.workspace = self.workspace.clone();
        }
        let restored = session.id.clone();
        if let Err(error) = self.store.save(&session) {
            return Err(SessionRepoError::Store(error.to_string()));
        }
        self.store.insert(session.clone());
        // The park moved the pages into the bin; a restore moves them back
        // before the transcript is read, so the chat comes out whole.
        if !entry.envelope.log.is_empty() {
            let parked = crate::trash::log_dir(&self.trash_dir, &entry.envelope);
            let into = History::dir_for(&self.store.dir_for_id(&restored), &restored);
            if let Some(parent) = into.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_dir_all(&into);
            let _ = std::fs::rename(&parked, &into);
        }
        self.store.rehydrate(&restored);
        self.store.sort();
        crate::trash::purge(path);
        Ok(session)
    }

    /// Delete one parked chat for good.
    pub(crate) fn purge(&self, path: &Path) -> bool {
        let Some(entry) = crate::trash::entries(&self.trash_dir)
            .into_iter()
            .find(|entry| entry.path == path)
        else {
            return false;
        };
        let session_dir = serde_json::from_value::<Session>(entry.envelope.payload.clone())
            .ok()
            .map(|session| self.store.dir_for_workspace(&session.workspace));
        crate::trash::purge_entry(&entry, session_dir.as_deref());
        true
    }

    /// Append the reader's turn and an empty assistant row, then persist both.
    ///
    /// The pair is written before the model is asked anything, the way the
    /// desktop writes its prompt: a crash mid-turn leaves the question on disk,
    /// and the empty answer is the row the finished turn fills in. Returns the
    /// global index the turn's mutable tail starts at, for checkpoints.
    pub(crate) fn begin_turn(&mut self, id: &str, text: &str) -> Result<usize, SessionRepoError> {
        // The window must be in memory before a row is appended: a metadata-only
        // row that grew two messages would look like a truncation of the
        // transcript above it, and the save refuses to write it.
        self.store.rehydrate(id);
        let Some(session) = self.store.session_mut(id) else {
            return Err(SessionRepoError::Missing(id.to_string()));
        };
        let base = session.loaded_from + session.messages.len();
        session.messages.push(ChatMessage::user(text));
        session.messages.push(ChatMessage::assistant());
        session.updated_at = crate::runtime::clock::now();
        let session = session.clone();
        self.save(&session)?;
        Ok(base)
    }

    /// Record one tool call on the turn's answer row.
    pub(crate) fn note_tool_call(
        &mut self,
        id: &str,
        index: usize,
        name: &str,
        args: &str,
        at: u64,
    ) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        let Some(row) = session.messages.get_mut(index) else {
            return;
        };
        row.tools.push(ToolEvent {
            name: name.to_string(),
            args: args.to_string(),
            output: None,
            ms: None,
            at: Some(at),
        });
        row.note_tool();
    }

    /// Fill in one tool call's result on the turn's answer row.
    pub(crate) fn note_tool_result(
        &mut self,
        id: &str,
        index: usize,
        name: &str,
        output: &str,
        ms: u64,
    ) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        let Some(row) = session.messages.get_mut(index) else {
            return;
        };
        // The newest call with this name that has no result yet is the pair
        // the engine just answered; a parallel batch may interleave names, so
        // the pairing is by name and "open", never by position.
        if let Some(call) = row
            .tools
            .iter_mut()
            .rev()
            .find(|call| call.name == name && call.output.is_none())
        {
            call.output = Some(output.to_string());
            call.ms = Some(ms);
        }
    }

    /// Close the turn's answer row with its text and usage.
    pub(crate) fn finish_turn(
        &mut self,
        id: &str,
        index: usize,
        text: &str,
        usage: Option<&TurnUsage>,
    ) -> Result<(), SessionRepoError> {
        let Some(session) = self.store.session_mut(id) else {
            return Err(SessionRepoError::Missing(id.to_string()));
        };
        let Some(row) = session.messages.get_mut(index) else {
            return Err(SessionRepoError::Missing(format!("{id} row {index}")));
        };
        row.text = text.to_string();
        if let Some(usage) = usage {
            row.meta.get_or_insert_with(MessageMeta::default).usage = Some(MessageUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                cached_tokens: usage.cached_tokens,
                ..MessageUsage::default()
            });
        }
        let finished = crate::runtime::clock::now();
        let timing = row.timing_mut();
        timing.ms = timing
            .start
            .map(|start| finished.saturating_sub(start).saturating_mul(1000));
        timing.start.get_or_insert(finished);
        session.updated_at = finished;
        let session = session.clone();
        self.save(&session)
    }

    /// Mark the turn's answer row as stopped before it produced an answer.
    ///
    /// The desktop records a stop so the transcript can say so; the row is not
    /// replayed into the next request, because an empty assistant row is not a
    /// thing the model said.
    pub(crate) fn cancel_turn(&mut self, id: &str, index: usize) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        let Some(row) = session.messages.get_mut(index) else {
            return;
        };
        let finished = crate::runtime::clock::now();
        row.meta.get_or_insert_with(MessageMeta::default).cancelled = true;
        let timing = row.timing_mut();
        timing.ms = timing
            .start
            .map(|start| finished.saturating_sub(start).saturating_mul(1000));
        timing.start.get_or_insert(finished);
        session.updated_at = finished;
        let session = session.clone();
        let _ = self.save(&session);
    }

    /// Close the last unanswered assistant row of a session.
    ///
    /// Recovery has no turn row to address: the process that held it is gone.
    /// The row it left is the last assistant row with no text and no error, and
    /// marking it stopped is what keeps the transcript from claiming a turn is
    /// still running.
    pub(crate) fn cancel_last_turn(&mut self, id: &str) {
        let Some(session) = self.store.session_mut(id) else {
            return;
        };
        let Some(index) = session.messages.iter().rposition(|row| {
            row.role == Role::Assistant && row.text.is_empty() && row.error.is_none()
        }) else {
            return;
        };
        self.cancel_turn(id, index);
    }

    /// Record why a turn failed on its answer row.
    pub(crate) fn fail_turn(
        &mut self,
        id: &str,
        index: usize,
        error: &str,
    ) -> Result<(), SessionRepoError> {
        let Some(session) = self.store.session_mut(id) else {
            return Err(SessionRepoError::Missing(id.to_string()));
        };
        let Some(row) = session.messages.get_mut(index) else {
            return Err(SessionRepoError::Missing(format!("{id} row {index}")));
        };
        row.error = Some(error.to_string());
        session.updated_at = crate::runtime::clock::now();
        let session = session.clone();
        self.save(&session)
    }

    /// Rename one session without moving its transcript.
    pub(crate) fn rename(&mut self, id: &str, title: &str) {
        if let Some(session) = self.store.session_mut(id) {
            session.title = title.to_string();
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Set the session's model, preset and plan mode.
    pub(crate) fn configure(&mut self, id: &str, config: &SessionConfig) {
        if let Some(session) = self.store.session_mut(id) {
            apply_config(session, config);
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Pin or unpin one session.
    pub(crate) fn set_pinned(&mut self, id: &str, pinned: bool) {
        if let Some(session) = self.store.session_mut(id) {
            session.pinned = pinned;
            let session = session.clone();
            let _ = self.save_document(&session);
            self.store.sort();
        }
    }

    /// Archive or unarchive one session.
    pub(crate) fn set_archived(&mut self, id: &str, archived: bool) {
        if let Some(session) = self.store.session_mut(id) {
            session.archived_at = archived.then(crate::runtime::clock::now);
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Record the reader's note, or clear it with an empty one.
    pub(crate) fn set_feedback(&mut self, id: &str, feedback: Option<&str>) {
        if let Some(session) = self.store.session_mut(id) {
            session.feedback = feedback.map(str::to_string);
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Replace the session's goal record.
    ///
    /// A goal keeps its identity and revision across an edit the way the
    /// desktop's goal store does: a corrected objective must not start a new
    /// round count, and a round that started for the old text must still be
    /// recognizable.
    pub(crate) fn set_goal(&mut self, id: &str, goal: Option<&SessionGoalView>) {
        if let Some(session) = self.store.session_mut(id) {
            let next = match goal {
                Some(view) => match session.goal.as_mut() {
                    Some(existing) => {
                        existing.objective = view.objective.clone();
                        existing.state = goal_state(view.state);
                        existing.rounds = view.rounds;
                        existing.max_rounds = view.max_rounds;
                        existing.blocked_reason =
                            view.blocked_reason.as_deref().and_then(|message| {
                                agent::goal::BlockedReason::new("model-reported", message)
                            });
                        existing.revision = existing.revision.saturating_add(1);
                        existing.clone()
                    }
                    None => goal_from_view(view),
                },
                None => {
                    session.goal = None;
                    let session = session.clone();
                    let _ = self.save_document(&session);
                    return;
                }
            };
            session.goal = Some(next);
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Record the model's working plan.
    pub(crate) fn set_plan(&mut self, id: &str, plan: &[PlanItemView]) {
        if let Some(session) = self.store.session_mut(id) {
            session.plan = plan
                .iter()
                .map(|item| agent::tools::plan::PlanItem {
                    text: item.text.clone(),
                    status: match item.status {
                        PlanItemStatus::Pending => agent::tools::plan::PlanStatus::Pending,
                        PlanItemStatus::InProgress => agent::tools::plan::PlanStatus::InProgress,
                        PlanItemStatus::Completed => agent::tools::plan::PlanStatus::Completed,
                    },
                })
                .collect();
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Record one landed compaction in the session's own history.
    pub(crate) fn record_compaction(&mut self, id: &str, folded: usize, summary: &str) {
        if let Some(session) = self.store.session_mut(id) {
            let at = crate::runtime::clock::now();
            session.compactions.push(CompactionRecord {
                at,
                items: folded,
                tokens: 0,
                summary: summary.to_string(),
                manual: true,
                after: session.loaded_from + session.messages.len(),
            });
            session.compacted = session.compacted.max(folded);
            session.updated_at = at;
            let session = session.clone();
            let _ = self.save_document(&session);
        }
    }

    /// Build the runtime read model for one session document.
    ///
    /// The permission is the Host's own session permission, which the desktop
    /// keeps on the workspace rather than on the chat; a document that does
    /// not carry one takes the Host's.
    pub(crate) fn project(
        &self,
        session: &Session,
        permission: &str,
        version: u64,
    ) -> SessionProjection {
        projection_from_session(session, permission, version)
    }
}

/// Apply one [SessionConfig] to a stored session.
fn apply_config(session: &mut Session, config: &SessionConfig) {
    session.provider = config.provider.clone();
    session.model = config.model.clone();
    session.agent_preset = config.preset.clone();
    session.plan_mode = config.plan_mode;
}

/// The contract's goal state for one stored state.
fn goal_state(state: GoalState) -> agent::goal::GoalState {
    match state {
        GoalState::Active => agent::goal::GoalState::Active,
        GoalState::Paused => agent::goal::GoalState::Paused,
        GoalState::Completed => agent::goal::GoalState::Completed,
        GoalState::Blocked => agent::goal::GoalState::Blocked,
    }
}

/// Map a stored goal onto the contract's view.
fn goal_view(goal: &Goal) -> SessionGoalView {
    SessionGoalView {
        objective: goal.objective.clone(),
        state: match goal.state {
            agent::goal::GoalState::Active => GoalState::Active,
            agent::goal::GoalState::Paused => GoalState::Paused,
            agent::goal::GoalState::Completed => GoalState::Completed,
            agent::goal::GoalState::Blocked => GoalState::Blocked,
        },
        rounds: goal.rounds,
        max_rounds: goal.max_rounds,
        blocked_reason: goal
            .blocked_reason
            .as_ref()
            .map(|reason| reason.message.clone()),
    }
}

/// Map the contract's goal onto a stored record.
fn goal_from_view(view: &SessionGoalView) -> Goal {
    let mut goal = Goal::new(view.objective.clone(), Some(view.max_rounds))
        .unwrap_or_else(|| Goal::new("untitled goal", Some(view.max_rounds)).expect("a goal"));
    goal.state = goal_state(view.state);
    goal.rounds = view.rounds;
    goal.blocked_reason = view
        .blocked_reason
        .as_deref()
        .and_then(|message| agent::goal::BlockedReason::new("model-reported", message));
    goal
}

/// A message id derived from the row it describes.
///
/// A stored transcript has no message ids, so a projection must mint them. The
/// mint is deterministic - (session, global row index) - so the same stored row
/// reads as the same message after a restart and a client's dedupe works.
fn message_id(session: &SessionId, row: usize) -> MessageId {
    MessageId::derived(&format!("{session}:{row}"))
}

/// Map the stored transcript onto the contract's read model.
fn projection_from_session(session: &Session, permission: &str, version: u64) -> SessionProjection {
    let session_id: SessionId = session.id.parse().unwrap_or_else(|_| SessionId::new());
    let created_at = seconds(session.created_at);
    let updated_at = seconds(session.updated_at);
    let mut messages = Vec::with_capacity(session.messages.len());
    let mut tool_calls = Vec::new();
    for (offset, message) in session.messages.iter().enumerate() {
        let row = session.loaded_from + offset;
        let at = message
            .timing()
            .and_then(|timing| timing.start)
            .map(seconds)
            .unwrap_or(created_at);
        messages.push(MessageView {
            message_id: message_id(&session_id, row),
            role: match message.role {
                Role::User => MessageRole::User,
                Role::Assistant => MessageRole::Assistant,
            },
            text: message.text.clone(),
            at,
            execution_id: None,
            compaction: false,
            usage: message
                .meta
                .as_ref()
                .and_then(|meta| meta.usage.as_ref())
                .map(|usage| TurnUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    cached_tokens: usage.cached_tokens,
                }),
        });
        for (step, call) in message.tools.iter().enumerate() {
            let started_at = call.at.map(millis).unwrap_or(at);
            let finished_at = call
                .ms
                .map(|ms| millis(started_at.timestamp_millis().max(0) as u64 + ms));
            tool_calls.push(ToolCallView {
                execution_id: None,
                step: (step + 1) as u64,
                tool: call.name.clone(),
                summary: summarize(&call.args),
                args: call.args.clone(),
                status: match &call.output {
                    None => ToolCallStatus::Running,
                    Some(output) if react::tool::ToolResult::text_is_error(output) => {
                        ToolCallStatus::Failed
                    }
                    Some(_) => ToolCallStatus::Completed,
                },
                output: call.output.clone(),
                duration_ms: call.ms,
                started_at,
                finished_at,
            });
        }
    }
    let compaction = session
        .compactions
        .last()
        .map(|record| CompactionOutcomeView {
            ok: true,
            folded: record.items as u64,
            summary: Some(record.summary.clone()),
            error: None,
            at: seconds(record.at),
        });
    SessionProjection {
        session_id,
        version,
        title: session.title.clone(),
        created_at,
        updated_at,
        messages,
        tool_calls,
        plan: session
            .plan
            .iter()
            .map(|item| PlanItemView {
                text: item.text.clone(),
                status: match item.status {
                    agent::tools::plan::PlanStatus::Pending => PlanItemStatus::Pending,
                    agent::tools::plan::PlanStatus::InProgress => PlanItemStatus::InProgress,
                    agent::tools::plan::PlanStatus::Completed => PlanItemStatus::Completed,
                },
            })
            .collect(),
        executions: Vec::new(),
        approvals: Vec::new(),
        config: SessionConfig {
            provider: session.provider.clone(),
            model: session.model.clone(),
            preset: session.agent_preset.clone(),
            permission: permission.to_string(),
            plan_mode: session.plan_mode,
        },
        compaction,
        goal: session.goal.as_ref().map(goal_view),
        feedback: session.feedback.clone(),
        trashed: false,
        pinned: session.pinned,
        archived: session.archived_at.is_some(),
    }
}

/// Import one journal-only projection as a stored session.
///
/// Sessions a Host created before this repository existed live only in the
/// journal. The first Host that opens the store writes them, so a reader does
/// not lose a conversation - or its tool trail - when the interfaces are
/// unified. Rows the journal knew, the store now owns.
pub(crate) fn session_from_projection(projection: &SessionProjection, workspace: &str) -> Session {
    let mut session = Session::new(workspace, projection.title.clone());
    session.id = projection.session_id.to_string();
    session.created_at = projection.created_at.timestamp().max(0) as u64;
    session.updated_at = projection.updated_at.timestamp().max(0) as u64;
    session.provider = projection.config.provider.clone();
    session.model = projection.config.model.clone();
    session.agent_preset = projection.config.preset.clone();
    session.plan_mode = projection.config.plan_mode;
    session.pinned = projection.pinned;
    session.archived_at = projection.archived.then(crate::runtime::clock::now);
    session.feedback = projection.feedback.clone();
    session.goal = projection.goal.as_ref().map(goal_from_view);
    session.plan = projection
        .plan
        .iter()
        .map(|item| agent::tools::plan::PlanItem {
            text: item.text.clone(),
            status: match item.status {
                PlanItemStatus::Pending => agent::tools::plan::PlanStatus::Pending,
                PlanItemStatus::InProgress => agent::tools::plan::PlanStatus::InProgress,
                PlanItemStatus::Completed => agent::tools::plan::PlanStatus::Completed,
            },
        })
        .collect();
    // A call the journal attributed to an execution belongs to the answer row
    // of that execution. Calls from before executions were recorded (a legacy
    // journal) have no owner; they ride the last answer row, which is where a
    // reader looks for them and where the desktop's audit walks.
    let last_assistant = projection
        .messages
        .iter()
        .rposition(|message| message.role == MessageRole::Assistant);
    session.messages = projection
        .messages
        .iter()
        .enumerate()
        .map(|(row_index, message)| {
            let mut row = match message.role {
                MessageRole::User => ChatMessage::user(message.text.clone()),
                MessageRole::Assistant => {
                    let mut row = ChatMessage::assistant();
                    row.text = message.text.clone();
                    row
                }
            };
            row.meta = message.usage.as_ref().map(|usage| MessageMeta {
                usage: Some(MessageUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    cached_tokens: usage.cached_tokens,
                    ..MessageUsage::default()
                }),
                timing: Some(MessageTiming {
                    start: Some(message.at.timestamp().max(0) as u64),
                    ..MessageTiming::default()
                }),
                ..MessageMeta::default()
            });
            let owned: Vec<&ToolCallView> = projection
                .tool_calls
                .iter()
                .filter(|call| {
                    call.execution_id.is_some() && call.execution_id == message.execution_id
                })
                .collect();
            let unowned = call_without_owner(projection, last_assistant == Some(row_index));
            row.tools = owned
                .into_iter()
                .chain(unowned)
                .map(|call| ToolEvent {
                    name: call.tool.clone(),
                    args: call.args.clone(),
                    output: call.output.clone(),
                    ms: call.duration_ms,
                    at: Some(call.started_at.timestamp_millis().max(0) as u64),
                })
                .collect();
            for (index, _) in row.tools.iter().enumerate() {
                row.parts.push(TurnPart::Tool(index));
            }
            row
        })
        .collect();
    session
}

/// Calls the journal recorded without an execution, on the row that owns them.
fn call_without_owner(projection: &SessionProjection, last_assistant: bool) -> Vec<&ToolCallView> {
    if !last_assistant {
        return Vec::new();
    }
    projection
        .tool_calls
        .iter()
        .filter(|call| call.execution_id.is_none())
        .collect()
}

/// A one-line description of a tool call's arguments, as the terminal shows it.
fn summarize(args: &str) -> String {
    const KEYS: [&str; 6] = ["command", "path", "pattern", "query", "url", "id"];
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
        return String::new();
    };
    for key in KEYS {
        if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
            return text.trim().chars().take(120).collect();
        }
    }
    String::new()
}

/// Unix seconds as a UTC instant.
fn seconds(at: u64) -> DateTime<Utc> {
    Utc.timestamp_opt(at as i64, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Unix milliseconds as a UTC instant.
fn millis(at: u64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(at as i64)
        .single()
        .unwrap_or_else(Utc::now)
}
