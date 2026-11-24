//! Standalone Runtime Host (RFC-011 … RFC-016).
//!
//! The Host owns execution, session state, approvals, recovery and durable
//! state. Clients send transport-neutral requests and receive domain outcomes;
//! none of them can mutate state directly, and none of them can outlive the
//! work they started.
//!
//! # Ownership model
//!
//! - One `Core` behind an `Arc`; every request path is a short critical
//!   section over one `Mutex<Inner>`.
//! - A mutation appends its durable event, folds it into the projections and
//!   publishes it, in that order, while holding that one lock. A snapshot, the
//!   head sequence and the read models are therefore mutually consistent, and
//!   a subscriber never observes a projection that lags the event it received.
//! - Model work runs in a spawned task the Host owns. A client disconnect
//!   removes a subscriber, never a turn.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::runtime::audit::{AuditLog, AuditRecord};
use crate::runtime::authz::{Access, Authz, Principal};
use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::{
    ApprovalDecision, ApprovalRequest, ApprovalStatus, ApprovalTarget, ApprovalView,
    CommandEnvelope, CommandOutcome, CommandPayload, ControlEnvelope, ControlPayload, Cursor,
    CursorExpired, EventEnvelope, EventPayload, ExecutionActivity, ExecutionStatus, GoalState,
    HostActivity, HostPhase, HostStatus, PlanItemStatus, PlanItemView, PrincipalView,
    ProtocolError, ProtocolErrorCode, QueryEnvelope, QueryPayload, QueryResult, Rejection,
    RejectionCode, Request, Response, SessionConfig, SessionGoalView, ShutdownMode, Snapshot,
    Subscribe, Subscribed,
};
use crate::runtime::executor::{
    CompactContext, HistoryMessage, ProgressHandle, TurnContext, TurnError, TurnExecutor,
    TurnOutput, TurnProgress,
};
use crate::runtime::ids::{
    ActionDigest, ApprovalId, ClientInstanceId, CommandId, ExecutionId, HostInstanceId, MessageId,
    OperationId, PayloadHash, SessionId, WorkspaceId,
};
use crate::runtime::journal::{EventDraft, Journal, SubscriptionError};
use crate::runtime::projection::Projections;

/// How much live turn activity one subscription may fall behind before the
/// transport resets it.
///
/// Activity is not durable, so the bound is about memory per connection, not
/// recovery: a client that misses frames is told to drop partial text and keep
/// showing committed state.
const ACTIVITY_BUFFER: usize = 256;

/// Ceiling on a goal objective the Host accepts, matching the desktop's goal
/// store so a goal set from a client lands with the same bound it would have
/// had in the GUI.
const MAX_GOAL_OBJECTIVE: usize = 2000;

/// Longest feedback note the Host records. The desktop's note editor has no
/// visible cap, but a note is session state that every snapshot carries, so the
/// Host bounds it the way it bounds a goal objective.
pub(crate) const MAX_FEEDBACK_NOTE: usize = 2000;

/// A goal state's name in the spelling a client prints: lower case, the way
/// the contract serializes it.
fn goal_state_label(state: GoalState) -> String {
    format!("{state:?}").to_lowercase()
}

/// Failures that stop a Host from starting.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// Another live Host owns this data directory.
    #[error("another Runtime Host owns {path}; one Host per data root")]
    AlreadyRunning {
        /// The lock path involved.
        path: String,
    },
    /// The durable journal cannot be opened or recovered.
    #[error("runtime journal: {0}")]
    Storage(String),
    /// The configuration is unusable.
    #[error("runtime configuration: {0}")]
    Config(String),
    /// The execution backend cannot be constructed.
    #[error("execution backend: {0}")]
    Executor(String),
}

/// One authenticated connection, as the transport established it.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionContext {
    /// Principal the Host derived; never supplied by the client.
    pub principal: Principal,
    /// Client instance id minted for this connection.
    pub client_instance_id: ClientInstanceId,
}

/// A response plus the live side of a subscription, when one was created.
pub(crate) struct HostReply {
    /// The application response.
    pub response: Response,
    /// Live events to forward to the connection.
    pub subscription: Option<LiveSubscription>,
}

/// The live half of a subscription.
pub(crate) struct LiveSubscription {
    /// Subscription handle reported to the client.
    pub id: u64,
    /// Committed events from the journal.
    pub receiver: mpsc::Receiver<EventEnvelope>,
    /// Non-durable activity from running turns.
    pub activity: broadcast::Receiver<ActivityEvent>,
}

/// One non-durable fact, fanned out to every subscription of this workspace.
///
/// The Host broadcasts activity rather than filtering per subscription: a
/// workspace has one set of running turns, and a client discards the turns it
/// does not show.
#[derive(Debug, Clone)]
pub(crate) struct ActivityEvent {
    /// Session whose turn reported it.
    pub session_id: SessionId,
    /// Execution whose turn reported it.
    pub execution_id: ExecutionId,
    /// What the turn reported.
    pub activity: ExecutionActivity,
}

struct LedgerEntry {
    payload_hash: PayloadHash,
    outcome: CommandOutcome,
}

struct ExecutionRecord {
    session_id: SessionId,
    status: ExecutionStatus,
    handle: Option<JoinHandle<()>>,
}

struct ApprovalRecord {
    session_id: SessionId,
    execution_id: ExecutionId,
    action_digest: ActionDigest,
    expires_at: DateTime<Utc>,
    decided: Option<ApprovalDecision>,
    waiter: Option<oneshot::Sender<ApprovalDecision>>,
}

struct Inner {
    phase: HostPhase,
    projections: Projections,
    ledger: HashMap<CommandId, LedgerEntry>,
    executions: HashMap<ExecutionId, ExecutionRecord>,
    approvals: HashMap<ApprovalId, ApprovalRecord>,
    degraded: bool,
}

impl Inner {
    fn activity(&self) -> HostActivity {
        if self.degraded {
            HostActivity::Degraded
        } else if self.projections.active_execution_count() > 0 {
            HostActivity::Active
        } else {
            HostActivity::Idle
        }
    }
}

struct Core {
    instance_id: HostInstanceId,
    workspace_id: WorkspaceId,
    config: RuntimeConfig,
    journal: Journal,
    authz: Authz,
    audit: AuditLog,
    executor: Arc<dyn TurnExecutor>,
    /// Which executor this Host chose, for the banner its owner prints.
    executor_label: String,
    /// Fanout of non-durable activity from running turns. Sending is dropped
    /// when nobody listens, which is why a Host with no client reads nothing.
    activity_tx: broadcast::Sender<ActivityEvent>,
    started_at: DateTime<Utc>,
    phase_tx: watch::Sender<HostPhase>,
    next_connection: AtomicU64,
    inner: Mutex<Inner>,
    // Held for the lifetime of the Host: the OS releases it on crash, so a
    // stale process never blocks a restart.
    _instance_lock: hostkit::coord::FileLock,
}

/// A standalone BSH Runtime Host.
#[derive(Clone)]
pub struct RuntimeHost {
    core: Arc<Core>,
}

impl RuntimeHost {
    /// Open (or recover) the Host that owns `config.data_dir`.
    ///
    /// Recovery rebuilds every projection from the journal and marks executions
    /// that never reached a terminal fact as `Interrupted` before the lifecycle
    /// reaches `Ready`. No mutation is accepted before that.
    pub async fn open(config: RuntimeConfig) -> Result<Self, HostError> {
        let (executor, executor_label) = select_executor(&config)?;
        Self::open_with_executor(config, executor, executor_label).await
    }

    /// Open a Host around an executor the caller chose.
    ///
    /// Everything else - the instance lock, recovery, the journal and the
    /// phase machine - is exactly [`Self::open`]. Tests use this to drive a
    /// turn whose tool work is known; an embedding can use it to serve the
    /// desktop's agent over its own seam.
    pub(crate) async fn open_with_executor(
        config: RuntimeConfig,
        executor: Arc<dyn TurnExecutor>,
        executor_label: String,
    ) -> Result<Self, HostError> {
        std::fs::create_dir_all(&config.data_dir).map_err(|error| {
            HostError::Config(format!("{}: {error}", config.data_dir.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config.data_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| {
                    HostError::Config(format!("{}: {error}", config.data_dir.display()))
                })?;
        }

        // One execution authority per data root: the lock, not the PID, is the
        // ownership proof.
        let instance_lock = hostkit::coord::FileLock::try_acquire(&config.lock_path()).map_err(
            |error| match error {
                hostkit::durable::DurableError::Locked { path } => {
                    HostError::AlreadyRunning { path }
                }
                other => HostError::Config(format!("{}: {other}", config.lock_path().display())),
            },
        )?;

        let workspace_root = config.canonical_workspace();
        let workspace_id = WorkspaceId::derived(&workspace_root.display().to_string());
        let journal = Journal::open(
            config.journal_path(),
            workspace_id,
            config.replay_window,
            config.subscription_buffer,
        )
        .map_err(|error| HostError::Storage(error.to_string()))?;
        let audit = AuditLog::open(config.audit_path()).map_err(|error| {
            HostError::Config(format!("{}: {error}", config.audit_path().display()))
        })?;

        // Bound only what one subscriber may miss; committed events have their
        // own buffer and their own recovery story.
        let (activity_tx, _) = broadcast::channel(ACTIVITY_BUFFER);
        let core = Arc::new(Core {
            instance_id: HostInstanceId::new(),
            workspace_id,
            journal,
            authz: Authz::new(workspace_id),
            audit,
            executor,
            executor_label,
            activity_tx,
            started_at: Utc::now(),
            phase_tx: watch::Sender::new(HostPhase::Initializing),
            next_connection: AtomicU64::new(1),
            inner: Mutex::new(Inner {
                phase: HostPhase::Initializing,
                projections: Projections::new(),
                ledger: HashMap::new(),
                executions: HashMap::new(),
                approvals: HashMap::new(),
                degraded: false,
            }),
            config,
            _instance_lock: instance_lock,
        });
        core.recover()?;
        Ok(Self { core })
    }

    /// Identity of this Host process.
    #[must_use]
    pub fn instance_id(&self) -> HostInstanceId {
        self.core.instance_id
    }

    /// Which executor this Host runs, as its banner names it.
    ///
    /// The label is decided at open, where the shell settings, `host.toml` and
    /// the echo flag are all in one place; a reader checking which
    /// configuration a running Host took reads this rather than re-deriving it.
    #[must_use]
    pub fn executor_label(&self) -> &str {
        &self.core.executor_label
    }

    /// Workspace this Host serves.
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.core.workspace_id
    }

    /// Configuration in force.
    #[must_use]
    pub fn config(&self) -> &RuntimeConfig {
        &self.core.config
    }

    /// The OS user this Host runs as; the only principal Stage 1 trusts.
    #[must_use]
    pub fn local_uid(&self) -> u32 {
        self.core.authz.local_uid()
    }

    /// Lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> HostPhase {
        *self.core.phase_tx.borrow()
    }

    /// Watch the lifecycle phase; the transport uses it to stop accepting
    /// connections when the Host starts draining.
    #[must_use]
    pub fn watch_phase(&self) -> watch::Receiver<HostPhase> {
        self.core.phase_tx.subscribe()
    }

    /// Current status projection.
    #[must_use]
    pub fn status(&self) -> HostStatus {
        let inner = self.core.lock();
        self.core.status(&inner)
    }

    /// Begin draining. Idempotent.
    pub fn request_shutdown(&self, mode: ShutdownMode) {
        self.core.begin_draining(mode);
    }

    /// Finish draining: let in-flight turns run for the configured grace,
    /// interrupt the rest, drop subscribers and record `Stopped`.
    pub async fn drain(&self) {
        self.core.drain().await;
    }

    /// Mint the context for one freshly authenticated connection.
    pub(crate) fn connection(&self, principal: Principal) -> ConnectionContext {
        ConnectionContext {
            principal,
            client_instance_id: ClientInstanceId::new(),
        }
    }

    /// Handle one application request. Transport adapters call exactly this.
    ///
    /// `Hello` is answered by the adapter (it owns registration and the
    /// `Welcome` frame); a repeat here is a protocol error.
    pub(crate) fn handle_request(&self, ctx: &ConnectionContext, request: Request) -> HostReply {
        match request {
            Request::Hello(_) => HostReply {
                response: Response::Error(ProtocolError {
                    code: ProtocolErrorCode::BadRequest,
                    message: "connection is already registered".to_string(),
                }),
                subscription: None,
            },
            Request::Ping => HostReply {
                response: Response::Pong,
                subscription: None,
            },
            Request::Command(envelope) => HostReply {
                response: Response::Command(self.core.command(ctx, envelope)),
                subscription: None,
            },
            Request::Control(envelope) => HostReply {
                response: Response::Command(self.core.control(ctx, envelope)),
                subscription: None,
            },
            Request::Query(envelope) => HostReply {
                response: self.core.query(ctx, envelope),
                subscription: None,
            },
            Request::Subscribe(subscribe) => self.core.subscribe(subscribe),
            Request::Unsubscribe { subscription_id } => {
                self.core.journal.unsubscribe(subscription_id);
                HostReply {
                    response: Response::Unsubscribed { subscription_id },
                    subscription: None,
                }
            }
        }
    }
}

/// Choose the executor from the configuration, in order of explicitness.
///
/// The deterministic echo of a smoke run, a provider block written into
/// `host.toml` (the Stage-1 seam, kept for a deployment that pins its own
/// endpoint), and the shell's own agent - the desktop's whole tool surface,
/// resolved from the same settings and vault a `bsh` window reads.
fn select_executor(config: &RuntimeConfig) -> Result<(Arc<dyn TurnExecutor>, String), HostError> {
    if config.echo {
        return Ok((
            Arc::new(crate::runtime::executor::EchoExecutor),
            "echo (deterministic)".to_string(),
        ));
    }
    // The shell's own agent is the default: it reads the settings and vault a
    // `bsh` window reads, so a terminal turn and a window turn send one
    // request through one tool surface. `host.toml` decides only where the
    // shell resolves no model at all - a Host started against a scratch data
    // directory.
    let agent = crate::runtime::agent_exec::AgentExecutor::open(&config.workspace_root);
    if agent.has_model() {
        let label = agent.describe();
        return Ok((Arc::new(agent), label));
    }
    if config.provider.is_configured() {
        let label = format!("model (react) {}", config.provider.model);
        return Ok((
            Arc::new(crate::runtime::executor::ReactExecutor::new(
                config.provider.clone(),
            )),
            label,
        ));
    }
    Err(HostError::Executor(
        "no model configured; choose a model in bsh, add [agent] model/base_url to host.toml, or run with --echo"
            .to_string(),
    ))
}

impl Core {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("host state lock")
    }

    fn status(&self, inner: &Inner) -> HostStatus {
        HostStatus {
            instance_id: self.instance_id,
            workspace_id: self.workspace_id,
            protocol_version: crate::runtime::contract::PROTOCOL_VERSION,
            phase: inner.phase,
            activity: inner.activity(),
            journal_head: self.journal.head(),
            started_at: self.started_at,
            active_executions: inner.projections.active_execution_count(),
        }
    }

    fn recover(&self) -> Result<(), HostError> {
        let events = self.journal.all_events();
        let mut inner = self.lock();
        inner.phase = HostPhase::Recovering;
        let _ = self.phase_tx.send_replace(HostPhase::Recovering);
        inner.projections = Projections::from_events(&events);
        inner.ledger = rebuild_ledger(&events);

        // Anything that never reached a terminal fact was in flight when the
        // previous process died. Its external side effects are unknown, so it
        // is interrupted and never retried automatically (RFC-014).
        let interrupted: Vec<(SessionId, ExecutionId)> = inner
            .projections
            .sessions()
            .iter()
            .flat_map(|session| {
                let session_id = session.session_id;
                session
                    .executions
                    .iter()
                    .filter(|execution| is_live(execution.status))
                    .map(move |execution| (session_id, execution.execution_id))
            })
            .collect();
        for (session_id, execution_id) in interrupted {
            let draft = EventDraft::new(EventPayload::ExecutionInterrupted {
                reason: "Host restarted while this execution was live; external side effects are unknown"
                    .to_string(),
            })
            .session(session_id)
            .execution(execution_id)
            .caused_by("recovery");
            self.append_and_apply(&mut inner, draft)
                .map_err(|error| HostError::Storage(error.to_string()))?;
        }

        let draft = EventDraft::new(EventPayload::HostReady {
            instance_id: self.instance_id,
        })
        .caused_by("recovery");
        self.append_and_apply(&mut inner, draft)
            .map_err(|error| HostError::Storage(error.to_string()))?;
        self.journal.trim_to_window();
        inner.phase = HostPhase::Ready;
        let _ = self.phase_tx.send_replace(HostPhase::Ready);
        self.audit.try_record(&AuditRecord::new(
            "host",
            "host_ready",
            self.workspace_id,
            "allowed",
            format!(
                "instance {} recovered {} events",
                self.instance_id,
                events.len()
            ),
        ));
        Ok(())
    }

    /// Append a fact, fold it into the projections, then publish it.
    ///
    /// Must be called while holding [`Core::inner`]: the projection fold and
    /// the head advance have to be one step from a query's point of view.
    fn append_and_apply(
        &self,
        inner: &mut Inner,
        draft: EventDraft,
    ) -> Result<EventEnvelope, crate::runtime::journal::JournalError> {
        let event = self.journal.append(draft)?;
        inner.projections.apply(&event);
        self.journal.publish(&event);
        Ok(event)
    }

    fn rejected(command_id: CommandId, rejection: Rejection) -> CommandOutcome {
        CommandOutcome::Rejected {
            command_id,
            rejection,
        }
    }

    fn refuse(code: RejectionCode, message: impl Into<String>) -> Rejection {
        Rejection {
            code,
            message: message.into(),
        }
    }

    fn accepted(command_id: CommandId, operation_id: OperationId) -> CommandOutcome {
        CommandOutcome::Accepted {
            command_id,
            operation_id,
            accepted_at: Utc::now(),
        }
    }

    fn record_ledger(
        inner: &mut Inner,
        command_id: CommandId,
        payload_hash: &PayloadHash,
        outcome: &CommandOutcome,
    ) {
        inner.ledger.insert(
            command_id,
            LedgerEntry {
                payload_hash: payload_hash.clone(),
                outcome: outcome.clone(),
            },
        );
    }

    /// Journal failure: fail closed instead of accepting work whose durability
    /// is unknown.
    fn fail_closed(&self, inner: &mut Inner, error: &crate::runtime::journal::JournalError) {
        inner.phase = HostPhase::Failed;
        inner.degraded = true;
        let _ = self.phase_tx.send_replace(HostPhase::Failed);
        self.audit.try_record(&AuditRecord::new(
            "host",
            "journal_failure",
            self.workspace_id,
            "failed",
            error.to_string(),
        ));
    }

    fn journal_rejection(
        &self,
        inner: &mut Inner,
        error: &crate::runtime::journal::JournalError,
    ) -> Rejection {
        self.fail_closed(inner, error);
        Self::refuse(RejectionCode::Internal, format!("journal failure: {error}"))
    }

    fn check_phase(inner: &Inner) -> Result<(), Rejection> {
        if inner.phase == HostPhase::Ready {
            Ok(())
        } else {
            Err(Self::refuse(
                RejectionCode::NotReady,
                format!("Host is {:?} and is not accepting mutations", inner.phase),
            ))
        }
    }

    fn authorize(
        &self,
        ctx: &ConnectionContext,
        envelope_client: ClientInstanceId,
        target: &crate::runtime::contract::Target,
        access: Access,
    ) -> Result<(), Rejection> {
        if envelope_client != ctx.client_instance_id {
            return Err(Self::refuse(
                RejectionCode::Unauthorized,
                "client_instance_id does not belong to this connection",
            ));
        }
        self.authz.authorize(&ctx.principal, target, access)
    }

    fn query(&self, ctx: &ConnectionContext, envelope: QueryEnvelope) -> Response {
        let inner = self.lock();
        if let Err(rejection) = self
            .authz
            .authorize(&ctx.principal, &envelope.target, Access::Read)
        {
            return Response::QueryRejected(rejection);
        }
        let result = match envelope.payload {
            QueryPayload::ListSessions => QueryResult::Sessions {
                sessions: inner.projections.summaries(),
            },
            QueryPayload::Session { session_id } => QueryResult::Session {
                session: inner.projections.session(session_id).cloned().map(Box::new),
            },
            // The bin is the one reader that asks for parked sessions: every
            // other list and the snapshot skip them, so a client reconciles
            // against the workspace it can see.
            QueryPayload::Trash => QueryResult::Sessions {
                sessions: inner.projections.trashed(),
            },
            // The Archived filter is the one reader that still lists a chat
            // the sidebar hid: every other list and the snapshot skip it, so
            // a client reconciles against the workspace it can see.
            QueryPayload::Archived => QueryResult::Sessions {
                sessions: inner.projections.archived(),
            },
            QueryPayload::PendingApprovals { session_id } => QueryResult::Approvals {
                approvals: inner
                    .projections
                    .approvals(session_id)
                    .into_iter()
                    .filter(|approval| approval.status == ApprovalStatus::Pending)
                    .collect(),
            },
            QueryPayload::HostStatus => QueryResult::Host {
                status: self.status(&inner),
            },
            QueryPayload::Operation { operation_id } => QueryResult::Operation {
                operation: inner.projections.operation(&operation_id),
            },
            QueryPayload::Snapshot => QueryResult::Snapshot {
                snapshot: Box::new(self.snapshot(&inner)),
            },
            QueryPayload::Catalogue => QueryResult::Catalogue {
                catalogue: self.executor.catalogue(),
            },
            QueryPayload::Memories { query } => QueryResult::Memories {
                report: self.executor.memories(query.as_deref()),
            },
            QueryPayload::Files { path } => QueryResult::Files {
                report: self.executor.files(&path),
            },
            QueryPayload::Diff { path } => QueryResult::Diff {
                view: self.executor.diff(&path),
            },
            QueryPayload::Tasks => QueryResult::Tasks {
                report: self.executor.tasks(),
            },
            QueryPayload::TaskDetail { id } => QueryResult::TaskDetail {
                report: Box::new(self.executor.task_detail(&id)),
            },
            QueryPayload::Extensions => QueryResult::Extensions {
                report: self.executor.extensions(),
            },
            QueryPayload::Federation => QueryResult::Federation {
                report: self.executor.federation(),
            },
            QueryPayload::Review { session_id, change } => QueryResult::Review {
                report: Box::new(
                    self.executor
                        .review(session_id.to_string().as_str(), change.as_deref()),
                ),
            },
            // The browser is process-wide, so this query names no session: one
            // browser serves every chat the Host runs, the way the desktop
            // window's tab serves every conversation it opened.
            QueryPayload::Browser => QueryResult::Browser {
                view: Box::new(self.executor.browser()),
            },
            // The settings are read from the executor that runs the turns, so
            // the rows a client lists are the values a turn will resolve from
            // rather than a second read of the file that could drift.
            QueryPayload::Settings { section } => QueryResult::Settings {
                report: Box::new(self.executor.settings(section.as_deref())),
            },
        };
        Response::Query(result)
    }

    fn snapshot(&self, inner: &Inner) -> Snapshot {
        Snapshot {
            head: self.journal.head(),
            generated_at: Utc::now(),
            sessions: inner.projections.snapshot(),
            status: self.status(inner),
        }
    }

    fn subscribe(&self, subscribe: Subscribe) -> HostReply {
        let inner = self.lock();
        let cursor = subscribe.cursor.map(|cursor| cursor.sequence);
        match self.journal.subscribe(cursor) {
            Ok(subscription) => HostReply {
                response: Response::Subscribed(Subscribed {
                    subscription_id: subscription.id,
                    replay: subscription.replay,
                    next_cursor: Cursor {
                        sequence: subscription.next_cursor,
                    },
                }),
                subscription: Some(LiveSubscription {
                    id: subscription.id,
                    receiver: subscription.receiver,
                    activity: self.activity_tx.subscribe(),
                }),
            },
            Err(SubscriptionError::Expired { head, .. }) => HostReply {
                response: Response::CursorExpired(CursorExpired {
                    head,
                    snapshot: self.snapshot(&inner),
                }),
                subscription: None,
            },
            Err(SubscriptionError::Invalid { requested, head }) => HostReply {
                response: Response::Error(ProtocolError {
                    code: ProtocolErrorCode::BadRequest,
                    message: format!("cursor {requested} is ahead of journal head {head}"),
                }),
                subscription: None,
            },
        }
    }

    fn command(
        self: &Arc<Self>,
        ctx: &ConnectionContext,
        envelope: CommandEnvelope,
    ) -> CommandOutcome {
        let payload_hash = PayloadHash::of(&envelope.payload);
        let mut prepared = None;
        let outcome = {
            let mut inner = self.lock();
            let command_id = envelope.command_id;
            if let Err(rejection) = Self::check_phase(&inner) {
                return Self::rejected(command_id, rejection);
            }
            // Same command id + same payload returns the prior outcome; the
            // same id with a different payload is a client bug, not a retry.
            if let Some(entry) = inner.ledger.get(&command_id) {
                return if entry.payload_hash == payload_hash {
                    entry.outcome.clone()
                } else {
                    Self::rejected(
                        command_id,
                        Self::refuse(
                            RejectionCode::Conflict,
                            "command_id was already used with a different payload",
                        ),
                    )
                };
            }
            if let Err(rejection) = self.authorize(
                ctx,
                envelope.client_instance_id,
                &envelope.target,
                Access::Mutate,
            ) {
                return Self::rejected(command_id, rejection);
            }
            match self.apply_command(&mut inner, &envelope, &payload_hash) {
                Ok((outcome, turn)) => {
                    Self::record_ledger(&mut inner, command_id, &payload_hash, &outcome);
                    prepared = Some(turn);
                    outcome
                }
                Err(rejection) => Self::rejected(command_id, rejection),
            }
        };
        if let Some(Some(work)) = prepared {
            self.spawn_work(work);
        }
        outcome
    }

    /// The goal a session holds right now, as its projection last saw it.
    fn current_goal(inner: &Inner, session_id: SessionId) -> Option<SessionGoalView> {
        inner
            .projections
            .session(session_id)
            .and_then(|session| session.goal.clone())
    }

    /// Trim one objective to the shape a goal can hold.
    ///
    /// A blank objective is refused rather than stored: a goal with no text
    /// would spend rounds working toward nothing. Overlength is refused too, with
    /// the limit and the actual count, the way the desktop's goal store reports
    /// it - silently cutting an objective would change what the work is for.
    fn goal_objective(objective: &str) -> Result<String, Rejection> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                "goal objective is empty",
            ));
        }
        let actual = objective.chars().count();
        if actual > MAX_GOAL_OBJECTIVE {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                format!("goal objective is {actual} characters; the limit is {MAX_GOAL_OBJECTIVE}"),
            ));
        }
        Ok(objective.to_string())
    }

    /// Validate one feedback note, trimmed and otherwise verbatim.
    ///
    /// `None` is a clear rather than an empty note: the desktop's own note
    /// editor treats a blank draft as "nothing recorded", and a session with no
    /// note must stay distinguishable from one with a blank one.
    fn feedback_note(text: &str) -> Result<Option<String>, Rejection> {
        let note = text.trim();
        if note.is_empty() {
            return Ok(None);
        }
        let actual = note.chars().count();
        if actual > MAX_FEEDBACK_NOTE {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                format!("feedback note is {actual} characters; the limit is {MAX_FEEDBACK_NOTE}"),
            ));
        }
        Ok(Some(note.to_string()))
    }

    /// Journal one goal fact and answer the command that asked for it.
    ///
    /// Every goal verb travels as the same `GoalChanged` event carrying the
    /// whole goal, so a replay rebuilds the same state from one fact and a
    /// client renders one shape no matter which verb moved it.
    fn commit_goal(
        &self,
        inner: &mut Inner,
        session_id: SessionId,
        goal: Option<SessionGoalView>,
        command_id: CommandId,
        payload_hash: &PayloadHash,
    ) -> Result<(CommandOutcome, Option<PreparedWork>), Rejection> {
        let draft = EventDraft::new(EventPayload::GoalChanged {
            goal,
            command_id: Some(command_id),
            payload_hash: Some(payload_hash.clone()),
        })
        .session(session_id)
        .caused_by(command_id);
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        Ok((
            Self::accepted(command_id, OperationId::new(session_id)),
            None,
        ))
    }

    #[allow(clippy::type_complexity)]
    fn apply_command(
        &self,
        inner: &mut Inner,
        envelope: &CommandEnvelope,
        payload_hash: &PayloadHash,
    ) -> Result<(CommandOutcome, Option<PreparedWork>), Rejection> {
        let command_id = envelope.command_id;
        match &envelope.payload {
            CommandPayload::CreateSession { title } => {
                if envelope.target.session_id.is_some() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "CreateSession must not name a session in its target",
                    ));
                }
                if title.trim().is_empty() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "session title is empty",
                    ));
                }
                let session_id = SessionId::new();
                // A session is created with the configuration the executor can
                // actually run, recorded as a durable fact: a client reading
                // the session later sees what its first turn used, not what
                // today's settings would have chosen.
                let config = self.executor.default_config();
                let draft = EventDraft::new(EventPayload::SessionCreated {
                    title: title.clone(),
                    config,
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(session_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::RenameSession { session_id, title } => {
                self.require_session(inner, envelope, *session_id)?;
                if title.trim().is_empty() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "session title is empty",
                    ));
                }
                let draft = EventDraft::new(EventPayload::SessionRenamed {
                    title: title.clone(),
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(*session_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::SendMessage { session_id, text } => {
                self.require_session(inner, envelope, *session_id)?;
                if text.trim().is_empty() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "message text is empty",
                    ));
                }
                let execution_id = ExecutionId::new();
                let message_id = MessageId::new();
                let (history, config) = inner
                    .projections
                    .session(*session_id)
                    .map(|session| {
                        let history = session
                            .messages
                            .iter()
                            .map(|message| HistoryMessage {
                                role: message.role,
                                text: message.text.clone(),
                            })
                            .collect();
                        (history, session.config.clone())
                    })
                    .unwrap_or_default();
                let draft = EventDraft::new(EventPayload::TurnRequested {
                    message_id,
                    text: text.clone(),
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(*session_id)
                .execution(execution_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                inner.executions.insert(
                    execution_id,
                    ExecutionRecord {
                        session_id: *session_id,
                        status: ExecutionStatus::Queued,
                        handle: None,
                    },
                );
                Ok((
                    Self::accepted(command_id, OperationId::new(execution_id)),
                    Some(PreparedWork::Turn(PreparedTurn {
                        execution_id,
                        session_id: *session_id,
                        text: text.clone(),
                        history,
                        config,
                    })),
                ))
            }
            CommandPayload::CancelExecution { execution_id } => {
                let Some(session_id) = envelope.target.session_id else {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "CancelExecution requires a session in its target",
                    ));
                };
                let Some(record) = inner.executions.get(execution_id) else {
                    return Err(Self::refuse(
                        RejectionCode::ExecutionNotRunning,
                        "execution is not running",
                    ));
                };
                if record.session_id != session_id {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "execution does not belong to the named session",
                    ));
                }
                if !is_live(record.status) {
                    return Err(Self::refuse(
                        RejectionCode::ExecutionNotRunning,
                        "execution is not running",
                    ));
                }
                self.cancel_execution(inner, *execution_id, &session_id, command_id, payload_hash)?;
                Ok((
                    Self::accepted(command_id, OperationId::new(execution_id)),
                    None,
                ))
            }
            CommandPayload::ConfigureSession { session_id, change } => {
                self.require_session(inner, envelope, *session_id)?;
                // The executor owns what a value means, so the Host asks it
                // before it journals the change: a model this build cannot
                // call is refused here rather than at the first turn.
                let current = inner
                    .projections
                    .session(*session_id)
                    .map(|session| session.config.clone())
                    .unwrap_or_default();
                let config = self
                    .executor
                    .configure(&current, change)
                    .map_err(|message| Self::refuse(RejectionCode::InvalidRequest, message))?;
                let draft = EventDraft::new(EventPayload::SessionConfigured {
                    config,
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(*session_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                Ok((
                    Self::accepted(command_id, OperationId::new(*session_id)),
                    None,
                ))
            }
            CommandPayload::CompactSession { session_id } => {
                self.require_session(inner, envelope, *session_id)?;
                let Some(session) = inner.projections.session(*session_id) else {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        format!("session {session_id} does not exist"),
                    ));
                };
                // Folding a transcript a live turn is still appending to would
                // drop messages that turn already reads, so compaction waits
                // for the transcript to stand still.
                let running = inner
                    .executions
                    .values()
                    .any(|record| record.session_id == *session_id && is_live(record.status));
                if running {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        "session is running a turn; compact it after the turn ends",
                    ));
                }
                if session.messages.is_empty() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "session has no history to compact",
                    ));
                }
                let history = session
                    .messages
                    .iter()
                    .map(|message| HistoryMessage {
                        role: message.role,
                        text: message.text.clone(),
                    })
                    .collect();
                let config = session.config.clone();
                Ok((
                    Self::accepted(command_id, OperationId::new(*session_id)),
                    Some(PreparedWork::Compaction(PreparedCompaction {
                        session_id: *session_id,
                        config,
                        history,
                        command_id,
                        payload_hash: payload_hash.clone(),
                    })),
                ))
            }
            CommandPayload::SetGoal {
                session_id,
                objective,
            } => {
                self.require_session(inner, envelope, *session_id)?;
                // Validate the objective before looking at the goal, the way the
                // desktop's goal store does: a malformed command is refused for
                // its own shape even when a goal exists to conflict with.
                let objective = Self::goal_objective(objective)?;
                // The desktop's create rule: an unfinished goal must be edited,
                // paused, resumed, blocked or cleared before another takes its
                // place. A completed goal is work that is over, so a fresh one
                // replaces it with a fresh identity and round count.
                if let Some(current) = Self::current_goal(inner, *session_id)
                    && current.state != GoalState::Completed
                {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        format!(
                            "session already has a goal in state {}; use /goal edit, pause, resume or clear",
                            goal_state_label(current.state)
                        ),
                    ));
                }
                self.commit_goal(
                    inner,
                    *session_id,
                    Some(SessionGoalView::active(objective)),
                    command_id,
                    payload_hash,
                )
            }
            CommandPayload::EditGoal {
                session_id,
                objective,
            } => {
                self.require_session(inner, envelope, *session_id)?;
                let objective = Self::goal_objective(objective)?;
                let Some(current) = Self::current_goal(inner, *session_id) else {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        "session has no goal to edit",
                    ));
                };
                // Editing keeps the phase and the rounds the goal already
                // spent: the operator corrected the words, not the work. A
                // completed goal is replaced instead, so the fresh goal starts
                // with the whole cap.
                let goal = if current.state == GoalState::Completed {
                    SessionGoalView::active(objective)
                } else {
                    SessionGoalView {
                        objective,
                        ..current
                    }
                };
                self.commit_goal(inner, *session_id, Some(goal), command_id, payload_hash)
            }
            CommandPayload::PauseGoal { session_id } => {
                self.require_session(inner, envelope, *session_id)?;
                let Some(goal) = Self::current_goal(inner, *session_id) else {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        "session has no goal to pause",
                    ));
                };
                if goal.state != GoalState::Active {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        format!(
                            "goal is {} and cannot be paused",
                            goal_state_label(goal.state)
                        ),
                    ));
                }
                let goal = SessionGoalView {
                    state: GoalState::Paused,
                    ..goal
                };
                self.commit_goal(inner, *session_id, Some(goal), command_id, payload_hash)
            }
            CommandPayload::ResumeGoal { session_id } => {
                self.require_session(inner, envelope, *session_id)?;
                let Some(goal) = Self::current_goal(inner, *session_id) else {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        "session has no goal to resume",
                    ));
                };
                if goal.state == GoalState::Active {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        "goal is already active",
                    ));
                }
                if goal.state == GoalState::Completed {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        "goal is completed; set a new one with /goal <objective>",
                    ));
                }
                // A resumable goal still needs cap left: resuming an exhausted
                // goal would only spend a round the Host refuses to count.
                if goal.rounds >= goal.max_rounds {
                    return Err(Self::refuse(
                        RejectionCode::Conflict,
                        format!("goal round cap {} is spent", goal.max_rounds),
                    ));
                }
                let goal = SessionGoalView {
                    state: GoalState::Active,
                    blocked_reason: None,
                    ..goal
                };
                self.commit_goal(inner, *session_id, Some(goal), command_id, payload_hash)
            }
            CommandPayload::ClearGoal { session_id } => {
                self.require_session(inner, envelope, *session_id)?;
                if Self::current_goal(inner, *session_id).is_none() {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        "session has no goal to clear",
                    ));
                }
                self.commit_goal(inner, *session_id, None, command_id, payload_hash)
            }
            CommandPayload::SetFeedback { session_id, text } => {
                self.require_session(inner, envelope, *session_id)?;
                let note = Self::feedback_note(text)?;
                let draft = EventDraft::new(EventPayload::FeedbackChanged {
                    feedback: note,
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(*session_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                Ok((
                    Self::accepted(command_id, OperationId::new(*session_id)),
                    None,
                ))
            }
            // Parking and restoring are the same fact with one field flipped:
            // one helper owns the requirement (a known session, not already in
            // the state asked for, no turn in flight) so the two directions
            // cannot drift apart.
            CommandPayload::TrashSession { session_id } => {
                self.park(inner, envelope, command_id, payload_hash, *session_id, true)?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::RestoreSession { session_id } => {
                self.park(
                    inner,
                    envelope,
                    command_id,
                    payload_hash,
                    *session_id,
                    false,
                )?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::PurgeSession { session_id } => {
                self.require_session(inner, envelope, *session_id)?;
                let Some(session) = inner.projections.session(*session_id) else {
                    return Err(Self::refuse(
                        RejectionCode::NotFound,
                        format!("session {session_id} does not exist"),
                    ));
                };
                if !session.trashed {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "purge only drops a session that is in the bin",
                    ));
                }
                if session
                    .executions
                    .iter()
                    .any(|execution| is_live(execution.status))
                {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "session has a turn in flight; purge it after it settles",
                    ));
                }
                let draft = EventDraft::new(EventPayload::SessionPurged {
                    command_id,
                    payload_hash: payload_hash.clone(),
                })
                .session(*session_id)
                .caused_by(command_id);
                self.append_and_apply(inner, draft)
                    .map_err(|error| self.journal_rejection(inner, &error))?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::PinSession { session_id, pinned } => {
                self.pin(
                    inner,
                    envelope,
                    command_id,
                    payload_hash,
                    *session_id,
                    *pinned,
                )?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::ArchiveSession {
                session_id,
                archived,
            } => {
                self.archive(
                    inner,
                    envelope,
                    command_id,
                    payload_hash,
                    *session_id,
                    *archived,
                )?;
                Ok((
                    Self::accepted(command_id, OperationId::new(session_id)),
                    None,
                ))
            }
            CommandPayload::UpdateSettings { edits } => {
                // Settings belong to the workspace, not to a chat: a client that
                // names a session here is asking to configure one thing with a
                // field that belongs to another. Refusing says so instead of
                // quietly editing every session of this workspace.
                if envelope.target.session_id.is_some() {
                    return Err(Self::refuse(
                        RejectionCode::InvalidRequest,
                        "UpdateSettings targets the workspace; it takes no session",
                    ));
                }
                // The executor owns the documents a turn reads, so it owns the
                // write: a value this Host persisted but could not apply would
                // make the next turn disagree with the pane that wrote it.
                match self.executor.update_settings(edits) {
                    Ok(_report) => Ok((
                        Self::accepted(command_id, OperationId::new("settings")),
                        None,
                    )),
                    Err(message) => Err(Self::refuse(RejectionCode::InvalidRequest, message)),
                }
            }
        }
    }

    /// Park one session in the bin (`trashed`) or take it back out.
    ///
    /// A session with a turn in flight is refused in both directions: parking
    /// it would hide a running approval behind an overlay every client treats
    /// as history, and restoring it would put a chat the reader cannot act on
    /// back in the list.
    fn park(
        &self,
        inner: &mut Inner,
        envelope: &CommandEnvelope,
        command_id: CommandId,
        payload_hash: &PayloadHash,
        session_id: SessionId,
        trashed: bool,
    ) -> Result<(), Rejection> {
        self.require_session(inner, envelope, session_id)?;
        let Some(session) = inner.projections.session(session_id) else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("session {session_id} does not exist"),
            ));
        };
        if session.trashed == trashed {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                if trashed {
                    "session is already in the bin"
                } else {
                    "session is not in the bin"
                },
            ));
        }
        if session
            .executions
            .iter()
            .any(|execution| is_live(execution.status))
        {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                "session has a turn in flight; settle it first",
            ));
        }
        let draft = EventDraft::new(EventPayload::SessionTrashChanged {
            trashed,
            command_id,
            payload_hash: payload_hash.clone(),
        })
        .session(session_id)
        .caused_by(command_id);
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        Ok(())
    }

    fn require_session(
        &self,
        inner: &Inner,
        envelope: &CommandEnvelope,
        session_id: SessionId,
    ) -> Result<(), Rejection> {
        if envelope.target.session_id != Some(session_id) {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                "target.session_id must name the session the payload acts on",
            ));
        }
        let Some(session) = inner.projections.session(session_id) else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("session {session_id} does not exist"),
            ));
        };
        if let Some(expected) = envelope.expected_version
            && expected != session.version
        {
            return Err(Self::refuse(
                RejectionCode::Conflict,
                format!(
                    "session {session_id} moved from version {expected} to {}",
                    session.version
                ),
            ));
        }
        Ok(())
    }

    /// Pin one session (`pinned`) or unpin it.
    ///
    /// A turn in flight is allowed in both directions: the flag only decides
    /// where the row sorts, and the desktop's star works on a streaming chat,
    /// so refusing here would be a second rule about the same click.
    fn pin(
        &self,
        inner: &mut Inner,
        envelope: &CommandEnvelope,
        command_id: CommandId,
        payload_hash: &PayloadHash,
        session_id: SessionId,
        pinned: bool,
    ) -> Result<(), Rejection> {
        self.require_session(inner, envelope, session_id)?;
        let Some(session) = inner.projections.session(session_id) else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("session {session_id} does not exist"),
            ));
        };
        if session.pinned == pinned {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                if pinned {
                    "session is already pinned"
                } else {
                    "session is not pinned"
                },
            ));
        }
        let draft = EventDraft::new(EventPayload::SessionPinChanged {
            pinned,
            command_id,
            payload_hash: payload_hash.clone(),
        })
        .session(session_id)
        .caused_by(command_id);
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        Ok(())
    }

    /// Archive one session (`archived`) or return it to the list.
    ///
    /// A session with a turn in flight is refused in both directions, for the
    /// reason parking refuses one: a chat hidden from the list must not be
    /// hiding a live approval, and returning one would put a chat the reader
    /// cannot act on back in the list.
    fn archive(
        &self,
        inner: &mut Inner,
        envelope: &CommandEnvelope,
        command_id: CommandId,
        payload_hash: &PayloadHash,
        session_id: SessionId,
        archived: bool,
    ) -> Result<(), Rejection> {
        self.require_session(inner, envelope, session_id)?;
        let Some(session) = inner.projections.session(session_id) else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("session {session_id} does not exist"),
            ));
        };
        if session.archived == archived {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                if archived {
                    "session is already archived"
                } else {
                    "session is not archived"
                },
            ));
        }
        if session
            .executions
            .iter()
            .any(|execution| is_live(execution.status))
        {
            return Err(Self::refuse(
                RejectionCode::InvalidRequest,
                "session has a turn in flight; settle it first",
            ));
        }
        let draft = EventDraft::new(EventPayload::SessionArchivedChanged {
            archived,
            command_id,
            payload_hash: payload_hash.clone(),
        })
        .session(session_id)
        .caused_by(command_id);
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        Ok(())
    }

    /// Cancel one live execution: pending approvals first, then the execution
    /// fact, then the task itself. The durable facts land before the abort so a
    /// crash cannot leave an unclassified duplicate side effect.
    fn cancel_execution(
        &self,
        inner: &mut Inner,
        execution_id: ExecutionId,
        session_id: &SessionId,
        command_id: CommandId,
        payload_hash: &PayloadHash,
    ) -> Result<(), Rejection> {
        let pending: Vec<(ApprovalId, ActionDigest)> = inner
            .approvals
            .iter()
            .filter(|(_, record)| record.execution_id == execution_id && record.decided.is_none())
            .map(|(id, record)| (*id, record.action_digest.clone()))
            .collect();
        for (approval_id, digest) in pending {
            let draft = EventDraft::new(EventPayload::ApprovalDecided {
                approval_id,
                decision: ApprovalDecision::Cancelled,
                action_digest: digest,
                command_id: None,
            })
            .session(*session_id)
            .execution(execution_id);
            self.append_and_apply(inner, draft)
                .map_err(|error| self.journal_rejection(inner, &error))?;
            if let Some(record) = inner.approvals.get_mut(&approval_id) {
                record.decided = Some(ApprovalDecision::Cancelled);
                if let Some(waiter) = record.waiter.take() {
                    let _ = waiter.send(ApprovalDecision::Cancelled);
                }
            }
        }
        let draft = EventDraft::new(EventPayload::ExecutionCancelled {
            command_id,
            payload_hash: payload_hash.clone(),
        })
        .session(*session_id)
        .execution(execution_id)
        .caused_by(command_id);
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        if let Some(record) = inner.executions.get_mut(&execution_id) {
            record.status = ExecutionStatus::Cancelled;
            if let Some(handle) = record.handle.take() {
                handle.abort();
            }
        }
        Ok(())
    }

    fn control(&self, ctx: &ConnectionContext, envelope: ControlEnvelope) -> CommandOutcome {
        let payload_hash = PayloadHash::of(&envelope.payload);
        let mut inner = self.lock();
        let command_id = envelope.command_id;
        if let Err(rejection) = Self::check_phase(&inner) {
            return Self::rejected(command_id, rejection);
        }
        if let Some(entry) = inner.ledger.get(&command_id) {
            return if entry.payload_hash == payload_hash {
                entry.outcome.clone()
            } else {
                Self::rejected(
                    command_id,
                    Self::refuse(
                        RejectionCode::Conflict,
                        "command_id was already used with a different payload",
                    ),
                )
            };
        }
        let access = match &envelope.payload {
            ControlPayload::DecideApproval { .. } => Access::Approve,
            ControlPayload::Shutdown { .. } => Access::Shutdown,
        };
        if let Err(rejection) =
            self.authorize(ctx, envelope.client_instance_id, &envelope.target, access)
        {
            return Self::rejected(command_id, rejection);
        }
        let outcome = match &envelope.payload {
            ControlPayload::DecideApproval {
                approval_id,
                decision,
                action_digest,
                note,
            } => match self.decide_approval(
                &mut inner,
                command_id,
                *approval_id,
                *decision,
                action_digest,
                note.as_deref(),
                &ctx.principal.view,
                envelope.expected_version,
            ) {
                Ok(outcome) => outcome,
                Err(rejection) => Self::rejected(command_id, rejection),
            },
            ControlPayload::Shutdown { mode } => {
                let operation = OperationId::new("host");
                if inner.phase == HostPhase::Ready {
                    let draft = EventDraft::new(EventPayload::HostDraining { mode: *mode })
                        .caused_by(command_id);
                    match self.append_and_apply(&mut inner, draft) {
                        Ok(_) => {
                            inner.phase = HostPhase::Draining;
                            let _ = self.phase_tx.send_replace(HostPhase::Draining);
                            self.audit.try_record(&AuditRecord::new(
                                ctx.principal.view.id.clone(),
                                "host_shutdown",
                                self.workspace_id,
                                "allowed",
                                format!(
                                    "{mode:?} shutdown requested by {}",
                                    ctx.principal.view.display
                                ),
                            ));
                            Self::accepted(command_id, operation)
                        }
                        Err(error) => {
                            Self::rejected(command_id, self.journal_rejection(&mut inner, &error))
                        }
                    }
                } else {
                    // Already draining or stopped: the intent is satisfied.
                    Self::accepted(command_id, operation)
                }
            }
        };
        Self::record_ledger(&mut inner, command_id, &payload_hash, &outcome);
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn decide_approval(
        &self,
        inner: &mut Inner,
        command_id: CommandId,
        approval_id: ApprovalId,
        verdict: crate::runtime::contract::ApprovalVerdict,
        action_digest: &ActionDigest,
        note: Option<&str>,
        principal: &PrincipalView,
        expected_version: Option<u64>,
    ) -> Result<CommandOutcome, Rejection> {
        let Some((session_id, _execution_id, recorded_digest, expires_at, decided)) =
            inner.approvals.get(&approval_id).map(|record| {
                (
                    record.session_id,
                    record.execution_id,
                    record.action_digest.clone(),
                    record.expires_at,
                    record.decided,
                )
            })
        else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("approval {approval_id} does not exist"),
            ));
        };
        if let Some(decided) = decided {
            return Err(Self::refuse(
                RejectionCode::ApprovalAlreadyDecided,
                format!("approval {approval_id} was already decided: {decided:?}"),
            ));
        }
        if let Some(expected) = expected_version
            && inner
                .projections
                .session(session_id)
                .is_some_and(|session| session.version != expected)
        {
            return Err(Self::refuse(
                RejectionCode::Conflict,
                "session version moved since the operator read the approval",
            ));
        }
        // The digest binds the decision to this approval, this action, this
        // scope and this deadline. A client cannot approve something else.
        if action_digest != &recorded_digest {
            self.audit.try_record(
                &AuditRecord::new(
                    principal.id.clone(),
                    "approval_decision",
                    self.workspace_id,
                    "denied",
                    format!("digest mismatch for approval {approval_id}"),
                )
                .session(session_id),
            );
            return Err(Self::refuse(
                RejectionCode::ApprovalDigestMismatch,
                "action_digest does not match the pending approval",
            ));
        }
        if Utc::now() >= expires_at {
            self.settle_approval(inner, approval_id, ApprovalDecision::Expired, None)?;
            return Err(Self::refuse(
                RejectionCode::ApprovalExpired,
                "approval expired before a decision",
            ));
        }
        let decision = match verdict {
            crate::runtime::contract::ApprovalVerdict::Approve => ApprovalDecision::Approved,
            crate::runtime::contract::ApprovalVerdict::Reject => ApprovalDecision::Rejected,
        };
        self.settle_approval(inner, approval_id, decision, Some(command_id))?;
        let detail = match note {
            Some(note) => format!("{:?} by {}: {note}", decision, principal.display),
            None => format!("{:?} by {}", decision, principal.display),
        };
        self.audit.try_record(
            &AuditRecord::new(
                principal.id.clone(),
                "approval_decision",
                self.workspace_id,
                "allowed",
                detail,
            )
            .session(session_id),
        );
        Ok(Self::accepted(command_id, OperationId::new(approval_id)))
    }

    /// Commit one approval decision and wake the waiting executor.
    fn settle_approval(
        &self,
        inner: &mut Inner,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        command_id: Option<CommandId>,
    ) -> Result<(), Rejection> {
        let Some((session_id, execution_id, action_digest)) =
            inner.approvals.get(&approval_id).map(|record| {
                (
                    record.session_id,
                    record.execution_id,
                    record.action_digest.clone(),
                )
            })
        else {
            return Err(Self::refuse(
                RejectionCode::NotFound,
                format!("approval {approval_id} does not exist"),
            ));
        };
        let draft = EventDraft::new(EventPayload::ApprovalDecided {
            approval_id,
            decision,
            action_digest: action_digest.clone(),
            command_id,
        })
        .session(session_id)
        .execution(execution_id);
        let draft = match command_id {
            Some(command_id) => draft.caused_by(command_id),
            None => draft,
        };
        self.append_and_apply(inner, draft)
            .map_err(|error| self.journal_rejection(inner, &error))?;
        if let Some(record) = inner.approvals.get_mut(&approval_id) {
            record.decided = Some(decision);
            if let Some(waiter) = record.waiter.take() {
                let _ = waiter.send(decision);
            }
        }
        if let Some(record) = inner.executions.get_mut(&execution_id)
            && record.status == ExecutionStatus::WaitingApproval
        {
            record.status = ExecutionStatus::Running;
        }
        Ok(())
    }

    fn spawn_work(self: &Arc<Self>, work: PreparedWork) {
        match work {
            PreparedWork::Turn(turn) => self.spawn_turn(turn),
            PreparedWork::Compaction(compaction) => self.spawn_compaction(compaction),
        }
    }

    /// Fold one accepted session's history through the executor, then commit
    /// the checkpoint as a durable fact.
    ///
    /// The model call runs off the request path, so a slow summarizer never
    /// holds the lock the Host serves every other request with.
    fn spawn_compaction(self: &Arc<Self>, work: PreparedCompaction) {
        let core = Arc::clone(self);
        let context = CompactContext {
            workspace_id: self.workspace_id,
            session_id: work.session_id,
            config: work.config,
            history: work.history,
        };
        tokio::spawn(async move {
            let outcome = core.executor.compact(context).await;
            core.finish_compaction(
                work.session_id,
                work.command_id,
                &work.payload_hash,
                outcome,
            );
        });
    }

    /// Commit one compaction result. A failed attempt journals its failure and
    /// leaves the transcript alone: the client sees why history did not move.
    fn finish_compaction(
        &self,
        session_id: SessionId,
        command_id: CommandId,
        payload_hash: &PayloadHash,
        outcome: Result<crate::runtime::executor::CompactionOutput, TurnError>,
    ) {
        let draft = match outcome {
            Ok(output) => EventDraft::new(EventPayload::SessionCompacted {
                summary: output.summary,
                folded: output.folded,
                command_id,
                payload_hash: payload_hash.clone(),
            }),
            Err(error) => EventDraft::new(EventPayload::SessionCompactionFailed {
                error: error.message,
                command_id,
                payload_hash: payload_hash.clone(),
            }),
        };
        let draft = draft.session(session_id).caused_by(command_id);
        let mut inner = self.lock();
        // A compaction that cannot be journaled is a storage failure, not a
        // property of the attempt: the Host fails closed like any other append
        // the journal refuses.
        if let Err(error) = self.append_and_apply(&mut inner, draft) {
            self.fail_closed(&mut inner, &error);
        }
    }

    fn spawn_turn(self: &Arc<Self>, turn: PreparedTurn) {
        // Tool facts are journaled by a pump, not by the executor: the
        // executor reports them, and the Host alone decides what a durable
        // fact is. An unbounded queue keeps a slow journal from stalling the
        // tool loop itself.
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let context = TurnContext {
            workspace_id: self.workspace_id,
            session_id: turn.session_id,
            execution_id: turn.execution_id,
            text: turn.text,
            history: turn.history,
            config: turn.config,
            approvals: ApprovalHandle::attached(ApprovalGate {
                core: Arc::clone(self),
                session_id: turn.session_id,
                execution_id: turn.execution_id,
            }),
            progress: ProgressHandle::attached(progress_tx),
        };
        let execution_id = turn.execution_id;
        let session_id = turn.session_id;
        let core = Arc::clone(self);
        // Queued -> Running under the same lock that owns the ledger, so a
        // cancel that lands before the task is polled still sees it live.
        let mut inner = self.lock();
        let Some(record) = inner.executions.get_mut(&execution_id) else {
            return;
        };
        if !is_live(record.status) {
            return;
        }
        record.status = ExecutionStatus::Running;
        let handle = tokio::spawn(async move {
            let pump = core.pump_progress(session_id, execution_id, progress_rx);
            let outcome = core.executor.execute(context).await;
            // The executor dropped its progress handle when it answered, so
            // the pump drains and stops here. Waiting for it is what keeps a
            // tool result out of the journal after the turn's terminal fact.
            let _ = pump.await;
            core.finish_turn(execution_id, session_id, outcome);
        });
        if let Some(record) = inner.executions.get_mut(&execution_id) {
            record.handle = Some(handle);
        }
    }

    /// Journal one turn's tool facts until the executor stops reporting.
    ///
    /// The pump holds the same lock every mutation does, per fact, so a
    /// projection never sees a tool call the journal does not have. A turn
    /// that was cancelled or interrupted stops being live, and the pump exits:
    /// a terminal fact is never followed by tool traffic from that turn.
    fn pump_progress(
        self: &Arc<Self>,
        session_id: SessionId,
        execution_id: ExecutionId,
        mut receiver: mpsc::UnboundedReceiver<TurnProgress>,
    ) -> JoinHandle<()> {
        let core = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(progress) = receiver.recv().await {
                let payload = match progress {
                    TurnProgress::ToolCallStarted {
                        step,
                        tool,
                        summary,
                        args,
                    } => EventPayload::ToolCallStarted {
                        step,
                        tool,
                        summary,
                        args,
                    },
                    TurnProgress::ToolCallFinished {
                        step,
                        tool,
                        ok,
                        output,
                        duration_ms,
                    } => EventPayload::ToolCallFinished {
                        step,
                        tool,
                        ok,
                        output,
                        duration_ms,
                    },
                    // The plan is durable session state, so it is journalled
                    // like a tool call instead of broadcast as activity: a
                    // client that reconnects reads the same list.
                    TurnProgress::PlanUpdated { items } => EventPayload::PlanUpdated {
                        plan: items
                            .into_iter()
                            .map(|item| PlanItemView {
                                text: item.text,
                                status: match item.status {
                                    agent::tools::plan::PlanStatus::Pending => {
                                        PlanItemStatus::Pending
                                    }
                                    agent::tools::plan::PlanStatus::InProgress => {
                                        PlanItemStatus::InProgress
                                    }
                                    agent::tools::plan::PlanStatus::Completed => {
                                        PlanItemStatus::Completed
                                    }
                                },
                            })
                            .collect(),
                    },
                    // A live answer is not a durable fact. It is broadcast
                    // to whoever is watching and never reaches the journal:
                    // the turn's commit carries the whole text.
                    TurnProgress::AssistantText { text } => {
                        let inner = core.lock();
                        let live = inner
                            .executions
                            .get(&execution_id)
                            .is_some_and(|record| is_live(record.status));
                        drop(inner);
                        if !live {
                            break;
                        }
                        let _ = core.activity_tx.send(ActivityEvent {
                            session_id,
                            execution_id,
                            activity: ExecutionActivity::AssistantText { text },
                        });
                        continue;
                    }
                };
                let mut inner = core.lock();
                let Some(record) = inner.executions.get(&execution_id) else {
                    break;
                };
                if !is_live(record.status) {
                    break;
                }
                let draft = EventDraft::new(payload)
                    .session(session_id)
                    .execution(execution_id)
                    .caused_by("executor");
                if let Err(error) = core.append_and_apply(&mut inner, draft) {
                    core.fail_closed(&mut inner, &error);
                    break;
                }
            }
        })
    }

    fn finish_turn(
        &self,
        execution_id: ExecutionId,
        session_id: SessionId,
        outcome: Result<TurnOutput, TurnError>,
    ) {
        let mut inner = self.lock();
        let Some(record) = inner.executions.get(&execution_id) else {
            return;
        };
        if !is_live(record.status) {
            // Cancelled or interrupted already committed a terminal fact; the
            // late result must not append a second one.
            return;
        }
        let (status, draft) = match outcome {
            Ok(output) => (
                ExecutionStatus::Completed,
                EventDraft::new(EventPayload::TurnSucceeded {
                    message_id: MessageId::new(),
                    text: output.text,
                    usage: output.usage,
                }),
            ),
            Err(error) => (
                ExecutionStatus::Failed,
                EventDraft::new(EventPayload::TurnFailed {
                    error: error.message,
                }),
            ),
        };
        let draft = draft.session(session_id).execution(execution_id);
        match self.append_and_apply(&mut inner, draft) {
            Ok(_) => {
                if let Some(record) = inner.executions.get_mut(&execution_id) {
                    record.status = status;
                    record.handle.take();
                }
                // An executor error is a property of that turn, not of the
                // Host: the Host stays Ready and keeps serving sessions.
            }
            Err(error) => self.fail_closed(&mut inner, &error),
        }
    }

    fn open_approval(
        self: &Arc<Self>,
        session_id: SessionId,
        execution_id: ExecutionId,
        request: ApprovalRequest,
    ) -> Result<OpenedApproval, Rejection> {
        let expires_at = Utc::now()
            + chrono::Duration::from_std(Duration::from_secs(request.expires_in_secs.max(1)))
                .unwrap_or(chrono::Duration::seconds(300));
        let approval_id = ApprovalId::new();
        let digest_input = ApprovalDigestInput {
            workspace_id: self.workspace_id,
            session_id,
            execution_id,
            approval_id,
            action: &request.action,
            expires_at,
        };
        let action_digest = ActionDigest::of(&digest_input);
        let mut inner = self.lock();
        Self::check_phase(&inner)?;
        let Some(record) = inner.executions.get(&execution_id) else {
            return Err(Self::refuse(
                RejectionCode::ExecutionNotRunning,
                "execution is not running",
            ));
        };
        if !is_live(record.status) {
            return Err(Self::refuse(
                RejectionCode::ExecutionNotRunning,
                "execution is not running",
            ));
        }
        let draft = EventDraft::new(EventPayload::ApprovalRequested {
            approval_id,
            summary: request.summary.clone(),
            action: request.action.clone(),
            action_digest: action_digest.clone(),
            expires_at,
        })
        .session(session_id)
        .execution(execution_id)
        .caused_by(approval_id);
        self.append_and_apply(&mut inner, draft)
            .map_err(|error| self.journal_rejection(&mut inner, &error))?;
        if let Some(record) = inner.executions.get_mut(&execution_id) {
            record.status = ExecutionStatus::WaitingApproval;
        }
        let (waiter, wait) = oneshot::channel();
        inner.approvals.insert(
            approval_id,
            ApprovalRecord {
                session_id,
                execution_id,
                action_digest: action_digest.clone(),
                expires_at,
                decided: None,
                waiter: Some(waiter),
            },
        );
        // Expiry is a Host-owned deadline, not a polling loop in the client.
        let core = Arc::clone(self);
        let secs = (expires_at - Utc::now()).num_seconds().max(0) as u64;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            core.expire_approval(approval_id);
        });
        Ok(OpenedApproval {
            approval_id,
            action_digest,
            wait,
        })
    }

    fn expire_approval(&self, approval_id: ApprovalId) {
        let mut inner = self.lock();
        let Some(decided) = inner
            .approvals
            .get(&approval_id)
            .map(|record| (record.decided, record.expires_at))
        else {
            return;
        };
        if decided.0.is_some() || Utc::now() < decided.1 {
            return;
        }
        let _ = self.settle_approval(&mut inner, approval_id, ApprovalDecision::Expired, None);
    }

    fn begin_draining(&self, mode: ShutdownMode) {
        let mut inner = self.lock();
        if matches!(
            inner.phase,
            HostPhase::Draining | HostPhase::Stopped | HostPhase::Failed
        ) {
            return;
        }
        let draft = EventDraft::new(EventPayload::HostDraining { mode }).caused_by("host");
        if let Err(error) = self.append_and_apply(&mut inner, draft) {
            self.fail_closed(&mut inner, &error);
            return;
        }
        inner.phase = HostPhase::Draining;
        let _ = self.phase_tx.send_replace(HostPhase::Draining);
    }

    async fn drain(&self) {
        if self.config.shutdown_grace.is_zero() {
            self.interrupt_in_flight("Host shutdown interrupted this execution");
        } else {
            let deadline = tokio::time::Instant::now() + self.config.shutdown_grace;
            loop {
                let active = {
                    let inner = self.lock();
                    inner
                        .executions
                        .values()
                        .filter(|record| is_live(record.status))
                        .count()
                };
                if active == 0 || tokio::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            self.interrupt_in_flight(
                "Host drained after its in-flight grace; external side effects are unknown",
            );
        }
        self.journal.close_subscribers();
        let mut inner = self.lock();
        if inner.phase == HostPhase::Stopped {
            return;
        }
        let draft = EventDraft::new(EventPayload::HostStopped {
            phase: HostPhase::Stopped,
        })
        .caused_by("host");
        if let Err(error) = self.append_and_apply(&mut inner, draft) {
            self.fail_closed(&mut inner, &error);
            return;
        }
        inner.phase = HostPhase::Stopped;
        let _ = self.phase_tx.send_replace(HostPhase::Stopped);
        self.audit.try_record(&AuditRecord::new(
            "host",
            "host_stopped",
            self.workspace_id,
            "allowed",
            "durable state flushed",
        ));
    }

    fn interrupt_in_flight(&self, reason: &str) {
        let mut inner = self.lock();
        let live: Vec<(SessionId, ExecutionId)> = inner
            .executions
            .iter()
            .filter(|(_, record)| is_live(record.status))
            .map(|(execution_id, record)| (record.session_id, *execution_id))
            .collect();
        for (session_id, execution_id) in live {
            let draft = EventDraft::new(EventPayload::ExecutionInterrupted {
                reason: reason.to_string(),
            })
            .session(session_id)
            .execution(execution_id)
            .caused_by("host");
            if let Err(error) = self.append_and_apply(&mut inner, draft) {
                self.fail_closed(&mut inner, &error);
                return;
            }
            if let Some(record) = inner.executions.get_mut(&execution_id) {
                record.status = ExecutionStatus::Interrupted;
                if let Some(handle) = record.handle.take() {
                    handle.abort();
                }
            }
        }
    }
}

struct PreparedTurn {
    execution_id: ExecutionId,
    session_id: SessionId,
    text: String,
    history: Vec<HistoryMessage>,
    /// The configuration the session held when the turn was accepted.
    ///
    /// Resolved once at acceptance: a configuration change that lands while
    /// the turn runs applies to the NEXT turn, the way a desktop chat's model
    /// switch never rewrites a request already in flight.
    config: SessionConfig,
}

/// Work one accepted command queued for the executor.
///
/// Only these two commands do model work, and both must answer their client
/// before the model runs, so acceptance hands back what to spawn instead of
/// doing it under the lock.
enum PreparedWork {
    Turn(PreparedTurn),
    Compaction(PreparedCompaction),
}

struct PreparedCompaction {
    session_id: SessionId,
    /// The configuration the session held when compaction was accepted.
    config: SessionConfig,
    /// Transcript to fold, oldest first, as the projection held it.
    history: Vec<HistoryMessage>,
    command_id: CommandId,
    payload_hash: PayloadHash,
}

fn is_live(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Queued | ExecutionStatus::Running | ExecutionStatus::WaitingApproval
    )
}

/// What the approval digest commits to: the action, but also the approval id,
/// the execution, the scope and the deadline. Approving something else is then
/// a digest mismatch rather than a judgement call.
#[derive(Serialize)]
struct ApprovalDigestInput<'a> {
    workspace_id: WorkspaceId,
    session_id: SessionId,
    execution_id: ExecutionId,
    approval_id: ApprovalId,
    action: &'a ApprovalTarget,
    expires_at: DateTime<Utc>,
}

struct OpenedApproval {
    approval_id: ApprovalId,
    action_digest: ActionDigest,
    wait: oneshot::Receiver<ApprovalDecision>,
}

/// The Host side of an approval gate, handed to exactly one executor turn.
#[derive(Clone)]
pub(crate) struct ApprovalGate {
    core: Arc<Core>,
    session_id: SessionId,
    execution_id: ExecutionId,
}

/// The executor-facing approval seam.
///
/// It is the only Host capability an executor holds: it can ask the operator
/// for a decision on one concrete action and learn the verdict. It cannot
/// decide, mutate session state, or read another session.
pub struct ApprovalHandle {
    gate: Option<ApprovalGate>,
}

/// The outcome of one approval request.
pub struct ApprovalOutcome {
    /// Approval that was decided.
    pub approval_id: ApprovalId,
    /// Digest the decision is bound to.
    pub action_digest: ActionDigest,
    /// Terminal decision.
    pub decision: ApprovalDecision,
}

impl ApprovalOutcome {
    /// Whether the operator allowed the action.
    #[must_use]
    pub fn is_approved(&self) -> bool {
        self.decision == ApprovalDecision::Approved
    }
}

/// Why an approval request could not produce a decision.
#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    /// The executor has no Host gate (for example in a unit test).
    #[error("approval gate is not attached to a Runtime Host")]
    Detached,
    /// The Host refused to open the request.
    #[error("approval request refused: {}", .0.message)]
    Rejected(Rejection),
    /// The decision channel closed without a verdict.
    #[error("approval channel closed without a decision")]
    Closed,
}

impl ApprovalHandle {
    /// A handle with no Host, for executors built in isolation.
    #[must_use]
    pub fn detached() -> Self {
        Self { gate: None }
    }

    /// Bind one handle to one Host execution. Never exposed to a transport.
    pub(crate) fn attached(gate: ApprovalGate) -> Self {
        Self { gate: Some(gate) }
    }

    /// Ask the operator to decide one action and wait for the verdict.
    pub async fn request(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalOutcome, ApprovalError> {
        let Some(gate) = self.gate.as_ref() else {
            return Err(ApprovalError::Detached);
        };
        let opened = gate
            .core
            .open_approval(gate.session_id, gate.execution_id, request)
            .map_err(ApprovalError::Rejected)?;
        let decision = opened.wait.await.map_err(|_| ApprovalError::Closed)?;
        Ok(ApprovalOutcome {
            approval_id: opened.approval_id,
            action_digest: opened.action_digest,
            decision,
        })
    }
}

/// Fold the durable history into the command ledger so a retry after a Host
/// restart still returns the original outcome instead of replaying the effect.
fn rebuild_ledger(events: &[EventEnvelope]) -> HashMap<CommandId, LedgerEntry> {
    let mut ledger = HashMap::new();
    for event in events {
        let (command_id, payload_hash, operation) = match (&event.payload, event.session_id) {
            (
                EventPayload::SessionCreated {
                    command_id,
                    payload_hash,
                    ..
                },
                Some(session_id),
            ) => (
                *command_id,
                payload_hash.clone(),
                OperationId::new(session_id),
            ),
            (
                EventPayload::SessionRenamed {
                    command_id,
                    payload_hash,
                    ..
                },
                Some(session_id),
            ) => (
                *command_id,
                payload_hash.clone(),
                OperationId::new(session_id),
            ),
            (
                EventPayload::TurnRequested {
                    command_id,
                    payload_hash,
                    ..
                },
                _,
            ) => match event.execution_id {
                Some(execution_id) => (
                    *command_id,
                    payload_hash.clone(),
                    OperationId::new(execution_id),
                ),
                None => continue,
            },
            (
                EventPayload::SessionConfigured {
                    command_id,
                    payload_hash,
                    ..
                },
                Some(session_id),
            ) => (
                *command_id,
                payload_hash.clone(),
                OperationId::new(session_id),
            ),
            (
                EventPayload::GoalChanged {
                    command_id: Some(command_id),
                    payload_hash: Some(payload_hash),
                    ..
                },
                Some(session_id),
            ) => (
                *command_id,
                payload_hash.clone(),
                OperationId::new(session_id),
            ),
            (
                EventPayload::FeedbackChanged {
                    command_id,
                    payload_hash,
                    ..
                },
                Some(session_id),
            ) => (
                *command_id,
                payload_hash.clone(),
                OperationId::new(session_id),
            ),
            (
                EventPayload::ExecutionCancelled {
                    command_id,
                    payload_hash,
                },
                _,
            ) => match event.execution_id {
                Some(execution_id) => (
                    *command_id,
                    payload_hash.clone(),
                    OperationId::new(execution_id),
                ),
                None => continue,
            },
            // Controls are not replayed into the ledger: their payload hash
            // cannot be recomputed from the committed fact, and a decided
            // approval is refused by its own state on retry anyway.
            _ => continue,
        };
        ledger.insert(
            command_id,
            LedgerEntry {
                payload_hash,
                outcome: CommandOutcome::Accepted {
                    command_id,
                    operation_id: operation,
                    accepted_at: event.occurred_at,
                },
            },
        );
    }
    ledger
}

/// Approvals pending anywhere in the workspace; used by the TUI and tests.
impl RuntimeHost {
    /// Pending approvals across every session, soonest deadline first.
    #[must_use]
    pub fn pending_approvals(&self) -> Vec<ApprovalView> {
        let inner = self.core.lock();
        inner
            .projections
            .approvals(None)
            .into_iter()
            .filter(|approval| approval.status == ApprovalStatus::Pending)
            .collect()
    }
}

#[cfg(test)]
mod tests;
