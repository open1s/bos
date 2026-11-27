//! Transport-neutral Application Contract.
//!
//! These types are the shared vocabulary of every BSH interface. A transport
//! adapter (local IPC today, HTTP/SSE later) only frames them; it must not add
//! domain semantics of its own.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ids::{
    ActionDigest, ApprovalId, ClientInstanceId, CommandId, EventId, ExecutionId, HostInstanceId,
    MessageId, OperationId, PayloadHash, SessionId, TaskId, WorkspaceId,
};

/// Protocol version negotiated on every connection.
pub const PROTOCOL_VERSION: u16 = 1;

/// Maximum size of one encoded frame accepted by an adapter.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Which interface a client identifies itself as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// The GPUI desktop shell.
    Gui,
    /// A terminal client.
    Tui,
    /// A scripted or administrative command-line client.
    Cli,
    /// A browser/PWA client.
    Web,
    /// A native mobile client.
    Mobile,
}

/// Client-supplied registration data. It never establishes identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Human-readable client name shown in Host diagnostics.
    pub name: String,
    /// Interface family.
    pub kind: ClientKind,
    /// Client version string, for diagnostics only.
    pub version: String,
}

/// First request on a connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol version the client intends to speak.
    pub protocol_version: u16,
    /// Client registration data.
    pub client: ClientInfo,
}

/// Who is acting, as derived by the Host.
///
/// A client never sends this in a command payload; the transport adapter
/// derives it from OS peer credentials or an authenticated session and the
/// Host attaches it to the request before authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalView {
    /// Stable principal identity string.
    pub id: String,
    /// Principal family.
    pub kind: PrincipalKind,
    /// Display label.
    pub display: String,
}

/// The identity families the contract distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    /// A local OS user, proven by local IPC peer credentials.
    LocalUser,
    /// An enrolled device acting for a principal.
    Device,
    /// A remote authenticated principal.
    Remote,
}

/// What a Host tells a freshly registered client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Welcome {
    /// Negotiated protocol version (the Host's value).
    pub protocol_version: u16,
    /// Identity of the Host process; clients compare it with endpoint discovery.
    pub instance_id: HostInstanceId,
    /// Host-issued client instance id for this connection.
    pub client_instance_id: ClientInstanceId,
    /// Derived principal.
    pub principal: PrincipalView,
    /// Workspace this Host serves.
    pub workspace_id: WorkspaceId,
    /// Contract features available over this connection.
    pub capabilities: Vec<String>,
}

/// The resource scope a command or query names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Workspace the operation belongs to.
    pub workspace_id: WorkspaceId,
    /// Session the operation belongs to, when one is required.
    pub session_id: Option<SessionId>,
}

/// One mutating application command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEnvelope {
    /// Client-chosen idempotency key. Retries reuse it.
    pub command_id: CommandId,
    /// Connection-bound client identity issued at registration.
    pub client_instance_id: ClientInstanceId,
    /// Resource scope.
    pub target: Target,
    /// Optimistic-concurrency guard: the version the client last observed.
    pub expected_version: Option<u64>,
    /// The mutation.
    pub payload: CommandPayload,
}

/// Mutations an interface may request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandPayload {
    /// Create a session in the workspace.
    CreateSession {
        /// Display title.
        title: String,
    },
    /// Rename a session; `expected_version` guards the write.
    RenameSession {
        /// Session to rename.
        session_id: SessionId,
        /// New title.
        title: String,
    },
    /// Branch one session: create a new chat that opens with its transcript.
    ///
    /// Forking is the desktop session menu's branch row, and it is a Host fact
    /// rather than a client copy: an interface that forked in its own memory
    /// would leave a chat the next client cannot see and a reconnect cannot
    /// fold. The Host journals the branch with the source it came from, so the
    /// fork opens with the same messages, calls and plan its source had at that
    /// instant, and the answer names the fork's own session id through the
    /// operation id - the same way a creation does.
    ///
    /// The fork is a new line of work, not a running duplicate: the source's
    /// executions, approvals and goal stay behind, because a fork that carried
    /// an unfinished goal would run every round of it twice.
    ForkSession {
        /// Session whose transcript the fork opens with.
        session_id: SessionId,
    },
    /// Submit a user turn. The Host durably accepts it and owns the execution.
    SendMessage {
        /// Session receiving the turn.
        session_id: SessionId,
        /// User text.
        text: String,
    },
    /// Cancel one Host-owned execution.
    CancelExecution {
        /// Execution to cancel.
        execution_id: ExecutionId,
    },
    /// Replace the fields of one session's configuration.
    ///
    /// The payload is a patch: a `None` field keeps its current value, because
    /// a client that switches the model must not restate - and so overwrite -
    /// the permission it never asked about.
    ConfigureSession {
        /// Session to configure.
        session_id: SessionId,
        /// The fields to replace.
        change: SessionConfigChange,
    },
    /// Fold the session's older history into one durable checkpoint.
    ///
    /// Compaction is a Host operation, not a client one: the transcript the
    /// checkpoint replaces lives in the Host's journal, so only a Host that
    /// owns that transcript can rewrite what the next turn reads. The desktop's
    /// `/compact` reaches the same executor through this command.
    CompactSession {
        /// Session whose history is folded.
        session_id: SessionId,
    },
    /// Give the session a goal.
    ///
    /// A goal is session state, not a prompt: the desktop's `/goal` stores it
    /// on the chat, every round of it reads the same objective, and a client
    /// that reconnects sees the goal instead of having to remember it. A
    /// session that already pursues an unfinished goal must edit, pause,
    /// resume, block or clear it first; a completed goal is replaced by a
    /// fresh one.
    SetGoal {
        /// Session that takes the goal.
        session_id: SessionId,
        /// What the work is for, as one statement.
        objective: String,
    },
    /// Rewrite an existing goal's objective without restarting it.
    ///
    /// The desktop's `/goal edit` keeps the state and round count: a corrected
    /// typo must not spend the cap on rounds already worked. A completed goal
    /// is replaced by a fresh one, because its round count belongs to work that
    /// is already done.
    EditGoal {
        /// Session whose goal is edited.
        session_id: SessionId,
        /// The corrected objective.
        objective: String,
    },
    /// Stop the session's goal without dropping it.
    ///
    /// Pausing keeps the objective and the round count; the goal is resumable
    /// later, so it is not the same thing as clearing it.
    PauseGoal {
        /// Session whose goal is paused.
        session_id: SessionId,
    },
    /// Arm a paused or blocked goal again, provided its cap still has room.
    ResumeGoal {
        /// Session whose goal is resumed.
        session_id: SessionId,
    },
    /// Drop the session's goal.
    ClearGoal {
        /// Session whose goal is dropped.
        session_id: SessionId,
    },
    /// Record the reader's note about one session, or clear it with blank text.
    ///
    /// The desktop's `/feedback` row writes this note into the session it
    /// belongs to, so the note is durable session state rather than client
    /// memory: it survives a reload and a second client reads the same note.
    /// Blank text clears the note instead of storing an empty one.
    SetFeedback {
        /// Session the note belongs to.
        session_id: SessionId,
        /// The note, taken verbatim after trimming; blank clears it.
        text: String,
    },
    /// Park one session in the workspace's bin.
    ///
    /// The desktop's bin exists because a delete nobody can undo is a delete
    /// nobody dares to use, and the same rule holds for a terminal: a chat that
    /// was parked is out of every list, out of the snapshot a reconnecting
    /// client reconciles against, and restorable until it is purged. The
    /// transcript is a journal fact, so parking it is a fact too - the fold that
    /// rebuilds the projection must reach the same answer after a restart.
    ///
    /// A session with work in flight is refused: a parked chat whose turn is
    /// still running would hide an approval behind an overlay every client
    /// treats as history.
    TrashSession {
        /// Session to park.
        session_id: SessionId,
    },
    /// Take one parked session back out of the bin.
    ///
    /// Restoring is the mirror of parking and nothing more: the chat returns to
    /// every list with the transcript, calls, plan and goal it had, because the
    /// fold kept all of them while it was hidden.
    RestoreSession {
        /// Session to take back.
        session_id: SessionId,
    },
    /// Drop one parked session for good.
    ///
    /// Purge is the bin's second decision, and the only irreversible one: from
    /// this fact on, no client folds the session, `Session` stops answering for
    /// it and restoring it is no longer possible. The journal keeps the events
    /// that already happened, because a replay must stay ordered, but nothing
    /// reads them again as a live session.
    PurgeSession {
        /// Session to drop.
        session_id: SessionId,
    },
    /// Pin or unpin one session in the workspace's list.
    ///
    /// Pinning is the desktop sidebar's star: a pinned chat sorts ahead of
    /// every unpinned one, whichever is newer. The flag is a durable fact, so
    /// a client asks for a STATE rather than sending a toggle - a repeated
    /// `/sessions pin` is a refused no-op, never an accidental unpin.
    PinSession {
        /// Session to pin or unpin.
        session_id: SessionId,
        /// Whether the session is pinned after this fact.
        pinned: bool,
    },
    /// Archive one session, or return it to the list.
    ///
    /// Archiving is the desktop sidebar's Archived filter: the chat leaves the
    /// session list while its rows stay folded, which is what makes this
    /// different from parking in the bin and from deleting. A session with a
    /// turn in flight is refused, for the reason parking refuses one: a chat
    /// hidden from the list must not be hiding a live approval.
    ArchiveSession {
        /// Session to archive or unarchive.
        session_id: SessionId,
        /// Whether the session is archived after this fact.
        archived: bool,
    },
    /// Change machine- or workspace-level settings through the Host.
    ///
    /// This is the write side of the Settings query: a client sends the keys the
    /// report marked editable, and the Host applies each value with the rules
    /// the desktop's Settings dialog applies, writes the same two documents and
    /// answers with the resolution the NEXT turn will use. The write is a
    /// command, not a control, because it is not privileged beyond the permission
    /// the caller already holds to run a turn in this workspace.
    ///
    /// A patch, not a whole document: a client that changes one key must not
    /// restate - and so overwrite - the fields it never showed.
    UpdateSettings {
        /// One edit per key, applied in order.
        edits: Vec<SettingEdit>,
    },
}

/// One settings field a client is writing.
///
/// The value is text because that is what a form holds: the reader typed
/// `0.4`, `8K` or a command. The Host parses it exactly as the Settings dialog
/// parses the field, so the two surfaces accept the same spellings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingEdit {
    /// The key the report's editable row carries.
    pub key: String,
    /// The text form of the new value; empty has the meaning each key gives it.
    pub value: String,
}

/// The permission mode a session runs under when nothing else decides it.
///
/// The contract cannot import the shell's own constants: it is the vocabulary
/// every interface shares, so the one string it needs is stated here and the
/// build maps it onto the shell's mode.
pub const DEFAULT_PERMISSION: &str = "workspace-write";

/// The permission modes every interface accepts, narrowest first.
///
/// Stated here for the same reason as [`DEFAULT_PERMISSION`]: a client only
/// ever offers the modes the contract names, and the Host refuses anything
/// else instead of passing an unknown string to the build's policy.
pub const PERMISSION_MODES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

/// What one session's next turns run with.
///
/// The desktop records the model and preset on the chat itself, and the
/// permission on the workspace it belongs to. A Host serves exactly one
/// workspace, so the three arrive as one per-session record, and every turn a
/// session accepts resolves from this record instead of from a global default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Provider node the session's model belongs to; `None` follows the
    /// machine default.
    pub provider: Option<String>,
    /// Model nickname inside that provider.
    pub model: Option<String>,
    /// Agent preset the session runs; `None` follows the workspace default.
    pub preset: Option<String>,
    /// Permission mode the session's turns run under.
    pub permission: String,
    /// Whether the next turn is asked to inspect and plan before acting.
    pub plan_mode: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            preset: None,
            permission: DEFAULT_PERMISSION.to_string(),
            plan_mode: false,
        }
    }
}

impl SessionConfig {
    /// The model reference this session names, when it names a whole one.
    ///
    /// Half a reference is no reference: a provider with no model, or a model
    /// with no provider, falls back to the machine default instead of guessing
    /// a pairing.
    #[must_use]
    pub fn model_ref(&self) -> Option<(String, String)> {
        match (self.provider.as_deref(), self.model.as_deref()) {
            (Some(provider), Some(model))
                if !provider.trim().is_empty() && !model.trim().is_empty() =>
            {
                Some((provider.to_string(), model.to_string()))
            }
            _ => None,
        }
    }
}

/// A patch to one session's configuration.
///
/// Every `Some` field replaces the stored value and every `None` leaves it
/// alone. A model is replaced, never unset: the desktop's picker only ever
/// selects a model, and a client asking for "no model" would be asking the
/// turn to fall back mid-session, which is a different operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfigChange {
    /// Provider to replace the session's provider with.
    pub provider: Option<String>,
    /// Model nickname to replace the session's model with.
    pub model: Option<String>,
    /// Preset id to replace the session's preset with.
    pub preset: Option<String>,
    /// Permission mode to replace the session's mode with.
    pub permission: Option<String>,
    /// Whether to enter or leave plan mode.
    pub plan_mode: Option<bool>,
}

/// One privileged control request. Controls carry stronger authorization and
/// an audit record on every path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEnvelope {
    /// Client-chosen idempotency key.
    pub command_id: CommandId,
    /// Connection-bound client identity.
    pub client_instance_id: ClientInstanceId,
    /// Resource scope.
    pub target: Target,
    /// Optimistic-concurrency guard.
    pub expected_version: Option<u64>,
    /// The control.
    pub payload: ControlPayload,
}

/// Controls: approval decisions and Host lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlPayload {
    /// Approve or reject one pending action, bound to its exact digest.
    DecideApproval {
        /// Pending approval.
        approval_id: ApprovalId,
        /// The verdict.
        decision: ApprovalVerdict,
        /// Digest shown to the operator; must match the pending action.
        action_digest: ActionDigest,
        /// Optional operator note, audited.
        note: Option<String>,
    },
    /// Ask the Host to drain and stop.
    Shutdown {
        /// Shutdown policy.
        mode: ShutdownMode,
    },
}

/// The two decisions an operator can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalVerdict {
    /// Permit the action.
    Approve,
    /// Refuse the action.
    Reject,
}

/// How the Host should stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMode {
    /// Stop accepting work, let in-flight turns finish within a grace period,
    /// then interrupt the remainder.
    Graceful,
    /// Interrupt in-flight turns immediately and flush durable state.
    Immediate,
}

/// One read-only query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryEnvelope {
    /// Resource scope.
    pub target: Target,
    /// The projection to read.
    pub payload: QueryPayload,
}

/// Read models an interface may request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QueryPayload {
    /// All sessions, newest activity first.
    ListSessions,
    /// One full session projection.
    Session {
        /// Session to read.
        session_id: SessionId,
    },
    /// The workspace's bin: the sessions parked by `TrashSession`.
    ///
    /// This is the desktop's trash list. The rows are the same summaries
    /// `ListSessions` answers with, because a parked chat is a chat in every
    /// respect but its visibility: a terminal shows the title and age a
    /// window shows, and names the session by it when the reader restores or
    /// purges one.
    Trash,
    /// The workspace's archived sessions: the desktop sidebar's Archived
    /// filter.
    ///
    /// Archiving hides a chat from `ListSessions` without dropping a row, so
    /// this is the one reader that still lists it. The answer is the same
    /// summary shape every list answers with, because an archived chat is a
    /// chat in every respect but its place in the list.
    Archived,
    /// Pending approvals, optionally scoped to one session.
    PendingApprovals {
        /// Optional session filter.
        session_id: Option<SessionId>,
    },
    /// Host lifecycle, activity and journal head.
    HostStatus,
    /// Completion of one accepted command's operation reference.
    Operation {
        /// Operation reference returned at acceptance.
        operation_id: OperationId,
    },
    /// Models and presets a session may be configured with.
    Catalogue,
    /// The workspace's durable memories.
    ///
    /// No query lists the records newest first, the way the desktop pane opens
    /// on the newest row. A query runs the kernel's hybrid recall, which is the
    /// same ranking a turn's context assembly asks for.
    Memories {
        /// The search text; `None` lists instead of searching.
        #[serde(default)]
        query: Option<String>,
    },
    /// One directory of the workspace the Host serves, or one file in it.
    ///
    /// This is the desktop's Workspace Files browser: a client lists one
    /// directory at a time, descending by asking for the child path. The Host
    /// resolves every path against its own root and classifies the file before
    /// reading it, so a client never joins a path itself and a binary or an
    /// oversized file is refused where the desktop refuses it.
    Files {
        /// Path relative to the workspace root; empty asks for the root.
        #[serde(default)]
        path: String,
    },
    /// The workspace's changed files, or one path's comparison.
    ///
    /// This is the desktop Diff tab. The window opens it two ways: the Diff
    /// tab itself lists what the working copy changed against its baseline, and
    /// selecting a changed file in the Files tree opens that file's unified
    /// comparison beside the list. A terminal asks for the same two answers
    /// with one query, so the reader is never handed a second opinion about
    /// what changed: no path lists the changes, and a path is the comparison.
    ///
    /// `path` is resolved by the Host against its own root, exactly as the
    /// Files query resolves it, so a client never joins a path itself and a
    /// path that climbs out of the workspace is refused where the desktop
    /// refuses it.
    Diff {
        /// Path relative to the workspace root; empty asks for the change list.
        #[serde(default)]
        path: String,
    },
    /// The workspace's durable task board.
    ///
    /// This is the desktop's Tasks card: the same four sections, the same
    /// records, so a terminal shows the board the desktop shows rather than a
    /// second opinion about what is running. The Host reads its own workspace
    /// root, which is the directory the scheduler and the task tools use.
    Tasks,
    /// One task's full record: the desktop Tasks card's expanded row.
    ///
    /// A board row carries only what fits on one line, so a reader who opens a
    /// task asked for more than the summary can say: the objective it was
    /// given, what must finish first, the caps each attempt runs under and what
    /// every attempt returned. This query answers that same record, read from
    /// the Host's own store, instead of making the terminal guess at it from the
    /// row.
    ///
    /// `state` is one of:
    /// - `ready` - the record was read; the fields hold it.
    /// - `unknown` - no record in the store carries that id.
    /// - `unsupported` - this executor carries no task surface, or the Host
    ///   names no workspace directory to read.
    /// - `unreadable` - the store could not be read; `error` says why.
    TaskDetail {
        /// The id a board row carried; empty names no task.
        #[serde(default)]
        id: String,
    },
    /// The extensions this build has installed.
    ///
    /// This is the desktop's settings > extensions list: the same host status
    /// rows the desktop draws, so a terminal shows which packages are active,
    /// which failed and which tools each one added.
    Extensions,
    /// The federation node this Host runs and the peers it trusts.
    ///
    /// This is the desktop's Federation tab: the same node identity and the
    /// same peer store, read from the live node when this process runs one and
    /// from the files otherwise, so a terminal lists the peers a window lists
    /// instead of a second opinion about who is trusted. Grants travel with
    /// each peer, because a peer that is trusted for nothing can call nothing.
    Federation,
    /// One session's latest review: every file its turns changed.
    ///
    /// This is the desktop's Review tab: the same changeset the Host captured
    /// at the tool boundary, the same deterministic findings and the same
    /// structural diff. A terminal cannot click a row, so the request names
    /// the change whose prepared diff it wants beside the list.
    Review {
        /// Session whose latest sealed changeset is read.
        session_id: SessionId,
        /// Change id whose diff the report should carry; absent lists only.
        #[serde(default)]
        change: Option<String>,
    },
    /// The delegated child runs a conversation started: the desktop's Tasks
    /// graph children.
    ///
    /// The desktop draws these under the main node of its task graph and lists
    /// them in the header, from the same in-process registry the `subagent`
    /// tool writes: the run's title, its status, how deep it sits, its token
    /// use and its answer. A client that attached to this Host cannot see that
    /// registry, so the Host answers for it rather than letting a terminal
    /// pretend a turn never delegated anything.
    ///
    /// Runs outlive the turn that started them, which is why the filter is the
    /// conversation and not the turn: a reader who opens this after the turn
    /// settled still sees what its children did.
    Subagents {
        /// Conversation whose lineage is read; absent lists every run this
        /// Host holds.
        #[serde(default)]
        session_id: Option<SessionId>,
    },
    /// The browser's page: the desktop's Browser tab, in the form a terminal
    /// can read.
    ///
    /// The desktop pane is a text view of the page the agent's browser tools
    /// read - the address, the navigation state, the extracted title, text and
    /// interactive elements - so a terminal shows exactly the facts the pane
    /// shows rather than pretending to paint a page. It is process-wide and
    /// not session-scoped: one browser serves every session this Host runs,
    /// the way one desktop window serves every chat it opened.
    Browser,
    /// What the Host's settings resolve to: the desktop settings dialog's read
    /// side.
    ///
    /// The settings documents are what a turn actually runs from - the model,
    /// its endpoint and credential source, the workspace root and permission,
    /// the tool switches, the search endpoint and the vault's state. A terminal
    /// reader who needs to know why a turn calls one endpoint and not another
    /// asks here instead of opening `settings.toml` by hand, and the answer is
    /// the Host's own resolution rather than a client-side re-read that could
    /// disagree with the turn.
    Settings {
        /// One section to read; `None` lists every section.
        #[serde(default)]
        section: Option<String>,
    },
    /// Full snapshot for cursor-expired reconciliation.
    Snapshot,
}

/// The outcome of a mutating request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandOutcome {
    /// The Host durably accepted responsibility. Completion is separate.
    Accepted {
        /// Idempotency key echoed back.
        command_id: CommandId,
        /// Reference to query completion.
        operation_id: OperationId,
        /// Acceptance timestamp.
        accepted_at: DateTime<Utc>,
    },
    /// Nothing was mutated.
    Rejected {
        /// Idempotency key echoed back.
        command_id: CommandId,
        /// Typed refusal.
        rejection: Rejection,
    },
}

/// Why the Host refused a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rejection {
    /// Machine-readable class.
    pub code: RejectionCode,
    /// Human-readable detail safe to show a client.
    pub message: String,
}

/// Refusal classes shared across transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    /// The Host is initializing, recovering, draining or stopped.
    NotReady,
    /// The principal lacks authority for this operation.
    Unauthorized,
    /// The payload or target is structurally invalid.
    InvalidRequest,
    /// A conflicting command id, stale version, or already-decided approval.
    Conflict,
    /// The named resource does not exist.
    NotFound,
    /// The approval digest does not match the pending action.
    ApprovalDigestMismatch,
    /// The approval expired before a decision.
    ApprovalExpired,
    /// The approval already has a committed decision.
    ApprovalAlreadyDecided,
    /// The execution is not running.
    ExecutionNotRunning,
    /// The client exceeded its request budget.
    RateLimited,
    /// An unexpected Host failure.
    Internal,
}

impl RejectionCode {
    /// A short stable string for logs and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotReady => "not_ready",
            Self::Unauthorized => "unauthorized",
            Self::InvalidRequest => "invalid_request",
            Self::Conflict => "conflict",
            Self::NotFound => "not_found",
            Self::ApprovalDigestMismatch => "approval_digest_mismatch",
            Self::ApprovalExpired => "approval_expired",
            Self::ApprovalAlreadyDecided => "approval_already_decided",
            Self::ExecutionNotRunning => "execution_not_running",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
        }
    }
}

/// Result of a query.
///
/// Tagged `result`, not `type`: a query result is the payload of
/// `Response::Query`, which is itself internally tagged `type`, and two
/// flattened tags under one key serialize as a duplicate field the client
/// cannot read back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum QueryResult {
    /// Session summaries.
    Sessions {
        /// Summaries, newest activity first.
        sessions: Vec<SessionSummary>,
    },
    /// One session projection.
    Session {
        /// The projection, or `None` when the session does not exist.
        ///
        /// Boxed for the same reason as the snapshot below: a projection
        /// carries the whole transcript, so every other answer would pay its
        /// size in every `Response` frame. Serde carries the same JSON and
        /// field access dereferences through the box.
        session: Option<Box<SessionProjection>>,
    },
    /// Approval views.
    Approvals {
        /// Pending and recently decided approvals.
        approvals: Vec<ApprovalView>,
    },
    /// Host status.
    Host {
        /// Lifecycle, activity and journal head.
        status: HostStatus,
    },
    /// Operation completion.
    Operation {
        /// The operation view, or `None` for an unknown reference.
        operation: Option<OperationView>,
    },
    /// Full reconciliation snapshot.
    Snapshot {
        /// Snapshot value.
        ///
        /// Boxed because a snapshot is the largest projection on the wire and
        /// every other answer would pay its size in every `Response` frame. The
        /// indirection is invisible to a client: serde carries the same JSON,
        /// and field access dereferences through it.
        snapshot: Box<Snapshot>,
    },
    /// Models and presets a session may be configured with.
    Catalogue {
        /// Providers, models and presets.
        catalogue: CatalogueView,
    },
    /// The workspace's durable memories and the store's lifecycle.
    Memories {
        /// The records and the state that explains their count.
        report: MemoryReport,
    },
    /// One directory listing, or one file's text.
    Files {
        /// The listing or the document, plus the root it was resolved in.
        report: WorkspaceReport,
    },
    /// The workspace's changed files, or one path's comparison.
    Diff {
        /// The change list or the patch, plus what the VCS said.
        view: WorkspaceDiffView,
    },
    /// The workspace's durable task board and its sections.
    Tasks {
        /// The four sections and the rows each one holds.
        report: TaskBoardReport,
    },
    /// One task's full record, as the desktop's expanded row draws it.
    TaskDetail {
        /// The record, or the state that explains why there is none. Boxed for
        /// the same reason as a review: it carries every attempt a long-running
        /// task has made, so every other answer would pay its size.
        report: Box<TaskDetailReport>,
    },
    /// The installed extensions and whether each is in force.
    Extensions {
        /// The roster and the state that explains its count.
        report: ExtensionsReport,
    },
    /// One session's latest review: the files its turns changed.
    Review {
        /// The review report, boxed for the same reason as a session
        /// projection: it carries a change list and, when asked, a prepared
        /// diff with its rows, so every other answer would pay its size.
        report: Box<ReviewReport>,
    },
    /// The federation node and the peers it trusts.
    Federation {
        /// The node, its roster and the state that explains the count.
        report: FederationReport,
    },
    /// One conversation's delegated child runs.
    Subagents {
        /// The runs and the state that explains their count.
        report: SubagentReport,
    },
    /// The browser's page and its navigation state.
    Browser {
        /// The view, boxed because a page carries its whole extracted text and
        /// its elements, so every other answer would pay their size.
        view: Box<BrowserView>,
    },
    /// The settings the Host's turns resolve from.
    Settings {
        /// The sections and rows, boxed for the same reason as a projection:
        /// every other answer would otherwise pay the size of the whole rail.
        report: Box<SettingsReport>,
    },
}

/// One provider's models, as a model picker shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModels {
    /// Provider id.
    pub provider: String,
    /// Model nicknames inside it.
    pub models: Vec<String>,
}

/// One skill a client may offer as a `/name` row.
///
/// The desktop palette lists the skills its build registered, name and one-line
/// description, because choosing a row writes `/name ` into the draft and the
/// send resolves the gesture. A terminal that only guess-typed names would hide
/// the same catalogue behind a help page, so the Host states it here and every
/// interface offers the rows its build actually has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillView {
    /// The skill's own name, from its manifest.
    pub name: String,
    /// The one-line description from its frontmatter.
    pub description: String,
    /// Whether the manifest restricts it to user invocation
    /// (`disable-model-invocation: true`).
    ///
    /// A client shows the marker; the row is offered either way, because the
    /// reader is exactly who may invoke it.
    pub user_only: bool,
}

/// One MCP server this build knows about.
///
/// The desktop's MCP pane lists the servers its settings name, whether each
/// connected and what it offers. A terminal turn now runs the same pool, so the
/// same roster is what a client needs to explain which tools exist: a server
/// that failed says why, and a connected one names the tools the model can
/// call. A transport description never carries a credential - the headers an
/// HTTP entry holds are where one lives, so only the endpoint travels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerView {
    /// The server's name, which is also its tool namespace.
    pub name: String,
    /// How it is reached: `command args`, or the endpoint URL.
    pub transport: String,
    /// Whether the settings enable it. A disabled row is listed and never
    /// connected, the way the pane keeps a switched-off entry visible.
    pub enabled: bool,
    /// Whether an installed extension declares it rather than the settings.
    pub extension: bool,
    /// The exact tool names the model can call, once connected.
    pub tools: Vec<String>,
    /// Why it is not connected, when it is not.
    #[serde(default)]
    pub error: Option<String>,
}

/// One interactive element of a page, as the Browser pane lists it.
///
/// The ref is the handle the model passes back to act on the element, so a
/// terminal lists the same ref a model would use - a reader can see what the
/// turn could click before it clicks it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserElementView {
    /// The ref, `e1`, `e2`, ...: what the model passes back.
    pub reference: String,
    /// `link`, `button`, `submit`, `input`, `textarea` or `select`.
    pub kind: String,
    /// The visible label: a link's text, a button's text, a field's label.
    pub label: String,
    /// Where a link goes.
    pub href: Option<String>,
    /// A field's current value. A password's value travels as `(hidden)`.
    pub value: Option<String>,
    /// A select's options.
    pub options: Vec<String>,
    /// A field's own type (text, search, password, submit, ...).
    pub input_type: Option<String>,
}

/// What the Browser pane shows: the page the agent's browser tools read.
///
/// The status is stated rather than inferred from the page being absent: an
/// empty browser, a navigation in flight and a refused host are three answers,
/// and the desktop pane draws each of them in its own words. `error` carries
/// the failure's message and `rule` the policy rule that refused the host, so
/// a terminal can say which permission blocked the page instead of blaming the
/// network.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserView {
    /// `empty`, `loading`, `ready` or `failed`.
    pub status: String,
    /// The address bar's value: where the browser is or is going.
    pub address: String,
    /// The page's title, or `(untitled)` when it has none.
    pub title: String,
    /// The URL that answered.
    pub url: String,
    /// Whether going back is possible.
    pub can_back: bool,
    /// Whether going forward is possible.
    pub can_forward: bool,
    /// The readable text, bounded by the browser's own cap.
    pub text: String,
    /// Characters actually returned.
    pub text_chars: usize,
    /// Characters the page held in total, so a cut page is not a short one.
    pub text_total_chars: usize,
    /// The interactive elements, bounded by the browser's own cap.
    pub elements: Vec<BrowserElementView>,
    /// Elements the page held in total.
    pub elements_total: usize,
    /// The focused ref, when one is.
    pub focused: Option<String>,
    /// Why the last navigation failed, in the browser's own words.
    pub error: Option<String>,
    /// The policy rule that refused the navigation, when a rule did.
    pub rule: Option<String>,
}

/// One durable memory record as a client lists it.
///
/// The desktop's Memory pane is the reader's surface over the Durable Record
/// Kernel: every record, its kind, scope, lifecycle and age, plus the hybrid
/// recall hits while searching. A terminal turn recalls from the same stores,
/// so the same rows answer "what does this workspace remember" whichever
/// interface asks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryView {
    /// The record id.
    pub id: String,
    /// The remembered text.
    pub content: String,
    /// The kind label (`fact`, `preference`, `decision`, ...).
    pub kind: String,
    /// The scope level: `user`, `workspace`, `session` or `agent`.
    pub scope: String,
    /// The most specific scope id, shortened for a row.
    pub scope_id: String,
    /// The lifecycle label (`active`, `superseded`, `tombstoned`, ...).
    pub lifecycle: String,
    /// Where the content came from.
    pub source: String,
    /// The record's revision counter.
    pub revision: u32,
    /// Last committed update time.
    pub updated_at: DateTime<Utc>,
    /// The hybrid-recall score, when this row came from a search.
    #[serde(default)]
    pub score: Option<f64>,
    /// The score's named contributions, when this row came from a search.
    #[serde(default)]
    pub reasons: String,
}

/// The workspace's memories plus the store's lifecycle.
///
/// The state is stated rather than inferred from an empty list: memory that is
/// switched off and memory that holds nothing are different answers, and the
/// desktop pane distinguishes them the same way.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryReport {
    /// The records, newest first when listing and best first when searching.
    pub memories: Vec<MemoryView>,
    /// The memory lifecycle this Host resolved: `disabled`, `empty`, `ready`
    /// or `init_failed`.
    pub state: String,
    /// Why the store could not be opened, when it could not.
    pub error: Option<String>,
}

/// One directory listing, or one file's text, as the Host resolved it.
///
/// The state is stated rather than inferred from the payload, the way
/// [`MemoryReport`] states its lifecycle: a directory, a document, a refusal
/// and a file the pane will not show are four different answers, and a client
/// that drew an empty list for all four would make an unreadable file look
/// like an empty folder.
///
/// `state` is one of:
/// - `dir` - `entries` holds the listing, directories first.
/// - `file` - `content` holds the text, capped and flagged by `truncated`.
/// - `opaque` - the file is an image, a binary or too large for the pane.
/// - `missing` - nothing exists at the path.
/// - `refused` - the path climbs out of the workspace root.
/// - `unreadable` - the path exists but could not be read; `error` says why.
/// - `unsupported` - this executor carries no workspace surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceReport {
    /// The directory the Host serves, expanded, so a client can show it.
    pub root: String,
    /// The path this report is about, relative to the root; empty is the root.
    pub path: String,
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// One directory's rows, directories first and each group by name.
    pub entries: Vec<WorkspaceEntryView>,
    /// A text file's rows, newline-joined, when `state` is `file`.
    pub content: Option<String>,
    /// Whether `content` holds only the head of a longer file.
    pub truncated: bool,
    /// Why the path could not be read, when it could not.
    pub error: Option<String>,
}

/// One row of a workspace listing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceEntryView {
    /// The file or directory name, without a parent path.
    pub name: String,
    /// Whether the row is a directory.
    pub dir: bool,
    /// Size in bytes, zero for a directory.
    pub size: u64,
}

/// One path the working copy changed, as the desktop Files tree badges it.
///
/// The letter is the pane's own vocabulary - `A`, `R`, `M`, `D`, `C` for a
/// conflict and `?` for a change the VCS does not classify - and the label is
/// the same status in words, so a client never has to own a second table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceChangeView {
    /// The path, relative to the root and `/`-separated.
    pub path: String,
    /// The one-letter status.
    pub letter: String,
    /// The status in words.
    pub label: String,
}

/// The workspace's changed files, or one path's comparison against its
/// baseline.
///
/// This is the desktop Diff tab in the form a terminal reads: the list the
/// pane opens on, and the unified patch the pane draws when a path is opened.
/// The patch is the VCS's own output, never a reconstruction: bsh keeps no
/// before-image of a file, so a comparison it cannot ask for honestly is one
/// it does not draw.
///
/// `state` is one of:
/// - `listed` - the working copy differs from its baseline; `changes` holds
///   every path, sorted the way the tree orders them.
/// - `clean` - nothing differs. When `path` names a file, that file is clean;
///   when it is empty, the whole working copy is.
/// - `patch` - `path`'s comparison is in `patch`; `changes` still holds the
///   list, so a client redrawing the tab after a diff keeps its rows.
/// - `unknown` - the root is not a jj or git checkout, so nothing is compared.
/// - `refused` - the path climbs out of the workspace root.
/// - `unreadable` - the VCS was there but refused to answer; `error` says why.
/// - `unsupported` - this executor carries no workspace surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceDiffView {
    /// The directory the Host serves, expanded, so a client can show it.
    pub root: String,
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// The VCS in force: `git` or `jj`, empty when the root is no checkout.
    pub vcs: String,
    /// Every changed path, sorted by path.
    pub changes: Vec<WorkspaceChangeView>,
    /// The path this view compares, relative to the root; empty is the list.
    pub path: String,
    /// The unified patch, when `state` is `patch`.
    pub patch: Option<String>,
    /// Whether `patch` holds only the head of a longer comparison.
    pub truncated: bool,
    /// Why the comparison could not be read, when it could not.
    pub error: Option<String>,
}

/// The workspace's durable task board, section for section.
///
/// The shape is the desktop Tasks card's, because the terminal should not
/// invent a second reading of the same records: `sections` is always the four
/// the board draws, in the order it draws them, and each carries the rows it
/// holds. A section with no rows is present and empty, so a client never has to
/// guess whether "no running tasks" means "none" or "not asked for".
///
/// `state` is one of:
/// - `ready` - the store was read; `sections` holds the board.
/// - `unsupported` - this executor carries no task surface, or the Host names
///   no workspace directory to read.
/// - `unreadable` - the task directory exists but could not be read; `error`
///   says why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskBoardReport {
    /// The workspace root the board was read from, expanded.
    pub root: String,
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// Running, queued, scheduled, history - always four.
    pub sections: Vec<TaskSectionView>,
    /// Why the store could not be read, when it could not.
    pub error: Option<String>,
}

/// One board section and the rows it holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskSectionView {
    /// `running`, `queued`, `scheduled` or `history`.
    pub section: String,
    /// The rows, in the order the board sorts them.
    pub rows: Vec<TaskRowView>,
}

/// One task as the board lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskRowView {
    /// Stable task id; the record is named after it.
    pub id: String,
    /// The short label the board shows.
    pub title: String,
    /// The attempt tier: `turn` or `child`.
    pub tier: String,
    /// The lifecycle state, in the record's snake_case spelling.
    pub status: String,
    /// Who executes it: `local` or `peer`.
    pub host: String,
    /// The origin session id, when the record names one.
    pub session: String,
    /// The parent task id, when the record names one: the edge the board's
    /// graph nests this row under, empty when it hangs off the board itself.
    pub parent: String,
    /// Tasks that must succeed first, in stored order.
    pub deps: Vec<String>,
    /// When it was created, in unix milliseconds.
    pub created_ms: u64,
    /// The next scheduled fire, for an at or every trigger.
    pub next_fire_ms: Option<u64>,
    /// How many attempts have been made.
    pub attempts: u32,
    /// The latest attempt's verification state.
    pub verification: String,
}

/// One task's full record: what the desktop's expanded Tasks row draws.
///
/// The board row is the one-line summary a list can paint. This is the record
/// behind it: the objective, the dependency edges the graph draws, the budget
/// each attempt is held to, and the attempt history the card prints under the
/// intent. Numeric caps travel as numbers so a client can format them; anything
/// with a closed set of spellings travels as the snake_case label the store and
/// the desktop both use, so a terminal never invents a vocabulary of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskDetailReport {
    /// The workspace root the record was read from, expanded.
    pub root: String,
    /// `ready`, `unknown`, `unsupported` or `unreadable`, per the query.
    pub state: String,
    /// The record's id, echoed from the query.
    pub id: String,
    /// The short label the board shows.
    pub title: String,
    /// The full prompt or objective the task was given.
    pub intent: String,
    /// The attempt tier: `turn` or `child`.
    pub tier: String,
    /// The lifecycle state, in the record's snake_case spelling.
    pub status: String,
    /// Who executes it: `local` or `peer`.
    pub host: String,
    /// The parent task id, when the record names one.
    pub parent: String,
    /// For a delegation: the peer node id.
    pub target: String,
    /// For a job: the command line.
    pub command: String,
    /// How it fires: `now`, `at <ms>` or `every <seconds>s`.
    pub trigger: String,
    /// The next scheduled fire, in unix milliseconds.
    pub next_fire_ms: Option<u64>,
    /// When it was created, in unix milliseconds.
    pub created_ms: u64,
    /// When it last changed, in unix milliseconds.
    pub updated_ms: u64,
    /// Tasks that must succeed first, in stored order.
    pub deps: Vec<String>,
    /// The caps one attempt runs under: steps, tokens and wall clock.
    pub max_steps: u32,
    /// The token cap.
    pub max_tokens: u64,
    /// The wall-clock cap, in milliseconds.
    pub deadline_ms: u64,
    /// The retry mode: `manual` or `automatic`.
    pub retry: String,
    /// The cap on attempts, including the first.
    pub max_attempts: u32,
    /// The latest verification state, in the record's spelling.
    pub verification: String,
    /// The contract a verifier must satisfy: `none`, `review` or `human`.
    pub verification_contract: String,
    /// Every attempt, oldest first.
    pub attempts: Vec<TaskAttemptView>,
    /// Why the record could not be read, when it could not.
    pub error: Option<String>,
}

/// One attempt of a task, as the expanded card lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskAttemptView {
    /// Stable attempt id.
    pub id: String,
    /// The tier this attempt bound to: `turn` or `child`.
    pub tier: String,
    /// The attempt's state, in the record's snake_case spelling.
    pub status: String,
    /// The tier object's id once started: subagent, job, delegation or turn.
    pub run: String,
    /// When it started, in unix milliseconds.
    pub started_ms: Option<u64>,
    /// When it ended, in unix milliseconds.
    pub ended_ms: Option<u64>,
    /// The bounded result text, when the attempt settled.
    pub result: String,
    /// The error or refusal reason, when it has one.
    pub error: String,
    /// The exact-change set the attempt produced, when review is available.
    pub change_set: String,
    /// This attempt's own verification state, in the record's spelling.
    pub verification: String,
}

/// One row of a settings view: what it is called, what it is set to, and where
/// that value comes from.
///
/// The row is the desktop settings pane's read side in the form a terminal can
/// paint: a label a reader recognizes, the effective value a turn will use (not
/// the raw field), and a short provenance word so "workspace", "settings",
/// "vault" and "unset" are distinguishable without opening the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsRowView {
    /// Stable row id, e.g. `provider` or `permission`.
    pub label: String,
    /// The effective value, already rendered for a one-line row.
    pub value: String,
    /// Where the value came from: `settings`, `workspace`, `vault`, `default`
    /// or `unset`.
    pub source: String,
    /// The key `UpdateSettings` writes this row with, or `None` for a row that
    /// is derived from other settings and has no field of its own.
    ///
    /// It travels with the row so a client never keeps a second vocabulary: the
    /// row a reader points at is the key it sends. A row without a key is one the
    /// desktop's form does not edit either - an effective credential, a resolved
    /// path, a preset ceiling - and a write naming it is refused.
    pub key: Option<String>,
}

/// One settings section: the desktop rail's cell, as rows a terminal lists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsSectionView {
    /// Stable section id, e.g. `model` or `web-search`.
    pub section: String,
    /// The rows this section holds, in display order.
    pub rows: Vec<SettingsRowView>,
}

/// What the Host's settings currently resolve to, as a client reads them.
///
/// This is the read side of the desktop settings dialog: the same documents the
/// running turn resolves from, folded into the sections a rail draws, so a
/// terminal shows the configuration that will actually be used rather than a
/// second opinion about it. Secrets never travel: a credential row reports
/// *where* the key comes from (the vault, the environment, or nothing), never
/// the key itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsReport {
    /// The sections, in the rail's display order.
    pub sections: Vec<SettingsSectionView>,
    /// `ready` when the Host read its settings, `unsupported` for an executor
    /// that carries none.
    pub state: String,
    /// Why the requested section is not here, when a client named one the Host
    /// does not have.
    pub error: Option<String>,
}

/// One installed extension, as the desktop's extensions list draws it.
///
/// The row is the extension host's own status record, translated: the state and
/// health spellings are the host's (`active`, `failed`, `degraded`), so a client
/// shows the lifecycle the host resolved rather than inventing one. What the
/// extension contributed travels with the row, because that is what explains why
/// a turn can call a tool nobody configured: the package declared it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionView {
    /// Extension id, the row's key.
    pub id: String,
    /// Display name; falls back to the id when the manifest names none.
    pub name: String,
    /// The version in force.
    pub version: String,
    /// Every installed version of this id, ascending.
    pub versions: Vec<String>,
    /// Runtime kind, in the host's spelling (`process`, `mcp`, `native`, ...).
    pub runtime: String,
    /// Trust level the host resolved for the package.
    pub trust: String,
    /// Signature verdict: `unsigned`, `verified`, `untrusted` or `invalid`.
    pub signature: String,
    /// Where the package came from.
    pub source: String,
    /// Lifecycle state: `active`, `installed`, `failed`, ...
    pub state: String,
    /// Health: `healthy`, `degraded`, `unhealthy` or `unknown`.
    pub health: String,
    /// Whether configuration enabled it.
    pub configured: bool,
    /// Whether it is loaded and its contributions applied.
    pub applied: bool,
    /// Qualified tool names it contributed.
    pub tools: Vec<String>,
    /// Qualified command names it contributed.
    pub commands: Vec<String>,
    /// Qualified panel names it contributed.
    pub panels: Vec<String>,
    /// Capability decisions resolved by the extension host's policy engine.
    #[serde(default)]
    pub capabilities: Vec<ExtensionCapabilityView>,
    /// The last failure, when it failed.
    pub error: Option<String>,
}

/// One extension capability's policy result and scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionCapabilityView {
    /// Stable capability name.
    pub capability: String,
    /// Whether policy currently allows the capability.
    pub allowed: bool,
    /// Granted scope, when allowed.
    pub scope: Option<String>,
    /// Policy's denial reason, when denied.
    pub reason: Option<String>,
    /// Risk class assigned by the capability registry.
    pub risk: String,
    /// Whether the extension requires this capability to activate.
    pub required: bool,
}

/// The extensions this build has installed, and whether each is in force.
///
/// `state` is one of:
/// - `ready` - the roster was read; `extensions` is what the host reported.
/// - `failed` - the extension host could not be opened; `error` says why.
/// - `unsupported` - this executor carries no extension host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionsReport {
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// The rows, name-ordered.
    pub extensions: Vec<ExtensionView>,
    /// Why the roster could not be read, when it could not.
    pub error: Option<String>,
}

/// One delegated child run, as the desktop's task graph and header draw it.
///
/// The row is the registry's own record, translated: the state spelling is the
/// registry's (`running`, `done`, `failed`), and the token counts are the ones
/// the child reported, with the estimate kept beside them for a run that has
/// not priced its request yet. The prompt and the result travel with the row
/// because a terminal has no hover card: opening a run shows the task it was
/// given and the answer it produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentRunView {
    /// Stable run id, the row's key.
    pub id: String,
    /// The conversation this run belongs to.
    pub session: String,
    /// The run id of the direct parent, or the session id at the root.
    pub parent: String,
    /// How deep below the session root this run sits; one is a direct child.
    pub depth: usize,
    /// The short description the model supplied.
    pub title: String,
    /// The task text handed to the child.
    pub prompt: String,
    /// Lifecycle state: `running`, `done` or `failed`.
    pub status: String,
    /// When the child started, in unix milliseconds.
    pub started_at_ms: u64,
    /// When the child finished, in unix milliseconds, when it has.
    pub finished_at_ms: Option<u64>,
    /// A rough token estimate, shown while the child has not reported usage.
    pub estimated_tokens: u64,
    /// Prompt tokens the child reported.
    pub prompt_tokens: u64,
    /// Completion tokens the child reported.
    pub completion_tokens: u64,
    /// Monotonic counter for live updates; a client rebuilds when it moves.
    pub revision: u64,
    /// How many prompts the reader queued behind the child's current turn.
    pub pending: usize,
    /// Transcript rows the run has recorded, for the graph's step count.
    pub steps: usize,
    /// The final answer, clipped the way the desktop's hint is.
    pub result_hint: Option<String>,
    /// The failure text, when it failed.
    pub error: Option<String>,
}

/// A conversation's delegated child runs.
///
/// `state` is one of:
/// - `ready` - the registry was read; `runs` is what it holds.
/// - `unsupported` - this executor carries no subagent registry, so no turn it
///   ran could delegate. That is not the same answer as an empty roster: a
///   client that showed "no child runs" for a build without the tool would
///   send the reader looking for a delegation that was never possible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentReport {
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// The runs, oldest first, filtered to the requested conversation.
    pub runs: Vec<SubagentRunView>,
}

/// The federation node this Host runs and the peers it trusts.
///
/// The desktop's Federation tab reads two things: the node's identity, which
/// names it to its peers, and the peer store, which says who is trusted for
/// what. A Host serves the same two from the same files - or from the live
/// node when this process runs one - so a terminal and a window cannot disagree
/// about a peer's state or its grants.
///
/// `state` is one of:
/// - `ready` - the store was read; `node` and `peers` hold what it says.
/// - `off` - federation is switched off in this build's settings. The roster
///   is still read, because a node can be switched off after being trusted by
///   peers.
/// - `no_node` - no identity file exists yet, so this machine has no node to
///   name; this is the first-launch state, not an error.
/// - `unsupported` - this executor carries no federation surface.
/// - `unreadable` - the peer store exists but could not be decoded; `error`
///   says why, and `peers` is empty because no row could be trusted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FederationReport {
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// Whether this build's settings run a federation node.
    pub enabled: bool,
    /// The node profile these files belong to.
    pub profile: String,
    /// The node's own id, empty when no identity exists yet.
    pub node: String,
    /// The name the node shows to peers, as the desktop's hero row shows it.
    pub name: String,
    /// The peers, in store order.
    pub peers: Vec<FederationPeerView>,
    /// Why the store could not be read, when it could not.
    pub error: Option<String>,
}

/// One peer relationship, as the desktop's Federation tab lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FederationPeerView {
    /// The peer's node id, full length: a terminal has no clip to shorten it.
    pub node: String,
    /// The note the enrollment carried, when it carried one.
    pub name: Option<String>,
    /// The trust state: `invited`, `pending`, `trusted`, `suspended` or
    /// `revoked`.
    pub state: String,
    /// The capabilities it may invoke.
    pub grants: Vec<FederationGrantView>,
    /// When it was enrolled, in unix milliseconds.
    pub enrolled_ms: u64,
    /// When the enrollment stops being valid, when it does.
    pub expires_ms: Option<u64>,
}

/// One capability a peer may invoke, with the scope that bounds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FederationGrantView {
    /// The granted capability id.
    pub capability: String,
    /// Operations it permits; empty permits every operation.
    pub operations: Vec<String>,
    /// Resource selectors it permits; empty permits every resource.
    pub resources: Vec<String>,
    /// Workspace selectors it permits; empty permits every workspace.
    pub workspaces: Vec<String>,
    /// When it stops being valid, when it does.
    pub expires_ms: Option<u64>,
}

/// One session's latest review, as the desktop's Review tab reads it.
///
/// The desktop prepares this from the workspace's review store: a changeset is
/// the set of files one turn changed, captured at the tool boundary before and
/// after every write, and the review is the deterministic reading of that set -
/// status, risk, findings, verifications and any recorded decision. A terminal
/// reads the same store through the same query, so the two interfaces cannot
/// disagree about what a turn touched.
///
/// `state` is one of:
/// - `ready` - a sealed changeset was read; `changes` holds its rows.
/// - `none` - no sealed changeset exists for this session yet: no turn changed
///   a file, or the turn that did has not finished.
/// - `unsupported` - this executor carries no review surface, or the Host names
///   no workspace root to read the store from.
/// - `unreadable` - the store exists but its records could not be read; `error`
///   says why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewReport {
    /// What was found, per the list in the type documentation.
    pub state: String,
    /// Why the review could not be read, or why the asked-for diff is empty.
    pub error: Option<String>,
    /// The session this report is about.
    pub session: String,
    /// The changeset id, when one exists.
    pub change_set: Option<String>,
    /// The turn that produced it, when the record names one.
    pub turn: Option<String>,
    /// The task that produced it, when the record names one.
    pub task: Option<String>,
    /// How the changes were captured: `exact`, `observed`, `mixed`, `legacy`.
    pub capture: String,
    /// The changeset lifecycle: `open`, `sealed`, `partial` or `failed`.
    pub changeset_status: String,
    /// Digest of the sealed manifest; the value a decision binds to.
    pub digest: Option<String>,
    /// The review lifecycle: `draft`, `in_review`, `changes_requested`,
    /// `approved`, `rejected` or `stale`.
    pub status: String,
    /// The deterministic risk level: `low`, `medium`, `high` or `critical`.
    pub risk: String,
    /// Whether the risk findings say a human must decide.
    pub requires_human: bool,
    /// The deterministic findings, in the order the review recorded them.
    pub findings: Vec<ReviewFindingView>,
    /// Verifications attached to this review, oldest first.
    pub verifications: Vec<ReviewVerificationView>,
    /// Decisions recorded against this review, oldest first.
    pub decisions: Vec<ReviewDecisionView>,
    /// Every changed file, in the order the changeset recorded them.
    pub changes: Vec<ReviewChangeView>,
    /// The prepared diff of the change the request named, when it named one.
    pub diff: Option<ReviewDiffView>,
}

/// One file a turn changed, as the review list draws it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewChangeView {
    /// Stable change id; a request names it to ask for the diff.
    pub id: String,
    /// Path relative to the workspace root, with forward slashes.
    pub path: String,
    /// The net effect: `created`, `modified`, `deleted`, `renamed`, `mode`
    /// or `unchanged`.
    pub kind: String,
    /// The previous path, for a rename.
    pub from: Option<String>,
    /// The tools whose calls touched this file, in first-use order.
    pub tools: Vec<String>,
    /// How many revisions the file has. A file edited twice in one turn is one
    /// row with two revisions, the way the desktop lists it.
    pub revisions: usize,
    /// Whether any side of a revision could not be captured.
    pub partial: bool,
}

/// One deterministic finding, as the review detail draws it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewFindingView {
    /// The affected change id, when the finding is narrow enough.
    pub change: Option<String>,
    /// Severity: `info`, `low`, `medium`, `high` or `critical`.
    pub severity: String,
    /// Category: `correctness`, `security`, `test`, `performance`, ...
    pub kind: String,
    /// Lifecycle: `open`, `accepted`, `dismissed`, `fixed` or `needs_human`.
    pub status: String,
    /// Short title.
    pub title: String,
    /// Explanation.
    pub detail: String,
}

/// One verification record attached to a review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewVerificationView {
    /// Kind label: `build`, `test`, `lint`, `manual`, ...
    pub kind: String,
    /// Status label, in the recorder's spelling.
    pub status: String,
    /// The command that ran, when one did.
    pub command: Option<String>,
    /// Its exit code, when it reported one.
    pub exit_code: Option<i32>,
    /// One-line summary.
    pub summary: String,
}

/// One recorded review decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewDecisionView {
    /// `approve`, `reject` or `request_changes`.
    pub decision: String,
    /// Who decided.
    pub actor: String,
    /// Why, when a rationale was recorded.
    pub rationale: String,
    /// The digest the decision was made against.
    pub digest: String,
    /// When it was recorded, in unix milliseconds.
    pub at_ms: u64,
}

/// The prepared diff of one changed file.
///
/// A diff is read on demand, never with the change list: the desktop builds it
/// when a row is clicked, and the terminal asks for it by naming the change. The
/// rows are the same prepared comparison - text lines with their structural
/// nodes - so a file reads the same in both interfaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewDiffView {
    /// Path relative to the workspace root.
    pub path: String,
    /// Net effect label.
    pub kind: String,
    /// Whether the content is binary, where a text diff cannot be built.
    pub binary: bool,
    /// Added lines, counted while the diff was built.
    pub added: usize,
    /// Removed lines, counted the same way.
    pub removed: usize,
    /// Old byte length.
    pub old_bytes: u64,
    /// New byte length.
    pub new_bytes: u64,
    /// Named definitions the change added, removed or modified.
    pub structural: Vec<ReviewNodeView>,
    /// The text rows, in one unified column.
    pub lines: Vec<ReviewDiffLineView>,
    /// Why a full diff is unavailable, when it is.
    pub note: Option<String>,
}

/// One text row of a prepared diff.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewDiffLineView {
    /// `+`, `-` or one space for context.
    pub mark: String,
    /// Old line number, one-based.
    pub old_line: Option<u32>,
    /// New line number, one-based.
    pub new_line: Option<u32>,
    /// The line's text.
    pub text: String,
}

/// One structural node the diff named.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewNodeView {
    /// Stable id inside the diff.
    pub id: String,
    /// Parent id, for indentation.
    pub parent: Option<String>,
    /// Kind label: `function`, `struct`, `config-key`, `section`, ...
    pub kind: String,
    /// Reader-facing qualified name.
    pub name: String,
    /// `added`, `removed`, `modified` or `moved`.
    pub change: String,
    /// Old line, one-based.
    pub old_line: Option<u32>,
    /// New line, one-based.
    pub new_line: Option<u32>,
    /// Extra detail.
    pub detail: String,
}

/// What a client needs to offer the desktop's model and preset pickers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatalogueView {
    /// Providers that carry models, the active provider first.
    pub providers: Vec<ProviderModels>,
    /// Preset ids this workspace may run.
    pub presets: Vec<String>,
    /// Skills this build registered, name-ordered.
    ///
    /// Empty on a workspace that installed none, which is why a client draws
    /// the section only when it has rows.
    pub skills: Vec<SkillView>,
    /// MCP servers the Host's build knows about, name-ordered.
    ///
    /// The list is the pool's roster rather than the settings document: a
    /// client that shows a configured server and a client that shows a
    /// connectable one must agree, or the tools a client claims exist are not
    /// the tools a turn can call.
    pub mcp: Vec<McpServerView>,
    /// The directory this Host serves, expanded.
    ///
    /// One Host serves one workspace, and a client that attached to a Host
    /// another process started cannot know which folder that is. Naming it
    /// here keeps a client's `@` list, shell and workspace reads on the tree
    /// the Host's tools work in rather than on the directory the client
    /// happened to be launched from.
    pub workspace_root: String,
}

/// Ask for an event subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscribe {
    /// Resume position. `None` means "follow from the current head"; the
    /// client should already hold a snapshot in that case.
    pub cursor: Option<Cursor>,
}

/// A durable journal position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cursor {
    /// Monotonic sequence within the Host's declared workspace stream.
    pub sequence: u64,
}

/// Successful subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscribed {
    /// Connection-local subscription handle.
    pub subscription_id: u64,
    /// Events after the requested cursor, in sequence order. Empty when the
    /// client asked to follow from the current head.
    pub replay: Vec<EventEnvelope>,
    /// Position of the last replayed event (or the head when no replay was
    /// requested). Live events start at `next_cursor.sequence + 1`.
    pub next_cursor: Cursor,
}

/// The requested cursor fell outside the retained replay window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorExpired {
    /// Current head sequence.
    pub head: u64,
    /// Reconciliation snapshot; replace local state with it and resubscribe
    /// from `head`.
    pub snapshot: Snapshot,
}

/// Envelope fields required for every committed fact (RFC-013).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Stable event identity; consumers deduplicate on it.
    pub event_id: EventId,
    /// Workspace the event belongs to.
    pub workspace_id: WorkspaceId,
    /// Declared stream whose sequence this event advances.
    pub stream_id: String,
    /// Monotonic sequence within `stream_id`.
    pub sequence: u64,
    /// Session the event belongs to, when any.
    pub session_id: Option<SessionId>,
    /// Execution the event belongs to, when any.
    pub execution_id: Option<ExecutionId>,
    /// Task the event belongs to, when any.
    pub task_id: Option<TaskId>,
    /// The command or event that caused this event.
    pub causation_id: Option<String>,
    /// Correlation across a larger operation.
    pub correlation_id: Option<String>,
    /// Commit timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Payload schema version.
    pub schema_version: u16,
    /// The fact.
    pub payload: EventPayload,
}

/// Durable domain facts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// A session was created.
    SessionCreated {
        /// Display title.
        title: String,
        /// The configuration the session started with.
        #[serde(default)]
        config: SessionConfig,
        /// The session this one forked, when it is a branch rather than a fresh
        /// chat. The rows the fork opens with are not journalled twice: the fold
        /// reads them from the source session's own events, which already
        /// precede this one, so a branch costs one durable fact instead of a copy
        /// of the transcript.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forked_from: Option<SessionId>,
        /// Idempotency key of the accepting command.
        command_id: CommandId,
        /// Hash of the accepted command payload.
        payload_hash: PayloadHash,
    },
    /// One session's model, preset, permission or plan mode changed.
    SessionConfigured {
        /// The configuration in force after the change.
        config: SessionConfig,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// The model asked for one tool call inside a turn.
    ToolCallStarted {
        /// Position of the call within its turn, counting from one.
        step: u64,
        /// Tool name the model called.
        tool: String,
        /// One-line description of the arguments.
        summary: String,
        /// The arguments as JSON, capped for transport.
        args: String,
    },
    /// A tool call returned its result.
    ToolCallFinished {
        /// Position of the call within its turn.
        step: u64,
        /// Tool name.
        tool: String,
        /// Whether the result reports a failure.
        ok: bool,
        /// Output text the model received, capped for transport.
        output: String,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },
    /// The model replaced its working plan through `update_plan`.
    ///
    /// The plan is the desktop's to-do panel and the terminal's plan block:
    /// it is durable session state, so it is journalled as a fact and every
    /// client fold reads the same list instead of keeping its own copy.
    PlanUpdated {
        /// The whole plan as the model last wrote it.
        #[serde(default)]
        plan: Vec<PlanItemView>,
    },
    /// A session title changed.
    SessionRenamed {
        /// New title.
        title: String,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// One session was parked in the bin, or taken back out of it.
    ///
    /// The whole lifecycle state travels, like the goal's: a parked session
    /// leaves every list and the snapshot a reconnecting client reconciles
    /// against, while the fold keeps its rows so a restore brings the same
    /// chat back. `trashed` is `false` only on a restore.
    SessionTrashChanged {
        /// Whether the session is parked after this fact.
        trashed: bool,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// One session was pinned or unpinned.
    ///
    /// Ordering is answered by the fold, not by the client: `ListSessions`
    /// sorts pinned rows ahead of the rest, so two windows that pinned the
    /// same chat show it in the same place after the event.
    SessionPinChanged {
        /// Whether the session is pinned after this fact.
        pinned: bool,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// One session was archived or returned to the list.
    ///
    /// The rows stay folded, unlike a purge: the session leaves
    /// `ListSessions` and the snapshot a reconnecting client reconciles
    /// against, and `Archived` is the one reader that asks for it.
    SessionArchivedChanged {
        /// Whether the session is archived after this fact.
        archived: bool,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// One parked session was dropped for good.
    ///
    /// Purge is the bin's only irreversible fact: from this event on the fold
    /// drops the projection, so `Session` stops answering for it and a restore
    /// is refused. The journal keeps the earlier events because a replay must
    /// stay ordered.
    SessionPurged {
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// The Host durably accepted one user turn and owns its execution.
    TurnRequested {
        /// Projected user message id.
        message_id: MessageId,
        /// User text.
        text: String,
        /// Idempotency key.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// A turn produced its final assistant message. This event also commits
    /// `Completed`, so a crash cannot separate the text from its completion.
    TurnSucceeded {
        /// Projected assistant message id.
        message_id: MessageId,
        /// Assistant text.
        text: String,
        /// Provider-reported token usage for the turn, when the model reported
        /// one. The desktop's usage pill and its token dialog read a session's
        /// turns the same way, so the buckets ride on the message the turn
        /// committed rather than on a client-side counter a reconnect loses.
        /// Absent for a turn whose provider sent no usage block, and for a
        /// journal written before the field existed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TurnUsage>,
    },
    /// A turn failed.
    TurnFailed {
        /// Error detail safe to show a client.
        error: String,
    },
    /// A turn was cancelled by an explicit command.
    ExecutionCancelled {
        /// Idempotency key of the cancelling command.
        command_id: CommandId,
        /// Payload hash.
        payload_hash: PayloadHash,
    },
    /// Recovery found an execution without a terminal event. Unknown side
    /// effects are never retried automatically.
    ExecutionInterrupted {
        /// Why recovery interrupted it.
        reason: String,
    },
    /// An executor is waiting for an operator decision.
    ApprovalRequested {
        /// Pending approval id.
        approval_id: ApprovalId,
        /// Human-readable action summary.
        summary: String,
        /// Exact action the decision authorizes.
        action: ApprovalTarget,
        /// Digest binding approval id, action, scope and expiry.
        action_digest: ActionDigest,
        /// Deadline.
        expires_at: DateTime<Utc>,
    },
    /// An approval reached a terminal decision.
    ApprovalDecided {
        /// Approval id.
        approval_id: ApprovalId,
        /// Terminal decision.
        decision: ApprovalDecision,
        /// Digest the decision was bound to.
        action_digest: ActionDigest,
        /// Command id for operator decisions; absent for expiry/cancel.
        command_id: Option<CommandId>,
    },
    /// One session's older history was replaced by a checkpoint.
    ///
    /// The checkpoint is a durable fact of its own: a client that reconnects
    /// reads the folded transcript, and recovery folds it back the same way.
    SessionCompacted {
        /// The checkpoint text the summarizer wrote.
        summary: String,
        /// How many of the oldest transcript messages it replaced.
        folded: u64,
        /// Idempotency key of the accepting command.
        command_id: CommandId,
        /// Hash of the accepted command payload.
        payload_hash: PayloadHash,
    },
    /// One compaction attempt did not produce a checkpoint.
    ///
    /// The transcript is untouched: a refused or failed summarizer leaves the
    /// history the client already has, and the error is what the client shows.
    SessionCompactionFailed {
        /// Why no checkpoint was installed.
        error: String,
        /// Idempotency key of the accepting command.
        command_id: CommandId,
        /// Hash of the accepted command payload.
        payload_hash: PayloadHash,
    },
    /// One session's goal was set, replaced, advanced or cleared.
    ///
    /// The whole goal travels, never an increment: a client renders exactly
    /// what the Host holds, and a replay rebuilds the same state from one fact.
    /// `goal` is `None` only after a clear.
    GoalChanged {
        /// The session's goal after the change.
        goal: Option<SessionGoalView>,
        /// Idempotency key of the accepting command; absent when a turn's own
        /// goal tools moved the goal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        /// Hash of the accepted command payload; absent with no command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload_hash: Option<PayloadHash>,
    },
    /// The reader's note about one session was recorded or cleared.
    ///
    /// The whole note travels, never a patch: a client renders what the Host
    /// holds, and `feedback` is `None` only after a clear.
    FeedbackChanged {
        /// The session's note after the change.
        feedback: Option<String>,
        /// Idempotency key of the accepting command.
        command_id: CommandId,
        /// Hash of the accepted command payload.
        payload_hash: PayloadHash,
    },
    /// The Host entered the `Ready` phase.
    HostReady {
        /// Host process identity.
        instance_id: HostInstanceId,
    },
    /// The Host began draining.
    HostDraining {
        /// Chosen shutdown mode.
        mode: ShutdownMode,
    },
    /// The Host stopped.
    HostStopped {
        /// Final lifecycle phase.
        phase: HostPhase,
    },
}

/// The exact operation an approval authorizes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalTarget {
    /// Tool or capability name.
    pub tool: String,
    /// Action within the tool.
    pub action: String,
    /// Resource the action touches, when any.
    pub resource: Option<String>,
}

/// An executor's request for operator approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Human-readable action summary.
    pub summary: String,
    /// Exact action.
    pub action: ApprovalTarget,
    /// How long the request stays pending, in seconds.
    pub expires_in_secs: u64,
}

/// Terminal approval state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Operator allowed the action.
    Approved,
    /// Operator refused it.
    Rejected,
    /// The deadline passed.
    Expired,
    /// The owning execution was cancelled.
    Cancelled,
}

/// Projected chat role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Human input.
    User,
    /// Assistant output.
    Assistant,
}

/// Provider-reported token usage for one turn, as a client sums it.
///
/// The three buckets are the desktop's [`crate::model::MessageUsage`]: the
/// provider's prompt figure, how much of that it served from cache, and the
/// completion. A client derives a session total by adding them, so the
/// distinction between cached and uncached input survives the wire instead of
/// being collapsed into one number the reader cannot explain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TurnUsage {
    /// Prompt tokens the provider billed for, cached or not.
    pub prompt_tokens: u64,
    /// Prompt tokens it served from cache.
    pub cached_tokens: u64,
    /// Completion tokens.
    pub completion_tokens: u64,
}

impl TurnUsage {
    /// Prompt tokens the provider did not serve from cache.
    #[must_use]
    pub fn uncached_input(&self) -> u64 {
        self.prompt_tokens.saturating_sub(self.cached_tokens)
    }

    /// Billed input plus output: the figure the desktop's pill counts.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }

    /// Whether no provider reported a figure for this turn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// One projected message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageView {
    /// Message identity.
    pub message_id: MessageId,
    /// Role.
    pub role: MessageRole,
    /// Text.
    pub text: String,
    /// When it was committed.
    pub at: DateTime<Utc>,
    /// Execution that produced or consumed it.
    pub execution_id: Option<ExecutionId>,
    /// Whether this message is a compaction checkpoint rather than something
    /// a participant said. The transcript renders it as its own row, and a
    /// resumed turn reads it as background instead of as the reader's words.
    #[serde(default)]
    pub compaction: bool,
    /// Usage the turn behind this assistant message reported, when one did.
    ///
    /// A client reads a session's usage by summing the rows that carry it, the
    /// way the desktop sums its chat's turns; a message with no usage (the
    /// reader's own turns, a checkpoint) contributes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
}

/// Execution lifecycle (RFC-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Accepted, not yet started.
    Queued,
    /// Running normally.
    Running,
    /// Blocked on an operator decision.
    WaitingApproval,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled by explicit command.
    Cancelled,
    /// Host restarted while the execution was live. External side effects are
    /// unknown; never retried automatically.
    Interrupted,
    /// Requires an operator to reconcile the unknown outcome.
    NeedsIntervention,
}

/// One projected execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionView {
    /// Execution identity.
    pub execution_id: ExecutionId,
    /// Current state.
    pub status: ExecutionStatus,
    /// When the turn was accepted.
    pub started_at: DateTime<Utc>,
    /// When it reached a terminal state.
    pub finished_at: Option<DateTime<Utc>>,
    /// Failure or interruption detail.
    pub detail: Option<String>,
}

/// Projected approval state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Awaiting an operator.
    Pending,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
    /// Deadline passed.
    Expired,
    /// Owning execution cancelled.
    Cancelled,
}

/// One projected approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalView {
    /// Approval identity.
    pub approval_id: ApprovalId,
    /// Owning session.
    pub session_id: SessionId,
    /// Owning execution.
    pub execution_id: ExecutionId,
    /// Summary shown to the operator.
    pub summary: String,
    /// Exact action.
    pub action: ApprovalTarget,
    /// Digest to echo back with the decision.
    pub action_digest: ActionDigest,
    /// Status.
    pub status: ApprovalStatus,
    /// Deadline.
    pub expires_at: DateTime<Utc>,
}

/// One tool call a turn made, as the transcript shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallView {
    /// Execution whose turn made the call.
    #[serde(default)]
    pub execution_id: Option<ExecutionId>,
    /// Position of the call within its turn, counting from one.
    pub step: u64,
    /// Tool name.
    pub tool: String,
    /// One-line description of the arguments.
    pub summary: String,
    /// The arguments as JSON.
    pub args: String,
    /// Where the call stands.
    pub status: ToolCallStatus,
    /// Output the model received, once the call returned.
    pub output: Option<String>,
    /// Wall-clock duration, once the call returned.
    pub duration_ms: Option<u64>,
    /// When the model asked for the call.
    pub started_at: DateTime<Utc>,
    /// When the call returned.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Where one step of the model's working plan stands.
///
/// The vocabulary is the agent's own: the Host journals what the model wrote,
/// so a client never invents a fourth state for a step the plan does not
/// describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    /// Not started yet.
    #[default]
    Pending,
    /// The step the model is working on.
    InProgress,
    /// Finished.
    Completed,
}

/// One step of the model's working plan, as a panel draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItemView {
    /// Short, human-readable step description.
    pub text: String,
    /// Where the step stands.
    #[serde(default)]
    pub status: PlanItemStatus,
}

/// Where one projected tool call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// The model asked for the call; the tool is running.
    Running,
    /// The call returned a non-error result.
    Completed,
    /// The call returned an error result.
    Failed,
}

/// Full projected session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionProjection {
    /// Session identity.
    pub session_id: SessionId,
    /// Journal sequence this projection represents.
    pub version: u64,
    /// Display title.
    pub title: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last mutation time.
    pub updated_at: DateTime<Utc>,
    /// Transcript.
    pub messages: Vec<MessageView>,
    /// Tool calls the session's turns made, oldest first.
    #[serde(default)]
    pub tool_calls: Vec<ToolCallView>,
    /// The model's working plan as it last wrote it, empty before the first
    /// `update_plan` call or after a call that cleared it.
    #[serde(default)]
    pub plan: Vec<PlanItemView>,
    /// Executions, oldest first.
    pub executions: Vec<ExecutionView>,
    /// Approvals, oldest first.
    pub approvals: Vec<ApprovalView>,
    /// Configuration the session's next turns run with.
    #[serde(default)]
    pub config: SessionConfig,
    /// The last compaction outcome, when the session was ever compacted.
    ///
    /// A checkpoint replaces the transcript rows it folded, so the outcome is
    /// what tells a client WHY history got shorter and how much it folded. It
    /// is absent until a compaction lands or fails.
    #[serde(default)]
    pub compaction: Option<CompactionOutcomeView>,
    /// The objective the session pursues, once an operator or the model set
    /// one. Absent means this session has no goal.
    #[serde(default)]
    pub goal: Option<SessionGoalView>,
    /// The reader's note about this session, written by `/feedback`. Absent
    /// means no note was ever recorded, which is what makes clearing it
    /// different from storing an empty one.
    #[serde(default)]
    pub feedback: Option<String>,
    /// Whether the session is parked in the workspace's bin.
    ///
    /// A parked session is out of every list and out of the snapshot a
    /// reconnecting client reconciles against, but its rows stay folded: a
    /// restore returns the chat with its transcript, calls, plan and goal
    /// intact. Only a purge drops it.
    #[serde(default)]
    pub trashed: bool,
    /// Whether the session is pinned ahead of the unpinned rows.
    ///
    /// Pinning changes where a row sorts, never whether it is there: a pinned
    /// session is in `ListSessions` exactly as it would be unpinned.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the session is archived out of the list.
    ///
    /// Archiving is the sidebar's filter, not the bin: the rows stay folded
    /// and `Archived` still lists the session, so an unarchive returns the
    /// same chat with its transcript, calls, plan and goal.
    #[serde(default)]
    pub archived: bool,
}

/// Where a session's goal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    /// Being pursued; rounds may advance.
    #[default]
    Active,
    /// Deliberately stopped, resumable.
    Paused,
    /// The objective is achieved.
    Completed,
    /// Stopped by a concrete condition that must change first.
    Blocked,
}

impl GoalState {
    /// Whether further rounds may be counted from this state.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Whether the goal is settled for good, rather than stopped and resumable.
    #[must_use]
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Completed | Self::Blocked)
    }
}

/// The goal one session pursues, as a client renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGoalView {
    /// What the work is for, as one statement.
    pub objective: String,
    /// Where the goal stands.
    pub state: GoalState,
    /// Rounds spent pursuing it.
    #[serde(default)]
    pub rounds: u32,
    /// Cap on rounds; a goal always has one.
    #[serde(default = "default_max_goal_rounds")]
    pub max_rounds: u32,
    /// Why a blocked goal stopped, kept only while it is blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
}

/// The round cap a goal takes when it names none.
///
/// The same 256 the desktop uses: a goal is meant to finish, and a cap turns a
/// runaway loop into something an operator can see instead of an open-ended
/// bill.
fn default_max_goal_rounds() -> u32 {
    256
}

impl SessionGoalView {
    /// A fresh goal for `objective`, the way `/goal` creates one.
    #[must_use]
    pub fn active(objective: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            state: GoalState::Active,
            rounds: 0,
            max_rounds: default_max_goal_rounds(),
            blocked_reason: None,
        }
    }
}

/// One session's last compaction outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionOutcomeView {
    /// Whether a checkpoint was installed.
    pub ok: bool,
    /// Transcript messages the checkpoint replaced; zero when it failed.
    pub folded: u64,
    /// Checkpoint text, when one was installed.
    pub summary: Option<String>,
    /// Why no checkpoint was installed.
    pub error: Option<String>,
    /// When the Host committed the outcome.
    pub at: DateTime<Utc>,
}

/// Compact session row for pickers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identity.
    pub session_id: SessionId,
    /// Title.
    pub title: String,
    /// Version.
    pub version: u64,
    /// Last mutation time.
    pub updated_at: DateTime<Utc>,
    /// Message count.
    pub message_count: usize,
    /// The newest message's first line, if any.
    pub preview: Option<String>,
    /// An execution that is not terminal.
    pub active_execution: Option<ExecutionStatus>,
    /// Number of pending approvals.
    pub pending_approvals: usize,
    /// Whether the session is pinned ahead of the unpinned rows.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the session left the list into the Archived filter.
    #[serde(default)]
    pub archived: bool,
}

/// Host lifecycle phase (RFC-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostPhase {
    /// Starting up.
    Initializing,
    /// Rebuilding durable state; mutations are refused.
    Recovering,
    /// Accepting commands and queries.
    Ready,
    /// Refusing new work; finishing or interrupting in-flight work.
    Draining,
    /// Stopped cleanly.
    Stopped,
    /// Unusable.
    Failed,
}

/// Host activity, modeled separately from lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostActivity {
    /// No in-flight execution.
    Idle,
    /// At least one execution is running.
    Active,
    /// Running with a recorded degradation (for example executor errors).
    Degraded,
}

/// Host status projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostStatus {
    /// Host process identity.
    pub instance_id: HostInstanceId,
    /// Workspace served.
    pub workspace_id: WorkspaceId,
    /// Negotiated protocol version.
    pub protocol_version: u16,
    /// Lifecycle phase.
    pub phase: HostPhase,
    /// Activity.
    pub activity: HostActivity,
    /// Current journal head.
    pub journal_head: u64,
    /// Process start time.
    pub started_at: DateTime<Utc>,
    /// Number of non-terminal executions.
    pub active_executions: usize,
}

/// One operation's completion view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationView {
    /// The reference.
    pub operation_id: OperationId,
    /// `session`, `execution` or `approval`.
    pub kind: String,
    /// Domain status string.
    pub status: String,
    /// Owning session, when any.
    pub session_id: Option<SessionId>,
    /// Owning execution, when any.
    pub execution_id: Option<ExecutionId>,
}

/// Reconciliation state for a client whose cursor expired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Journal head this snapshot represents.
    pub head: u64,
    /// Generation time.
    pub generated_at: DateTime<Utc>,
    /// Every session projection.
    pub sessions: Vec<SessionProjection>,
    /// Host status.
    pub status: HostStatus,
}

/// One client request frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientFrame {
    /// Client-chosen correlation for the response.
    pub request_id: u64,
    /// The request.
    pub request: Request,
}

/// Application requests, independent of transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Register the connection. Must be first.
    Hello(Hello),
    /// Mutate state.
    Command(CommandEnvelope),
    /// Read a projection.
    Query(QueryEnvelope),
    /// Invoke a privileged control.
    Control(ControlEnvelope),
    /// Subscribe to durable events.
    Subscribe(Subscribe),
    /// Drop one subscription.
    Unsubscribe {
        /// Subscription handle.
        subscription_id: u64,
    },
    /// Liveness check.
    Ping,
}

/// One Host response frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    /// Echo of the client's request correlation.
    pub request_id: u64,
    /// The response.
    pub response: Response,
}

/// Responses to application requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Registration succeeded.
    Welcome(Welcome),
    /// Mutating request outcome.
    Command(CommandOutcome),
    /// Query result.
    Query(QueryResult),
    /// Subscription established.
    Subscribed(Subscribed),
    /// Cursor outside the replay window.
    CursorExpired(CursorExpired),
    /// Subscription dropped.
    Unsubscribed {
        /// Subscription handle.
        subscription_id: u64,
    },
    /// A query was refused; no projection is disclosed.
    QueryRejected(Rejection),
    /// Liveness response.
    Pong,
    /// Transport or protocol error.
    Error(ProtocolError),
}

/// A protocol-level error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    /// Machine-readable class.
    pub code: ProtocolErrorCode,
    /// Human-readable detail.
    pub message: String,
}

/// Protocol error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    /// Dialect version mismatch.
    ProtocolVersion,
    /// Peer identity is not authorized.
    Unauthorized,
    /// Malformed frame or request.
    BadRequest,
    /// Frame exceeded the adapter bound.
    FrameTooLarge,
    /// Request budget exhausted.
    RateLimited,
    /// Host not ready for this request.
    NotReady,
    /// Unexpected internal failure.
    Internal,
}

impl ProtocolErrorCode {
    /// A short stable string for logs and client-side errors.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolVersion => "protocol_version",
            Self::Unauthorized => "unauthorized",
            Self::BadRequest => "bad_request",
            Self::FrameTooLarge => "frame_too_large",
            Self::RateLimited => "rate_limited",
            Self::NotReady => "not_ready",
            Self::Internal => "internal",
        }
    }
}

/// One frame the Host sends, including asynchronous events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// Response to a client request.
    Response(ResponseFrame),
    /// One committed event for a subscription.
    Event {
        /// Subscription handle.
        subscription_id: u64,
        /// The committed event.
        ///
        /// Boxed like the projections above: an event carries a whole payload,
        /// and a subscription channel holds frames while a client catches up.
        event: Box<EventEnvelope>,
    },
    /// One non-durable fact from a running turn.
    ///
    /// Activity exists so a client can show a turn while it runs - the desktop
    /// streams assistant text the same way. It is never journaled: the
    /// committed facts remain the only source of truth, and a dropped activity
    /// frame costs a reader nothing it cannot recover from the turn's commit.
    Activity {
        /// Subscription handle.
        subscription_id: u64,
        /// Session whose turn reported it.
        session_id: SessionId,
        /// Execution whose turn reported it.
        execution_id: ExecutionId,
        /// What the turn reported.
        activity: ExecutionActivity,
    },
    /// This subscriber fell behind and missed activity.
    ///
    /// Committed events are unaffected - replay from the last cursor recovers
    /// them - but a client holding partial live text must drop it instead of
    /// presenting a gap as if it were the answer.
    ActivityReset {
        /// Subscription handle.
        subscription_id: u64,
    },
    /// The subscription ended; the client should resubscribe from its cursor.
    SubscriptionClosed {
        /// Subscription handle.
        subscription_id: u64,
        /// Why it ended.
        reason: SubscriptionClosedReason,
    },
}

/// What a running turn reports between its acceptance and its terminal fact.
///
/// The payload names the fact, not the turn: the frame carries the session and
/// execution, so one activity kind serves every turn on a subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecutionActivity {
    /// More assistant text arrived. Deltas concatenate in arrival order.
    AssistantText {
        /// The next piece of the answer.
        text: String,
    },
}

/// Why a subscription ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionClosedReason {
    /// The connection's bounded event queue overflowed. Durable events remain
    /// available by replay from the client's last cursor.
    SlowConsumer,
    /// The Host is stopping.
    HostShutdown,
    /// The client asked to unsubscribe.
    ClientUnsubscribed,
}

impl SessionProjection {
    /// Compact summary for pickers.
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        let active = self
            .executions
            .iter()
            .rev()
            .find(|execution| {
                matches!(
                    execution.status,
                    ExecutionStatus::Queued
                        | ExecutionStatus::Running
                        | ExecutionStatus::WaitingApproval
                )
            })
            .map(|execution| execution.status);
        let preview = self.messages.last().map(|message| {
            let first = message.text.lines().next().unwrap_or_default().trim();
            if first.chars().count() > 80 {
                let text: String = first.chars().take(80).collect();
                format!("{text}…")
            } else {
                first.to_string()
            }
        });
        SessionSummary {
            session_id: self.session_id,
            title: self.title.clone(),
            version: self.version,
            updated_at: self.updated_at,
            message_count: self.messages.len(),
            preview,
            active_execution: active,
            pending_approvals: self
                .approvals
                .iter()
                .filter(|approval| approval.status == ApprovalStatus::Pending)
                .count(),
            pinned: self.pinned,
            archived: self.archived,
        }
    }
}
