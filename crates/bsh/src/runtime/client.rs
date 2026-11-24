//! Blocking client for the Runtime Host over local IPC.
//!
//! One client is one connection: a reader thread owns the socket's read half,
//! routes responses to the request that asked for them and forwards committed
//! events to the caller's frame channel. A client is a *view*: it never owns
//! execution, so dropping it (or crashing) cannot stop a turn.
//!
//! The TUI is the first consumer. The same client is the shape the GUI
//! migration (RFC-028) will use once it stops owning execution itself.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::runtime::contract::{
    ClientFrame, ClientInfo, ClientKind, CommandEnvelope, CommandOutcome, CommandPayload,
    ControlEnvelope, ControlPayload, Cursor, FederationReport, HostStatus, PrincipalView,
    QueryEnvelope, QueryPayload, QueryResult, Request, Response, ResponseFrame, ServerFrame,
    Subscribe, Target, Welcome,
};
use crate::runtime::endpoint::{Endpoint, EndpointError};
use crate::runtime::ids::{ClientInstanceId, CommandId, HostInstanceId, SessionId, WorkspaceId};

/// How long one request may wait for its response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Client failures.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No Host published a record.
    #[error(transparent)]
    Discovery(#[from] EndpointError),
    /// The endpoint exists but cannot be reached.
    #[error("cannot connect to {path}: {source}")]
    Connect {
        /// Socket path.
        path: String,
        /// OS error.
        #[source]
        source: std::io::Error,
    },
    /// The connection failed mid-request.
    #[error("runtime connection: {0}")]
    Io(#[from] std::io::Error),
    /// The Host answered with a protocol error.
    #[error("{}: {}", .0.code.as_str(), .0.message)]
    Protocol(crate::runtime::contract::ProtocolError),
    /// The Host rejected the request; nothing was mutated.
    #[error("{}: {}", .0.code.as_str(), .0.message)]
    Rejected(crate::runtime::contract::Rejection),
    /// The Host closed the connection while a request was outstanding.
    #[error("the Runtime Host closed the connection")]
    Closed,
    /// The Host's answer did not match what this client speaks.
    #[error("runtime contract mismatch: {0}")]
    Contract(String),
    /// A response arrived that this client could not attribute.
    #[error("unexpected response for request {0}")]
    Unexpected(u64),
}

impl ClientError {
    /// Whether the failure means the Host is gone rather than the request bad.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        matches!(
            self,
            Self::Closed | Self::Connect { .. } | Self::Discovery(_)
        )
    }
}

struct Pending {
    waiters: Mutex<HashMap<u64, Sender<Response>>>,
    closed: AtomicBool,
}

struct Inner {
    writer: Mutex<Conn>,
    pending: Arc<Pending>,
    next_request: AtomicU64,
    client_instance_id: ClientInstanceId,
    principal: PrincipalView,
    endpoint: Endpoint,
}

/// A connected client of one Runtime Host.
#[derive(Clone)]
pub struct RuntimeClient {
    inner: Arc<Inner>,
}

impl RuntimeClient {
    /// Discover the Host that serves `data_dir`, connect and register.
    ///
    /// Returns the client and the frame channel. `frames` carries only the
    /// asynchronous side (committed events and subscription closures).
    pub fn connect(
        data_dir: &Path,
        info: ClientInfo,
    ) -> Result<(Self, Receiver<ServerFrame>), ClientError> {
        let endpoint = Endpoint::discover(data_dir)?;
        let (conn, reader) = connect_stream(&endpoint)?;
        let pending = Arc::new(Pending {
            waiters: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let (frame_tx, frame_rx) = channel::<ServerFrame>();

        let hello = Request::Hello(crate::runtime::contract::Hello {
            protocol_version: crate::runtime::contract::PROTOCOL_VERSION,
            client: info.clone(),
        });
        let (mut write_half, mut read_half) = (conn, reader);
        write_request(&mut write_half, 1, &hello)?;
        // One reader for the whole connection: a read can carry the welcome and
        // the head of the next frame, so the handshake and the thread below must
        // share the buffer that keeps the tail.
        let mut frames = FrameReader::default();
        let welcome = read_welcome(&mut frames, &mut read_half)?;
        if welcome.instance_id != endpoint.instance_id {
            return Err(ClientError::Contract(format!(
                "endpoint names instance {}, Host answered as {}",
                endpoint.instance_id, welcome.instance_id
            )));
        }
        if welcome.workspace_id != endpoint.workspace_id {
            return Err(ClientError::Contract(format!(
                "endpoint names workspace {}, Host serves {}",
                endpoint.workspace_id, welcome.workspace_id
            )));
        }

        // The reader thread owns the read half from here on.
        let pending_for_reader = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("bsh-runtime-client".to_string())
            .spawn(move || {
                let max_frame = crate::runtime::contract::DEFAULT_MAX_FRAME_BYTES;
                loop {
                    match frames.next(&mut read_half, max_frame) {
                        Ok(Some(bytes)) => match serde_json::from_slice::<ServerFrame>(&bytes) {
                            Ok(ServerFrame::Response(ResponseFrame {
                                request_id,
                                response,
                            })) => {
                                log::debug!("runtime client response {request_id}");
                                let waiter = pending_for_reader
                                    .waiters
                                    .lock()
                                    .expect("pending lock")
                                    .remove(&request_id);
                                if let Some(waiter) = waiter {
                                    let _ = waiter.send(response);
                                }
                            }
                            Ok(frame) => {
                                if frame_tx.send(frame).is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                log::warn!("runtime client dropped a malformed frame: {error}");
                            }
                        },
                        Ok(None) => {
                            log::debug!("runtime client saw EOF");
                            break;
                        }
                        Err(error) => {
                            log::warn!("runtime client read failed: {error}");
                            break;
                        }
                    }
                }
                pending_for_reader.closed.store(true, Ordering::SeqCst);
            })
            .map_err(ClientError::Io)?;

        let inner = Arc::new(Inner {
            writer: Mutex::new(write_half),
            pending,
            next_request: AtomicU64::new(2),
            client_instance_id: welcome.client_instance_id,
            principal: welcome.principal,
            endpoint,
        });
        Ok((Self { inner }, frame_rx))
    }

    /// The Host process this client is connected to.
    #[must_use]
    pub fn instance_id(&self) -> HostInstanceId {
        self.inner.endpoint.instance_id
    }

    /// The workspace the Host serves.
    #[must_use]
    pub fn workspace_id(&self) -> WorkspaceId {
        self.inner.endpoint.workspace_id
    }

    /// The identity the Host derived for this connection.
    #[must_use]
    pub fn principal(&self) -> &PrincipalView {
        &self.inner.principal
    }

    /// The discovery record this client validated.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    fn target(&self, session_id: Option<SessionId>) -> Target {
        Target {
            workspace_id: self.inner.endpoint.workspace_id,
            session_id,
        }
    }

    /// Send one request and wait for its response.
    pub fn request(&self, request: Request) -> Result<Response, ClientError> {
        let request_id = self.inner.next_request.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        self.inner
            .pending
            .waiters
            .lock()
            .expect("pending lock")
            .insert(request_id, tx);
        if self.inner.pending.closed.load(Ordering::SeqCst) {
            return Err(ClientError::Closed);
        }
        {
            let mut writer = self.inner.writer.lock().expect("client writer");
            write_request(&mut writer, request_id, &request)?;
        }
        match rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(response) => classify(response),
            Err(RecvTimeoutError::Timeout) => {
                self.inner
                    .pending
                    .waiters
                    .lock()
                    .expect("pending lock")
                    .remove(&request_id);
                Err(ClientError::Closed)
            }
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Closed),
        }
    }

    /// Create a session and return its id.
    pub fn create_session(&self, title: impl Into<String>) -> Result<SessionId, ClientError> {
        let command_id = CommandId::new();
        let outcome = self.command(
            None,
            None,
            CommandPayload::CreateSession {
                title: title.into(),
            },
            command_id,
        )?;
        accepted_operation(&outcome).and_then(|operation| {
            operation
                .parse()
                .map_err(|_| ClientError::Contract("session id is not a uuid".to_string()))
        })
    }

    /// Rename one session.
    ///
    /// The title is a durable fact the Host journals, so a second client sees
    /// the new name without being told: the answer is the same accepted
    /// outcome every other command returns.
    pub fn rename_session(
        &self,
        session_id: SessionId,
        title: impl Into<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::RenameSession {
                session_id,
                title: title.into(),
            },
            CommandId::new(),
        )
    }

    /// Park one session in the workspace's bin.
    ///
    /// Parking is a durable visibility fact, so the answer is only an accepted
    /// outcome: the chat leaves every list and the snapshot a reconnecting
    /// client reconciles against, and every client reads the same
    /// `SessionTrashChanged` fact.
    pub fn trash_session(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::TrashSession { session_id },
            CommandId::new(),
        )
    }

    /// Take one parked session back out of the bin.
    ///
    /// The fold kept every row while the chat was parked, so a restore returns
    /// it with the transcript, calls, plan and goal it had.
    pub fn restore_session(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::RestoreSession { session_id },
            CommandId::new(),
        )
    }

    /// Drop one parked session for good.
    ///
    /// The bin's only irreversible decision, so the Host refuses it for a
    /// session that is not parked and a client only offers it from `/trash`.
    pub fn purge_session(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::PurgeSession { session_id },
            CommandId::new(),
        )
    }

    /// Pin one session ahead of the unpinned rows, or unpin it.
    ///
    /// The flag only decides where the row sorts, so the answer is an accepted
    /// outcome: `ListSessions` answers with the new order, and every client
    /// reads the same `SessionPinChanged` fact.
    pub fn pin_session(
        &self,
        session_id: SessionId,
        pinned: bool,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::PinSession { session_id, pinned },
            CommandId::new(),
        )
    }

    /// Archive one session out of the list, or return it to it.
    ///
    /// Archiving hides the chat from `ListSessions` without dropping a row,
    /// which is the one thing that separates it from parking in the bin: the
    /// transcript, calls, plan and goal all stay folded for the unarchive.
    pub fn archive_session(
        &self,
        session_id: SessionId,
        archived: bool,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::ArchiveSession {
                session_id,
                archived,
            },
            CommandId::new(),
        )
    }

    /// Send a user turn to a session.
    pub fn send_message(
        &self,
        session_id: SessionId,
        text: impl Into<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::SendMessage {
                session_id,
                text: text.into(),
            },
            CommandId::new(),
        )
    }

    /// Write settings fields the Settings query marked editable.
    ///
    /// The edit is a patch keyed by the rows the Host reported, and the answer
    /// carries the resolution the next turn will use - the caller does not have
    /// to guess what the Host stored. A refused value comes back as a rejection
    /// and neither document is touched.
    pub fn update_settings(
        &self,
        edits: Vec<crate::runtime::contract::SettingEdit>,
    ) -> Result<crate::runtime::contract::SettingsReport, ClientError> {
        let outcome = self.command(
            None,
            None,
            CommandPayload::UpdateSettings { edits },
            CommandId::new(),
        )?;
        match outcome {
            crate::runtime::contract::CommandOutcome::Accepted { .. } => self.settings(None),
            other => Err(ClientError::Contract(format!(
                "settings write answered with {other:?}"
            ))),
        }
    }

    /// Replace fields of one session's configuration.
    ///
    /// The Host validates the patch against the executor that will run the
    /// turn, so a refused value comes back as a rejection and nothing is
    /// journalled.
    pub fn configure_session(
        &self,
        session_id: SessionId,
        change: crate::runtime::contract::SessionConfigChange,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::ConfigureSession { session_id, change },
            CommandId::new(),
        )
    }

    /// Fold one session's older history into a checkpoint.
    ///
    /// The summary is produced by the Host's executor, so the outcome arrives
    /// as an event rather than in this answer: the command only asks for it.
    pub fn compact_session(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::CompactSession { session_id },
            CommandId::new(),
        )
    }

    /// Replace one session's goal with a single statement.
    ///
    /// A goal is durable Host state, so the answer is only an accepted outcome:
    /// the whole goal comes back as a `GoalChanged` event, and a client renders
    /// what the Host journalled rather than what it sent.
    pub fn set_goal(
        &self,
        session_id: SessionId,
        objective: impl Into<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::SetGoal {
                session_id,
                objective: objective.into(),
            },
            CommandId::new(),
        )
    }

    /// Drop one session's goal; a session with no goal has nothing to clear.
    pub fn clear_goal(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::ClearGoal { session_id },
            CommandId::new(),
        )
    }

    /// Rewrite one session's goal without restarting it.
    ///
    /// The state and round count stay, so a corrected objective is not charged
    /// for the rounds already worked.
    pub fn edit_goal(
        &self,
        session_id: SessionId,
        objective: impl Into<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::EditGoal {
                session_id,
                objective: objective.into(),
            },
            CommandId::new(),
        )
    }

    /// Record the reader's note about one session; blank text clears it.
    ///
    /// The note is durable Host state, so the answer is only an accepted
    /// outcome: the stored note comes back as a `FeedbackChanged` event.
    pub fn set_feedback(
        &self,
        session_id: SessionId,
        text: impl Into<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::SetFeedback {
                session_id,
                text: text.into(),
            },
            CommandId::new(),
        )
    }

    /// Stop one session's goal; its objective and rounds are kept.
    pub fn pause_goal(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::PauseGoal { session_id },
            CommandId::new(),
        )
    }

    /// Arm a paused or blocked goal again, when its round cap still has room.
    pub fn resume_goal(&self, session_id: SessionId) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::ResumeGoal { session_id },
            CommandId::new(),
        )
    }

    /// Cancel one execution.
    pub fn cancel_execution(
        &self,
        session_id: SessionId,
        execution_id: crate::runtime::ids::ExecutionId,
    ) -> Result<CommandOutcome, ClientError> {
        self.command(
            Some(session_id),
            None,
            CommandPayload::CancelExecution { execution_id },
            CommandId::new(),
        )
    }

    /// Approve or reject a pending approval, echoing the digest the operator saw.
    pub fn decide_approval(
        &self,
        session_id: SessionId,
        approval: &crate::runtime::contract::ApprovalView,
        approve: bool,
        note: Option<String>,
    ) -> Result<CommandOutcome, ClientError> {
        self.control(
            Some(session_id),
            ControlPayload::DecideApproval {
                approval_id: approval.approval_id,
                decision: if approve {
                    crate::runtime::contract::ApprovalVerdict::Approve
                } else {
                    crate::runtime::contract::ApprovalVerdict::Reject
                },
                action_digest: approval.action_digest.clone(),
                note,
            },
        )
    }

    /// Ask the Host to drain and stop.
    pub fn shutdown(
        &self,
        mode: crate::runtime::contract::ShutdownMode,
    ) -> Result<CommandOutcome, ClientError> {
        self.control(None, ControlPayload::Shutdown { mode })
    }

    /// Send one command.
    pub fn command(
        &self,
        session_id: Option<SessionId>,
        expected_version: Option<u64>,
        payload: CommandPayload,
        command_id: CommandId,
    ) -> Result<CommandOutcome, ClientError> {
        let target = self.target(session_id);
        let envelope = CommandEnvelope {
            command_id,
            client_instance_id: self.inner.client_instance_id,
            target,
            expected_version,
            payload,
        };
        match self.request(Request::Command(envelope))? {
            Response::Command(outcome) => Ok(outcome),
            other => Err(ClientError::Contract(format!(
                "command answered with {other:?}"
            ))),
        }
    }

    /// Send one privileged control.
    pub fn control(
        &self,
        session_id: Option<SessionId>,
        payload: ControlPayload,
    ) -> Result<CommandOutcome, ClientError> {
        let target = self.target(session_id);
        let envelope = ControlEnvelope {
            command_id: CommandId::new(),
            client_instance_id: self.inner.client_instance_id,
            target,
            expected_version: None,
            payload,
        };
        match self.request(Request::Control(envelope))? {
            Response::Command(outcome) => Ok(outcome),
            other => Err(ClientError::Contract(format!(
                "control answered with {other:?}"
            ))),
        }
    }

    /// Read one projection.
    pub fn query(
        &self,
        session_id: Option<SessionId>,
        payload: QueryPayload,
    ) -> Result<QueryResult, ClientError> {
        let envelope = QueryEnvelope {
            target: self.target(session_id),
            payload,
        };
        match self.request(Request::Query(envelope))? {
            Response::Query(result) => Ok(result),
            Response::QueryRejected(rejection) => Err(ClientError::Rejected(rejection)),
            other => Err(ClientError::Contract(format!(
                "query answered with {other:?}"
            ))),
        }
    }

    /// Every session summary, newest activity first.
    pub fn sessions(&self) -> Result<Vec<crate::runtime::contract::SessionSummary>, ClientError> {
        match self.query(None, QueryPayload::ListSessions)? {
            QueryResult::Sessions { sessions } => Ok(sessions),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The workspace's bin: the parked sessions, newest activity first.
    pub fn trash(&self) -> Result<Vec<crate::runtime::contract::SessionSummary>, ClientError> {
        match self.query(None, QueryPayload::Trash)? {
            QueryResult::Sessions { sessions } => Ok(sessions),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The workspace's archived sessions: the sidebar's Archived filter,
    /// newest activity first.
    pub fn archived(&self) -> Result<Vec<crate::runtime::contract::SessionSummary>, ClientError> {
        match self.query(None, QueryPayload::Archived)? {
            QueryResult::Sessions { sessions } => Ok(sessions),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// One session's full projection, `None` when it does not exist.
    pub fn session(
        &self,
        session_id: SessionId,
    ) -> Result<Option<crate::runtime::contract::SessionProjection>, ClientError> {
        match self.query(Some(session_id), QueryPayload::Session { session_id })? {
            QueryResult::Session { session } => Ok(session.map(|session| *session)),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// Models and presets a session on this Host may be configured with.
    pub fn catalogue(&self) -> Result<crate::runtime::contract::CatalogueView, ClientError> {
        match self.query(None, QueryPayload::Catalogue)? {
            QueryResult::Catalogue { catalogue } => Ok(catalogue),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The workspace's durable memories, listed or recalled.
    ///
    /// `None` lists the records newest first, the way the desktop Memory pane
    /// opens; text runs the kernel's hybrid recall, which is the same ranking a
    /// turn's context assembly asks for.
    pub fn memories(
        &self,
        query: Option<&str>,
    ) -> Result<crate::runtime::contract::MemoryReport, ClientError> {
        let payload = QueryPayload::Memories {
            query: query.map(str::to_string),
        };
        match self.query(None, payload)? {
            QueryResult::Memories { report } => Ok(report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// One directory of the workspace the Host serves, or one file in it.
    ///
    /// The path is relative to the Host's root and resolved there, so a client
    /// never joins a path itself and the Host classifies a file before it reads
    /// it: the terminal refuses exactly what the desktop editor refuses.
    pub fn files(
        &self,
        path: &str,
    ) -> Result<crate::runtime::contract::WorkspaceReport, ClientError> {
        let payload = QueryPayload::Files {
            path: path.to_string(),
        };
        match self.query(None, payload)? {
            QueryResult::Files { report } => Ok(report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The workspace's changed files, or one path's comparison.
    ///
    /// This is the desktop Diff tab's two answers in one query: no path asks
    /// what the working copy changed, and a path asks for that file's unified
    /// comparison. Both are read by the Host's own VCS call, against the same
    /// baseline its Files tree badges, so a terminal never computes a diff of
    /// its own.
    pub fn diff(
        &self,
        path: &str,
    ) -> Result<crate::runtime::contract::WorkspaceDiffView, ClientError> {
        let payload = QueryPayload::Diff {
            path: path.to_string(),
        };
        match self.query(None, payload)? {
            QueryResult::Diff { view } => Ok(view),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The workspace's durable task board: the desktop Tasks card's data.
    ///
    /// The board is workspace-wide, not session-wide, exactly as it is on the
    /// desktop: a scheduled task outlives the session that created it, so
    /// scoping the rows to the open session would hide the work still to come.
    pub fn tasks(&self) -> Result<crate::runtime::contract::TaskBoardReport, ClientError> {
        match self.query(None, QueryPayload::Tasks)? {
            QueryResult::Tasks { report } => Ok(report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// One task's full record: the desktop Tasks card's expanded row.
    ///
    /// The board row is the summary; this asks for the record behind it - the
    /// objective, the dependencies, the caps and the attempt history - so a
    /// terminal opens the task the desktop opens instead of re-reading a
    /// one-line row. The Host answers `unknown` for an id no record carries,
    /// which is a fact the pane shows rather than an error this client raises.
    pub fn task_detail(
        &self,
        id: &str,
    ) -> Result<crate::runtime::contract::TaskDetailReport, ClientError> {
        match self.query(None, QueryPayload::TaskDetail { id: id.to_string() })? {
            QueryResult::TaskDetail { report } => Ok(*report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The settings the Host's turns resolve from, as a settings pane reads
    /// them.
    ///
    /// `section` names one rail cell; `None` lists every section. The report is
    /// the Host's own resolution of its settings documents, so a terminal and
    /// the desktop settings dialog read the same value for the same row instead
    /// of two reads that could drift.
    pub fn settings(
        &self,
        section: Option<&str>,
    ) -> Result<crate::runtime::contract::SettingsReport, ClientError> {
        let payload = QueryPayload::Settings {
            section: section.map(str::to_string),
        };
        match self.query(None, payload)? {
            QueryResult::Settings { report } => Ok(*report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The extensions this build installed, and whether each is in force.
    ///
    /// The roster is the extension host's own status rows, so a terminal and
    /// the desktop settings list cannot disagree about which package is active
    /// or which tool each one added.
    pub fn extensions(&self) -> Result<crate::runtime::contract::ExtensionsReport, ClientError> {
        match self.query(None, QueryPayload::Extensions)? {
            QueryResult::Extensions { report } => Ok(report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The federation node this build runs, its peers and their grants.
    ///
    /// The report is the desktop Federation tab's data: the same identity file
    /// and the same peer store, read through the executor that serves this Host.
    pub fn federation(&self) -> Result<FederationReport, ClientError> {
        match self.query(None, QueryPayload::Federation)? {
            QueryResult::Federation { report } => Ok(report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// One session's latest sealed review: the desktop Review tab's data.
    ///
    /// `change` names the change whose prepared diff the report should carry -
    /// the id the list printed, or the path a reader typed. Passing `None`
    /// lists the changeset without building any diff.
    pub fn review(
        &self,
        session_id: SessionId,
        change: Option<&str>,
    ) -> Result<crate::runtime::contract::ReviewReport, ClientError> {
        let payload = QueryPayload::Review {
            session_id,
            change: change.map(str::to_string),
        };
        match self.query(Some(session_id), payload)? {
            QueryResult::Review { report } => Ok(*report),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// The browser's page: what the agent's browser tools last read.
    ///
    /// The browser belongs to the Host process, not to a session, so the query
    /// names no session and a client shows the same page whichever chat is
    /// open - the way one desktop window's Browser tab serves every chat.
    pub fn browser(&self) -> Result<crate::runtime::contract::BrowserView, ClientError> {
        match self.query(None, QueryPayload::Browser)? {
            QueryResult::Browser { view } => Ok(*view),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// Host lifecycle and activity.
    pub fn status(&self) -> Result<HostStatus, ClientError> {
        match self.query(None, QueryPayload::HostStatus)? {
            QueryResult::Host { status } => Ok(status),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// Pending approvals for one session.
    pub fn pending_approvals(
        &self,
        session_id: Option<SessionId>,
    ) -> Result<Vec<crate::runtime::contract::ApprovalView>, ClientError> {
        match self.query(session_id, QueryPayload::PendingApprovals { session_id })? {
            QueryResult::Approvals { approvals } => Ok(approvals),
            other => Err(ClientError::Contract(format!(
                "unexpected query result {other:?}"
            ))),
        }
    }

    /// Subscribe to committed events from `cursor` (or from the head).
    ///
    /// A `CursorExpired` answer carries the reconciliation snapshot; the caller
    /// replaces its state with it and resubscribes from `head`.
    pub fn subscribe(&self, cursor: Option<Cursor>) -> Result<Response, ClientError> {
        self.request(Request::Subscribe(Subscribe { cursor }))
    }

    /// Drop one subscription.
    pub fn unsubscribe(&self, subscription_id: u64) -> Result<Response, ClientError> {
        self.request(Request::Unsubscribe { subscription_id })
    }

    /// Liveness check.
    pub fn ping(&self) -> Result<Response, ClientError> {
        self.request(Request::Ping)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.pending.closed.store(true, Ordering::SeqCst);
    }
}

/// A client registration for the terminal interface.
#[must_use]
pub fn tui_info() -> ClientInfo {
    ClientInfo {
        name: "bsh-tui".to_string(),
        kind: ClientKind::Tui,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn classify(response: Response) -> Result<Response, ClientError> {
    match response {
        Response::Error(error) => Err(ClientError::Protocol(error)),
        Response::QueryRejected(rejection) => Err(ClientError::Rejected(rejection)),
        other => Ok(other),
    }
}

fn accepted_operation(outcome: &CommandOutcome) -> Result<String, ClientError> {
    match outcome {
        CommandOutcome::Accepted { operation_id, .. } => Ok(operation_id.to_string()),
        CommandOutcome::Rejected { rejection, .. } => Err(ClientError::Rejected(rejection.clone())),
    }
}

fn write_request(writer: &mut Conn, request_id: u64, request: &Request) -> Result<(), ClientError> {
    let frame = ClientFrame {
        request_id,
        request: request.clone(),
    };
    let bytes = serde_json::to_vec(&frame)
        .map_err(|error| ClientError::Contract(format!("request cannot be encoded: {error}")))?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_welcome(frames: &mut FrameReader, reader: &mut Conn) -> Result<Welcome, ClientError> {
    let bytes = frames
        .next(reader, crate::runtime::contract::DEFAULT_MAX_FRAME_BYTES)?
        .ok_or(ClientError::Closed)?;
    match serde_json::from_slice::<ServerFrame>(&bytes) {
        Ok(ServerFrame::Response(ResponseFrame {
            response: Response::Welcome(welcome),
            ..
        })) => Ok(welcome),
        Ok(ServerFrame::Response(ResponseFrame { response, .. })) => match classify(response) {
            Err(error) => Err(error),
            Ok(other) => Err(ClientError::Contract(format!(
                "registration answered with {other:?}"
            ))),
        },
        Ok(other) => Err(ClientError::Contract(format!(
            "registration answered with {other:?}"
        ))),
        Err(error) => Err(ClientError::Contract(format!(
            "malformed registration answer: {error}"
        ))),
    }
}

#[cfg(unix)]
type Conn = std::os::unix::net::UnixStream;

#[cfg(windows)]
type Conn = std::fs::File;

#[cfg(unix)]
fn connect_stream(endpoint: &Endpoint) -> Result<(Conn, Conn), ClientError> {
    let stream =
        std::os::unix::net::UnixStream::connect(&endpoint.socket_path).map_err(|source| {
            ClientError::Connect {
                path: endpoint.socket_path.display().to_string(),
                source,
            }
        })?;
    let reader = stream.try_clone()?;
    Ok((stream, reader))
}

#[cfg(windows)]
fn connect_stream(endpoint: &Endpoint) -> Result<(Conn, Conn), ClientError> {
    let stream = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&endpoint.socket_path)
        .map_err(|source| ClientError::Connect {
            path: endpoint.socket_path.display().to_string(),
            source,
        })?;
    let reader = stream.try_clone()?;
    Ok((stream, reader))
}

/// Newline-delimited frame reader over a blocking socket.
#[derive(Default)]
struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    fn next(&mut self, reader: &mut Conn, limit: usize) -> Result<Option<Vec<u8>>, std::io::Error> {
        // self.buf owns every byte read past the previous newline: one socket
        // read can carry the tail of this frame and the head of the next, and a
        // buffer rebuilt per call would throw that tail away.
        let mut chunk = [0u8; 8 * 1024];
        loop {
            if let Some(index) = self.buf.iter().position(|byte| *byte == b'\n') {
                let mut frame: Vec<u8> = self.buf.drain(..=index).collect();
                frame.pop();
                return Ok(Some(frame));
            }
            if self.buf.len() > limit {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("frame exceeds the {limit}-byte limit"),
                ));
            }
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                return if self.buf.is_empty() {
                    Ok(None)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "frame ended without a newline",
                    ))
                };
            }
            self.buf.extend_from_slice(&chunk[..read]);
        }
    }
}
