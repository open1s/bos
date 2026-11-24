//! Execution seam: the Host owns the execution lifecycle, an executor owns
//! the model/tool work for exactly one accepted turn.
//!
//! The Host never awaits a model on its request path. It spawns the executor,
//! keeps the task handle, publishes durable boundaries, and lets a client
//! disconnect without touching the task.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::contract::{
    BrowserView, CatalogueView, ExtensionsReport, FederationReport, MemoryReport, MessageRole,
    ReviewReport, SessionConfig, SessionConfigChange, SettingEdit, SettingsReport, TaskBoardReport,
    TaskDetailReport, TurnUsage, WorkspaceDiffView, WorkspaceReport,
};
use super::host::ApprovalHandle;
use super::ids::{ExecutionId, SessionId, WorkspaceId};

/// One prior message supplied to an executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    /// Who said it.
    pub role: MessageRole,
    /// What was said.
    pub text: String,
}

/// One fact a running turn reports about its tool work.
///
/// A turn publishes these while it runs so a client can show the tool calls
/// the model makes. They are not part of the turn's result: the Host journals
/// them as they arrive, and the durable transcript gets them whether or not a
/// client is watching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnProgress {
    /// The model asked for one tool call.
    ToolCallStarted {
        /// Position of the call within its turn, counting from one.
        step: u64,
        /// Tool name.
        tool: String,
        /// One-line description of the arguments.
        summary: String,
        /// The arguments as JSON.
        args: String,
    },
    /// The call returned.
    ToolCallFinished {
        /// Position of the call within its turn, counting from one.
        step: u64,
        /// Tool name.
        tool: String,
        /// Whether the result reports a failure.
        ok: bool,
        /// Output text the model received.
        output: String,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// More assistant text arrived. Never journaled: the committed message
    /// carries the whole answer, so activity only makes a running turn
    /// readable.
    AssistantText {
        /// The next piece of the answer.
        text: String,
    },
    /// The model replaced its working plan through `update_plan`.
    ///
    /// A plan is a fact about the session, not about one frame: the Host
    /// journals it, so the panel a client draws after a restart is the plan
    /// the model last wrote rather than whatever the client happened to see.
    PlanUpdated {
        /// The whole plan, in the model's order.
        items: Vec<agent::tools::plan::PlanItem>,
    },
}

/// The executor-facing seam for reporting tool work.
///
/// It is deliberately one-way: an executor can describe what a tool did but
/// cannot read session state or decide how the Host journals it. A detached
/// handle (a unit test, or a Host with no journal) drops every fact.
#[derive(Clone, Default)]
pub struct ProgressHandle {
    sender: Option<mpsc::UnboundedSender<TurnProgress>>,
}

impl ProgressHandle {
    /// A handle that reports nowhere.
    #[must_use]
    pub fn detached() -> Self {
        Self::default()
    }

    /// Bind a handle to the Host's progress channel.
    pub(crate) fn attached(sender: mpsc::UnboundedSender<TurnProgress>) -> Self {
        Self {
            sender: Some(sender),
        }
    }

    /// Report one fact. A closed channel means the turn was cancelled and the
    /// Host stopped listening; the tool still ran, so this is not an error.
    pub fn emit(&self, progress: TurnProgress) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(progress);
        }
    }

    /// Whether this handle reaches a Host.
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.sender.is_some()
    }
}

/// Everything an executor needs for one turn. It contains no Host handles other
/// than the approval gate and the progress channel, so an executor cannot
/// mutate session state directly.
pub struct TurnContext {
    /// Workspace.
    pub workspace_id: WorkspaceId,
    /// Session.
    pub session_id: SessionId,
    /// Execution identity.
    pub execution_id: ExecutionId,
    /// The new user text.
    pub text: String,
    /// Transcript before this turn, oldest first.
    pub history: Vec<HistoryMessage>,
    /// Configuration this session's turn runs with.
    pub config: SessionConfig,
    /// Gate for operator approval of a concrete action.
    pub approvals: ApprovalHandle,
    /// Channel for the tool work this turn does.
    pub progress: ProgressHandle,
}

/// Everything an executor needs to fold one session's older history.
///
/// Compaction is a model call, so it belongs to the executor: the Host owns
/// the journal and the projection, and the executor owns the only thing that
/// can summarize a transcript.
pub struct CompactContext {
    /// Workspace.
    pub workspace_id: WorkspaceId,
    /// Session whose history is folded.
    pub session_id: SessionId,
    /// Configuration the session runs with.
    pub config: SessionConfig,
    /// Transcript to fold, oldest first.
    pub history: Vec<HistoryMessage>,
}

/// One checkpoint an executor produced.
#[derive(Debug, Clone)]
pub struct CompactionOutput {
    /// Checkpoint text the summarizer wrote.
    pub summary: String,
    /// Transcript messages the checkpoint replaces.
    pub folded: u64,
}

/// A successful turn.
#[derive(Debug, Clone, Default)]
pub struct TurnOutput {
    /// Final assistant text.
    pub text: String,
    /// Provider-reported usage for the turn, when the model reported one.
    pub usage: Option<TurnUsage>,
}

/// A failed turn.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct TurnError {
    /// Detail safe to publish to clients.
    pub message: String,
}

impl TurnError {
    /// Build a failure from any displayable error.
    pub fn new(message: impl ToString) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

/// One turn executor implementation.
#[async_trait]
pub trait TurnExecutor: Send + Sync {
    /// Run one accepted turn. Cancellation is implemented by dropping this
    /// future, so an implementation must not rely on running destructors for
    /// durable side effects.
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError>;

    /// Fold this session's older history into one checkpoint.
    ///
    /// The default refuses instead of inventing a checkpoint: an executor is
    /// where a model lives, so a Host whose executor carries none has nothing
    /// to summarize with, and saying so is the honest answer.
    async fn compact(&self, _context: CompactContext) -> Result<CompactionOutput, TurnError> {
        Err(TurnError::new(
            "this executor cannot compact a session: it carries no model",
        ))
    }

    /// What a session created on this Host starts with.
    ///
    /// A Host does not invent a model, a preset or a permission: it asks the
    /// executor that will run the turn, so what a client shows and what a
    /// turn resolves cannot disagree.
    fn default_config(&self) -> SessionConfig {
        SessionConfig::default()
    }

    /// Models and presets a session may be configured with.
    fn catalogue(&self) -> CatalogueView {
        CatalogueView::default()
    }

    /// The workspace's durable memories, listed or recalled.
    ///
    /// The default reports an executor with no memory surface rather than an
    /// empty workspace: a client that showed "no memories" for a build that
    /// never opened the kernel would mistake an absent seam for an empty
    /// store.
    fn memories(&self, _query: Option<&str>) -> MemoryReport {
        MemoryReport {
            memories: Vec::new(),
            state: "unsupported".to_string(),
            error: None,
        }
    }

    /// One directory of the workspace this executor serves, or one file in it.
    ///
    /// The default reports an executor with no workspace surface rather than
    /// an empty directory: a client that drew "no files here" for a build that
    /// never resolved a root would mistake an absent seam for an empty
    /// workspace.
    fn files(&self, _path: &str) -> WorkspaceReport {
        WorkspaceReport {
            state: "unsupported".to_string(),
            ..WorkspaceReport::default()
        }
    }

    /// The workspace's changed files, or one path's comparison.
    ///
    /// The default answers the way [`Self::files`] does - with the state that
    /// says there is no workspace surface here - because an empty list would
    /// claim the working copy is clean, which is a different answer.
    fn diff(&self, _path: &str) -> WorkspaceDiffView {
        WorkspaceDiffView {
            state: "unsupported".to_string(),
            ..WorkspaceDiffView::default()
        }
    }

    /// The workspace's durable task board.
    ///
    /// The default reports an executor with no task surface rather than an
    /// empty board: four empty sections would say the workspace has no tasks,
    /// which is a different answer from "this Host never read the store".
    fn tasks(&self) -> TaskBoardReport {
        TaskBoardReport {
            state: "unsupported".to_string(),
            ..TaskBoardReport::default()
        }
    }

    /// One task's full record.
    ///
    /// The default answers the same way [`Self::tasks`] does - with the state
    /// that says there is no task surface here - so a reader who opens a row on
    /// a build that could not have listed it is told why rather than handed an
    /// empty record that looks like a task with nothing in it.
    fn task_detail(&self, _id: &str) -> TaskDetailReport {
        TaskDetailReport {
            state: "unsupported".to_string(),
            ..TaskDetailReport::default()
        }
    }

    /// The extensions this build has installed, and whether each is in force.
    ///
    /// The default reports an executor with no extension host rather than an
    /// empty roster: "nothing is installed" and "this executor cannot read an
    /// extension" are different answers, and a client that showed the first
    /// for the second would send the reader looking for a package that was
    /// never missing.
    fn extensions(&self) -> ExtensionsReport {
        ExtensionsReport {
            state: "unsupported".to_string(),
            ..ExtensionsReport::default()
        }
    }

    /// The federation node this executor's build runs and the peers it trusts.
    ///
    /// The default reports an executor with no federation surface rather than
    /// an empty roster: a reader who saw "no peers" for a build that never read
    /// a peer store would think their machine is trusted by nobody, when the
    /// truth is that this Host never looked.
    fn federation(&self) -> FederationReport {
        FederationReport {
            state: "unsupported".to_string(),
            ..FederationReport::default()
        }
    }

    /// One session's latest review: the files its turn changed, and the diff of
    /// the change this request names.
    ///
    /// The default reports an executor with no review surface rather than an
    /// empty changeset: "this turn changed nothing" and "this executor never
    /// captured a change" are different answers, and a client that showed the
    /// first for the second would tell the reader their files were untouched.
    fn review(&self, _session: &str, _change: Option<&str>) -> ReviewReport {
        ReviewReport {
            state: "unsupported".to_string(),
            ..ReviewReport::default()
        }
    }

    /// The browser's page: what the agent's browser tools last read.
    ///
    /// The default reports an executor with no browser rather than an empty
    /// page: "nothing is open" and "this executor never ran a browser" are
    /// different answers, and a client that drew the empty state for the
    /// second would tell the reader to type a URL into a pane that cannot
    /// fetch one.
    fn browser(&self) -> BrowserView {
        BrowserView {
            status: "unsupported".to_string(),
            error: Some(String::from(
                "this executor carries no browser: its build cannot read a page",
            )),
            ..BrowserView::default()
        }
    }

    /// What this executor's settings resolve to, as a settings pane reads them.
    ///
    /// The default reports an executor that carries no settings rather than an
    /// empty rail: "nothing is configured" and "this executor never loaded a
    /// settings document" are different answers, and a client that showed the
    /// first for the second would tell the reader their model was unset when
    /// nobody had looked.
    fn settings(&self, _section: Option<&str>) -> SettingsReport {
        SettingsReport {
            state: "unsupported".to_string(),
            error: Some(String::from(
                "this executor carries no settings: it was not built from a shell configuration",
            )),
            ..SettingsReport::default()
        }
    }

    /// Write settings fields and answer with what the next turn resolves.
    ///
    /// The executor owns the documents a turn reads, so it owns the write: a
    /// Host that persisted a value it could not apply would tell the reader their
    /// turn changed when it did not. The default refuses for the same reason the
    /// read does - an executor built without a shell configuration has no field
    /// to write.
    fn update_settings(&self, _edits: &[SettingEdit]) -> Result<SettingsReport, String> {
        Err(String::from(
            "this executor carries no settings: it was not built from a shell configuration",
        ))
    }

    /// Apply one configuration patch and say whether the session may run it.
    ///
    /// The executor owns what a value means, so it owns what a value may be:
    /// a Host that validated a model against a list it keeps itself would
    /// accept a name this build cannot call.
    fn configure(
        &self,
        current: &SessionConfig,
        change: &SessionConfigChange,
    ) -> Result<SessionConfig, String> {
        apply_config_change(current, change)
    }
}

/// Apply one configuration patch, refusing a permission mode the contract does
/// not name.
///
/// Shared by the trait's default and by every executor that adds its own
/// validation, so a patch means the same thing whichever executor runs it and
/// the catalogue check never replaces the patch semantics.
pub fn apply_config_change(
    current: &SessionConfig,
    change: &SessionConfigChange,
) -> Result<SessionConfig, String> {
    let mut next = current.clone();
    if let Some(provider) = &change.provider {
        next.provider = Some(provider.clone());
    }
    if let Some(model) = &change.model {
        next.model = Some(model.clone());
    }
    if let Some(preset) = &change.preset {
        next.preset = Some(preset.clone());
    }
    if let Some(permission) = &change.permission {
        if !crate::runtime::contract::PERMISSION_MODES.contains(&permission.as_str()) {
            return Err(format!(
                "unknown permission mode '{permission}'; expected one of {}",
                crate::runtime::contract::PERMISSION_MODES.join(", ")
            ));
        }
        next.permission = permission.clone();
    }
    if let Some(plan_mode) = change.plan_mode {
        next.plan_mode = plan_mode;
    }
    Ok(next)
}

/// Model settings for the built-in ReAct executor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReactSettings {
    /// Model identifier.
    pub model: String,
    /// Provider base URL.
    pub base_url: String,
    /// Provider API key. Never logged.
    pub api_key: String,
    /// System prompt prefix.
    pub system_prompt: String,
    /// ReAct step cap.
    pub max_steps: usize,
}

impl ReactSettings {
    /// Whether enough configuration exists to call a provider.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.model.trim().is_empty() && !self.base_url.trim().is_empty()
    }
}

/// One `agent` ReAct turn through a provider written into `host.toml`.
///
/// This is the fallback for a Host whose data directory has no shell settings
/// to read: it attaches no tools, because it holds none of the settings, vault
/// or policy a tool registration is authorized against. A Host that can read
/// the shell's configuration runs [`super::agent_exec::AgentExecutor`] instead,
/// and that executor carries the desktop's whole tool surface.
pub struct ReactExecutor {
    settings: ReactSettings,
}

impl ReactExecutor {
    /// Build an executor from provider settings.
    #[must_use]
    pub fn new(settings: ReactSettings) -> Self {
        Self { settings }
    }

    /// The settings this executor uses, with the key redacted.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "model={} base_url={} max_steps={}",
            self.settings.model, self.settings.base_url, self.settings.max_steps
        )
    }
}

#[async_trait]
impl TurnExecutor for ReactExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        if !self.settings.is_configured() {
            return Err(TurnError::new(
                "no model provider configured for this Host (set [agent] in host.toml)",
            ));
        }
        let mut config =
            agent::AgentConfig::default().name(format!("bsh-runtime-{}", context.session_id));
        config.model = self.settings.model.clone();
        config.base_url = self.settings.base_url.clone();
        config.api_key = self.settings.api_key.clone();
        if self.settings.max_steps > 0 {
            config.max_steps = self.settings.max_steps;
        }
        if self.settings.system_prompt.trim().is_empty() {
            config.system_prompt = DEFAULT_SYSTEM_PROMPT.to_string();
        } else {
            config.system_prompt = self.settings.system_prompt.clone();
        }
        let agent = agent::Agent::from_config(config);
        let task = render_task(&context);
        let text = agent
            .react(task)
            .await
            .map_err(|error| TurnError::new(error.to_string()))?;
        Ok(TurnOutput {
            text,
            ..TurnOutput::default()
        })
    }
}

/// The system prompt for a `host.toml`-pinned, tool-less executor.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are the BSH runtime assistant. Answer the user's turn directly and plainly. \
     This Host was configured from host.toml alone and runs without tools; do not claim to \
     have executed commands or changed files. A Host that reads the shell's settings runs \
     the desktop's full agent instead.";

fn render_task(context: &TurnContext) -> String {
    if context.history.is_empty() {
        return context.text.clone();
    }
    let mut task = String::from("Conversation so far:\n");
    for message in &context.history {
        let role = match message.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
        };
        task.push_str(role);
        task.push_str(": ");
        task.push_str(&message.text);
        task.push('\n');
    }
    task.push_str("\nUser: ");
    task.push_str(&context.text);
    task
}

/// A deterministic executor that echoes the turn. It exists for smoke tests and
/// `bsh host --echo`; it is never selected by default.
pub struct EchoExecutor;

#[async_trait]
impl TurnExecutor for EchoExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[host echo {}] {}", context.execution_id, context.text),
            ..TurnOutput::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::host::ApprovalHandle;

    #[test]
    fn task_includes_history_in_order() {
        let context = TurnContext {
            workspace_id: WorkspaceId::new(),
            session_id: SessionId::new(),
            execution_id: ExecutionId::new(),
            text: "third".to_string(),
            history: vec![
                HistoryMessage {
                    role: MessageRole::User,
                    text: "first".into(),
                },
                HistoryMessage {
                    role: MessageRole::Assistant,
                    text: "second".into(),
                },
            ],
            config: crate::runtime::contract::SessionConfig::default(),
            approvals: ApprovalHandle::detached(),
            progress: ProgressHandle::detached(),
        };
        let task = render_task(&context);
        assert!(task.starts_with("Conversation so far:\nUser: first\nAssistant: second"));
        assert!(task.ends_with("User: third"));
    }
}
