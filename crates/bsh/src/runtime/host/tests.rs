//! Host behaviour tests: lifecycle, projections, idempotency, approvals,
//! cancellation and recovery.
//!
//! They drive [`RuntimeHost::handle_request`] exactly the way a transport
//! adapter does, so the protocol surface a client sees is the surface tested
//! here - no test-only request path exists.

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;

use crate::runtime::authz::Principal;
use crate::runtime::config::RuntimeConfig;
use crate::runtime::contract::{
    ApprovalRequest, ApprovalTarget, ApprovalVerdict, CommandEnvelope, CommandOutcome,
    CommandPayload, ControlEnvelope, ControlPayload, Cursor, EventPayload, ExecutionActivity,
    ExecutionStatus, GoalState, HostActivity, HostPhase, HostStatus, MemoryReport, MemoryView,
    MessageRole, QueryEnvelope, QueryPayload, QueryResult, Rejection, RejectionCode, Request,
    Response, SessionConfig, SessionConfigChange, SessionProjection, SettingEdit, SettingsReport,
    SettingsRowView, SettingsSectionView, ShutdownMode, Subscribe, Target, TaskAttemptView,
    TaskBoardReport, TaskDetailReport, TaskRowView, TaskSectionView, ToolCallStatus,
    WorkspaceChangeView, WorkspaceDiffView, WorkspaceEntryView, WorkspaceReport,
};
use crate::runtime::executor::{TurnContext, TurnError, TurnExecutor, TurnOutput, TurnProgress};
use crate::runtime::host::{
    ApprovalGate, ApprovalHandle, ConnectionContext, MAX_FEEDBACK_NOTE, RuntimeHost,
};
use crate::runtime::ids::{
    ActionDigest, ClientInstanceId, CommandId, ExecutionId, MessageId, OperationId, PayloadHash,
    SessionId, WorkspaceId,
};
use crate::runtime::journal::EventDraft;

/// One Host in a temporary directory, with a connection context like the one
/// the IPC adapter mints after it proves the peer.
struct Fixture {
    host: RuntimeHost,
    ctx: ConnectionContext,
    // Dropped last, after the Host released its files in the directory.
    _dir: TempDir,
}

impl Fixture {
    async fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = runtime_config(&dir);
        let host = RuntimeHost::open(config).await.unwrap();
        Self::new(host, dir)
    }

    /// The same fixture, with the Host running `executor` instead of choosing
    /// one from the configuration.
    async fn open_with(executor: Arc<dyn TurnExecutor>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = runtime_config(&dir);
        let host = RuntimeHost::open_with_executor(config, executor, "test executor".to_string())
            .await
            .unwrap();
        Self::new(host, dir)
    }

    fn new(host: RuntimeHost, dir: TempDir) -> Self {
        let ctx = host.connection(Principal::local(host.local_uid()));
        Self {
            host,
            ctx,
            _dir: dir,
        }
    }

    fn target(&self, session_id: Option<SessionId>) -> Target {
        Target {
            workspace_id: self.host.workspace_id(),
            session_id,
        }
    }

    fn send(&self, request: Request) -> Response {
        self.host.handle_request(&self.ctx, request).response
    }

    fn command(
        &self,
        session_id: Option<SessionId>,
        expected_version: Option<u64>,
        command_id: CommandId,
        payload: CommandPayload,
    ) -> CommandOutcome {
        let envelope = CommandEnvelope {
            command_id,
            client_instance_id: self.ctx.client_instance_id,
            target: self.target(session_id),
            expected_version,
            payload,
        };
        match self.send(Request::Command(envelope)) {
            Response::Command(outcome) => outcome,
            other => panic!("command answered with {other:?}"),
        }
    }

    fn control(&self, session_id: Option<SessionId>, payload: ControlPayload) -> CommandOutcome {
        let envelope = ControlEnvelope {
            command_id: CommandId::new(),
            client_instance_id: self.ctx.client_instance_id,
            target: self.target(session_id),
            expected_version: None,
            payload,
        };
        match self.send(Request::Control(envelope)) {
            Response::Command(outcome) => outcome,
            other => panic!("control answered with {other:?}"),
        }
    }

    fn query(&self, session_id: Option<SessionId>, payload: QueryPayload) -> QueryResult {
        let envelope = QueryEnvelope {
            target: self.target(session_id),
            payload,
        };
        match self.send(Request::Query(envelope)) {
            Response::Query(result) => result,
            other => panic!("query answered with {other:?}"),
        }
    }

    fn status(&self) -> HostStatus {
        match self.query(None, QueryPayload::HostStatus) {
            QueryResult::Host { status } => status,
            other => panic!("status answered with {other:?}"),
        }
    }

    fn session(&self, session_id: SessionId) -> Option<SessionProjection> {
        match self.query(Some(session_id), QueryPayload::Session { session_id }) {
            QueryResult::Session { session } => session.map(|session| *session),
            other => panic!("session answered with {other:?}"),
        }
    }

    fn create_session(&self, title: &str) -> SessionId {
        let outcome = self.command(
            None,
            None,
            CommandId::new(),
            CommandPayload::CreateSession {
                title: title.to_string(),
            },
        );
        accepted(&outcome)
            .to_string()
            .parse()
            .expect("session operation names a uuid")
    }

    /// Commit a `TurnRequested` fact the way an accepted turn does, without
    /// spawning an executor. Used to model a Host that died mid-turn.
    fn request_turn(&self, session_id: SessionId, execution_id: ExecutionId, text: &str) {
        let command_id = CommandId::new();
        let payload = CommandPayload::SendMessage {
            session_id,
            text: text.to_string(),
        };
        let mut inner = self.host.core.lock();
        let draft = EventDraft::new(EventPayload::TurnRequested {
            message_id: MessageId::new(),
            text: text.to_string(),
            command_id,
            payload_hash: PayloadHash::of(&payload),
        })
        .session(session_id)
        .execution(execution_id)
        .caused_by(command_id);
        self.host
            .core
            .append_and_apply(&mut inner, draft)
            .expect("journal accepts the turn");
        inner.executions.insert(
            execution_id,
            super::ExecutionRecord {
                session_id,
                status: ExecutionStatus::Running,
                handle: None,
            },
        );
    }
}

fn runtime_config(dir: &TempDir) -> RuntimeConfig {
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = workspace;
    config.echo = true;
    config.shutdown_grace = Duration::from_secs(1);
    config
}

/// An executor that carries a workspace store of one record, so a memory query
/// has an answer that only a real store could give.
struct MemoryExecutor;

#[async_trait::async_trait]
impl TurnExecutor for MemoryExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[memory echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn memories(&self, query: Option<&str>) -> MemoryReport {
        let searching = query.is_some();
        MemoryReport {
            memories: vec![MemoryView {
                id: "memory-1".to_string(),
                content: "the release checklist lives in docs/release.md".to_string(),
                kind: "fact".to_string(),
                scope: "workspace".to_string(),
                scope_id: "w1".to_string(),
                lifecycle: "active".to_string(),
                source: "agent".to_string(),
                revision: 4,
                updated_at: chrono::Utc::now(),
                score: searching.then_some(0.75),
                reasons: if searching {
                    "keyword 0.75".to_string()
                } else {
                    String::new()
                },
            }],
            state: "ready".to_string(),
            error: None,
        }
    }
}

/// An executor that serves one directory, so a files query proves the Host hands
/// the path to the executor and returns its report unchanged.
struct FilesExecutor;

#[async_trait::async_trait]
impl TurnExecutor for FilesExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[files echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn files(&self, path: &str) -> WorkspaceReport {
        if path.is_empty() {
            return WorkspaceReport {
                root: "/Users/me/bos".to_string(),
                state: "dir".to_string(),
                entries: vec![WorkspaceEntryView {
                    name: "src".to_string(),
                    dir: true,
                    size: 0,
                }],
                ..WorkspaceReport::default()
            };
        }
        WorkspaceReport {
            root: "/Users/me/bos".to_string(),
            path: path.to_string(),
            state: "file".to_string(),
            content: Some("// the executor's own text".to_string()),
            ..WorkspaceReport::default()
        }
    }
}

/// An executor whose defaults are distinctive, so a session's committed
/// configuration proves it came from the executor rather than from a constant.
struct ConfiguredExecutor;

#[async_trait::async_trait]
impl TurnExecutor for ConfiguredExecutor {
    async fn execute(&self, _context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: "configured".to_string(),
            ..TurnOutput::default()
        })
    }

    fn default_config(&self) -> SessionConfig {
        SessionConfig {
            preset: Some("review".to_string()),
            permission: "read-only".to_string(),
            plan_mode: true,
            ..SessionConfig::default()
        }
    }
}

/// An executor that reports one tool call before it answers.
struct ToolProgressExecutor;

#[async_trait::async_trait]
impl TurnExecutor for ToolProgressExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        context.progress.emit(TurnProgress::ToolCallStarted {
            step: 1,
            tool: "shell".to_string(),
            summary: "ls -la".to_string(),
            args: "{\"command\":\"ls -la\"}".to_string(),
        });
        tokio::time::sleep(Duration::from_millis(5)).await;
        context.progress.emit(TurnProgress::ToolCallFinished {
            step: 1,
            tool: "shell".to_string(),
            ok: true,
            output: "total 0".to_string(),
            duration_ms: 5,
        });
        Ok(TurnOutput {
            text: "all done".to_string(),
            ..TurnOutput::default()
        })
    }
}

/// An executor that streams its answer in pieces before returning it.
///
/// The pieces are exactly the desktop's live token stream: the Host must
/// broadcast them and the committed answer must remain the whole text.
struct StreamingExecutor;

#[async_trait::async_trait]
impl TurnExecutor for StreamingExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        context.progress.emit(TurnProgress::AssistantText {
            text: "streamed ".to_string(),
        });
        context.progress.emit(TurnProgress::AssistantText {
            text: "answer".to_string(),
        });
        Ok(TurnOutput {
            text: "streamed answer".to_string(),
            ..TurnOutput::default()
        })
    }
}

fn accepted(outcome: &CommandOutcome) -> OperationId {
    match outcome {
        CommandOutcome::Accepted { operation_id, .. } => operation_id.clone(),
        other => panic!("command was rejected: {other:?}"),
    }
}

fn rejection(outcome: &CommandOutcome) -> Rejection {
    match outcome {
        CommandOutcome::Rejected { rejection, .. } => rejection.clone(),
        other => panic!("command was accepted: {other:?}"),
    }
}

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn opening_the_host_reaches_ready_and_reports_a_consistent_status() {
    let fix = Fixture::open().await;
    let status = fix.status();
    assert_eq!(fix.host.phase(), HostPhase::Ready);
    assert_eq!(status.instance_id, fix.host.instance_id());
    assert_eq!(status.workspace_id, fix.host.workspace_id());
    assert_eq!(status.phase, HostPhase::Ready);
    assert_eq!(status.activity, HostActivity::Idle);
    assert_eq!(status.active_executions, 0);
    // Recovery always commits `HostReady`, even for a fresh journal.
    assert!(status.journal_head >= 1);
    // The executor is chosen at open and named there, so a reader checking
    // which configuration a Host took has one string to read.
    assert_eq!(fix.host.executor_label(), "echo (deterministic)");
}

#[tokio::test]
async fn an_accepted_turn_runs_to_completion_and_projects_both_messages() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("first session");

    let sessions = match fix.query(None, QueryPayload::ListSessions) {
        QueryResult::Sessions { sessions } => sessions,
        other => panic!("list answered with {other:?}"),
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, session_id);
    assert_eq!(sessions[0].title, "first session");

    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id,
            text: "hello host".to_string(),
        },
    );
    let execution_id: ExecutionId = accepted(&outcome)
        .to_string()
        .parse()
        .expect("execution operation names a uuid");

    wait_until("the echo turn to finish", || {
        fix.session(session_id).and_then(|session| {
            session
                .executions
                .iter()
                .find(|execution| execution.execution_id == execution_id)
                .map(|execution| execution.status)
        }) == Some(ExecutionStatus::Completed)
    })
    .await;

    let projection = fix.session(session_id).expect("session exists");
    assert_eq!(projection.messages.len(), 2);
    assert_eq!(projection.messages[0].role, MessageRole::User);
    assert_eq!(projection.messages[0].text, "hello host");
    assert_eq!(projection.messages[0].execution_id, Some(execution_id));
    assert_eq!(projection.messages[1].role, MessageRole::Assistant);
    assert!(
        projection.messages[1]
            .text
            .starts_with(&format!("[host echo {execution_id}]")),
        "unexpected echo: {}",
        projection.messages[1].text
    );

    // The operation reference a client got at acceptance reports completion.
    match fix.query(
        None,
        QueryPayload::Operation {
            operation_id: OperationId::new(execution_id),
        },
    ) {
        QueryResult::Operation {
            operation: Some(operation),
        } => {
            assert_eq!(operation.kind, "execution");
            assert_eq!(operation.status, "completed");
            assert_eq!(operation.execution_id, Some(execution_id));
        }
        other => panic!("operation answered with {other:?}"),
    }

    // The committed event stream carries both boundaries in order.
    match fix.query(None, QueryPayload::Snapshot) {
        QueryResult::Snapshot { snapshot } => {
            assert_eq!(snapshot.head, fix.status().journal_head);
            assert_eq!(snapshot.sessions.len(), 1);
        }
        other => panic!("snapshot answered with {other:?}"),
    }
}

#[tokio::test]
async fn a_command_id_replays_its_outcome_and_conflicts_on_a_new_payload() {
    let fix = Fixture::open().await;
    let command_id = CommandId::new();
    let create = || CommandPayload::CreateSession {
        title: "idempotent".to_string(),
    };

    let first = fix.command(None, None, command_id, create());
    let replay = fix.command(None, None, command_id, create());
    assert_eq!(accepted(&first), accepted(&replay));

    let conflict = fix.command(
        None,
        None,
        command_id,
        CommandPayload::CreateSession {
            title: "different".to_string(),
        },
    );
    assert_eq!(rejection(&conflict).code, RejectionCode::Conflict);
    // The conflict did not create a second session.
    match fix.query(None, QueryPayload::ListSessions) {
        QueryResult::Sessions { sessions } => assert_eq!(sessions.len(), 1),
        other => panic!("list answered with {other:?}"),
    }
}

#[tokio::test]
async fn renaming_is_guarded_by_the_version_the_client_observed() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("draft");
    let observed = fix.session(session_id).expect("session exists").version;

    let stale = fix.command(
        Some(session_id),
        Some(observed.saturating_sub(1)),
        CommandId::new(),
        CommandPayload::RenameSession {
            session_id,
            title: "renamed".to_string(),
        },
    );
    assert_eq!(rejection(&stale).code, RejectionCode::Conflict);

    let rename = fix.command(
        Some(session_id),
        Some(observed),
        CommandId::new(),
        CommandPayload::RenameSession {
            session_id,
            title: "renamed".to_string(),
        },
    );
    accepted(&rename);
    assert_eq!(
        fix.session(session_id).expect("session exists").title,
        "renamed"
    );
}

#[tokio::test]
async fn unknown_sessions_and_foreign_workspaces_are_refused() {
    let fix = Fixture::open().await;

    let stray = SessionId::new();
    let missing = fix.command(
        Some(stray),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id: stray,
            text: "anyone home".to_string(),
        },
    );
    assert_eq!(rejection(&missing).code, RejectionCode::NotFound);

    // A second workspace id is never served by this Host.
    let foreign = fix.send(Request::Command(CommandEnvelope {
        command_id: CommandId::new(),
        client_instance_id: fix.ctx.client_instance_id,
        target: Target {
            workspace_id: WorkspaceId::new(),
            session_id: None,
        },
        expected_version: None,
        payload: CommandPayload::CreateSession {
            title: "foreign".to_string(),
        },
    }));
    match foreign {
        Response::Command(CommandOutcome::Rejected { rejection, .. }) => {
            assert_eq!(rejection.code, RejectionCode::Unauthorized);
        }
        other => panic!("foreign workspace answered with {other:?}"),
    }

    // A client instance id that belongs to another connection is refused.
    let impostor = fix.send(Request::Command(CommandEnvelope {
        command_id: CommandId::new(),
        client_instance_id: ClientInstanceId::new(),
        target: fix.target(None),
        expected_version: None,
        payload: CommandPayload::CreateSession {
            title: "impostor".to_string(),
        },
    }));
    match impostor {
        Response::Command(CommandOutcome::Rejected { rejection, .. }) => {
            assert_eq!(rejection.code, RejectionCode::Unauthorized);
        }
        other => panic!("impostor answered with {other:?}"),
    }
}

#[tokio::test]
async fn cancelling_a_live_execution_commits_one_terminal_fact() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("cancel me");
    let execution_id = ExecutionId::new();
    fix.request_turn(session_id, execution_id, "never finishes");

    let cancel = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::CancelExecution { execution_id },
    );
    accepted(&cancel);
    let projection = fix.session(session_id).expect("session exists");
    assert_eq!(projection.executions[0].status, ExecutionStatus::Cancelled);

    // The execution already reached a terminal fact; a second cancel is
    // refused instead of committing a duplicate boundary.
    let again = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::CancelExecution { execution_id },
    );
    assert_eq!(rejection(&again).code, RejectionCode::ExecutionNotRunning);
}

#[tokio::test]
async fn an_operator_decision_must_echo_the_pending_digest_and_wakes_the_executor() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("approvals");
    let execution_id = ExecutionId::new();
    fix.request_turn(session_id, execution_id, "needs approval");

    let handle = ApprovalHandle::attached(ApprovalGate {
        core: Arc::clone(&fix.host.core),
        session_id,
        execution_id,
    });
    let request = ApprovalRequest {
        summary: "delete the build directory".to_string(),
        action: ApprovalTarget {
            tool: "shell".to_string(),
            action: "exec".to_string(),
            resource: Some("rm -rf target".to_string()),
        },
        expires_in_secs: 60,
    };
    let waiting = tokio::spawn(async move { handle.request(request).await });

    let approval = loop {
        match fix.query(
            Some(session_id),
            QueryPayload::PendingApprovals {
                session_id: Some(session_id),
            },
        ) {
            QueryResult::Approvals { approvals } if !approvals.is_empty() => {
                break approvals[0].clone();
            }
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    assert_eq!(approval.execution_id, execution_id);
    assert_eq!(
        fix.session(session_id).expect("session exists").executions[0].status,
        ExecutionStatus::WaitingApproval
    );

    // Approving a different digest is a refusal, not a judgement call.
    let wrong = fix.control(
        Some(session_id),
        ControlPayload::DecideApproval {
            approval_id: approval.approval_id,
            decision: ApprovalVerdict::Approve,
            action_digest: ActionDigest::of(&"something else"),
            note: None,
        },
    );
    assert_eq!(
        rejection(&wrong).code,
        RejectionCode::ApprovalDigestMismatch
    );

    let decided = fix.control(
        Some(session_id),
        ControlPayload::DecideApproval {
            approval_id: approval.approval_id,
            decision: ApprovalVerdict::Approve,
            action_digest: approval.action_digest.clone(),
            note: Some("reviewed".to_string()),
        },
    );
    accepted(&decided);

    let outcome = waiting.await.expect("approval task did not panic");
    let outcome = outcome.expect("operator approved the action");
    assert!(outcome.is_approved());
    assert_eq!(outcome.approval_id, approval.approval_id);

    // Deciding again is refused: the approval already has its fact.
    let again = fix.control(
        Some(session_id),
        ControlPayload::DecideApproval {
            approval_id: approval.approval_id,
            decision: ApprovalVerdict::Reject,
            action_digest: approval.action_digest.clone(),
            note: None,
        },
    );
    assert_eq!(
        rejection(&again).code,
        RejectionCode::ApprovalAlreadyDecided
    );
}

#[tokio::test]
async fn drain_stops_the_host_and_refuses_new_work() {
    let fix = Fixture::open().await;
    fix.host.request_shutdown(ShutdownMode::Graceful);
    fix.host.drain().await;
    assert_eq!(fix.host.phase(), HostPhase::Stopped);

    let refused = fix.command(
        None,
        None,
        CommandId::new(),
        CommandPayload::CreateSession {
            title: "too late".to_string(),
        },
    );
    assert_eq!(rejection(&refused).code, RejectionCode::NotReady);
    // A query still works: a stopped Host explains itself instead of vanishing.
    assert_eq!(fix.status().phase, HostPhase::Stopped);
}

#[tokio::test]
async fn recovery_interrupts_a_turn_that_never_reached_a_terminal_fact() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = workspace;
    config.echo = true;
    config.shutdown_grace = Duration::from_secs(1);

    let session_id;
    let execution_id = ExecutionId::new();
    {
        let host = RuntimeHost::open(config.clone()).await.unwrap();
        let ctx = host.connection(Principal::local(host.local_uid()));
        let fixture = Fixture {
            host,
            ctx,
            _dir: tempfile::tempdir().unwrap(),
        };
        session_id = fixture.create_session("crash mid-turn");
        fixture.request_turn(session_id, execution_id, "the Host died here");
    }

    // The lock, journal and audit log all belong to the dropped Host now.
    let host = RuntimeHost::open(config).await.unwrap();
    let ctx = host.connection(Principal::local(host.local_uid()));
    let fixture = Fixture {
        host,
        ctx,
        _dir: tempfile::tempdir().unwrap(),
    };
    let session = fixture.session(session_id).expect("session survived");
    assert_eq!(session.executions.len(), 1);
    assert_eq!(session.executions[0].execution_id, execution_id);
    assert_eq!(session.executions[0].status, ExecutionStatus::Interrupted);
    assert!(
        session.executions[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("unknown"))
    );
    assert_eq!(fixture.host.phase(), HostPhase::Ready);

    // Recovery is idempotent: a second open appends its own `HostReady` but no
    // second `ExecutionInterrupted` boundary.
    let head = fixture.status().journal_head;
    drop(fixture);
    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = dir.path().join("workspace");
    config.echo = true;
    let host = RuntimeHost::open(config).await.unwrap();
    assert_eq!(host.phase(), HostPhase::Ready);
    assert_eq!(host.status().journal_head, head + 1);
}

/// A session's configuration is the executor's to resolve, not a constant in
/// the contract: a Host must record what the turn will actually run.
#[tokio::test]
async fn a_session_records_the_configuration_its_executor_resolves() {
    let fix = Fixture::open_with(Arc::new(ConfiguredExecutor)).await;
    let session_id = fix.create_session("configured");
    let session = fix.session(session_id).expect("session exists");
    assert_eq!(session.config, ConfiguredExecutor.default_config());
    assert_eq!(session.config.preset.as_deref(), Some("review"));
}

#[tokio::test]
async fn configuring_a_session_journals_the_patch_and_refuses_an_unknown_mode() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("configure me");
    assert_eq!(
        fix.session(session_id).expect("session exists").config,
        SessionConfig::default()
    );

    let change = SessionConfigChange {
        permission: Some("read-only".to_string()),
        plan_mode: Some(true),
        ..SessionConfigChange::default()
    };
    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::ConfigureSession { session_id, change },
    );
    accepted(&outcome);
    let config = fix.session(session_id).expect("session exists").config;
    assert_eq!(config.permission, "read-only");
    assert!(config.plan_mode);
    // Fields the patch did not name kept their stored value.
    assert_eq!(config.provider, None);
    assert_eq!(config.model, None);

    // The executor refuses a mode the contract does not name, and the refusal
    // leaves the committed configuration untouched.
    let bad = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::ConfigureSession {
            session_id,
            change: SessionConfigChange {
                permission: Some("yolo".to_string()),
                ..SessionConfigChange::default()
            },
        },
    );
    let refusal = rejection(&bad);
    assert_eq!(refusal.code, RejectionCode::InvalidRequest);
    assert!(
        refusal.message.contains("unknown permission"),
        "the refusal names the value: {}",
        refusal.message
    );
    assert_eq!(
        fix.session(session_id)
            .expect("session exists")
            .config
            .permission,
        "read-only"
    );
}

/// A running turn's answer reaches subscribers as activity, and the journal
/// keeps no trace of the pieces: the commit carries the whole text.
#[tokio::test]
async fn a_live_answer_is_broadcast_to_subscribers_and_never_journaled() {
    let fix = Fixture::open_with(Arc::new(StreamingExecutor)).await;
    let session_id = fix.create_session("streaming");
    let head = fix.status().journal_head;
    let subscription = fix
        .host
        .handle_request(
            &fix.ctx,
            Request::Subscribe(Subscribe {
                cursor: Some(Cursor { sequence: head }),
            }),
        )
        .subscription
        .expect("subscribe returns a live subscription");
    let mut activity = subscription.activity;

    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id,
            text: "stream the answer".to_string(),
        },
    );
    let execution_id: ExecutionId = accepted(&outcome)
        .to_string()
        .parse()
        .expect("execution operation names a uuid");
    wait_until("the streamed turn to finish", || {
        fix.session(session_id).and_then(|session| {
            session
                .executions
                .iter()
                .find(|execution| execution.execution_id == execution_id)
                .map(|execution| execution.status)
        }) == Some(ExecutionStatus::Completed)
    })
    .await;

    let mut streamed = String::new();
    let mut pieces = 0;
    while let Ok(frame) = activity.try_recv() {
        assert_eq!(frame.session_id, session_id);
        assert_eq!(frame.execution_id, execution_id);
        let ExecutionActivity::AssistantText { text } = frame.activity;
        pieces += 1;
        streamed.push_str(&text);
    }
    assert_eq!(pieces, 2, "every emitted piece reached the subscriber");
    assert_eq!(streamed, "streamed answer");

    // The pieces are not durable facts: the turn appended its request and its
    // terminal fact, and nothing else.
    let appended = fix
        .host
        .core
        .journal
        .all_events()
        .iter()
        .filter(|event| event.sequence > head)
        .count();
    assert_eq!(appended, 2, "activity must never reach the journal");

    let session = fix.session(session_id).expect("session exists");
    assert_eq!(
        session.messages.last().expect("assistant message").text,
        streamed,
        "the commit carries the whole answer the client streamed"
    );
}

/// Tool facts are durable before the turn's terminal fact, so a client that
/// reconnects after the turn still renders the calls it missed.
#[tokio::test]
async fn tool_progress_is_journaled_before_the_turns_terminal_fact() {
    let fix = Fixture::open_with(Arc::new(ToolProgressExecutor)).await;
    let session_id = fix.create_session("tools");
    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id,
            text: "list the files".to_string(),
        },
    );
    let execution_id: ExecutionId = accepted(&outcome)
        .to_string()
        .parse()
        .expect("execution operation names a uuid");

    wait_until("the tool turn to finish", || {
        fix.session(session_id).and_then(|session| {
            session
                .executions
                .iter()
                .find(|execution| execution.execution_id == execution_id)
                .map(|execution| execution.status)
        }) == Some(ExecutionStatus::Completed)
    })
    .await;

    let session = fix.session(session_id).expect("session exists");
    assert_eq!(session.tool_calls.len(), 1);
    let call = &session.tool_calls[0];
    assert_eq!(call.execution_id, Some(execution_id));
    assert_eq!(call.step, 1);
    assert_eq!(call.tool, "shell");
    assert_eq!(call.status, ToolCallStatus::Completed);
    assert_eq!(call.output.as_deref(), Some("total 0"));
    assert_eq!(call.duration_ms, Some(5));

    let events = fix.host.core.journal.all_events();
    let started = events
        .iter()
        .position(|event| matches!(&event.payload, EventPayload::ToolCallStarted { .. }))
        .expect("a started fact was journaled");
    let finished = events
        .iter()
        .position(|event| matches!(&event.payload, EventPayload::ToolCallFinished { .. }))
        .expect("a finished fact was journaled");
    let terminal = events
        .iter()
        .position(|event| matches!(&event.payload, EventPayload::TurnSucceeded { .. }))
        .expect("a terminal fact was journaled");
    assert!(
        started < finished && finished < terminal,
        "tool facts must precede the terminal fact: {started} {finished} {terminal}"
    );
}

/// The goal verbs are the desktop's `/goal` line, and each one is a committed
/// fact: a second client reads the same state, and the create rule refuses to
/// overwrite an unfinished goal.
#[tokio::test]
async fn goal_verbs_commit_one_fact_each_and_guard_the_create_rule() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("pursue a goal");
    assert_eq!(fix.session(session_id).expect("session exists").goal, None);

    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetGoal {
            session_id,
            objective: "  ship the parity slice  ".to_string(),
        },
    );
    accepted(&outcome);
    let goal = fix
        .session(session_id)
        .expect("session exists")
        .goal
        .expect("a goal landed");
    assert_eq!(goal.state, GoalState::Active);
    // The objective is trimmed and otherwise verbatim, the way the desktop's
    // goal store keeps the operator's own words.
    assert_eq!(goal.objective, "ship the parity slice");
    assert_eq!(goal.rounds, 0);
    assert_eq!(goal.max_rounds, 256);

    // An unfinished goal is edited, paused, resumed or cleared - never
    // replaced by a second `/goal <objective>`.
    let replaced = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetGoal {
            session_id,
            objective: "something else".to_string(),
        },
    );
    let refusal = rejection(&replaced);
    assert_eq!(refusal.code, RejectionCode::Conflict);
    assert!(
        refusal.message.contains("active"),
        "the refusal names the state: {}",
        refusal.message
    );
    assert_eq!(
        fix.session(session_id).expect("session exists").goal,
        Some(goal.clone())
    );

    // Editing keeps the state and the rounds the goal already spent, so a
    // correction is not a fresh start.
    let edited = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::EditGoal {
            session_id,
            objective: "ship the whole slice".to_string(),
        },
    );
    accepted(&edited);
    let goal = fix
        .session(session_id)
        .expect("session exists")
        .goal
        .expect("the goal survived the edit");
    assert_eq!(goal.objective, "ship the whole slice");
    assert_eq!(goal.state, GoalState::Active);
    assert_eq!(goal.rounds, 0);

    let paused = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PauseGoal { session_id },
    );
    accepted(&paused);
    assert_eq!(
        fix.session(session_id)
            .expect("session exists")
            .goal
            .map(|goal| goal.state),
        Some(GoalState::Paused)
    );
    // A paused goal is still a goal: pausing twice is a conflict, not a
    // second pause.
    let twice = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PauseGoal { session_id },
    );
    assert_eq!(rejection(&twice).code, RejectionCode::Conflict);

    let resumed = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::ResumeGoal { session_id },
    );
    accepted(&resumed);
    assert_eq!(
        fix.session(session_id)
            .expect("session exists")
            .goal
            .map(|goal| goal.state),
        Some(GoalState::Active)
    );

    let cleared = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::ClearGoal { session_id },
    );
    accepted(&cleared);
    assert_eq!(fix.session(session_id).expect("session exists").goal, None);

    // Every verb needs a goal to act on, and says so instead of inventing one.
    let nothing_to_pause = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PauseGoal { session_id },
    );
    let refusal = rejection(&nothing_to_pause);
    assert_eq!(refusal.code, RejectionCode::NotFound);
    assert!(refusal.message.contains("no goal"), "{}", refusal.message);

    // A blank objective is refused for its own shape, before the goal is read.
    let blank = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetGoal {
            session_id,
            objective: "   ".to_string(),
        },
    );
    assert_eq!(rejection(&blank).code, RejectionCode::InvalidRequest);
}

/// `/feedback` is the desktop's session note, and it is durable Host state:
/// the note is trimmed, a blank one clears rather than stores an empty string,
/// and the cap refuses an oversized note instead of jamming every snapshot.
#[tokio::test]
async fn feedback_is_one_durable_note_and_blank_text_clears_it() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("record a note");
    assert_eq!(
        fix.session(session_id).expect("session exists").feedback,
        None
    );

    let saved = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetFeedback {
            session_id,
            text: "  the transcript mirrors the desktop  ".to_string(),
        },
    );
    accepted(&saved);
    assert_eq!(
        fix.session(session_id).expect("session exists").feedback,
        Some("the transcript mirrors the desktop".to_string()),
        "the note is trimmed and otherwise verbatim"
    );

    // A second write replaces the whole note; there is one note, not a list.
    let replaced = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetFeedback {
            session_id,
            text: "one note only".to_string(),
        },
    );
    accepted(&replaced);
    assert_eq!(
        fix.session(session_id).expect("session exists").feedback,
        Some("one note only".to_string())
    );

    let cleared = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetFeedback {
            session_id,
            text: "   ".to_string(),
        },
    );
    accepted(&cleared);
    assert_eq!(
        fix.session(session_id).expect("session exists").feedback,
        None,
        "blank text clears the note instead of storing an empty one"
    );

    // A note over the cap is refused for its own shape, before it is stored.
    let oversized = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SetFeedback {
            session_id,
            text: "x".repeat(MAX_FEEDBACK_NOTE + 1),
        },
    );
    let refusal = rejection(&oversized);
    assert_eq!(refusal.code, RejectionCode::InvalidRequest);
    assert!(
        refusal.message.contains("limit"),
        "the refusal names the cap: {}",
        refusal.message
    );
}

/// The memory query is the executor's answer, not a Host-side invention: a
/// terminal that browses the store lists what the executor reports, and a Host
/// whose executor has no store says `unsupported` rather than an empty list that
/// would read like "nothing is remembered".
#[tokio::test]
async fn a_memory_query_returns_the_executors_report() {
    let fix = Fixture::open_with(Arc::new(MemoryExecutor)).await;
    let QueryResult::Memories { report } = fix.query(
        None,
        QueryPayload::Memories {
            query: Some("release".to_string()),
        },
    ) else {
        panic!("the Host answered a memory query with another projection")
    };
    assert_eq!(report.state, "ready");
    assert_eq!(report.memories.len(), 1);
    assert_eq!(report.memories[0].id, "memory-1");
    assert_eq!(report.memories[0].revision, 4);
    assert_eq!(
        report.memories[0].score,
        Some(0.75),
        "a search keeps its score all the way to the client"
    );

    // Listing omits the score: the same record, without a recall rank.
    let QueryResult::Memories { report } = fix.query(None, QueryPayload::Memories { query: None })
    else {
        panic!("the Host answered a memory query with another projection")
    };
    assert_eq!(report.memories[0].score, None);

    // The default executor never opened the kernel, and says exactly that.
    let fix = Fixture::open().await;
    let QueryResult::Memories { report } = fix.query(None, QueryPayload::Memories { query: None })
    else {
        panic!("the Host answered a memory query with another projection")
    };
    assert_eq!(report.state, "unsupported");
}

/// A files query is the desktop's Workspace Files browser as a protocol
/// surface: the Host hands the client's path to the executor, which owns what a
/// path means, and returns the report unchanged - a listing with rows, a
/// document with its text.
#[tokio::test]
async fn a_files_query_returns_the_executors_report() {
    let fix = Fixture::open_with(Arc::new(FilesExecutor)).await;
    let QueryResult::Files { report } = fix.query(
        None,
        QueryPayload::Files {
            path: String::new(),
        },
    ) else {
        panic!("the Host answered a files query with another projection")
    };
    assert_eq!(report.state, "dir");
    assert_eq!(report.root, "/Users/me/bos");
    assert_eq!(report.entries.len(), 1);
    assert!(report.entries[0].dir);

    // A child path comes back as a document: the Host did not decide what the
    // path meant, it asked.
    let QueryResult::Files { report } = fix.query(
        None,
        QueryPayload::Files {
            path: "src/main.rs".to_string(),
        },
    ) else {
        panic!("the Host answered a files query with another projection")
    };
    assert_eq!(report.state, "file");
    assert_eq!(report.path, "src/main.rs");
    assert_eq!(
        report.content.as_deref(),
        Some("// the executor's own text")
    );

    // A Host whose executor has no workspace surface says so, rather than
    // listing an empty directory.
    let fix = Fixture::open().await;
    let QueryResult::Files { report } = fix.query(
        None,
        QueryPayload::Files {
            path: String::new(),
        },
    ) else {
        panic!("the Host answered a files query with another projection")
    };
    assert_eq!(report.state, "unsupported");
}

/// An executor that compares one path, so a diff query proves the Host hands
/// the client's path to the executor and returns its view unchanged.
struct DiffExecutor;

#[async_trait::async_trait]
impl TurnExecutor for DiffExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[diff echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn diff(&self, path: &str) -> WorkspaceDiffView {
        let changes = vec![WorkspaceChangeView {
            path: "src/main.rs".to_string(),
            letter: "M".to_string(),
            label: "Modified".to_string(),
        }];
        if path.is_empty() {
            return WorkspaceDiffView {
                root: "/Users/me/bos".to_string(),
                state: "listed".to_string(),
                vcs: "git".to_string(),
                changes,
                ..WorkspaceDiffView::default()
            };
        }
        WorkspaceDiffView {
            root: "/Users/me/bos".to_string(),
            state: "patch".to_string(),
            vcs: "git".to_string(),
            changes,
            path: path.to_string(),
            patch: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            ..WorkspaceDiffView::default()
        }
    }
}

/// A diff query is the desktop Diff tab as a protocol surface: no path asks what
/// the working copy changed, a path asks for that comparison, and the Host
/// returns whatever the executor answered without deciding either way itself.
#[tokio::test]
async fn a_diff_query_returns_the_executors_comparison() {
    let fix = Fixture::open_with(Arc::new(DiffExecutor)).await;
    let QueryResult::Diff { view } = fix.query(
        None,
        QueryPayload::Diff {
            path: String::new(),
        },
    ) else {
        panic!("the Host answered a diff query with another projection")
    };
    assert_eq!(view.state, "listed");
    assert_eq!(view.vcs, "git");
    assert_eq!(view.changes.len(), 1);
    assert_eq!(view.changes[0].letter, "M");

    // A path comes back as that path's comparison: the Host did not decide
    // which file the reader meant, it asked.
    let QueryResult::Diff { view } = fix.query(
        None,
        QueryPayload::Diff {
            path: "src/main.rs".to_string(),
        },
    ) else {
        panic!("the Host answered a diff query with another projection")
    };
    assert_eq!(view.state, "patch");
    assert_eq!(view.path, "src/main.rs");
    assert_eq!(view.patch.as_deref(), Some("@@ -1 +1 @@\n-old\n+new"));

    // A Host whose executor has no workspace surface says so, rather than
    // reporting a clean working copy nobody compared.
    let fix = Fixture::open().await;
    let QueryResult::Diff { view } = fix.query(
        None,
        QueryPayload::Diff {
            path: String::new(),
        },
    ) else {
        panic!("the Host answered a diff query with another projection")
    };
    assert_eq!(view.state, "unsupported");
}

/// An executor that serves a board, so a tasks query proves the Host hands the
/// ask to the executor and returns its report unchanged.
struct TasksExecutor;

#[async_trait::async_trait]
impl TurnExecutor for TasksExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[tasks echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn tasks(&self) -> TaskBoardReport {
        TaskBoardReport {
            root: "/Users/me/bos".to_string(),
            state: "ready".to_string(),
            sections: vec![TaskSectionView {
                section: "running".to_string(),
                rows: vec![TaskRowView {
                    id: "task-1".to_string(),
                    title: "ship the release".to_string(),
                    tier: "turn".to_string(),
                    status: "running".to_string(),
                    host: "local".to_string(),
                    session: "s1".to_string(),
                    parent: String::new(),
                    deps: Vec::new(),
                    created_ms: 1_700_000_000_000,
                    next_fire_ms: None,
                    attempts: 2,
                    verification: "not_requested".to_string(),
                }],
            }],
            error: None,
        }
    }

    /// The record behind the row, with the id the query named echoed back: the
    /// Host must not answer a detail query with the board it already holds.
    fn task_detail(&self, id: &str) -> TaskDetailReport {
        TaskDetailReport {
            root: "/Users/me/bos".to_string(),
            state: "ready".to_string(),
            id: id.to_string(),
            title: "ship the release".to_string(),
            intent: "cut the release branch".to_string(),
            tier: "turn".to_string(),
            status: "running".to_string(),
            host: "local".to_string(),
            trigger: "now".to_string(),
            created_ms: 1_700_000_000_000,
            updated_ms: 1_700_000_060_000,
            deps: vec!["build".to_string()],
            max_steps: 20,
            max_tokens: 200_000,
            deadline_ms: 600_000,
            retry: "manual".to_string(),
            max_attempts: 3,
            verification: "not_requested".to_string(),
            verification_contract: "none".to_string(),
            attempts: vec![TaskAttemptView {
                id: format!("{id}-1"),
                tier: "turn".to_string(),
                status: "succeeded".to_string(),
                result: "pushed the tag".to_string(),
                change_set: "cs-17".to_string(),
                ..TaskAttemptView::default()
            }],
            ..TaskDetailReport::default()
        }
    }
}

/// The task board is the executor's answer, not a Host-side invention: a Host
/// whose executor carries the platform's store lists those records, and a Host
/// whose executor carries no task surface says `unsupported` rather than drawing
/// four empty sections that would read like "the workspace has no tasks".
#[tokio::test]
async fn a_task_board_query_returns_the_executors_report() {
    let fix = Fixture::open_with(Arc::new(TasksExecutor)).await;
    let QueryResult::Tasks { report } = fix.query(None, QueryPayload::Tasks) else {
        panic!("the Host answered a tasks query with another projection")
    };
    assert_eq!(report.state, "ready");
    assert_eq!(report.root, "/Users/me/bos");
    assert_eq!(report.sections.len(), 1);
    assert_eq!(report.sections[0].section, "running");
    assert_eq!(report.sections[0].rows.len(), 1);
    assert_eq!(report.sections[0].rows[0].title, "ship the release");
    assert_eq!(
        report.sections[0].rows[0].attempts, 2,
        "the attempt count travels with the row"
    );

    let fix = Fixture::open().await;
    let QueryResult::Tasks { report } = fix.query(None, QueryPayload::Tasks) else {
        panic!("the Host answered a tasks query with another projection")
    };
    assert_eq!(report.state, "unsupported");
}

/// A detail query is the executor's record for the id the client named, and a
/// Host whose executor carries no task surface answers `unsupported` rather
/// than an empty record that would read like a task with nothing in it.
#[tokio::test]
async fn a_task_detail_query_returns_the_record_for_the_named_id() {
    let fix = Fixture::open_with(Arc::new(TasksExecutor)).await;
    let QueryResult::TaskDetail { report } = fix.query(
        None,
        QueryPayload::TaskDetail {
            id: "task-1".to_string(),
        },
    ) else {
        panic!("the Host answered a task detail query with another projection")
    };
    assert_eq!(report.state, "ready");
    assert_eq!(
        report.id, "task-1",
        "the id the client named is the record's id"
    );
    assert_eq!(report.intent, "cut the release branch");
    assert_eq!(report.deps, ["build"]);
    assert_eq!(report.attempts[0].id, "task-1-1");
    assert_eq!(report.attempts[0].result, "pushed the tag");

    let fix = Fixture::open().await;
    let QueryResult::TaskDetail { report } = fix.query(
        None,
        QueryPayload::TaskDetail {
            id: "task-1".to_string(),
        },
    ) else {
        panic!("the Host answered a task detail query with another projection")
    };
    assert_eq!(report.state, "unsupported");
    assert!(report.attempts.is_empty(), "no attempts are invented");
}

/// Parking is a visibility fact, not a deletion: the chat leaves the session
/// list and the snapshot a reconnecting client reconciles against, its rows stay
/// folded, the bin is the one reader that shows it, and a restore puts it back.
#[tokio::test]
async fn a_parked_session_leaves_the_lists_and_comes_back_on_restore() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("keep me");

    accepted(&fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::TrashSession { session_id },
    ));

    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::ListSessions) else {
        panic!("the session list answered with another projection")
    };
    assert!(sessions.is_empty(), "a parked chat is out of the list");
    let QueryResult::Snapshot { snapshot } = fix.query(None, QueryPayload::Snapshot) else {
        panic!("the snapshot answered with another projection")
    };
    assert!(
        !snapshot
            .sessions
            .iter()
            .any(|session| session.session_id == session_id),
        "a parked chat is out of the snapshot"
    );

    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::Trash) else {
        panic!("the bin answered with another projection")
    };
    assert_eq!(sessions.len(), 1, "the bin is where the chat is now");
    assert_eq!(sessions[0].title, "keep me");
    assert!(
        fix.session(session_id)
            .expect("the fold kept the parked session")
            .trashed
    );

    // A second park is a refusal rather than a second fact.
    let again = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::TrashSession { session_id },
    );
    assert_eq!(rejection(&again).code, RejectionCode::InvalidRequest);

    accepted(&fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::RestoreSession { session_id },
    ));
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::ListSessions) else {
        panic!("the session list answered with another projection")
    };
    assert_eq!(sessions.len(), 1, "restore puts the chat back in the list");
    assert!(
        !fix.session(session_id)
            .expect("the restored session answers")
            .trashed
    );
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::Trash) else {
        panic!("the bin answered with another projection")
    };
    assert!(sessions.is_empty(), "the bin is empty after the restore");
}

/// A turn in flight is the one thing parking refuses: a parked chat whose
/// approval is still running would hide it behind an overlay every client treats
/// as history.
#[tokio::test]
async fn parking_waits_for_a_turn_in_flight_to_settle() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("busy");
    fix.request_turn(session_id, ExecutionId::new(), "still working");

    let refused = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::TrashSession { session_id },
    );
    let rejection = rejection(&refused);
    assert_eq!(rejection.code, RejectionCode::InvalidRequest);
    assert!(
        rejection.message.contains("in flight"),
        "the refusal names the reason: {}",
        rejection.message
    );
    assert!(
        !fix.session(session_id)
            .expect("the session is still there")
            .trashed
    );
}

/// Pinning is a sort key, archiving is the sidebar's filter: a starred chat
/// sorts first in `ListSessions`, an archived one leaves it without dropping a
/// row, and unarchiving returns the same chat with the same star.
#[tokio::test]
async fn a_pinned_session_sorts_first_and_an_archived_one_leaves_the_list() {
    let fix = Fixture::open().await;
    let starred = fix.create_session("starred");
    let other = fix.create_session("other");

    accepted(&fix.command(
        Some(starred),
        None,
        CommandId::new(),
        CommandPayload::PinSession {
            session_id: starred,
            pinned: true,
        },
    ));
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::ListSessions) else {
        panic!("the session list answered with another projection")
    };
    assert_eq!(sessions.len(), 2, "pinning keeps both chats in the list");
    assert_eq!(
        sessions[0].session_id, starred,
        "the pinned chat sorts ahead of the newer one"
    );
    assert!(sessions[0].pinned, "the summary carries the star");
    assert!(!sessions[0].archived);

    // A second pin is a refusal rather than a second fact, because a client
    // asks for the state it wants instead of sending a toggle.
    let again = fix.command(
        Some(starred),
        None,
        CommandId::new(),
        CommandPayload::PinSession {
            session_id: starred,
            pinned: true,
        },
    );
    assert_eq!(rejection(&again).code, RejectionCode::InvalidRequest);

    accepted(&fix.command(
        Some(starred),
        None,
        CommandId::new(),
        CommandPayload::ArchiveSession {
            session_id: starred,
            archived: true,
        },
    ));
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::ListSessions) else {
        panic!("the session list answered with another projection")
    };
    assert_eq!(sessions.len(), 1, "an archived chat leaves the list");
    assert_eq!(sessions[0].session_id, other);
    let QueryResult::Snapshot { snapshot } = fix.query(None, QueryPayload::Snapshot) else {
        panic!("the snapshot answered with another projection")
    };
    assert!(
        !snapshot
            .sessions
            .iter()
            .any(|session| session.session_id == starred),
        "an archived chat is out of the snapshot too"
    );
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::Archived) else {
        panic!("the archived filter answered with another projection")
    };
    assert_eq!(sessions.len(), 1, "the archived filter is where it is now");
    assert_eq!(sessions[0].session_id, starred);
    assert!(
        sessions[0].pinned,
        "archiving hides the chat without clearing its star"
    );
    assert!(sessions[0].archived);
    assert!(
        fix.session(starred)
            .expect("the fold kept the archived session")
            .archived
    );

    accepted(&fix.command(
        Some(starred),
        None,
        CommandId::new(),
        CommandPayload::ArchiveSession {
            session_id: starred,
            archived: false,
        },
    ));
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::ListSessions) else {
        panic!("the session list answered with another projection")
    };
    assert_eq!(sessions.len(), 2, "unarchiving puts the chat back");
    assert_eq!(sessions[0].session_id, starred, "with its star intact");
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::Archived) else {
        panic!("the archived filter answered with another projection")
    };
    assert!(sessions.is_empty(), "the archived filter is empty again");
}

/// A star is a sort key and nothing else, so it lands on a chat that is still
/// running; archiving waits for the turn to settle, for the reason parking does.
#[tokio::test]
async fn a_star_lands_on_a_running_chat_and_archiving_waits_for_it() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("busy");
    fix.request_turn(session_id, ExecutionId::new(), "still working");

    accepted(&fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PinSession {
            session_id,
            pinned: true,
        },
    ));
    assert!(
        fix.session(session_id)
            .expect("the running session answers")
            .pinned,
        "a running chat can be starred"
    );

    let refused = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::ArchiveSession {
            session_id,
            archived: true,
        },
    );
    let rejection = rejection(&refused);
    assert_eq!(rejection.code, RejectionCode::InvalidRequest);
    assert!(
        rejection.message.contains("in flight"),
        "the refusal names the reason: {}",
        rejection.message
    );
    assert!(
        !fix.session(session_id)
            .expect("the session is still there")
            .archived,
        "the refusal changed nothing"
    );
}

/// Purge is the bin's second decision and the only irreversible one.
#[tokio::test]
async fn purge_drops_the_chat_for_good_and_only_from_the_bin() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("gone");

    // A chat that was never parked is not purgeable: the bin is the gate.
    let direct = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PurgeSession { session_id },
    );
    assert_eq!(
        rejection(&direct).code,
        RejectionCode::InvalidRequest,
        "purge refuses a chat that was never parked"
    );
    assert!(
        fix.session(session_id).is_some(),
        "the refusal changed nothing"
    );

    accepted(&fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::TrashSession { session_id },
    ));
    accepted(&fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::PurgeSession { session_id },
    ));

    assert!(
        fix.session(session_id).is_none(),
        "a purged session stops answering"
    );
    let QueryResult::Sessions { sessions } = fix.query(None, QueryPayload::Trash) else {
        panic!("the bin answered with another projection")
    };
    assert!(sessions.is_empty(), "nothing is left to restore");

    let restore = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::RestoreSession { session_id },
    );
    assert_eq!(rejection(&restore).code, RejectionCode::NotFound);
}

/// the Host hands the ask to the executor and returns its report unchanged.
struct ExtensionsExecutor;

#[async_trait::async_trait]
impl TurnExecutor for ExtensionsExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[extensions echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn extensions(&self) -> crate::runtime::contract::ExtensionsReport {
        crate::runtime::contract::ExtensionsReport {
            state: "ready".to_string(),
            extensions: vec![crate::runtime::contract::ExtensionView {
                id: "com.example.hosted".to_string(),
                name: "Hosted".to_string(),
                version: "1.0.0".to_string(),
                versions: vec!["1.0.0".to_string()],
                runtime: "declarative".to_string(),
                trust: "workspace".to_string(),
                signature: "unsigned".to_string(),
                source: "local".to_string(),
                state: "active".to_string(),
                health: "healthy".to_string(),
                configured: true,
                applied: true,
                tools: vec!["com.example.hosted.echo".to_string()],
                commands: Vec::new(),
                panels: Vec::new(),
                capabilities: Vec::new(),
                error: None,
            }],
            error: None,
        }
    }
}

/// The extension roster is the executor's answer, not a Host-side invention:
/// a Host whose executor carries the platform's extension host lists the
/// packages it reports, and a Host whose executor carries none says
/// `unsupported` rather than drawing an empty roster that would read like "no
/// package is installed".
#[tokio::test]
async fn an_extensions_query_returns_the_executors_report() {
    let fix = Fixture::open_with(Arc::new(ExtensionsExecutor)).await;
    let QueryResult::Extensions { report } = fix.query(None, QueryPayload::Extensions) else {
        panic!("the Host answered an extensions query with another projection")
    };
    assert_eq!(report.state, "ready");
    assert_eq!(report.extensions.len(), 1);
    let row = &report.extensions[0];
    assert_eq!(row.id, "com.example.hosted");
    assert_eq!(row.name, "Hosted");
    assert_eq!(row.health, "healthy");
    assert!(row.applied && row.configured);
    assert_eq!(
        row.tools,
        ["com.example.hosted.echo"],
        "the qualified tool name travels with the row"
    );

    let fix = Fixture::open().await;
    let QueryResult::Extensions { report } = fix.query(None, QueryPayload::Extensions) else {
        panic!("the Host answered an extensions query with another projection")
    };
    assert_eq!(report.state, "unsupported");
}

/// A Host whose executor carries a browser, so the pane's query is proved to
/// reach it rather than answering from the Host's own memory.
struct BrowserExecutor;

#[async_trait::async_trait]
impl TurnExecutor for BrowserExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[browser echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn browser(&self) -> crate::runtime::contract::BrowserView {
        crate::runtime::contract::BrowserView {
            status: "ready".to_string(),
            address: "https://example.com/docs".to_string(),
            title: "Docs".to_string(),
            url: "https://example.com/docs".to_string(),
            can_back: true,
            text: "Read the release checklist.".to_string(),
            text_chars: 27,
            text_total_chars: 27,
            elements: vec![crate::runtime::contract::BrowserElementView {
                reference: "e1".to_string(),
                kind: "link".to_string(),
                label: "Changelog".to_string(),
                href: Some("https://example.com/changelog".to_string()),
                ..crate::runtime::contract::BrowserElementView::default()
            }],
            elements_total: 1,
            ..crate::runtime::contract::BrowserView::default()
        }
    }
}

/// The page a turn read is the page `/browser` opens: the query reaches the
/// executor's browser, and a Host whose executor carries none says so instead
/// of drawing an empty pane the reader could type into.
#[tokio::test]
async fn a_browser_query_returns_the_executors_view() {
    let fix = Fixture::open_with(Arc::new(BrowserExecutor)).await;
    let QueryResult::Browser { view } = fix.query(None, QueryPayload::Browser) else {
        panic!("the Host answered a browser query with another projection")
    };
    assert_eq!(view.status, "ready");
    assert_eq!(view.address, "https://example.com/docs");
    assert_eq!(view.title, "Docs");
    assert!(view.can_back && !view.can_forward);
    assert_eq!(view.text, "Read the release checklist.");
    assert_eq!(view.elements_total, 1, "the element list travels whole");
    assert_eq!(view.elements[0].reference, "e1");
    assert_eq!(
        view.elements[0].href.as_deref(),
        Some("https://example.com/changelog"),
        "a link's target travels with its row"
    );

    let fix = Fixture::open().await;
    let QueryResult::Browser { view } = fix.query(None, QueryPayload::Browser) else {
        panic!("the Host answered a browser query with another projection")
    };
    assert_eq!(view.status, "unsupported");
    assert!(
        view.error
            .as_deref()
            .is_some_and(|reason| !reason.is_empty()),
        "an executor with no browser states why instead of an empty page"
    );
}

/// An executor that serves a rail and records the edits it was handed, so a
/// settings command proves the Host asks the executor to write: the Host itself
/// persists no setting, it hands the patch to the owner of the documents.
#[derive(Default)]
struct SettingsExecutor {
    writes: std::sync::Mutex<Vec<Vec<SettingEdit>>>,
}

#[async_trait::async_trait]
impl TurnExecutor for SettingsExecutor {
    async fn execute(&self, context: TurnContext) -> Result<TurnOutput, TurnError> {
        Ok(TurnOutput {
            text: format!("[settings echo] {}", context.text),
            ..TurnOutput::default()
        })
    }

    fn settings(&self, _section: Option<&str>) -> SettingsReport {
        SettingsReport {
            sections: vec![SettingsSectionView {
                section: "model".to_string(),
                rows: vec![SettingsRowView {
                    label: "temperature".to_string(),
                    value: "0.2".to_string(),
                    source: "workspace".to_string(),
                    key: Some("temperature".to_string()),
                }],
            }],
            state: "ready".to_string(),
            error: None,
        }
    }

    fn update_settings(&self, edits: &[SettingEdit]) -> Result<SettingsReport, String> {
        self.writes
            .lock()
            .expect("the write log is not poisoned")
            .push(edits.to_vec());
        Ok(self.settings(None))
    }
}

/// The settings query is the executor's resolution and the settings command is
/// the executor's write. The Host adds exactly two rules of its own: a write
/// names the workspace and not a chat, and a Host whose executor owns no
/// documents refuses instead of pretending the value landed.
#[tokio::test]
async fn a_settings_query_and_write_are_the_executors_own() {
    let executor = Arc::new(SettingsExecutor::default());
    let observed = Arc::clone(&executor);
    let fix = Fixture::open_with(executor).await;

    let QueryResult::Settings { report } = fix.query(
        None,
        QueryPayload::Settings {
            section: Some("MODEL".to_string()),
        },
    ) else {
        panic!("the Host answered a settings query with another projection")
    };
    assert_eq!(report.state, "ready");
    assert_eq!(report.sections.len(), 1);
    assert_eq!(report.sections[0].section, "model");
    assert_eq!(
        report.sections[0].rows[0].key.as_deref(),
        Some("temperature")
    );

    // The command carries the key exactly as the row spelled it, and the answer
    // is the executor's resolution rather than a Host-side echo.
    let edits = vec![SettingEdit {
        key: "temperature".to_string(),
        value: "0.4".to_string(),
    }];
    let outcome = fix.command(
        None,
        None,
        CommandId::new(),
        CommandPayload::UpdateSettings {
            edits: edits.clone(),
        },
    );
    accepted(&outcome);
    assert_eq!(
        observed.writes.lock().expect("the write log").as_slice(),
        [edits],
        "the Host handed the patch to the executor unchanged"
    );

    // Settings belong to the workspace: a client that names a session is
    // refused before the executor is asked, so a chat cannot configure the
    // workspace as a side effect of its own target.
    let session = fix.create_session("settings");
    let outcome = fix.command(
        Some(session),
        None,
        CommandId::new(),
        CommandPayload::UpdateSettings {
            edits: vec![SettingEdit {
                key: "temperature".to_string(),
                value: "0.6".to_string(),
            }],
        },
    );
    let refusal = rejection(&outcome);
    assert_eq!(refusal.code, RejectionCode::InvalidRequest);
    assert!(
        refusal.message.contains("takes no session"),
        "the refusal says which target is wrong: {}",
        refusal.message
    );
    assert_eq!(
        observed.writes.lock().expect("the write log").len(),
        1,
        "a refused target never reaches the writer"
    );

    // The default executor was not built from a shell configuration, and says
    // so instead of reporting a write it could not make: a value the Host
    // persisted but could not apply would make the next turn disagree with the
    // pane that wrote it.
    let fix = Fixture::open().await;
    let outcome = fix.command(
        None,
        None,
        CommandId::new(),
        CommandPayload::UpdateSettings {
            edits: vec![SettingEdit {
                key: "temperature".to_string(),
                value: "0.4".to_string(),
            }],
        },
    );
    let refusal = rejection(&outcome);
    assert_eq!(refusal.code, RejectionCode::InvalidRequest);
    assert!(
        refusal.message.contains("no settings"),
        "the refusal names the missing surface: {}",
        refusal.message
    );
}

/// The chat a terminal wrote is a document the window's own store reads.
///
/// This is the unification's central promise: one session format, one
/// directory. The test opens the desktop's Store over the same workspace the
/// Host served and reads the turn the terminal ran.
#[tokio::test]
async fn a_terminal_turn_is_written_to_the_desktop_session_store() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("shared chat");
    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id,
            text: "hello from the terminal".to_string(),
        },
    );
    let execution_id: ExecutionId = accepted(&outcome)
        .to_string()
        .parse()
        .expect("the operation names an execution");
    wait_until("the echo turn to finish", || {
        fix.session(session_id).and_then(|session| {
            session
                .executions
                .iter()
                .find(|execution| execution.execution_id == execution_id)
                .map(|execution| execution.status)
        }) == Some(ExecutionStatus::Completed)
    })
    .await;

    // Open the window's store exactly as the shell does: the workspace's id,
    // the workspace's root. It must find the chat the terminal just wrote.
    let root = std::fs::canonicalize(&fix.host.config().workspace_root).expect("canonical root");
    let workspace = fix
        .host
        .config()
        .session_workspace
        .clone()
        .unwrap_or_else(|| WorkspaceId::derived(&root.display().to_string()).to_string());
    let mut store = crate::store::Store::open_over(
        &[(workspace, root.display().to_string())],
        fix._dir.path().join("quarantine"),
    );
    let id = session_id.to_string();
    store.ensure_document(&id);
    store.rehydrate(&id);
    let session = store
        .session(&id)
        .expect("the desktop store lists the chat");
    assert_eq!(session.title, "shared chat");
    assert_eq!(session.messages.len(), 2, "the turn's two rows are on disk");
    assert_eq!(session.messages[0].text, "hello from the terminal");
    assert!(
        session.messages[1]
            .text
            .starts_with(&format!("[host echo {execution_id}]")),
        "the answer is the one the turn produced: {}",
        session.messages[1].text
    );
}

/// A chat the window wrote is a session a terminal lists and replays.
#[tokio::test]
async fn a_desktop_chat_is_served_to_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let root = std::fs::canonicalize(&workspace_root).unwrap();
    let workspace = "shared-workspace".to_string();
    // The window writes its chat before any Host exists for this directory.
    {
        let mut store = crate::store::Store::open_over(
            &[(workspace.clone(), root.display().to_string())],
            dir.path().join("quarantine"),
        );
        let mut session = crate::model::Session::new(&workspace, "written by the window");
        session
            .messages
            .push(crate::model::ChatMessage::user("hello from the window"));
        store
            .save(&session)
            .expect("the window's store accepts the chat");
        store.insert(session);
        assert_eq!(store.sessions.len(), 1, "the store lists its chat");
    }

    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = workspace_root;
    config.echo = true;
    config.session_workspace = Some(workspace);
    let host = RuntimeHost::open(config).await.expect("the Host opens");
    let ctx = host.connection(Principal::local(host.local_uid()));
    let target = Target {
        workspace_id: host.workspace_id(),
        session_id: None,
    };
    let response = host.handle_request(
        &ctx,
        Request::Query(QueryEnvelope {
            target,
            payload: QueryPayload::ListSessions,
        }),
    );
    let Response::Query(QueryResult::Sessions { sessions }) = response.response else {
        panic!("the list query was refused");
    };
    assert_eq!(sessions.len(), 1, "the window's chat is the Host's list");
    assert_eq!(sessions[0].title, "written by the window");

    let target = Target {
        workspace_id: host.workspace_id(),
        session_id: Some(sessions[0].session_id),
    };
    let response = host.handle_request(
        &ctx,
        Request::Query(QueryEnvelope {
            target,
            payload: QueryPayload::Session {
                session_id: sessions[0].session_id,
            },
        }),
    );
    let Response::Query(QueryResult::Session {
        session: Some(session),
    }) = response.response
    else {
        panic!("the session query was refused");
    };
    assert_eq!(session.messages.len(), 1);
    assert_eq!(session.messages[0].text, "hello from the window");
}

/// A turn survives a Host restart because it is written to the shared store.
#[tokio::test]
async fn a_restarted_host_replays_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let mut config = RuntimeConfig::with_data_dir(dir.path().join("runtime"));
    config.workspace_root = workspace_root;
    config.echo = true;
    config.session_workspace = Some("restart-workspace".to_string());

    let session_id = {
        let host = RuntimeHost::open(config.clone()).await.expect("first open");
        let ctx = host.connection(Principal::local(host.local_uid()));
        let target = Target {
            workspace_id: host.workspace_id(),
            session_id: None,
        };
        let response = host.handle_request(
            &ctx,
            Request::Command(CommandEnvelope {
                command_id: CommandId::new(),
                client_instance_id: ctx.client_instance_id,
                target,
                expected_version: None,
                payload: CommandPayload::CreateSession {
                    title: "survives a restart".to_string(),
                },
            }),
        );
        let Response::Command(outcome) = response.response else {
            panic!("the creation was refused");
        };
        let session_id: SessionId = accepted(&outcome)
            .to_string()
            .parse()
            .expect("the operation names a session");
        let target = Target {
            workspace_id: host.workspace_id(),
            session_id: Some(session_id),
        };
        let response = host.handle_request(
            &ctx,
            Request::Command(CommandEnvelope {
                command_id: CommandId::new(),
                client_instance_id: ctx.client_instance_id,
                target,
                expected_version: None,
                payload: CommandPayload::SendMessage {
                    session_id,
                    text: "before the restart".to_string(),
                },
            }),
        );
        let Response::Command(_) = response.response else {
            panic!("the turn was refused");
        };
        wait_until("the turn to finish", || {
            host.core
                .lock()
                .projections
                .session(session_id)
                .and_then(|session| session.executions.first().map(|execution| execution.status))
                == Some(ExecutionStatus::Completed)
        })
        .await;
        drop(host);
        session_id
    };

    let host = RuntimeHost::open(config).await.expect("second open");
    let ctx = host.connection(Principal::local(host.local_uid()));
    let target = Target {
        workspace_id: host.workspace_id(),
        session_id: Some(session_id),
    };
    let response = host.handle_request(
        &ctx,
        Request::Query(QueryEnvelope {
            target,
            payload: QueryPayload::Session { session_id },
        }),
    );
    let Response::Query(QueryResult::Session {
        session: Some(session),
    }) = response.response
    else {
        panic!("the restarted Host does not list the chat");
    };
    assert_eq!(session.title, "survives a restart");
    assert_eq!(session.messages.len(), 2, "the transcript survived");
    assert_eq!(session.messages[0].text, "before the restart");
    assert!(session.messages[1].text.starts_with("[host echo "));
}

/// A parked chat is moved into the shared bin and comes back whole.
///
/// This is the desktop's delete, not a visibility flag: the store's document and
/// its transcript pages move into the bin, and a restore returns the same
/// conversation. The terminal drives the same two commands.
#[tokio::test]
async fn a_parked_chat_round_trips_through_the_shared_bin() {
    let fix = Fixture::open().await;
    let session_id = fix.create_session("bin round trip");
    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::SendMessage {
            session_id,
            text: "remember me".to_string(),
        },
    );
    let execution_id: ExecutionId = accepted(&outcome)
        .to_string()
        .parse()
        .expect("the operation names an execution");
    wait_until("the turn to finish", || {
        fix.session(session_id).and_then(|session| {
            session
                .executions
                .iter()
                .find(|execution| execution.execution_id == execution_id)
                .map(|execution| execution.status)
        }) == Some(ExecutionStatus::Completed)
    })
    .await;

    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::TrashSession { session_id },
    );
    accepted(&outcome);
    assert!(
        fix.session(session_id).expect("the fold keeps it").trashed,
        "the journal marks the chat parked"
    );
    let bin = fix._dir.path().join("runtime").join("trash");
    let parked: Vec<_> = std::fs::read_dir(&bin)
        .expect("the shared bin exists")
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    assert_eq!(parked.len(), 1, "one envelope for the parked chat");
    let envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(parked[0].path()).expect("the envelope"))
            .expect("the envelope parses");
    assert_eq!(envelope["payload"]["id"], session_id.to_string());
    assert_eq!(envelope["name"], "bin round trip");

    let outcome = fix.command(
        Some(session_id),
        None,
        CommandId::new(),
        CommandPayload::RestoreSession { session_id },
    );
    accepted(&outcome);
    let restored = fix.session(session_id).expect("the chat comes back");
    assert!(!restored.trashed, "the journal marks it visible again");
    assert_eq!(restored.title, "bin round trip");
    assert_eq!(restored.messages.len(), 2, "the transcript came back too");
    assert_eq!(restored.messages[0].text, "remember me");
}
