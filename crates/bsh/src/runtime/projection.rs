//! Deterministic read-model projection over the durable journal.
//!
//! The same fold runs at startup (recovery) and is the only place projections
//! are derived, so a client's query and a reconnecting client's snapshot never
//! diverge from committed facts.

use std::collections::HashMap;

use chrono::Utc;

use super::contract::{
    ApprovalStatus, ApprovalView, CompactionOutcomeView, EventEnvelope, EventPayload,
    ExecutionStatus, ExecutionView, MessageRole, MessageView, OperationView, PlanItemView,
    SessionConfig, SessionProjection, SessionSummary, ToolCallStatus, ToolCallView,
};
use super::ids::{MessageId, OperationId, SessionId};

/// The source rows a fork opens with.
///
/// They are copied out of the source projection before the fork's own entry
/// takes its mutable borrow, so the fold never holds two borrows of the
/// session map at once.
struct Inherited {
    messages: Vec<MessageView>,
    tool_calls: Vec<ToolCallView>,
    plan: Vec<PlanItemView>,
    compaction: Option<CompactionOutcomeView>,
    feedback: Option<String>,
}

/// Copy the history a fork opens with, ready to install on the fork.
///
/// A fork starts where its source stopped, so the rows it opens with are the
/// source's: the same messages, tool calls, plan, compaction outcome and
/// feedback note, at the same instants. They are copied here - not journalled
/// twice - because the source's events already precede the branch in the same
/// journal, which is what keeps a fork one durable fact.
///
/// The copy keeps the message ids it had. The fold has to stay a pure function
/// of the journal, so minting ids here would hand a reconnecting client rows it
/// never saw; a row that is the same row in both chats is the honest answer.
///
/// What a fork does NOT inherit is the source's running state: its executions,
/// approvals and goal belong to the chat that is doing that work. A fork that
/// carried an unfinished goal would run every round of it twice, and one that
/// carried a pending approval would offer a decision about work the fork never
/// asked for.
fn inherited(source: &SessionProjection) -> Inherited {
    Inherited {
        messages: source.messages.clone(),
        tool_calls: source.tool_calls.clone(),
        plan: source.plan.clone(),
        compaction: source.compaction.clone(),
        feedback: source.feedback.clone(),
    }
}

/// All projected sessions for one workspace.
#[derive(Debug, Default)]
pub(crate) struct Projections {
    sessions: HashMap<SessionId, SessionProjection>,
}

impl Projections {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold the full durable history.
    pub fn from_events(events: &[EventEnvelope]) -> Self {
        let mut projections = Self::new();
        for event in events {
            projections.apply(event);
        }
        projections
    }

    pub fn apply(&mut self, event: &EventEnvelope) {
        let Some(session_id) = event.session_id else {
            return;
        };
        match &event.payload {
            EventPayload::SessionCreated {
                title,
                config,
                forked_from,
                ..
            } => {
                // Whether this event is what creates the projection decides
                // whether the fork inherits: a creation replayed twice must not
                // copy the source's rows twice.
                let fresh = !self.sessions.contains_key(&session_id);
                let inherited = if fresh {
                    forked_from
                        .and_then(|id| self.sessions.get(&id))
                        .map(inherited)
                } else {
                    None
                };
                let projection =
                    self.sessions
                        .entry(session_id)
                        .or_insert_with(|| SessionProjection {
                            session_id,
                            version: event.sequence,
                            title: title.clone(),
                            created_at: event.occurred_at,
                            updated_at: event.occurred_at,
                            messages: Vec::new(),
                            tool_calls: Vec::new(),
                            plan: Vec::new(),
                            executions: Vec::new(),
                            approvals: Vec::new(),
                            config: SessionConfig::default(),
                            compaction: None,
                            goal: None,
                            feedback: None,
                            trashed: false,
                            pinned: false,
                            archived: false,
                        });
                projection.title = title.clone();
                projection.config = config.clone();
                projection.version = event.sequence;
                if let Some(rows) = inherited {
                    projection.messages = rows.messages;
                    projection.tool_calls = rows.tool_calls;
                    projection.plan = rows.plan;
                    projection.compaction = rows.compaction;
                    projection.feedback = rows.feedback;
                }
                projection.updated_at = event.occurred_at;
            }
            EventPayload::SessionConfigured { config, .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.config = config.clone();
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::ToolCallStarted {
                step,
                tool,
                summary,
                args,
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                projection.tool_calls.push(ToolCallView {
                    execution_id: event.execution_id,
                    step: *step,
                    tool: tool.clone(),
                    summary: summary.clone(),
                    args: args.clone(),
                    status: ToolCallStatus::Running,
                    output: None,
                    duration_ms: None,
                    started_at: event.occurred_at,
                    finished_at: None,
                });
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::ToolCallFinished {
                step,
                tool,
                ok,
                output,
                duration_ms,
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                // A session may run more than one turn at a time, and each
                // turn numbers its calls from one: the execution, not the
                // step alone, says which call this answer belongs to.
                // Resolve the index first: two `iter_mut` probes joined by
                // `or_else` would hold two mutable borrows at once.
                let by_identity = projection
                    .tool_calls
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, call)| {
                        call.step == *step
                            && call.execution_id == event.execution_id
                            && call.status == ToolCallStatus::Running
                    })
                    .map(|(index, _)| index);
                let by_step = projection
                    .tool_calls
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, call)| call.step == *step && call.tool == *tool)
                    .map(|(index, _)| index);
                if let Some(call) = by_identity
                    .or(by_step)
                    .map(|index| &mut projection.tool_calls[index])
                {
                    call.status = if *ok {
                        ToolCallStatus::Completed
                    } else {
                        ToolCallStatus::Failed
                    };
                    call.output = Some(output.clone());
                    call.duration_ms = Some(*duration_ms);
                    call.finished_at = Some(event.occurred_at);
                }
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::PlanUpdated { plan } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    // The model replaces the whole list in one call, so the
                    // fold replaces it too: an appended step would outlive a
                    // plan the model already rewrote.
                    projection.plan = plan.clone();
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::SessionRenamed { title, .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.title = title.clone();
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::TurnRequested {
                message_id, text, ..
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                projection.messages.push(MessageView {
                    message_id: *message_id,
                    role: MessageRole::User,
                    text: text.clone(),
                    at: event.occurred_at,
                    execution_id: event.execution_id,
                    compaction: false,
                    usage: None,
                });
                if let Some(execution_id) = event.execution_id {
                    projection.executions.push(ExecutionView {
                        execution_id,
                        status: ExecutionStatus::Running,
                        started_at: event.occurred_at,
                        finished_at: None,
                        detail: None,
                    });
                }
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::TurnSucceeded {
                message_id,
                text,
                usage,
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                projection.messages.push(MessageView {
                    message_id: *message_id,
                    role: MessageRole::Assistant,
                    text: text.clone(),
                    at: event.occurred_at,
                    execution_id: event.execution_id,
                    compaction: false,
                    usage: *usage,
                });
                if let Some(execution) = event.execution_id.and_then(|id| {
                    projection
                        .executions
                        .iter_mut()
                        .find(|execution| execution.execution_id == id)
                }) {
                    execution.status = ExecutionStatus::Completed;
                    execution.finished_at = Some(event.occurred_at);
                }
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::TurnFailed { error } => {
                if let Some(projection) = self.sessions.get_mut(&session_id)
                    && let Some(execution) = event.execution_id.and_then(|id| {
                        projection
                            .executions
                            .iter_mut()
                            .find(|execution| execution.execution_id == id)
                    })
                {
                    execution.status = ExecutionStatus::Failed;
                    execution.finished_at = Some(event.occurred_at);
                    execution.detail = Some(error.clone());
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::ExecutionCancelled { .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    if let Some(execution) = event.execution_id.and_then(|id| {
                        projection
                            .executions
                            .iter_mut()
                            .find(|execution| execution.execution_id == id)
                    }) {
                        execution.status = ExecutionStatus::Cancelled;
                        execution.finished_at = Some(event.occurred_at);
                    }
                    for approval in &mut projection.approvals {
                        if approval.execution_id == event.execution_id.unwrap_or_default()
                            && approval.status == ApprovalStatus::Pending
                        {
                            approval.status = ApprovalStatus::Cancelled;
                        }
                    }
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::ExecutionInterrupted { reason } => {
                if let Some(projection) = self.sessions.get_mut(&session_id)
                    && let Some(execution) = event.execution_id.and_then(|id| {
                        projection
                            .executions
                            .iter_mut()
                            .find(|execution| execution.execution_id == id)
                    })
                {
                    execution.status = ExecutionStatus::Interrupted;
                    execution.finished_at = Some(event.occurred_at);
                    execution.detail = Some(reason.clone());
                    for approval in &mut projection.approvals {
                        if execution.execution_id == approval.execution_id
                            && approval.status == ApprovalStatus::Pending
                        {
                            approval.status = ApprovalStatus::Cancelled;
                        }
                    }
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::ApprovalRequested {
                approval_id,
                summary,
                action,
                action_digest,
                expires_at,
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                if let Some(execution_id) = event.execution_id {
                    projection.approvals.push(ApprovalView {
                        approval_id: *approval_id,
                        session_id,
                        execution_id,
                        summary: summary.clone(),
                        action: action.clone(),
                        action_digest: action_digest.clone(),
                        status: ApprovalStatus::Pending,
                        expires_at: *expires_at,
                    });
                    if let Some(execution) = projection
                        .executions
                        .iter_mut()
                        .find(|execution| execution.execution_id == execution_id)
                        && !matches!(
                            execution.status,
                            ExecutionStatus::Completed
                                | ExecutionStatus::Failed
                                | ExecutionStatus::Cancelled
                                | ExecutionStatus::Interrupted
                                | ExecutionStatus::NeedsIntervention
                        )
                    {
                        execution.status = ExecutionStatus::WaitingApproval;
                    }
                }
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::ApprovalDecided {
                approval_id,
                decision,
                ..
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                let status = match decision {
                    super::contract::ApprovalDecision::Approved => ApprovalStatus::Approved,
                    super::contract::ApprovalDecision::Rejected => ApprovalStatus::Rejected,
                    super::contract::ApprovalDecision::Expired => ApprovalStatus::Expired,
                    super::contract::ApprovalDecision::Cancelled => ApprovalStatus::Cancelled,
                };
                if let Some(approval) = projection
                    .approvals
                    .iter_mut()
                    .find(|approval| approval.approval_id == *approval_id)
                {
                    approval.status = status;
                    let execution_id = approval.execution_id;
                    if let Some(execution) = projection
                        .executions
                        .iter_mut()
                        .find(|execution| execution.execution_id == execution_id)
                        && execution.status == ExecutionStatus::WaitingApproval
                    {
                        execution.status = ExecutionStatus::Running;
                    }
                }
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::SessionCompacted {
                summary, folded, ..
            } => {
                let Some(projection) = self.sessions.get_mut(&session_id) else {
                    return;
                };
                // The checkpoint replaces the rows it folded: dropping them
                // here is what makes a resumed turn read the checkpoint
                // instead of the transcript it condensed. The checkpoint row
                // stays visible, marked as one, so a client can say that
                // history got shorter on purpose.
                let folded = usize::try_from(*folded).unwrap_or(usize::MAX);
                let removed = folded.min(projection.messages.len());
                projection.messages.drain(..removed);
                // Tool rows describe calls in the span the checkpoint covers;
                // keeping them would show work whose transcript is gone.
                let boundary = projection.messages.first().map(|message| message.at);
                projection
                    .tool_calls
                    .retain(|call| boundary.is_none_or(|at| call.started_at >= at));
                projection.messages.insert(
                    0,
                    MessageView {
                        message_id: MessageId::new(),
                        role: MessageRole::Assistant,
                        text: summary.clone(),
                        at: event.occurred_at,
                        execution_id: None,
                        compaction: true,
                        usage: None,
                    },
                );
                projection.compaction = Some(CompactionOutcomeView {
                    ok: true,
                    folded: folded as u64,
                    summary: Some(summary.clone()),
                    error: None,
                    at: event.occurred_at,
                });
                projection.version = event.sequence;
                projection.updated_at = event.occurred_at;
            }
            EventPayload::SessionCompactionFailed { error, .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.compaction = Some(CompactionOutcomeView {
                        ok: false,
                        folded: 0,
                        summary: None,
                        error: Some(error.clone()),
                        at: event.occurred_at,
                    });
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::GoalChanged { goal, .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    // A clear travels as `None`: the projection holds no goal
                    // rather than a goal with an empty objective, so a client
                    // can tell "no goal" from "a goal with no text".
                    projection.goal = goal.clone();
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::FeedbackChanged { feedback, .. } => {
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    // A clear travels as `None`, so "no note" and "a blank note"
                    // stay distinguishable the way the goal's clear does.
                    projection.feedback = feedback.clone();
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::SessionTrashChanged { trashed, .. } => {
                // Parking keeps every row: the bin is a visibility change, not
                // a deletion, so a restore returns the chat the reader parked.
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.trashed = *trashed;
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::SessionPinChanged { pinned, .. } => {
                // The flag only decides where the row sorts, so nothing else
                // in the projection changes with it.
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.pinned = *pinned;
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::SessionArchivedChanged { archived, .. } => {
                // Archiving hides the row from the list and nothing more: the
                // chat the reader unarchives is the chat they hid.
                if let Some(projection) = self.sessions.get_mut(&session_id) {
                    projection.archived = *archived;
                    projection.version = event.sequence;
                    projection.updated_at = event.occurred_at;
                }
            }
            EventPayload::SessionPurged { .. } => {
                // The one irreversible fact: nothing folds this session again,
                // so `Session` stops answering for it and a restore is refused.
                self.sessions.remove(&session_id);
            }
            EventPayload::HostReady { .. }
            | EventPayload::HostDraining { .. }
            | EventPayload::HostStopped { .. } => {}
        }
    }

    pub fn session(&self, id: SessionId) -> Option<&SessionProjection> {
        self.sessions.get(&id)
    }

    pub fn sessions(&self) -> Vec<SessionProjection> {
        let mut sessions: Vec<_> = self.sessions.values().cloned().collect();
        sessions.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.version.cmp(&left.version))
        });
        sessions
    }

    pub fn summaries(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<_> = self
            .sessions
            .values()
            .filter(|projection| !projection.trashed && !projection.archived)
            .map(SessionProjection::summary)
            .collect();
        // Pinned rows first, then newest activity: the desktop's sidebar
        // sorts by the same two keys, so a star moves a chat to the top in
        // every client at once.
        summaries.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        summaries
    }

    /// The Archived filter's rows: the archived sessions, newest first.
    ///
    /// A parked session is out of every list including this one - the bin is
    /// the one reader that asks for a parked chat.
    pub fn archived(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<_> = self
            .sessions
            .values()
            .filter(|projection| projection.archived && !projection.trashed)
            .map(SessionProjection::summary)
            .collect();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at));
        summaries
    }

    pub fn approvals(&self, session: Option<SessionId>) -> Vec<ApprovalView> {
        let mut approvals: Vec<_> = self
            .sessions
            .values()
            .filter(|projection| session.is_none_or(|id| projection.session_id == id))
            .flat_map(|projection| projection.approvals.iter().cloned())
            .collect();
        approvals.sort_by_key(|approval| approval.expires_at);
        approvals
    }

    pub fn active_execution_count(&self) -> usize {
        self.sessions
            .values()
            .flat_map(|session| session.executions.iter())
            .filter(|execution| {
                matches!(
                    execution.status,
                    ExecutionStatus::Queued
                        | ExecutionStatus::Running
                        | ExecutionStatus::WaitingApproval
                )
            })
            .count()
    }

    pub fn has_open_approvals_for(&self, execution: super::ids::ExecutionId) -> bool {
        self.sessions.values().any(|session| {
            session.approvals.iter().any(|approval| {
                approval.execution_id == execution && approval.status == ApprovalStatus::Pending
            })
        })
    }

    pub fn operation(&self, operation_id: &OperationId) -> Option<OperationView> {
        let raw = operation_id.as_str();
        for session in self.sessions.values() {
            if session.session_id.to_string() == raw {
                return Some(OperationView {
                    operation_id: operation_id.clone(),
                    kind: "session".to_string(),
                    status: "exists".to_string(),
                    session_id: Some(session.session_id),
                    execution_id: None,
                });
            }
            for execution in &session.executions {
                if execution.execution_id.to_string() == raw {
                    return Some(OperationView {
                        operation_id: operation_id.clone(),
                        kind: "execution".to_string(),
                        status: format!("{:?}", execution.status).to_lowercase(),
                        session_id: Some(session.session_id),
                        execution_id: Some(execution.execution_id),
                    });
                }
            }
            for approval in &session.approvals {
                if approval.approval_id.to_string() == raw {
                    return Some(OperationView {
                        operation_id: operation_id.clone(),
                        kind: "approval".to_string(),
                        status: format!("{:?}", approval.status).to_lowercase(),
                        session_id: Some(session.session_id),
                        execution_id: Some(approval.execution_id),
                    });
                }
            }
        }
        None
    }

    /// The bin's rows: the parked sessions, newest activity first.
    ///
    /// Nothing else in this fold shows a parked session: [`Self::summaries`]
    /// and [`Self::snapshot`] both skip it, which is what "out of every list"
    /// means. The bin is the one reader that asks for them.
    pub fn trashed(&self) -> Vec<SessionSummary> {
        let mut summaries: Vec<_> = self
            .sessions
            .values()
            .filter(|projection| projection.trashed)
            .map(SessionProjection::summary)
            .collect();
        summaries.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at));
        summaries
    }

    /// The snapshot excludes parked and archived sessions: a client that
    /// reconnects reconciles against the workspace it can see, and a chat in
    /// the bin or behind the Archived filter is not part of it.
    pub fn snapshot(&self) -> Vec<SessionProjection> {
        self.sessions()
            .into_iter()
            .filter(|projection| !projection.trashed && !projection.archived)
            .collect()
    }

    pub fn generated_at(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ids::{CommandId, EventId, PayloadHash, WorkspaceId};

    fn event(sequence: u64, session_id: SessionId, payload: EventPayload) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId::new(),
            workspace_id: WorkspaceId::new(),
            stream_id: "test".to_string(),
            sequence,
            session_id: Some(session_id),
            execution_id: None,
            task_id: None,
            causation_id: None,
            correlation_id: None,
            occurred_at: Utc::now() + chrono::Duration::milliseconds(sequence as i64),
            schema_version: 1,
            payload,
        }
    }

    fn created(session_id: SessionId) -> EventEnvelope {
        event(
            1,
            session_id,
            EventPayload::SessionCreated {
                title: "test".to_string(),
                config: SessionConfig::default(),
                forked_from: None,
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"created"),
            },
        )
    }

    /// A session with two user turns and two answers.
    fn with_transcript(session_id: SessionId) -> Projections {
        let mut projections = Projections::new();
        projections.apply(&created(session_id));
        for step in 0..2 {
            let base = 2 + step * 2;
            projections.apply(&event(
                base,
                session_id,
                EventPayload::TurnRequested {
                    message_id: MessageId::new(),
                    text: format!("question {step}"),
                    command_id: CommandId::new(),
                    payload_hash: PayloadHash::of(&format!("q{step}")),
                },
            ));
            projections.apply(&event(
                base + 1,
                session_id,
                EventPayload::TurnSucceeded {
                    message_id: MessageId::new(),
                    text: format!("answer {step}"),
                    usage: None,
                },
            ));
        }
        projections
    }

    /// A branch is one durable fact, not a copy of the transcript.
    ///
    /// The fork inherits the rows its source had when it branched - messages,
    /// calls, plan and the note the reader wrote - and keeps them as the same
    /// rows, ids included, so folding the journal twice agrees. It inherits
    /// none of the source's live work: a fork that carried a running execution,
    /// a pending approval or an unfinished goal would offer decisions and
    /// rounds about work it never did.
    #[test]
    fn a_fork_folds_its_sources_history_and_leaves_its_work_behind() {
        let source = SessionId::new();
        let fork = SessionId::new();
        let mut projections = with_transcript(source);
        projections.apply(&event(
            6,
            source,
            EventPayload::PlanUpdated {
                plan: vec![PlanItemView {
                    text: "ship the branch".to_string(),
                    status: crate::runtime::contract::PlanItemStatus::InProgress,
                }],
            },
        ));
        projections.apply(&event(
            7,
            source,
            EventPayload::GoalChanged {
                goal: Some(crate::runtime::contract::SessionGoalView {
                    objective: "finish the release".to_string(),
                    state: crate::runtime::contract::GoalState::Active,
                    rounds: 2,
                    max_rounds: 256,
                    blocked_reason: None,
                }),
                command_id: None,
                payload_hash: None,
            },
        ));
        // The source's last turn is still running when the reader branches it.
        projections.apply(&event(
            8,
            source,
            EventPayload::TurnRequested {
                message_id: MessageId::new(),
                text: "the question in flight".to_string(),
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"in flight"),
            },
        ));
        let source_rows = projections.session(source).expect("the source answers");
        let expected = source_rows.messages.clone();
        let expected_calls = source_rows.tool_calls.clone();
        assert_eq!(expected.len(), 5, "four settled rows and one in flight");

        projections.apply(&event(
            9,
            fork,
            EventPayload::SessionCreated {
                title: "test".to_string(),
                config: SessionConfig::default(),
                forked_from: Some(source),
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"fork"),
            },
        ));

        let branch = projections.session(fork).expect("the fork answers");
        assert_eq!(
            branch.messages, expected,
            "the branch opens with the source's rows"
        );
        assert_eq!(
            branch.tool_calls, expected_calls,
            "and the calls those rows made"
        );
        assert_eq!(branch.plan.len(), 1, "a branch keeps the plan it inherited");
        assert!(
            branch.executions.is_empty(),
            "a fork owns no execution its source started"
        );
        assert!(
            branch.approvals.is_empty(),
            "and no approval it could be asked to settle"
        );
        assert!(
            branch.goal.is_none(),
            "a fork runs no rounds of its source's goal"
        );
        assert_eq!(
            projections
                .session(source)
                .expect("the source is still there")
                .messages,
            expected,
            "branching reads the source and changes nothing about it"
        );

        // The branch then grows on its own: later rows of the source's stay
        // behind, because the fork is a snapshot at the moment it branched.
        projections.apply(&event(
            10,
            source,
            EventPayload::TurnSucceeded {
                message_id: MessageId::new(),
                text: "the answer the fork never saw".to_string(),
                usage: None,
            },
        ));
        let branch = projections.session(fork).expect("the fork answers");
        assert_eq!(
            branch.messages.len(),
            expected.len(),
            "later source rows stay behind"
        );
        assert_eq!(
            projections.summaries().len(),
            2,
            "the list holds both chats"
        );
    }

    /// Parking keeps every row; a purge is the one fact that drops them.
    ///
    /// The two must not be the same fold: a restore has to return the chat the
    /// reader parked, so the bin is a visibility change and only `SessionPurged`
    /// removes a session from the fold.
    #[test]
    fn parking_hides_the_rows_a_purge_drops() {
        let session_id = SessionId::new();
        let mut projections = with_transcript(session_id);
        assert_eq!(projections.summaries().len(), 1);
        assert_eq!(projections.snapshot().len(), 1);

        projections.apply(&event(
            7,
            session_id,
            EventPayload::SessionTrashChanged {
                trashed: true,
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"trash"),
            },
        ));
        assert!(
            projections.summaries().is_empty(),
            "a parked chat leaves every list"
        );
        assert!(
            projections.snapshot().is_empty(),
            "and the snapshot a reconnecting client reconciles against"
        );
        assert_eq!(projections.trashed().len(), 1, "the bin is the one reader");
        let parked = projections.session(session_id).expect("the fold kept it");
        assert!(parked.trashed);
        assert_eq!(parked.messages.len(), 4, "parking drops no rows");

        projections.apply(&event(
            8,
            session_id,
            EventPayload::SessionTrashChanged {
                trashed: false,
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"restore"),
            },
        ));
        assert_eq!(projections.summaries().len(), 1, "restore puts it back");
        assert!(projections.trashed().is_empty());

        projections.apply(&event(
            9,
            session_id,
            EventPayload::SessionPurged {
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"purge"),
            },
        ));
        assert!(
            projections.session(session_id).is_none(),
            "a purge drops the session from the fold"
        );
    }

    #[test]
    fn a_checkpoint_replaces_the_rows_it_folded() {
        let session_id = SessionId::new();
        let mut projections = with_transcript(session_id);
        assert_eq!(
            projections
                .session(session_id)
                .expect("session")
                .messages
                .len(),
            4
        );
        projections.apply(&event(
            6,
            session_id,
            EventPayload::SessionCompacted {
                summary: "checkpoint".to_string(),
                folded: 2,
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"compact"),
            },
        ));
        let projection = projections.session(session_id).expect("session");
        assert_eq!(
            projection.messages.len(),
            3,
            "two folded rows became one checkpoint"
        );
        assert_eq!(projection.messages[0].text, "checkpoint");
        assert!(projection.messages[0].compaction);
        assert_eq!(projection.messages[1].text, "question 1");
        assert_eq!(projection.messages[2].text, "answer 1");
        let outcome = projection.compaction.as_ref().expect("an outcome");
        assert!(outcome.ok);
        assert_eq!(outcome.folded, 2);
        assert_eq!(outcome.summary.as_deref(), Some("checkpoint"));
    }

    #[test]
    fn a_refused_compaction_keeps_the_transcript_and_reports_why() {
        let session_id = SessionId::new();
        let mut projections = with_transcript(session_id);
        projections.apply(&event(
            6,
            session_id,
            EventPayload::SessionCompactionFailed {
                error: "nothing to compact".to_string(),
                command_id: CommandId::new(),
                payload_hash: PayloadHash::of(&"compact"),
            },
        ));
        let projection = projections.session(session_id).expect("session");
        assert_eq!(projection.messages.len(), 4, "the transcript is untouched");
        let outcome = projection.compaction.as_ref().expect("an outcome");
        assert!(!outcome.ok);
        assert_eq!(outcome.error.as_deref(), Some("nothing to compact"));
        assert_eq!(outcome.folded, 0);
    }
}
