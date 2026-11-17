//! The model-facing task tools.
//!
//! task_create opens a durable record and starts its run through the host.runs
//! seam; task_join waits on the runs the records name and settles them. Both
//! are ordinary async tools, so they ride the same registry, policy and wire
//! path as every other built-in.

use std::path::PathBuf;
use std::sync::Arc;

use agent::tools::{AsyncFunctionTool, DeclaredAsyncTool};
use agent::{Agent, ToolError};
use serde_json::{json, Value};

use crate::extension::runs::{Limits, RunSpec, RunState, RunTier, Runs};
use crate::extension::tasks::{
    self, ApprovalDecision, ApprovalReference, Attempt, AttemptStatus, FileTasksStore, Host,
    Origin, Task, TaskStatus, TasksStore, Tier, Trigger, VerificationContract,
    VerificationEvidence, VerificationReport, VerificationStatus,
};

/// What the tools need to know about the turn they run in.
#[derive(Clone)]
pub(crate) struct TaskToolsContext {
    /// The origin session.
    pub(crate) session: String,
    /// The workspace root whose .bsh/tasks directory holds the records.
    pub(crate) workspace: PathBuf,
    /// The turn's subagent machinery, when the capability is enabled. A
    /// subagent-tier task spawns through it, so the child gets the same
    /// policy, slots and runner as one started by the subagent tool.
    pub(crate) subagents: Option<Arc<crate::caps::SubagentContext>>,
    /// The turn's shell tool, for a job-tier task's command. It carries the
    /// sandbox and policy a plain shell call gets.
    pub(crate) bash: Option<Arc<dyn agent::tools::Tool>>,
    /// The turn's job roster, for observing and stopping a job-tier task.
    pub(crate) jobs: Option<Arc<agent::jobs::JobRegistry>>,
}

/// Attach the tools to an agent.
pub(crate) fn attach(agent: &mut Agent, context: TaskToolsContext) {
    attach_create(agent, context.clone());
    attach_join(agent, context.clone());
    attach_cancel(agent, context.clone());
    attach_status(agent, context.clone());
    attach_list(agent, context.clone());
    attach_retry(agent, context.clone());
    attach_approve(agent, context.clone());
    attach_graph(agent, context);
}

/// The create tool.
fn attach_create(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_create",
        "Create a durable task. Without a trigger it runs now as a turn in this session, and you collect it with task_join. With trigger.every_seconds or trigger.at_ms it is scheduled: the Tasks board fires it when due, and it survives this turn. tier subagent delegates to a child agent; tier delegation sends the objective to a peer node for one exposed capability. Use it for work worth tracking, not for a single obvious step.",
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short 3-8 word label for the Tasks board."
                },
                "intent": {
                    "type": "string",
                    "description": "The complete prompt for the turn this task runs. It cannot see this conversation, so include the goal, the paths and the expected result."
                },
                "tier": {
                    "type": "string",
                    "enum": ["turn", "subagent", "job", "delegation", "peer-task"],
                    "description": "How the task runs. turn starts a turn in this session; subagent delegates to a child agent; job runs a shell command in the background; delegation sends the objective (or one capability) to a peer node; peer-task asks a peer to own and schedule the task on its own clock."
                },
                "target": {
                    "type": "string",
                    "description": "For a delegation: the peer node id, as federation_peers reports it."
                },
                "capability": {
                    "type": "string",
                    "description": "For a delegation: optional. Without it the peer runs an agent turn on the objective; with it the peer invokes exactly that capability."
                },
                "input": {
                    "type": "object",
                    "description": "For a delegation: the capability's own input object."
                },
                "command": {
                    "type": "string",
                    "description": "For a job: the shell command to run in the background."
                },
                "deps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task ids that must succeed first. Only a scheduled turn task may wait; it sits in Waiting until they do."
                },
                "trigger": {
                    "type": "object",
                    "description": "Omit to run now. every_seconds repeats the task; at_ms runs it once at an absolute unix time in milliseconds.",
                    "properties": {
                        "every_seconds": { "type": "integer", "minimum": 60 },
                        "at_ms": { "type": "integer", "minimum": 0 }
                    }
                },
                "verification_contract": {
                    "type": "object",
                    "description": "Optional independent verification. command runs a command (exit 0 passes); artifact checks a workspace-relative file and optional SHA-256; review or human needs an external approval reference. Not available on peer-task.",
                    "properties": {
                        "kind": { "type": "string", "enum": ["command", "artifact", "review", "human"] },
                        "command": { "type": "string" },
                        "path": { "type": "string" },
                        "digest": { "type": "string" }
                    },
                    "required": ["kind"]
                },
                "retry_policy": {
                    "type": "string",
                    "enum": ["idempotent", "non-idempotent", "unknown"],
                    "description": "How safely this task may be retried. Unknown (the default) fails closed."
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "Optional stable key the executor can use to deduplicate side effects."
                }
            },
            "required": ["intent"]
        }),
        move |args| {
            let context = context.clone();
            async move {
                let runs = crate::extension::runs::runs();
                create_with(&runs, &context, &args).await
            }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_create")
        .expect("task_create metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_create tool was not registered: {error}");
    }
}

/// The join tool.
fn attach_join(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_join",
        "Wait for tasks you created and return their results. Waits up to timeout_ms (default 30000, max 300000); a task still running past that is reported as not settled, so you can do other work and join again.",
        json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task ids returned by task_create."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 300000,
                    "description": "How long to wait, in milliseconds. Default 30000."
                }
            },
            "required": ["tasks"]
        }),
        move |args| {
            let context = context.clone();
            async move {
                let runs = crate::extension::runs::runs();
                join_with(&runs, &context, &args).await
            }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_join")
        .expect("task_join metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_join tool was not registered: {error}");
    }
}

/// Stop the task's run through its tier, then mark the record cancelled.
pub(crate) async fn cancel_with(
    runs: &Runs,
    context: &TaskToolsContext,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("task is required".to_string()))?;
    let Some(mut record) = FileTasksStore
        .load(&context.workspace, id)
        .map_err(|error| ToolError::Failed(format!("the task could not be read: {error}")))?
    else {
        return Err(ToolError::Failed(format!("unknown task {id}")));
    };
    let run = record
        .attempts
        .last()
        .and_then(|attempt| attempt.run.clone());
    if let Host::Peer { node } = record.host.clone() {
        crate::federation_tools::cancel_peer_task(&node, id).await?;
    } else if let Some(run) = &run {
        match record.tier {
            Tier::Turn => {
                runs.cancel(run).map_err(ToolError::Failed)?;
            }
            Tier::Subagent => {
                let Some(subagents) = context.subagents.as_ref() else {
                    return Err(ToolError::Failed(
                        "subagents are not available in this turn".to_string(),
                    ));
                };
                if !subagents.interrupt_run(run) {
                    return Err(ToolError::Failed(
                        "the child run could not be stopped".to_string(),
                    ));
                }
            }
            Tier::Delegation => {
                let Some(target) = record.target.clone() else {
                    return Err(ToolError::Failed(
                        "the task names no peer to cancel on".to_string(),
                    ));
                };
                crate::federation_tools::cancel_delegation(&target, run).await?;
            }
            Tier::Job => {
                let Some(jobs) = context.jobs.as_ref() else {
                    return Err(ToolError::Failed(
                        "the job roster is not available in this turn".to_string(),
                    ));
                };
                if !jobs.kill(run) {
                    return Err(ToolError::Failed(
                        "the job could not be stopped".to_string(),
                    ));
                }
            }
            Tier::PeerTask => {}
        }
    } else if !matches!(
        record.status,
        TaskStatus::Scheduled | TaskStatus::Queued | TaskStatus::Waiting
    ) {
        return Err(ToolError::Failed("the task has no run to stop".to_string()));
    }
    let now = crate::settings::unix_now().saturating_mul(1_000);
    record.status = TaskStatus::Cancelled;
    record.updated_ms = now;
    if let Some(attempt) = record.attempts.last_mut() {
        attempt.status = AttemptStatus::Cancelled;
        attempt.ended_ms = Some(now);
    }
    FileTasksStore
        .save(&context.workspace, &record)
        .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
    Ok(json!({ "task": id, "state": "cancelled" }))
}

/// The retry tool.
fn attach_retry(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_retry",
        "Run a task again as a new attempt, reusing its intent and tier. Refused while an attempt is live, once the task has used all its attempts, and for delegations (their input is not stored; create a new task instead).",
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task id returned by task_create."
                },
                "confirm_side_effects": {
                    "type": "boolean",
                    "description": "Confirm that a non-idempotent or unknown-side-effect attempt may be repeated."
                },
                "compensation": {
                    "type": "string",
                    "description": "The recovery/compensation path for an unknown-side-effect retry."
                }
            },
            "required": ["task"]
        }),
        move |args| {
            let context = context.clone();
            async move {
                let runs = crate::extension::runs::runs();
                retry_with(&runs, &context, &args).await
            }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_retry")
        .expect("task_retry metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_retry tool was not registered: {error}");
    }
}

/// Record or revoke a review/human approval (issue #433).
fn approve_with(context: &TaskToolsContext, args: &Value) -> Result<Value, ToolError> {
    let id = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("task is required".to_string()))?;
    let mut record = FileTasksStore
        .load(&context.workspace, id)
        .map_err(|error| ToolError::Failed(format!("the task could not be read: {error}")))?
        .ok_or_else(|| ToolError::Failed(format!("unknown task {id}")))?;
    let now = crate::settings::unix_now().saturating_mul(1_000);
    if args.get("revoke").and_then(Value::as_bool).unwrap_or(false) {
        let reference = args.get("reference").and_then(Value::as_str).map(str::trim);
        let Some(approval) = record.approvals.iter_mut().rev().find(|approval| {
            approval.revoked_ms.is_none()
                && reference
                    .map(|reference| approval.reference == reference)
                    .unwrap_or(true)
        }) else {
            return Err(ToolError::Failed(
                "no matching active approval to revoke".to_string(),
            ));
        };
        approval.revoked_ms = Some(now);
        // Revocation invalidates the current verification so the next join
        // re-resolves the approval instead of keeping a stale Passed result.
        let attempt_id = record.attempts.last().map(|attempt| attempt.id.clone());
        record.verification = VerificationReport {
            status: VerificationStatus::Inconclusive,
            attempt_id: attempt_id.clone(),
            verifier: Some("approval".to_string()),
            verifier_version: None,
            verified_ms: Some(now),
            evidence: Vec::new(),
            message: Some("the approval was revoked".to_string()),
        };
        if let Some(attempt) = record.attempts.last_mut() {
            attempt.verification = record.verification.clone();
        }
        FileTasksStore
            .save(&context.workspace, &record)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({"task": id, "approvals": record.approvals}));
    }
    let approver = args
        .get("approver")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|approver| !approver.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("approver is required".to_string()))?;
    if matches!(
        approver.to_ascii_lowercase().as_str(),
        "model" | "assistant" | "self"
    ) {
        return Err(ToolError::InvalidInput(
            "a model cannot approve its own task; name the human or external reviewer".to_string(),
        ));
    }
    let decision = match args
        .get("decision")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("approved")
    {
        "approved" | "approve" => ApprovalDecision::Approved,
        "rejected" | "reject" => ApprovalDecision::Rejected,
        other => {
            return Err(ToolError::InvalidInput(format!(
                "unknown approval decision {other}; use approved or rejected"
            )));
        }
    };
    let reference = args
        .get("reference")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
        .ok_or_else(|| {
            ToolError::InvalidInput("an external approval reference is required".to_string())
        })?
        .to_string();
    let evidence = args
        .get("evidence")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|evidence| !evidence.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("approval evidence is required".to_string()))?
        .to_string();
    let attempt_id = match args.get("attempt_id").and_then(Value::as_str) {
        Some(attempt_id) => attempt_id.to_string(),
        None => record
            .attempts
            .last()
            .map(|attempt| attempt.id.clone())
            .ok_or_else(|| ToolError::Failed("the task has no attempt to approve".to_string()))?,
    };
    if !record
        .attempts
        .iter()
        .any(|attempt| attempt.id == attempt_id)
    {
        return Err(ToolError::InvalidInput(format!(
            "attempt {attempt_id} does not belong to task {id}"
        )));
    }
    let expires_ms = args.get("expires_ms").and_then(Value::as_u64);
    record.approvals.push(ApprovalReference {
        attempt_id,
        approver: approver.to_string(),
        decision,
        scope: args
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| record.id.clone()),
        evidence,
        reference,
        recorded_ms: now,
        expires_ms,
        revoked_ms: None,
        superseded_by: None,
    });
    FileTasksStore
        .save(&context.workspace, &record)
        .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
    Ok(json!({"task": id, "approvals": record.approvals}))
}

fn attach_approve(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_approve",
        "Record or revoke a structured review/human approval bound to one task attempt. A model cannot approve its own task; approvals cannot verify a later retry.",
        json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "The task id." },
                "approver": { "type": "string", "description": "The human/external approver identity." },
                "decision": { "type": "string", "enum": ["approved", "rejected"] },
                "scope": { "type": "string", "description": "The approved scope; defaults to the task id." },
                "evidence": { "type": "string", "description": "Evidence supporting the decision." },
                "reference": { "type": "string", "description": "The external approval reference id." },
                "attempt_id": { "type": "string", "description": "The attempt to bind; defaults to the current attempt." },
                "expires_ms": { "type": "integer", "description": "Optional expiry in unix milliseconds." },
                "revoke": { "type": "boolean", "description": "Revoke the latest matching approval." }
            },
            "required": ["task"]
        }),
        move |args| {
            let context = context.clone();
            async move { approve_with(&context, &args) }
        },
    );
    if let Err(error) = agent.try_add_async_tool(Arc::new(tool)) {
        log::warn!("task_approve tool was not registered: {error}");
    }
}

/// The status tool.
fn attach_status(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_status",
        "Read one task's record: its state, trigger, budget and every attempt's outcome. Use it to inspect a task created earlier without waiting on it.",
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task id returned by task_create."
                }
            },
            "required": ["task"]
        }),
        move |args| {
            let context = context.clone();
            async move { status_with(&context, &args) }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_status")
        .expect("task_status metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_status tool was not registered: {error}");
    }
}

/// The list tool.
fn attach_list(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_list",
        "List the tasks recorded for this workspace, newest first. Filter by session, or cap the count. A row names the id task_status and task_cancel take.",
        json!({
            "type": "object",
            "properties": {
                "session": {
                    "type": "string",
                    "description": "Only tasks whose origin session is this id."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "How many rows to return, newest first. Default 50."
                }
            }
        }),
        move |args| {
            let context = context.clone();
            async move { list_with(&context, &args) }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_list")
        .expect("task_list metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_list tool was not registered: {error}");
    }
}

/// Settle the record whose attempt run id matches, across the given workspaces.
///
/// The turn pump and the tick know an outcome before any join does; without
/// this the durable record would stay Running after its run ended.
pub(crate) fn settle_by_run(
    roots: &[std::path::PathBuf],
    run: &str,
    state: crate::extension::runs::RunState,
    result: Option<String>,
    error: Option<String>,
    tokens: u64,
) -> bool {
    use crate::extension::runs::RunState;
    let (task_status, attempt_status) = match state {
        RunState::Succeeded => (TaskStatus::Succeeded, AttemptStatus::Succeeded),
        RunState::Failed => (TaskStatus::Failed, AttemptStatus::Failed),
        RunState::Cancelled => (TaskStatus::Cancelled, AttemptStatus::Cancelled),
        RunState::Unknown => (TaskStatus::Unknown, AttemptStatus::Unknown),
        RunState::Queued | RunState::Running => return false,
    };
    let now = crate::settings::unix_now().saturating_mul(1_000);
    for root in roots {
        let Ok(tasks) = FileTasksStore.list(root) else {
            continue;
        };
        for mut record in tasks {
            let Some(attempt) = record
                .attempts
                .iter_mut()
                .find(|attempt| attempt.run.as_deref() == Some(run))
            else {
                continue;
            };
            attempt.status = attempt_status;
            attempt.ended_ms = Some(now);
            if result.is_some() {
                attempt.result = result.clone();
            }
            if error.is_some() {
                attempt.error = error.clone();
            }
            if tokens != 0 {
                attempt.usage = Some(crate::extension::tasks::Usage {
                    tokens,
                    steps: 0,
                    wall_ms: 0,
                });
            }
            record.status = if matches!(record.trigger, Trigger::Every { .. }) {
                TaskStatus::Scheduled
            } else {
                task_status
            };
            record.updated_ms = now;
            let _ = FileTasksStore.save(root, &record);
            return true;
        }
    }
    false
}

/// Mark local attempts that died with the previous process as interrupted.
///
/// Called once at shell assembly, before any turn opens: the runs service, the
/// subagent registry and the job roster are process-local, so nothing they held
/// survives, and a record still marked Running is lying.
pub(crate) fn reconcile(
    workspace: &std::path::Path,
    subagents: &crate::subagent::SubagentRegistry,
    jobs: &agent::jobs::JobRegistry,
) -> usize {
    let Ok(tasks) = FileTasksStore.list(workspace) else {
        return 0;
    };
    let mut changed = 0usize;
    let now = crate::settings::unix_now().saturating_mul(1_000);
    for mut task in tasks {
        if !matches!(task.host, Host::Local) {
            continue;
        }
        let Some(attempt) = task.attempts.last() else {
            // A now task that never started cannot start after a restart.
            if task.status == TaskStatus::Queued && matches!(task.trigger, Trigger::Now) {
                task.status = TaskStatus::Interrupted;
                task.updated_ms = now;
                let _ = FileTasksStore.save(workspace, &task);
                changed += 1;
            }
            continue;
        };
        if !matches!(
            attempt.status,
            AttemptStatus::Running | AttemptStatus::Queued
        ) {
            continue;
        }
        let Some(run) = attempt.run.clone() else {
            continue;
        };
        let live = match task.tier {
            // Nothing in the runs service survives a restart.
            Tier::Turn => false,
            Tier::Subagent => subagents
                .run(&run)
                .map(|run| run.status.is_running())
                .unwrap_or(false),
            Tier::Job => jobs
                .job(&run)
                .map(|job| job.status.is_running())
                .unwrap_or(false),
            // The peer may still own it; the join reports Unknown when it cannot
            // be reached, so reconciliation only flags the local tiers.
            Tier::Delegation | Tier::PeerTask => true,
        };
        if live {
            continue;
        }
        task.status = TaskStatus::Interrupted;
        task.updated_ms = now;
        if let Some(attempt) = task.attempts.last_mut() {
            attempt.status = AttemptStatus::Interrupted;
            attempt.ended_ms = Some(now);
            attempt.error = Some("interrupted by a restart".to_string());
        }
        let _ = FileTasksStore.save(workspace, &task);
        changed += 1;
    }
    changed
}

/// Start a new attempt for a settled task.
pub(crate) async fn retry_with(
    runs: &Runs,
    context: &TaskToolsContext,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("task is required".to_string()))?;
    let Some(mut record) = FileTasksStore
        .load(&context.workspace, id)
        .map_err(|error| ToolError::Failed(format!("the task could not be read: {error}")))?
    else {
        return Err(ToolError::Failed(format!("unknown task {id}")));
    };
    if record.active_attempt().is_some() {
        return Err(ToolError::Failed(
            "the task still has a live attempt; stop it before retrying".to_string(),
        ));
    }
    if !record.may_retry() {
        return Err(ToolError::Failed(
            "the task has used all of its attempts".to_string(),
        ));
    }
    // Retry safety is derived from the current attempt's persisted side-effect
    // evidence, not from task-level status (issue #430).
    let current_effect = record
        .attempts
        .last()
        .and_then(|attempt| record.side_effects.get(&attempt.id));
    let confirmed = args
        .get("confirm_side_effects")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compensation = args
        .get("compensation")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    match record.retry_policy {
        tasks::RetryPolicy::Idempotent => {}
        tasks::RetryPolicy::NonIdempotent => {
            let risky = current_effect
                .map(|effect| {
                    matches!(
                        effect.status,
                        tasks::SideEffectStatus::Started
                            | tasks::SideEffectStatus::Completed
                            | tasks::SideEffectStatus::Unknown
                    )
                })
                .unwrap_or(false);
            if risky && !confirmed {
                return Err(ToolError::Failed(
                    "this non-idempotent task may already have produced a side effect; pass confirm_side_effects=true after verifying or compensating it".to_string(),
                ));
            }
        }
        tasks::RetryPolicy::Unknown => {
            let risky = current_effect
                .map(|effect| effect.status != tasks::SideEffectStatus::None)
                .unwrap_or(false);
            if risky && (!confirmed || compensation.is_empty()) {
                return Err(ToolError::Failed(
                    "this task's side-effect status is unknown; pass confirm_side_effects=true and a compensation/recovery path before retrying".to_string(),
                ));
            }
        }
    }
    let now = crate::settings::unix_now().saturating_mul(1_000);
    let run = match record.tier {
        Tier::Turn => {
            let spec = RunSpec {
                session: record.origin.session.clone(),
                objective: record.intent.clone(),
                tier: RunTier::Turn,
                limits: Limits {
                    steps: record.budget.max_steps,
                    tokens: record.budget.max_tokens,
                    deadline_ms: record.budget.deadline_ms,
                },
                target: None,
                capability: None,
                trace: record.id.clone(),
            };
            runs.start(spec).map_err(ToolError::Failed)?.id
        }
        Tier::Subagent => {
            let Some(subagents) = context.subagents.as_ref() else {
                return Err(ToolError::Failed(
                    "subagents are not available in this turn".to_string(),
                ));
            };
            let (run, _) = subagents.spawn_child(&record.title, &record.intent)?;
            run
        }
        Tier::Job => {
            let Some(command) = record.command.clone() else {
                return Err(ToolError::Failed(
                    "the task has no command to run".to_string(),
                ));
            };
            let Some(bash) = context.bash.clone() else {
                return Err(ToolError::Failed(
                    "the shell tool is not available in this turn".to_string(),
                ));
            };
            let response = bash
                .run(&json!({ "command": command, "run_in_background": true }))
                .map_err(|error| {
                    ToolError::Failed(format!("the command could not start: {error:?}"))
                })?;
            response
                .get("job_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::Failed("the shell did not report a job id".to_string()))?
                .to_string()
        }
        Tier::Delegation => {
            let Some(target) = record.target.clone() else {
                return Err(ToolError::Failed(
                    "the task names no peer to retry on".to_string(),
                ));
            };
            let Some(input) = record.input.clone() else {
                return Err(ToolError::Failed(
                    "the delegation input was not stored".to_string(),
                ));
            };
            let remote = crate::federation_tools::delegate_record(
                &target,
                &record.intent,
                record.capability.as_deref(),
                input,
            )
            .await?;
            remote.request.id.to_string()
        }
        Tier::PeerTask => {
            return Err(ToolError::Failed(
                "a peer task retry needs a new id on the peer; create a new task".to_string(),
            ));
        }
    };
    let attempt_id = format!("{}-{}", record.id, record.attempts.len() + 1);
    let mut attempt = Attempt::queued(attempt_id, record.tier);
    attempt.run = Some(run.clone());
    attempt.status = AttemptStatus::Running;
    attempt.started_ms = Some(now);
    record.attempts.push(attempt);
    mark_attempt_pending(&mut record);
    mark_side_effect_started(&mut record);
    record.status = TaskStatus::Running;
    record.updated_ms = now;
    FileTasksStore
        .save(&context.workspace, &record)
        .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
    Ok(json!({ "task": id, "run": run, "state": "running" }))
}

/// One task as the tools report it, with long results clipped.
fn task_json(task: &Task) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "intent": task.intent,
        "host": task.host,
        "tier": task.tier,
        "status": task.status,
        "trigger": task.trigger,
        "target": task.target,
        "command": task.command,
        "input": task.input,
        "budget": task.budget,
        "retry": task.retry,
        "execution_succeeded": task.execution_succeeded(),
        "verified_completion": task.verified_completion(),
        "verification": task.verification,
        "verification_contract": task.verification_contract,
        "retry_policy": task.retry_policy,
        "idempotency_key": task.idempotency_key,
        "side_effects": task.side_effects,
        "approvals": task.approvals,
        "next_fire_ms": task.trigger.next_fire_ms(),
        "created_ms": task.created_ms,
        "updated_ms": task.updated_ms,
        "attempts": task
            .attempts
            .iter()
            .map(|attempt| {
                json!({
                    "id": attempt.id,
                    "tier": attempt.tier,
                    "run": attempt.run,
                    "status": attempt.status,
                    "result": attempt
                        .result
                        .as_deref()
                        .map(|text| text.chars().take(800).collect::<String>()),
                    "error": attempt.error,
                    "started_ms": attempt.started_ms,
                    "ended_ms": attempt.ended_ms,
                    "delivered_ms": attempt.delivered_ms,
                    "verified_success": attempt.verified_success(),
                    "verification": attempt.verification,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// Read one task record.
pub(crate) fn status_with(context: &TaskToolsContext, args: &Value) -> Result<Value, ToolError> {
    let id = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("task is required".to_string()))?;
    let record = FileTasksStore
        .load(&context.workspace, id)
        .map_err(|error| ToolError::Failed(format!("the task could not be read: {error}")))?
        .ok_or_else(|| ToolError::Failed(format!("unknown task {id}")))?;
    Ok(task_json(&record))
}

/// List this workspace's tasks, newest first.
pub(crate) fn list_with(context: &TaskToolsContext, args: &Value) -> Result<Value, ToolError> {
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    let session = args.get("session").and_then(Value::as_str);
    let mut tasks = FileTasksStore
        .list(&context.workspace)
        .map_err(|error| ToolError::Failed(format!("the tasks could not be read: {error}")))?;
    if let Some(session) = session {
        tasks.retain(|task| task.origin.session == session);
    }
    tasks.sort_by(|left, right| (right.created_ms, &right.id).cmp(&(left.created_ms, &left.id)));
    let total = tasks.len();
    let rows: Vec<Value> = tasks
        .into_iter()
        .take(limit)
        .map(|task| {
            json!({
                "id": task.id,
                "title": task.title,
                "host": task.host,
                "tier": task.tier,
                "status": task.status,
                "verification": task
                    .attempts
                    .last()
                    .map(|attempt| attempt.verification.status)
                    .unwrap_or(task.verification.status),
                "execution_succeeded": task.execution_succeeded(),
                "verified_completion": task.verified_completion(),
                "next_fire_ms": task.trigger.next_fire_ms(),
            })
        })
        .collect();
    Ok(json!({ "total": total, "tasks": rows }))
}

/// A graph executor that runs each task as a subagent child of this turn.
///
/// The child is the existing subagent tier: policy, slots and the runner are
/// the subagent tool's own path, and the graph only decides order and
/// concurrency. Dropping the executor mid-task (a cancelled turn) interrupts
/// the child it was waiting on, so cancellation propagates through the graph.
struct SubagentGraphExecutor {
    subagents: Arc<crate::caps::SubagentContext>,
}

/// Interrupt a child run when the waiting future is dropped.
struct ChildGuard {
    subagents: Arc<crate::caps::SubagentContext>,
    run: String,
    armed: bool,
}

impl ChildGuard {
    fn new(subagents: Arc<crate::caps::SubagentContext>, run: String) -> Self {
        Self {
            subagents,
            run,
            armed: true,
        }
    }

    /// The child finished on its own: do not interrupt it on drop.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.subagents.interrupt_run(&self.run);
        }
    }
}

#[async_trait::async_trait]
impl federation::TaskExecutor for SubagentGraphExecutor {
    async fn execute(
        &self,
        task: &federation::TaskNode,
    ) -> Result<federation::TaskOutcome, String> {
        let (run, _) = self
            .subagents
            .spawn_child(&task.id.to_string(), &task.objective)
            .map_err(|error| format!("{error}"))?;
        let mut guard = ChildGuard::new(Arc::clone(&self.subagents), run.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            let Some(snapshot) = self.subagents.snapshot(&run) else {
                return Err("the child run disappeared".to_string());
            };
            match snapshot.state {
                crate::extension::runs::RunState::Succeeded => {
                    guard.disarm();
                    return Ok(federation::TaskOutcome::value(serde_json::json!({
                        "run": run,
                        "text": snapshot.result,
                    })));
                }
                crate::extension::runs::RunState::Failed
                | crate::extension::runs::RunState::Cancelled
                | crate::extension::runs::RunState::Unknown => {
                    guard.disarm();
                    return Err(snapshot
                        .error
                        .unwrap_or_else(|| "the child run failed".to_string()));
                }
                crate::extension::runs::RunState::Queued
                | crate::extension::runs::RunState::Running => {}
            }
            if std::time::Instant::now() >= deadline {
                // Dropping the armed guard interrupts the child.
                return Err("the child run timed out".to_string());
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }
}

/// The graph tool.
fn attach_graph(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_graph",
        "Run a DAG of agent tasks inside this turn: each task runs as a subagent child, dependencies run first, and the result reports every task's state and output. Bounded to 32 tasks and 4 concurrent children.",
        json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 32,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "objective": { "type": "string" },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" }
                            },
                            "acceptance": { "type": "string" },
                            "max_attempts": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 5
                            }
                        },
                        "required": ["id", "objective"]
                    }
                },
                "concurrency": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4
                }
            },
            "required": ["tasks"]
        }),
        move |args| {
            let context = context.clone();
            async move { graph_with(&context, &args).await }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_graph")
        .expect("task_graph metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_graph tool was not registered: {error}");
    }
}

/// Build and run a task graph.
pub(crate) async fn graph_with(
    context: &TaskToolsContext,
    args: &Value,
) -> Result<Value, ToolError> {
    let tasks = args
        .get("tasks")
        .and_then(Value::as_array)
        .filter(|tasks| !tasks.is_empty())
        .ok_or_else(|| ToolError::InvalidInput("tasks is required".to_string()))?;
    if tasks.len() > 32 {
        return Err(ToolError::InvalidInput(
            "a graph may have at most 32 tasks".to_string(),
        ));
    }
    let concurrency = args
        .get("concurrency")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .clamp(1, 4) as usize;
    let mut graph = federation::TaskGraph::new();
    let mut seen = std::collections::BTreeSet::new();
    for task in tasks {
        let id = task
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("every task needs an id".to_string()))?;
        if !seen.insert(id.to_string()) {
            return Err(ToolError::InvalidInput(format!("duplicate task id {id}")));
        }
        let objective = task
            .get("objective")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|objective| !objective.is_empty())
            .ok_or_else(|| ToolError::InvalidInput(format!("task {id} needs an objective")))?;
        let depends_on: Vec<federation::TaskNodeId> = task
            .get("depends_on")
            .and_then(Value::as_array)
            .map(|deps| {
                deps.iter()
                    .filter_map(Value::as_str)
                    .map(federation::TaskNodeId::new)
                    .collect()
            })
            .unwrap_or_default();
        graph
            .add(federation::TaskNodeId::new(id), objective, depends_on)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        if let Some(acceptance) = task.get("acceptance").and_then(Value::as_str) {
            graph
                .set_acceptance(&federation::TaskNodeId::new(id), acceptance)
                .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        }
        if let Some(attempts) = task.get("max_attempts").and_then(Value::as_u64) {
            graph
                .set_max_attempts(&federation::TaskNodeId::new(id), attempts as u32)
                .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        }
    }
    let Some(subagents) = context.subagents.clone() else {
        return Err(ToolError::Failed(
            "subagents are not available in this turn".to_string(),
        ));
    };
    let mut orchestrator = federation::Orchestrator::new(graph);
    let executor = Arc::new(SubagentGraphExecutor { subagents });
    let progress = orchestrator
        .run(executor, concurrency)
        .await
        .map_err(|error| ToolError::Failed(error.to_string()))?;
    let rows: Vec<Value> = orchestrator
        .graph()
        .tasks
        .values()
        .map(|task| {
            json!({
                "id": task.id.to_string(),
                "state": format!("{:?}", task.state).to_lowercase(),
                "result": task.result,
                "error": task.error,
                "acceptance": task.acceptance_result,
            })
        })
        .collect();
    Ok(json!({
        "succeeded": progress.succeeded,
        "failed": progress.failed,
        "tasks": rows,
    }))
}

/// A short board label from the intent when the model names none.
fn derived_title(intent: &str) -> String {
    let line = intent.lines().next().unwrap_or("").trim();
    let mut title = String::new();
    for word in line.split_whitespace() {
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
        if title.chars().count() >= 56 {
            break;
        }
    }
    if title.is_empty() {
        "Task".to_string()
    } else {
        title
    }
}

/// The cancel tool.
fn attach_cancel(agent: &mut Agent, context: TaskToolsContext) {
    let tool = AsyncFunctionTool::from_fn(
        "task_cancel",
        "Stop a task you created. A turn or child run is stopped in place, a delegation is cancelled on its peer, and a scheduled task is cancelled so it stops firing. The record is marked cancelled.",
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task id returned by task_create."
                }
            },
            "required": ["task"]
        }),
        move |args| {
            let context = context.clone();
            async move {
                let runs = crate::extension::runs::runs();
                cancel_with(&runs, &context, &args).await
            }
        },
    );
    let metadata = crate::tool_metadata::builtin_metadata("task_cancel")
        .expect("task_cancel metadata is declared");
    if let Err(error) =
        agent.try_add_async_tool(Arc::new(DeclaredAsyncTool::new(Arc::new(tool), metadata)))
    {
        log::warn!("task_cancel tool was not registered: {error}");
    }
}

/// Parse the tool's trigger field. Omitted means run now.
fn parse_trigger(args: &Value, now: u64) -> Result<Trigger, ToolError> {
    let Some(value) = args.get("trigger").filter(|value| !value.is_null()) else {
        return Ok(Trigger::Now);
    };
    if let Some(every) = value.get("every_seconds").and_then(Value::as_u64) {
        if every < 60 {
            return Err(ToolError::InvalidInput(
                "every_seconds must be at least 60".to_string(),
            ));
        }
        return Ok(Trigger::Every {
            every_seconds: every,
            last_run_ms: now,
        });
    }
    if let Some(at) = value.get("at_ms").and_then(Value::as_u64) {
        return Ok(Trigger::At { at_ms: at });
    }
    Err(ToolError::InvalidInput(
        "trigger must carry every_seconds or at_ms".to_string(),
    ))
}

/// Parse a retry policy, defaulting to Unknown (fail closed) (issue #430).
fn parse_retry_policy(args: &Value) -> Result<tasks::RetryPolicy, ToolError> {
    match args
        .get("retry_policy")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "idempotent" => Ok(tasks::RetryPolicy::Idempotent),
        "non-idempotent" | "non_idempotent" => Ok(tasks::RetryPolicy::NonIdempotent),
        "unknown" => Ok(tasks::RetryPolicy::Unknown),
        other => Err(ToolError::InvalidInput(format!(
            "unknown retry_policy {other}; use idempotent, non-idempotent or unknown"
        ))),
    }
}

/// Record that an attempt started and may have external effects.
fn mark_side_effect_started(task: &mut Task) {
    let Some(attempt) = task.attempts.last() else {
        return;
    };
    let status = match task.retry_policy {
        tasks::RetryPolicy::Idempotent | tasks::RetryPolicy::NonIdempotent => {
            tasks::SideEffectStatus::Started
        }
        tasks::RetryPolicy::Unknown => tasks::SideEffectStatus::Unknown,
    };
    task.side_effects.insert(
        attempt.id.clone(),
        tasks::SideEffectRecord {
            status,
            idempotency_key: task.idempotency_key.clone(),
            detail: format!("attempt {} started", attempt.id),
            recorded_ms: crate::settings::unix_now().saturating_mul(1_000),
        },
    );
}

/// Parse and validate a verification contract from the tool input.
///
/// Invalid input is refused before any task is created or executed. A
/// contract is not offered for peer-owned tasks: the owner's runtime verifies
/// on its own side (issue #416).
fn parse_verification_contract(
    args: &Value,
    tier: Tier,
) -> Result<Option<VerificationContract>, ToolError> {
    let Some(value) = args
        .get("verification_contract")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    if tier == Tier::PeerTask {
        return Err(ToolError::InvalidInput(
            "a peer-owned task cannot carry a local verification contract".to_string(),
        ));
    }
    let object = value.as_object().ok_or_else(|| {
        ToolError::InvalidInput("verification_contract must be an object".to_string())
    })?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .ok_or_else(|| {
            ToolError::InvalidInput(
                "verification_contract.kind must be command, artifact, review or human".to_string(),
            )
        })?;
    let contract = match kind {
        "command" => {
            let command = object
                .get("command")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .ok_or_else(|| {
                    ToolError::InvalidInput(
                        "a command verification contract needs a command".to_string(),
                    )
                })?;
            if command.contains('\0') {
                return Err(ToolError::InvalidInput(
                    "the verification command must not contain NUL".to_string(),
                ));
            }
            VerificationContract::Command {
                command: command.to_string(),
            }
        }
        "artifact" => {
            let path = object
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    ToolError::InvalidInput(
                        "an artifact verification contract needs a path".to_string(),
                    )
                })?;
            validate_workspace_relative(path)?;
            let digest = match object.get("digest") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let digest = value.as_str().ok_or_else(|| {
                        ToolError::InvalidInput(
                            "verification_contract.digest must be a SHA-256 hex string".to_string(),
                        )
                    })?;
                    let digest = digest.trim().to_ascii_lowercase();
                    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err(ToolError::InvalidInput(
                            "verification_contract.digest must be 64 hex characters".to_string(),
                        ));
                    }
                    Some(digest)
                }
            };
            VerificationContract::Artifact {
                path: path.to_string(),
                digest,
            }
        }
        "review" => VerificationContract::Review,
        "human" => VerificationContract::Human,
        other => {
            return Err(ToolError::InvalidInput(format!(
                "unknown verification_contract.kind {other}; use command, artifact, review or human"
            )));
        }
    };
    Ok(Some(contract))
}

/// Reject absolute paths and parent traversal before any verifier runs.
fn validate_workspace_relative(path: &str) -> Result<(), ToolError> {
    use std::path::Component;
    let candidate = std::path::Path::new(path);
    if candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ToolError::InvalidInput(
            "verification_contract artifact paths must be workspace-relative and may not traverse outside it".to_string(),
        ));
    }
    Ok(())
}

/// Mark the latest attempt's verification pending when a contract was asked
/// for; previous attempt reports are never reused (issue #417).
fn mark_attempt_pending(task: &mut Task) {
    if task.verification_contract.is_none() {
        return;
    }
    if let Some(attempt) = task.attempts.last_mut() {
        attempt.verification = VerificationReport {
            status: VerificationStatus::Pending,
            attempt_id: Some(attempt.id.clone()),
            ..VerificationReport::default()
        };
        task.verification = attempt.verification.clone();
    }
}

/// Create a task record and start its run.
pub(crate) async fn create_with(
    runs: &Runs,
    context: &TaskToolsContext,
    args: &Value,
) -> Result<Value, ToolError> {
    let intent = args
        .get("intent")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if intent.is_empty() {
        return Err(ToolError::InvalidInput(
            "intent is required and must not be empty".to_string(),
        ));
    }
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| derived_title(intent));
    let deps: Vec<String> = args
        .get("deps")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|dep| !dep.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let tier = match args.get("tier").and_then(Value::as_str).unwrap_or("turn") {
        "turn" => Tier::Turn,
        "subagent" => Tier::Subagent,
        "job" => Tier::Job,
        "delegation" => Tier::Delegation,
        "peer-task" => Tier::PeerTask,
        other => {
            return Err(ToolError::InvalidInput(format!(
                "unknown tier {other}; use turn, subagent, delegation or peer-task"
            )));
        }
    };
    let now = crate::settings::unix_now().saturating_mul(1_000);
    let trigger = parse_trigger(args, now)?;
    let verification_contract = parse_verification_contract(args, tier)?;
    let retry_policy = parse_retry_policy(args)?;
    let idempotency_key = args
        .get("idempotency_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string);
    if !deps.is_empty() && (tier != Tier::Turn || matches!(trigger, Trigger::Now)) {
        return Err(ToolError::InvalidInput(
            "dependencies are supported on scheduled turn tasks".to_string(),
        ));
    }
    let id = tasks::new_id(now);
    if deps.iter().any(|dep| dep == &id) {
        return Err(ToolError::InvalidInput(
            "a task cannot depend on itself".to_string(),
        ));
    }
    let mut task = Task::open(
        &id,
        title.clone(),
        intent,
        Origin {
            session: context.session.clone(),
            turn: 0,
        },
        trigger,
        tier,
        now,
    )
    .map_err(|refusal| ToolError::Failed(refusal.message().to_string()))?;
    task.retry_policy = retry_policy;
    task.idempotency_key = idempotency_key;
    if let Some(contract) = verification_contract {
        task.verification_contract = Some(contract);
        task.verification = VerificationReport {
            status: VerificationStatus::Pending,
            ..VerificationReport::default()
        };
    }
    if tier == Tier::PeerTask {
        let Some(target) = args
            .get("target")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|target| !target.is_empty())
        else {
            return Err(ToolError::InvalidInput(
                "a peer task needs a target node".to_string(),
            ));
        };
        if matches!(trigger, Trigger::Now) {
            return Err(ToolError::InvalidInput(
                "a peer task is owned by the peer's clock, so it needs a trigger".to_string(),
            ));
        }
        let schedule = match trigger {
            Trigger::Every { every_seconds, .. } => {
                federation::TaskSchedule::Every { every_seconds }
            }
            Trigger::At { at_ms } => federation::TaskSchedule::At { at_ms },
            Trigger::Now => unreachable!("refused above"),
        };
        let request = federation::TaskCreateRequest {
            id: id.clone(),
            objective: intent.to_string(),
            schedule,
            budget_steps: task.budget.max_steps,
            budget_tokens: task.budget.max_tokens,
            deadline_ms: task.budget.deadline_ms,
        };
        let remote = crate::federation_tools::create_peer_task(target, request).await?;
        task.host = Host::Peer {
            node: target.to_string(),
        };
        task.target = Some(target.to_string());
        task.status = TaskStatus::Scheduled;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "state": "scheduled",
            "peer": target,
            "next_fire_ms": remote.next_run_ms,
        }));
    }
    if tier == Tier::Job && !trigger.is_scheduled() {
        let Some(command) = args
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
        else {
            return Err(ToolError::InvalidInput(
                "a job task needs a command".to_string(),
            ));
        };
        let Some(bash) = context.bash.clone() else {
            return Err(ToolError::Failed(
                "the shell tool is not available in this turn".to_string(),
            ));
        };
        let response = bash
            .run(&json!({ "command": command, "run_in_background": true }))
            .map_err(|error| {
                ToolError::Failed(format!("the command could not start: {error:?}"))
            })?;
        let Some(job) = response.get("job_id").and_then(Value::as_str) else {
            return Err(ToolError::Failed(
                "the shell did not report a job id".to_string(),
            ));
        };
        task.command = Some(command.to_string());
        let mut attempt = Attempt::queued(format!("{id}-1"), Tier::Job);
        attempt.run = Some(job.to_string());
        attempt.status = AttemptStatus::Running;
        attempt.started_ms = Some(now);
        task.attempts.push(attempt);
        mark_attempt_pending(&mut task);
        mark_side_effect_started(&mut task);
        task.status = TaskStatus::Running;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "run": job,
            "state": "running",
            "verification_contract": task.verification_contract,
            "verification": task.verification.status,
        }));
    }
    task.deps = deps.clone();
    if !deps.is_empty() {
        task.status = TaskStatus::Waiting;
    }
    if !matches!(trigger, Trigger::Now) {
        // A scheduled job or delegation stores what it will need when its
        // clock fires; the dispatcher starts it later.
        if tier == Tier::Job {
            task.command = args
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string);
            if task
                .command
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
            {
                return Err(ToolError::InvalidInput(
                    "a job task needs a command".to_string(),
                ));
            }
        }
        if tier == Tier::Delegation {
            task.target = args
                .get("target")
                .and_then(Value::as_str)
                .map(str::to_string);
            task.capability = args
                .get("capability")
                .and_then(Value::as_str)
                .map(str::to_string);
            task.input = args.get("input").cloned();
            if task
                .target
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
            {
                return Err(ToolError::InvalidInput(
                    "a delegation task needs a target peer".to_string(),
                ));
            }
        }
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "state": "scheduled",
            "next_fire_ms": task.trigger.next_fire_ms(),
            "verification_contract": task.verification_contract,
            "verification": task.verification.status,
        }));
    }
    if tier == Tier::Subagent {
        let Some(subagents) = context.subagents.clone() else {
            return Err(ToolError::Failed(
                "subagents are not available in this turn".to_string(),
            ));
        };
        // Policy, slots and the runner are the subagent tool's own path.
        let (run, _) = subagents.spawn_child(&title, intent)?;
        let mut attempt = Attempt::queued(format!("{id}-1"), Tier::Subagent);
        attempt.run = Some(run.clone());
        attempt.status = AttemptStatus::Running;
        attempt.started_ms = Some(now);
        task.attempts.push(attempt);
        mark_attempt_pending(&mut task);
        mark_side_effect_started(&mut task);
        task.status = TaskStatus::Running;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "run": run,
            "state": "running",
            "verification_contract": task.verification_contract,
            "verification": task.verification.status,
        }));
    }
    if tier == Tier::Delegation && !trigger.is_scheduled() {
        let Some(target) = args
            .get("target")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|target| !target.is_empty())
        else {
            return Err(ToolError::InvalidInput(
                "a delegated task needs a target node".to_string(),
            ));
        };
        // Without a capability the peer runs an agent turn on the objective;
        // with one it invokes exactly that capability.
        let capability = args
            .get("capability")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|capability| !capability.is_empty());
        let payload = args.get("input").cloned().unwrap_or(Value::Null);
        task.input = Some(payload.clone());
        let record =
            crate::federation_tools::delegate_record(target, intent, capability, payload).await?;
        let run = record.request.id.to_string();
        task.target = Some(target.to_string());
        task.capability = capability.map(str::to_string);
        let mut attempt = Attempt::queued(format!("{id}-1"), Tier::Delegation);
        attempt.run = Some(run.clone());
        attempt.status = AttemptStatus::Running;
        attempt.started_ms = Some(now);
        task.attempts.push(attempt);
        mark_attempt_pending(&mut task);
        mark_side_effect_started(&mut task);
        task.status = TaskStatus::Running;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "run": run,
            "state": "running",
            "verification_contract": task.verification_contract,
            "verification": task.verification.status,
        }));
    }
    let run = runs
        .start(RunSpec {
            session: context.session.clone(),
            objective: intent.to_string(),
            tier: RunTier::Turn,
            limits: Limits {
                steps: 20,
                tokens: 200_000,
                deadline_ms: 600_000,
            },
            target: None,
            capability: None,
            trace: id.clone(),
        })
        .map_err(ToolError::Failed)?;
    let mut attempt = Attempt::queued(format!("{id}-1"), Tier::Turn);
    attempt.run = Some(run.id.clone());
    attempt.status = AttemptStatus::Running;
    attempt.started_ms = Some(now);
    task.attempts.push(attempt);
    mark_attempt_pending(&mut task);
    mark_side_effect_started(&mut task);
    task.status = TaskStatus::Running;
    task.updated_ms = now;
    FileTasksStore
        .save(&context.workspace, &task)
        .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
    Ok(json!({
        "task": id,
        "run": run.id,
        "state": "running",
        "verification_contract": task.verification_contract,
        "verification": task.verification.status,
    }))
}

/// Load the named records, wait for their runs, and settle them.
pub(crate) async fn join_with(
    runs: &Runs,
    context: &TaskToolsContext,
    args: &Value,
) -> Result<Value, ToolError> {
    let ids = args
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::InvalidInput("tasks must be an array of ids".to_string()))?;
    if ids.is_empty() {
        return Err(ToolError::InvalidInput(
            "tasks must name at least one task".to_string(),
        ));
    }
    let timeout = args
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(30_000)
        .min(300_000);
    let mut records: Vec<Task> = Vec::new();
    for id in ids {
        let Some(id) = id.as_str() else {
            return Err(ToolError::InvalidInput(
                "every id in tasks must be a string".to_string(),
            ));
        };
        match FileTasksStore.load(&context.workspace, id) {
            Ok(Some(task)) => records.push(task),
            Ok(None) => {
                return Err(ToolError::Failed(format!("unknown task {id}")));
            }
            Err(error) => {
                return Err(ToolError::Failed(format!(
                    "the task {id} could not be read: {error}"
                )));
            }
        }
    }
    let now = crate::settings::unix_now().saturating_mul(1_000);
    let order: Vec<String> = records.iter().map(|task| task.id.clone()).collect();
    // Mirrored tasks are refreshed from their owner; they have no local run,
    // so they never wait in the join engine.
    let mut peer_rows: std::collections::BTreeMap<String, tasks::JoinRow> =
        std::collections::BTreeMap::new();
    let mut local: Vec<Task> = Vec::new();
    for mut record in records {
        if let Host::Peer { node } = record.host.clone() {
            let (state, result, error) =
                match crate::federation_tools::peer_task_status(&node, &record.id).await {
                    Ok(remote) => {
                        record.status = mirror_status(remote.state);
                        record.updated_ms = now;
                        if let Trigger::Every { last_run_ms, .. } = &mut record.trigger {
                            *last_run_ms = remote.last_run_ms;
                        }
                        let _ = FileTasksStore.save(&context.workspace, &record);
                        (
                            Some(mirror_run_state(remote.state)),
                            remote.result,
                            remote.error,
                        )
                    }
                    Err(_) => (None, None, Some("the peer is unreachable".to_string())),
                };
            peer_rows.insert(
                record.id.clone(),
                tasks::JoinRow {
                    task: record.id.clone(),
                    run: None,
                    state,
                    result,
                    error,
                },
            );
        } else {
            local.push(record);
        }
    }
    let observe = |id: &str| -> Option<crate::extension::runs::RunSnapshot> {
        runs.observe(id)
            .or_else(|| {
                context
                    .subagents
                    .as_ref()
                    .and_then(|subagents| subagents.snapshot(id))
            })
            .or_else(|| crate::federation_tools::delegation_snapshot(id))
            .or_else(|| {
                context
                    .jobs
                    .as_ref()
                    .and_then(|jobs| job_snapshot(jobs, id))
            })
    };
    // The verifier runs through the turn's controlled shell: the same sandbox,
    // policy, working directory and resource limits as normal execution
    // (issue #418). Without one, command verification is inconclusive.
    let verification_shell: Option<VerificationShell> = context.bash.clone().map(|bash| {
        std::sync::Arc::new(move |command: &str| {
            let response = bash
                .run(&json!({ "command": command }))
                .map_err(|error| format!("{error:?}"))?;
            let code = response
                .get("exit_code")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    if response
                        .get("success")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        0
                    } else {
                        -1
                    }
                }) as i32;
            let stdout = response
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok((code, stdout))
        }) as VerificationShell
    });
    let report = tasks::join(observe, &local, timeout).await;
    for row in &report.rows {
        let Some(record) = local.iter_mut().find(|task| task.id == row.task) else {
            continue;
        };
        let Some(state) = row.state else {
            continue;
        };
        record.status = task_status(state);
        record.updated_ms = now;
        // A task still running when the join returns is armed: the harness
        // will deliver one trigger turn when it settles.
        record.resume_armed_ms = if state.is_settled() { None } else { Some(now) };
        if let Some(attempt) = record.attempts.last_mut() {
            attempt.status = attempt_status(state);
            attempt.ended_ms = state.is_settled().then_some(now);
            if row.result.is_some() {
                attempt.result = row.result.clone();
            }
            if row.error.is_some() {
                attempt.error = row.error.clone();
            }
            if state.is_settled() {
                let policy = record.retry_policy;
                if let Some(effect) = record.side_effects.get_mut(&attempt.id) {
                    effect.status = match (state, policy) {
                        (RunState::Succeeded, tasks::RetryPolicy::Unknown) => {
                            tasks::SideEffectStatus::Unknown
                        }
                        (RunState::Succeeded, _) => tasks::SideEffectStatus::Completed,
                        (RunState::Failed, _) => tasks::SideEffectStatus::Failed,
                        (RunState::Cancelled, _) => tasks::SideEffectStatus::Unknown,
                        _ => effect.status,
                    };
                    effect.recorded_ms = now;
                    effect.detail = format!("attempt {} settled as {:?}", attempt.id, state);
                }
            }
        }
        // Execution success is not verification. A settled successful task
        // with a contract is verified by the runtime here, not by the model's
        // prose report (issue #414).
        if state.is_settled()
            && record.status == TaskStatus::Succeeded
            && !matches!(
                record.verification.status,
                VerificationStatus::Passed | VerificationStatus::Failed
            )
        {
            if let Some(contract) = record.verification_contract.clone() {
                let attempt_id = record.attempts.last().map(|attempt| attempt.id.clone());
                let mut report = match &contract {
                    VerificationContract::Review | VerificationContract::Human => {
                        approval_report(record, attempt_id.as_deref().unwrap_or(""), now)
                    }
                    _ => {
                        verify_contract(&contract, &context.workspace, verification_shell.as_ref())
                            .await
                    }
                };
                // Bind the report to the attempt that produced this outcome:
                // a previous attempt's report can never satisfy it (issue #417).
                report.attempt_id = attempt_id;
                record.verification = report.clone();
                if let Some(attempt) = record.attempts.last_mut() {
                    attempt.verification = report;
                }
            }
        }
        let _ = FileTasksStore.save(&context.workspace, record);
    }
    let mut rows: Vec<tasks::JoinRow> = Vec::with_capacity(order.len());
    for id in &order {
        if let Some(row) = peer_rows.remove(id) {
            rows.push(row);
        } else if let Some(row) = report.rows.iter().find(|row| &row.task == id) {
            rows.push(row.clone());
        }
    }
    Ok(json!({
        "settled": report.settled,
        "tasks": rows
            .iter()
            .map(|row| json!({
                "task": row.task,
                "run": row.run,
                "state": row.state.map(state_name),
                "result": row.result,
                "error": row.error,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// SHA-256 of a file's bytes, lowercase hex.
fn file_sha256(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let digest = Sha256::digest(&bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

/// Run a task's verification contract deterministically (issue #414).
///
/// Commands run through the host shell with a bounded deadline; artifacts are
/// checked for existence and, when pinned, SHA-256. Review and human contracts
/// need an external reference, so they conclude inconclusive here rather than
/// accepting a model's self-attestation.
/// A controlled shell runner: commands go through the same sandbox, policy,
/// working directory and resource limits as normal task execution (issue #418).
pub(crate) type VerificationShell =
    std::sync::Arc<dyn Fn(&str) -> Result<(i32, String), String> + Send + Sync>;

pub(crate) async fn verify_contract(
    contract: &VerificationContract,
    workspace: &std::path::Path,
    shell: Option<&VerificationShell>,
) -> VerificationReport {
    let verifier = Some("bsh-runtime".to_string());
    let verifier_version = Some(env!("CARGO_PKG_VERSION").to_string());
    match contract {
        VerificationContract::Command { command } => {
            // The command must run through the controlled process runtime; a
            // host-shell fallback is not an acceptable verifier boundary.
            let Some(shell) = shell.cloned() else {
                return VerificationReport {
                    status: VerificationStatus::Inconclusive,
                    attempt_id: None,
                    verifier,
                    verifier_version,
                    verified_ms: Some(crate::settings::unix_now().saturating_mul(1_000)),
                    evidence: Vec::new(),
                    message: Some(
                        "no sandboxed shell is available for command verification".to_string(),
                    ),
                };
            };
            let runner_command = command.clone();
            let outcome = tokio::task::spawn_blocking(move || shell(&runner_command))
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let (status, detail) = match outcome {
                Ok((0, _)) => (VerificationStatus::Passed, Some("exit 0".to_string())),
                Ok((code, _)) => (VerificationStatus::Failed, Some(format!("exit {code}"))),
                Err(error) => (VerificationStatus::Inconclusive, Some(error)),
            };
            VerificationReport {
                status,
                attempt_id: None,
                verifier,
                verifier_version,
                verified_ms: Some(crate::settings::unix_now().saturating_mul(1_000)),
                evidence: vec![VerificationEvidence {
                    kind: "command".to_string(),
                    reference: command.to_string(),
                    digest: None,
                    detail,
                }],
                message: None,
            }
        }
        VerificationContract::Artifact { path, digest } => {
            let target = if std::path::Path::new(path).is_absolute() {
                std::path::PathBuf::from(path)
            } else {
                workspace.join(path)
            };
            // Resolve symlinks and refuse anything outside the workspace: a
            // path that lexically looked relative can still escape (issue #418).
            let root = workspace.canonicalize();
            let canonical = target.canonicalize();
            let escaped = match (&root, &canonical) {
                (Ok(root), Ok(target)) => !target.starts_with(root),
                _ => true,
            };
            if escaped {
                return VerificationReport {
                    status: VerificationStatus::Failed,
                    attempt_id: None,
                    verifier,
                    verifier_version,
                    verified_ms: Some(crate::settings::unix_now().saturating_mul(1_000)),
                    evidence: vec![VerificationEvidence {
                        kind: "artifact".to_string(),
                        reference: target.display().to_string(),
                        digest: None,
                        detail: Some("outside the workspace or missing".to_string()),
                    }],
                    message: Some(format!(
                        "{} is outside the task workspace",
                        target.display()
                    )),
                };
            }
            let target = canonical.expect("checked above");
            let metadata = std::fs::metadata(&target);
            let (status, actual_digest, detail, message) = match metadata {
                Ok(metadata) if !metadata.is_file() => (
                    VerificationStatus::Failed,
                    None,
                    Some("not a regular file".to_string()),
                    Some(format!("{} is not a file", target.display())),
                ),
                Ok(_) => {
                    let actual = file_sha256(&target).ok();
                    match (digest, &actual) {
                        (Some(expected), Some(actual)) if expected != actual => (
                            VerificationStatus::Failed,
                            Some(actual.clone()),
                            Some("digest mismatch".to_string()),
                            Some(format!("expected {expected}, found {actual}")),
                        ),
                        _ => (
                            VerificationStatus::Passed,
                            actual.clone(),
                            Some("artifact present".to_string()),
                            None,
                        ),
                    }
                }
                Err(error) => (
                    VerificationStatus::Failed,
                    None,
                    Some(error.to_string()),
                    Some(format!("{} is missing", target.display())),
                ),
            };
            VerificationReport {
                status,
                attempt_id: None,
                verifier,
                verifier_version,
                verified_ms: Some(crate::settings::unix_now().saturating_mul(1_000)),
                evidence: vec![VerificationEvidence {
                    kind: "artifact".to_string(),
                    reference: target.display().to_string(),
                    digest: actual_digest,
                    detail,
                }],
                message,
            }
        }
        VerificationContract::Review | VerificationContract::Human => VerificationReport {
            status: VerificationStatus::Inconclusive,
            attempt_id: None,
            verifier: Some("external".to_string()),
            verifier_version,
            verified_ms: Some(crate::settings::unix_now().saturating_mul(1_000)),
            evidence: Vec::new(),
            message: Some("this contract needs an external approval reference".to_string()),
        },
    }
}

/// Resolve a review/human contract from the task's approvals (issue #433).
fn approval_report(task: &Task, attempt_id: &str, now: u64) -> VerificationReport {
    let approval = task.approvals.iter().rev().find(|approval| {
        approval.attempt_id == attempt_id
            && approval.revoked_ms.is_none()
            && approval.superseded_by.is_none()
            && approval
                .expires_ms
                .map(|expiry| now <= expiry)
                .unwrap_or(true)
            && (approval.scope == task.id || approval.scope == "*")
    });
    match approval {
        Some(approval) if approval.decision == ApprovalDecision::Approved => VerificationReport {
            status: VerificationStatus::Passed,
            attempt_id: Some(attempt_id.to_string()),
            verifier: Some(format!("approval:{}", approval.approver)),
            verifier_version: None,
            verified_ms: Some(now),
            evidence: vec![VerificationEvidence {
                kind: "approval".to_string(),
                reference: approval.reference.clone(),
                digest: None,
                detail: Some(approval.evidence.clone()),
            }],
            message: None,
        },
        Some(approval) => VerificationReport {
            status: VerificationStatus::Failed,
            attempt_id: Some(attempt_id.to_string()),
            verifier: Some(format!("approval:{}", approval.approver)),
            verifier_version: None,
            verified_ms: Some(now),
            evidence: vec![VerificationEvidence {
                kind: "approval".to_string(),
                reference: approval.reference.clone(),
                digest: None,
                detail: Some(approval.evidence.clone()),
            }],
            message: Some("the approval was rejected".to_string()),
        },
        None => VerificationReport {
            status: VerificationStatus::Inconclusive,
            attempt_id: Some(attempt_id.to_string()),
            verifier: Some("approval".to_string()),
            verifier_version: None,
            verified_ms: Some(now),
            evidence: Vec::new(),
            message: Some(
                "no valid approval matches this attempt (missing, expired, revoked or scope mismatch)"
                    .to_string(),
            ),
        },
    }
}

/// The run's snapshot from the job roster, when it exists.
fn job_snapshot(
    jobs: &agent::jobs::JobRegistry,
    id: &str,
) -> Option<crate::extension::runs::RunSnapshot> {
    use crate::extension::runs::RunState;
    let job = jobs.job(id)?;
    let state = match job.status {
        agent::jobs::JobStatus::Running => RunState::Running,
        agent::jobs::JobStatus::Succeeded { .. } => RunState::Succeeded,
        agent::jobs::JobStatus::Failed { .. } => RunState::Failed,
        agent::jobs::JobStatus::Killed => RunState::Cancelled,
    };
    let error = matches!(job.status, agent::jobs::JobStatus::Failed { .. }).then(|| {
        format!(
            "the command exited with {}",
            job.status.exit_code().unwrap_or(-1)
        )
    });
    Some(crate::extension::runs::RunSnapshot {
        id: id.to_string(),
        state,
        result: jobs.output_for(id).map(|output| output.text),
        tokens: 0,
        steps: 0,
        error,
    })
}

/// The local mirror status for a peer task state.
pub(crate) fn mirror_status(state: federation::FederatedTaskState) -> TaskStatus {
    match state {
        federation::FederatedTaskState::Queued => TaskStatus::Scheduled,
        federation::FederatedTaskState::Running => TaskStatus::Running,
        federation::FederatedTaskState::Succeeded => TaskStatus::Succeeded,
        federation::FederatedTaskState::Failed => TaskStatus::Failed,
        federation::FederatedTaskState::Cancelled => TaskStatus::Cancelled,
    }
}

/// The attempt state for a peer task state.
pub(crate) fn mirror_attempt_status(state: federation::FederatedTaskState) -> AttemptStatus {
    match state {
        federation::FederatedTaskState::Queued => AttemptStatus::Queued,
        federation::FederatedTaskState::Running => AttemptStatus::Running,
        federation::FederatedTaskState::Succeeded => AttemptStatus::Succeeded,
        federation::FederatedTaskState::Failed => AttemptStatus::Failed,
        federation::FederatedTaskState::Cancelled => AttemptStatus::Cancelled,
    }
}

/// The run state for a peer task state.
fn mirror_run_state(state: federation::FederatedTaskState) -> crate::extension::runs::RunState {
    match state {
        federation::FederatedTaskState::Queued => crate::extension::runs::RunState::Queued,
        federation::FederatedTaskState::Running => crate::extension::runs::RunState::Running,
        federation::FederatedTaskState::Succeeded => crate::extension::runs::RunState::Succeeded,
        federation::FederatedTaskState::Failed => crate::extension::runs::RunState::Failed,
        federation::FederatedTaskState::Cancelled => crate::extension::runs::RunState::Cancelled,
    }
}

/// The record status for a run state.
fn task_status(state: RunState) -> TaskStatus {
    match state {
        RunState::Queued => TaskStatus::Queued,
        RunState::Running => TaskStatus::Running,
        RunState::Succeeded => TaskStatus::Succeeded,
        RunState::Failed => TaskStatus::Failed,
        RunState::Cancelled => TaskStatus::Cancelled,
        RunState::Unknown => TaskStatus::Unknown,
    }
}

/// The attempt status for a run state.
fn attempt_status(state: RunState) -> AttemptStatus {
    match state {
        RunState::Queued => AttemptStatus::Queued,
        RunState::Running => AttemptStatus::Running,
        RunState::Succeeded => AttemptStatus::Succeeded,
        RunState::Failed => AttemptStatus::Failed,
        RunState::Cancelled => AttemptStatus::Cancelled,
        RunState::Unknown => AttemptStatus::Unknown,
    }
}

/// The harness-generated prompt a resume turn receives.
pub(crate) fn resume_prompt(settled: &[(Task, crate::extension::runs::RunSnapshot)]) -> String {
    let mut text = String::from("Background results (generated by the harness, not the reader):\n");
    for (task, snapshot) in settled {
        let state = state_name(snapshot.state);
        text.push_str(&format!("\n---\n{} [{}]\n", task.title, state));
        let result = snapshot.result.as_deref().unwrap_or("").trim();
        let clipped = result.lines().take(6).collect::<Vec<_>>().join("\n");
        if !clipped.is_empty() {
            text.push_str(&clipped);
            text.push('\n');
        }
        if let Some(error) = &snapshot.error {
            text.push_str(&format!("error: {error}\n"));
        }
    }
    text.push_str(
        "\nThese tasks finished after your last join. Integrate their results and continue \
         the work; do not start new work unless it is needed.",
    );
    text
}

/// The wire name of a run state.
fn state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Succeeded => "succeeded",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
        RunState::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::extension::runs::{RunHandle, RunSnapshot, RunsService};

    struct Fake {
        next: AtomicU64,
        runs: Mutex<BTreeMap<String, RunSnapshot>>,
    }

    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                next: AtomicU64::new(0),
                runs: Mutex::new(BTreeMap::new()),
            })
        }
    }

    impl RunsService for Fake {
        fn start(&self, _spec: RunSpec) -> Result<RunHandle, String> {
            let id = format!("run-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
            self.runs.lock().expect("runs").insert(
                id.clone(),
                RunSnapshot {
                    id: id.clone(),
                    state: RunState::Running,
                    result: None,
                    tokens: 0,
                    steps: 0,
                    error: None,
                },
            );
            Ok(RunHandle { id })
        }

        fn observe(&self, id: &str) -> Option<RunSnapshot> {
            self.runs.lock().expect("runs").get(id).cloned()
        }

        fn cancel(&self, id: &str) -> Result<(), String> {
            if let Some(run) = self.runs.lock().expect("runs").get_mut(id) {
                run.state = RunState::Cancelled;
            }
            Ok(())
        }
    }

    fn context(label: &str) -> TaskToolsContext {
        let workspace =
            std::env::temp_dir().join(format!("bsh-task-tools-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&workspace).expect("scratch workspace");
        TaskToolsContext {
            session: "ses-1".to_string(),
            workspace,
            subagents: None,
            bash: None,
            jobs: None,
        }
    }

    /// attach registers both tools on the agent's own registry.
    #[test]
    fn attach_registers_both_tools() {
        let mut agent = Agent::from_config(agent::AgentConfig::default());
        attach(&mut agent, context("attach"));
        let registry = agent.registry().expect("a registry");
        assert!(registry.get_async("task_create").is_some());
        assert!(registry.get_async("task_join").is_some());
        assert!(registry.get_async("task_cancel").is_some());
        assert!(registry.get_async("task_approve").is_some());
    }

    /// create saves a running record whose attempt names the started run.
    #[tokio::test]
    async fn create_starts_a_run_and_saves_the_record() {
        let context = context("create");
        let fake = Fake::new();
        let runs = Runs::with(fake);
        let value = create_with(&runs, &context, &json!({"intent": "Review the diff"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id");
        let stored = FileTasksStore
            .load(&context.workspace, id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Running);
        assert_eq!(
            stored.attempts.last().expect("attempt").run.as_deref(),
            Some("run-1")
        );
        assert_eq!(stored.tier, Tier::Turn);
        assert_eq!(stored.title, "Review the diff");
        assert_eq!(stored.origin.session, "ses-1");
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// task_create exposes and persists every supported verification
    /// contract, and marks the first attempt pending with its own id (#416).
    #[tokio::test]
    async fn create_accepts_each_supported_verification_contract() {
        let context = context("contracts");
        let fake = Fake::new();
        let runs = Runs::with(fake);
        let cases = vec![
            (
                json!({"kind": "command", "command": "cargo test"}),
                "command",
            ),
            (
                json!({"kind": "artifact", "path": "target/out.txt", "digest": "a".repeat(64)}),
                "artifact",
            ),
            (json!({"kind": "review"}), "review"),
            (json!({"kind": "human"}), "human"),
        ];
        for (contract, kind) in cases {
            let value = create_with(
                &runs,
                &context,
                &json!({"intent": "Verify the result", "verification_contract": contract}),
            )
            .await
            .expect("created");
            assert_eq!(value["verification_contract"]["kind"], kind);
            assert_eq!(value["verification"], "pending");
            let id = value["task"].as_str().expect("id");
            let task = FileTasksStore
                .load(&context.workspace, id)
                .expect("load")
                .expect("record");
            assert!(task.verification_contract.is_some(), "{kind} persisted");
            assert_eq!(task.verification.status, VerificationStatus::Pending);
            let attempt = task.attempts.last().expect("attempt");
            assert_eq!(attempt.verification.status, VerificationStatus::Pending);
            assert_eq!(
                attempt.verification.attempt_id.as_deref(),
                Some(attempt.id.as_str())
            );
        }
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Malformed, unsafe or peer-owned contracts are refused before a task is
    /// created or executed (#416).
    #[tokio::test]
    async fn create_refuses_invalid_verification_contracts() {
        let context = context("bad-contracts");
        let runs = Runs::with(Fake::new());
        let invalid = vec![
            json!({"kind": "artifact", "path": "/etc/passwd"}),
            json!({"kind": "artifact", "path": "../outside"}),
            json!({"kind": "artifact", "path": "ok", "digest": "not-hex"}),
            json!({"kind": "command", "command": "   "}),
            json!({"kind": "unknown"}),
            json!(["not-an-object"]),
        ];
        for contract in invalid {
            let error = create_with(
                &runs,
                &context,
                &json!({"intent": "Verify", "verification_contract": contract}),
            )
            .await
            .expect_err("refused");
            assert!(format!("{error:?}").contains("verification"), "{error:?}");
        }
        let peer = create_with(
            &runs,
            &context,
            &json!({
                "intent": "Remote",
                "tier": "peer-task",
                "target": "peer-1",
                "trigger": {"every_seconds": 60},
                "verification_contract": {"kind": "review"}
            }),
        )
        .await
        .expect_err("peer contract refused");
        assert!(format!("{peer:?}").contains("peer-owned"), "{peer:?}");
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A retry starts a new pending attempt; the previous attempt's Passed
    /// report never satisfies it, and re-verification binds to the new id
    /// (#417).
    #[tokio::test]
    async fn a_retry_never_reuses_the_previous_verification_report() {
        let context = context("retry-verify");
        std::fs::write(context.workspace.join("artifact.txt"), "done").expect("artifact");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(
            &runs,
            &context,
            &json!({
                "intent": "Verify the artifact",
                "retry_policy": "idempotent",
                "verification_contract": {"kind": "artifact", "path": "artifact.txt"}
            }),
        )
        .await
        .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-1").expect("run");
            snapshot.state = RunState::Succeeded;
            snapshot.result = Some("done".to_string());
        }
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let settled = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        let first_id = settled.attempts[0].id.clone();
        assert_eq!(
            settled.attempts[0].verification.status,
            VerificationStatus::Passed
        );
        assert_eq!(
            settled.attempts[0].verification.attempt_id.as_deref(),
            Some(first_id.as_str())
        );
        assert!(settled.verified_completion());

        retry_with(&runs, &context, &json!({"task": id}))
            .await
            .expect("retry");
        let retried = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        let second = retried.attempts.last().expect("second attempt");
        assert_ne!(second.id, first_id);
        assert_eq!(second.verification.status, VerificationStatus::Pending);
        assert_eq!(
            second.verification.attempt_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(
            retried.attempts[0].verification.status,
            VerificationStatus::Passed
        );
        assert!(!retried.verified_completion(), "pending, not verified");

        // A stale report naming the first attempt cannot verify the second.
        let mut stale = retried.clone();
        let second_index = stale.attempts.len() - 1;
        stale.attempts[second_index].status = AttemptStatus::Succeeded;
        stale.attempts[second_index].verification = VerificationReport {
            status: VerificationStatus::Passed,
            attempt_id: Some(first_id.clone()),
            verifier: Some("cargo test".to_string()),
            evidence: vec![VerificationEvidence {
                kind: "artifact".to_string(),
                reference: "artifact.txt".to_string(),
                digest: None,
                detail: None,
            }],
            ..VerificationReport::default()
        };
        assert!(!stale.verified_completion());

        // Re-verification at the new attempt's settle makes it verified.
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-2").expect("second run");
            snapshot.state = RunState::Succeeded;
            snapshot.result = Some("done again".to_string());
        }
        join_with(&runs, &context, &json!({"tasks": [id]}))
            .await
            .expect("join again");
        let reverified = FileTasksStore
            .load(&context.workspace, &retried.id)
            .expect("load")
            .expect("record");
        let second = reverified.attempts.last().expect("second attempt");
        assert_eq!(second.verification.status, VerificationStatus::Passed);
        assert_eq!(
            second.verification.attempt_id.as_deref(),
            Some(second.id.as_str())
        );
        assert!(reverified.verified_completion());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A successful execution whose verification is still owed is not blindly
    /// retried: that could repeat a non-idempotent side effect (#417).
    #[tokio::test]
    async fn retry_refuses_an_unverified_successful_execution() {
        let context = context("retry-unverified");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(
            &runs,
            &context,
            &json!({
                "intent": "Review",
                "retry_policy": "non-idempotent",
                "verification_contract": {"kind": "review"}
            }),
        )
        .await
        .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-1").expect("run");
            snapshot.state = RunState::Succeeded;
            snapshot.result = Some("done".to_string());
        }
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let error = retry_with(&runs, &context, &json!({"task": id}))
            .await
            .expect_err("retry refused");
        assert!(format!("{error:?}").contains("non-idempotent"), "{error:?}");
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// An empty intent and a missing service both refuse, and neither leaves a
    /// record behind.
    #[tokio::test]
    async fn create_refuses_and_saves_nothing() {
        let context = context("refuse");
        assert!(
            create_with(&Runs::empty(), &context, &json!({"intent": "  "}))
                .await
                .is_err()
        );
        let error = create_with(&Runs::empty(), &context, &json!({"intent": "x"}))
            .await
            .expect_err("no service");
        assert!(format!("{error:?}").contains("no host.runs service"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// join settles the record from the run's snapshot and returns the result.
    #[tokio::test]
    async fn join_settles_the_record_from_the_run() {
        let context = context("join");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(&runs, &context, &json!({"intent": "Review"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-1").expect("the run");
            snapshot.state = RunState::Succeeded;
            snapshot.result = Some("done".to_string());
        }
        let report = join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("joined");
        assert_eq!(report["settled"], true);
        let stored = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Succeeded);
        assert_eq!(
            stored.attempts.last().expect("attempt").result.as_deref(),
            Some("done")
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A trigger schedules instead of starting, and the record says so.
    #[tokio::test]
    async fn create_schedules_a_task() {
        let context = context("schedule");
        let value = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Hourly review", "trigger": {"every_seconds": 3600}}),
        )
        .await
        .expect("scheduled");
        let id = value["task"].as_str().expect("id");
        assert_eq!(value["state"], "scheduled");
        assert!(value["next_fire_ms"].as_u64().expect("next fire") > 0);
        let stored = FileTasksStore
            .load(&context.workspace, id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Scheduled);
        assert!(stored.attempts.is_empty());
        match stored.trigger {
            Trigger::Every {
                every_seconds,
                last_run_ms,
            } => {
                assert_eq!(every_seconds, 3_600);
                assert!(last_run_ms > 0);
            }
            other => panic!("unexpected trigger {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A malformed or too-frequent trigger is refused.
    #[tokio::test]
    async fn create_refuses_a_bad_trigger() {
        let context = context("bad-trigger");
        assert!(create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "x", "trigger": {"every_seconds": 5}})
        )
        .await
        .is_err());
        assert!(create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "x", "trigger": {}})
        )
        .await
        .is_err());
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A subagent task without the turn's subagent machinery refuses and saves
    /// nothing; the tier is never silently downgraded.
    #[tokio::test]
    async fn create_refuses_a_subagent_task_without_the_machinery() {
        let context = context("subagent-refused");
        let error = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Review", "tier": "subagent"}),
        )
        .await
        .expect_err("no subagent context");
        assert!(format!("{error:?}").contains("subagents are not available"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// An unknown tier is refused by name.
    #[tokio::test]
    async fn create_refuses_an_unknown_tier() {
        let context = context("tier");
        let error = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Review", "tier": "cron"}),
        )
        .await
        .expect_err("unknown tier");
        assert!(format!("{error:?}").contains("unknown tier"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A delegated task needs a target and a capability, and nothing is saved
    /// when either is missing.
    #[tokio::test]
    async fn create_refuses_an_incomplete_delegation() {
        let context = context("delegation-refused");
        let missing_target = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Audit", "tier": "delegation", "capability": "builtin/read_file"}),
        )
        .await
        .expect_err("no target");
        assert!(format!("{missing_target:?}").contains("target node"));

        // No capability is legal once a target is named: the peer runs an
        // agent turn. Without a node it still refuses at the federation seam.
        let no_node = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Audit", "tier": "delegation", "target": "node-b"}),
        )
        .await
        .expect_err("no node");
        assert!(format!("{no_node:?}").contains("federation node is not running"));

        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// With a target and a capability, the refusal comes from the federation
    /// plane itself (no node runs in this test), and still nothing is saved.
    #[tokio::test]
    async fn create_reports_a_missing_federation_node() {
        let context = context("delegation-node");
        let error = create_with(
            &Runs::empty(),
            &context,
            &json!({
                "intent": "Audit",
                "tier": "delegation",
                "target": "node-b",
                "capability": "builtin/read_file",
                "input": {"path": "README.md"}
            }),
        )
        .await
        .expect_err("no federation node");
        assert!(format!("{error:?}").contains("federation node is not running"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A join that returns with work still running arms a resume.
    #[tokio::test]
    async fn join_arms_a_resume_for_a_running_task() {
        let context = context("arm");
        let fake = Fake::new();
        let runs = Runs::with(fake);
        let value = create_with(&runs, &context, &json!({"intent": "Review"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        let report = join_with(
            &runs,
            &context,
            &json!({"tasks": [id.clone()], "timeout_ms": 0}),
        )
        .await
        .expect("joined");
        assert_eq!(report["settled"], false);
        let stored = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert!(
            stored.resume_armed_ms.is_some(),
            "an unsettled join arms the task"
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// The resume prompt names each settled task and says who generated it.
    #[test]
    fn the_resume_prompt_names_the_results() {
        let task = Task::open(
            "task-1",
            "Review the diff",
            "intent",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Now,
            Tier::Turn,
            1,
        )
        .expect("task");
        let snapshot = crate::extension::runs::RunSnapshot {
            id: "run-1".to_string(),
            state: RunState::Succeeded,
            result: Some("all good".to_string()),
            tokens: 10,
            steps: 2,
            error: None,
        };
        let text = resume_prompt(&[(task, snapshot)]);
        assert!(text.contains("Background results"));
        assert!(text.contains("Review the diff"));
        assert!(text.contains("all good"));
        assert!(text.contains("generated by the harness"));
    }

    /// Cancelling a running turn task stops its run through the service and
    /// marks the record.
    #[tokio::test]
    async fn cancel_stops_a_turn_task() {
        let context = context("cancel");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(&runs, &context, &json!({"intent": "Review"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        let report = cancel_with(&runs, &context, &json!({"task": id.clone()}))
            .await
            .expect("cancelled");
        assert_eq!(report["state"], "cancelled");
        let stored = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Cancelled);
        assert_eq!(
            stored.attempts.last().expect("attempt").status,
            AttemptStatus::Cancelled
        );
        assert_eq!(
            fake.runs.lock().expect("runs")["run-1"].state,
            RunState::Cancelled
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A scheduled task with no run is cancelled without one.
    #[tokio::test]
    async fn cancel_disables_a_scheduled_task() {
        let context = context("cancel-scheduled");
        let value = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Hourly", "trigger": {"every_seconds": 3600}}),
        )
        .await
        .expect("scheduled");
        let id = value["task"].as_str().expect("id").to_string();
        cancel_with(&Runs::empty(), &context, &json!({"task": id.clone()}))
            .await
            .expect("cancelled");
        let stored = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Cancelled);
        assert!(stored.attempts.is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// An unknown id is a refusal.
    #[tokio::test]
    async fn cancel_refuses_an_unknown_task() {
        let context = context("cancel-unknown");
        let error = cancel_with(&Runs::empty(), &context, &json!({"task": "nope"}))
            .await
            .expect_err("unknown");
        assert!(format!("{error:?}").contains("unknown task"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A subagent task cannot be stopped when the turn carries no machinery.
    #[tokio::test]
    async fn cancel_refuses_a_subagent_without_the_machinery() {
        let context = context("cancel-subagent");
        let mut task = Task::open(
            "task-1",
            "Review",
            "Review",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Now,
            Tier::Subagent,
            1,
        )
        .expect("task");
        task.status = TaskStatus::Running;
        let mut attempt = Attempt::queued("task-1-1", Tier::Subagent);
        attempt.run = Some("sub-1".to_string());
        task.attempts.push(attempt);
        FileTasksStore
            .save(&context.workspace, &task)
            .expect("save");
        let error = cancel_with(&Runs::empty(), &context, &json!({"task": "task-1"}))
            .await
            .expect_err("no machinery");
        assert!(format!("{error:?}").contains("subagents are not available"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A peer task needs a target and a schedule, and neither is optional.
    #[tokio::test]
    async fn create_refuses_an_incomplete_peer_task() {
        let context = context("peer-task-refused");
        let no_target = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Audit", "tier": "peer-task", "trigger": {"every_seconds": 3600}}),
        )
        .await
        .expect_err("no target");
        assert!(format!("{no_target:?}").contains("target node"));

        let no_trigger = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Audit", "tier": "peer-task", "target": "node-b"}),
        )
        .await
        .expect_err("no trigger");
        assert!(format!("{no_trigger:?}").contains("needs a schedule"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// With a target and a schedule the refusal comes from the federation
    /// plane (no node here), and nothing is mirrored.
    #[tokio::test]
    async fn create_reports_a_missing_node_for_a_peer_task() {
        let context = context("peer-task-node");
        let error = create_with(
            &Runs::empty(),
            &context,
            &json!({
                "intent": "Audit",
                "tier": "peer-task",
                "target": "node-b",
                "trigger": {"every_seconds": 3600}
            }),
        )
        .await
        .expect_err("no node");
        assert!(format!("{error:?}").contains("federation node is not running"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Cancelling a mirrored task asks its peer, and a missing node is reported.
    #[tokio::test]
    async fn cancel_reports_a_missing_node_for_a_mirror() {
        let context = context("mirror-cancel");
        let mut task = Task::open(
            "task-1",
            "Peer review",
            "Review",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Every {
                every_seconds: 3600,
                last_run_ms: 0,
            },
            Tier::PeerTask,
            1,
        )
        .expect("a peer task");
        task.host = Host::Peer {
            node: "node-b".to_string(),
        };
        FileTasksStore
            .save(&context.workspace, &task)
            .expect("save");
        let error = cancel_with(&Runs::empty(), &context, &json!({"task": "task-1"}))
            .await
            .expect_err("no node");
        assert!(format!("{error:?}").contains("federation node is not running"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A job task needs a command, and the command needs the turn's shell tool.
    #[tokio::test]
    async fn create_refuses_an_incomplete_job_task() {
        let context = context("job-refused");
        let no_command = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "x", "tier": "job"}),
        )
        .await
        .expect_err("no command");
        assert!(format!("{no_command:?}").contains("needs a command"));

        let no_shell = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "x", "tier": "job", "command": "echo hi"}),
        )
        .await
        .expect_err("no shell tool");
        assert!(format!("{no_shell:?}").contains("shell tool is not available"));
        assert!(FileTasksStore
            .list(&context.workspace)
            .expect("list")
            .is_empty());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A scheduled job task is saved for the scheduler instead of starting now.
    #[tokio::test]
    async fn create_saves_a_scheduled_job_task() {
        let context = context("job-scheduled");
        let value = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "x", "tier": "job", "command": "echo hi", "trigger": {"every_seconds": 3600}}),
        )
        .await
        .expect("scheduled job");
        let id = value["task"].as_str().expect("id");
        let stored = FileTasksStore
            .load(&context.workspace, id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Scheduled);
        assert_eq!(stored.command.as_deref(), Some("echo hi"));
        assert!(stored.attempts.is_empty(), "nothing started yet");
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// The job roster's snapshot maps each status.
    #[test]
    fn job_snapshots_map_the_roster() {
        let jobs = agent::jobs::JobRegistry::new();
        let id = jobs.begin("ses-1", "bash", "sleep 1");
        let snapshot = job_snapshot(&jobs, &id).expect("a snapshot");
        assert_eq!(snapshot.state, RunState::Running);
        assert!(job_snapshot(&jobs, "missing").is_none());
    }

    /// status reads a saved record and refuses an unknown id.
    #[tokio::test]
    async fn status_reads_a_record() {
        let context = context("status");
        let runs = Runs::with(Fake::new());
        let value = create_with(&runs, &context, &json!({"intent": "Review"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id");
        let record = status_with(&context, &json!({"task": id})).expect("status");
        assert_eq!(record["id"], id);
        assert_eq!(record["status"], "running");
        assert_eq!(record["attempts"][0]["run"], "run-1");
        assert!(status_with(&context, &json!({"task": "nope"})).is_err());
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// list returns newest first and respects the limit.
    #[tokio::test]
    async fn list_returns_newest_first() {
        let context = context("list");
        for (index, title) in ["a", "b", "c"].iter().enumerate() {
            let id = format!("task-{index}");
            let task = Task::open(
                &id,
                *title,
                "intent",
                Origin {
                    session: "ses-1".to_string(),
                    turn: 0,
                },
                Trigger::Now,
                Tier::Turn,
                index as u64,
            )
            .expect("task");
            FileTasksStore
                .save(&context.workspace, &task)
                .expect("save");
        }
        let list = list_with(&context, &json!({"limit": 2})).expect("list");
        assert_eq!(list["total"], 3);
        assert_eq!(list["tasks"].as_array().expect("rows").len(), 2);
        assert_eq!(list["tasks"][0]["id"], "task-2");
        let filtered = list_with(&context, &json!({"session": "other"})).expect("list");
        assert_eq!(filtered["total"], 0);
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// reconcile marks a local run that died with the process as interrupted,
    /// and leaves a scheduled task alone.
    #[test]
    fn reconcile_marks_dead_local_runs() {
        let context = context("reconcile");
        let mut task = Task::open(
            "task-1",
            "t",
            "i",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Now,
            Tier::Subagent,
            1,
        )
        .expect("task");
        task.status = TaskStatus::Running;
        let mut attempt = Attempt::queued("a1", Tier::Subagent);
        attempt.run = Some("sub-gone".to_string());
        attempt.status = AttemptStatus::Running;
        task.attempts.push(attempt);
        FileTasksStore
            .save(&context.workspace, &task)
            .expect("save");

        let scheduled = Task::open(
            "task-2",
            "s",
            "i",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Every {
                every_seconds: 3_600,
                last_run_ms: 0,
            },
            Tier::Turn,
            1,
        )
        .expect("scheduled");
        FileTasksStore
            .save(&context.workspace, &scheduled)
            .expect("save");

        let subagents = crate::subagent::SubagentRegistry::default();
        let jobs = agent::jobs::JobRegistry::new();
        assert_eq!(reconcile(&context.workspace, &subagents, &jobs), 1);
        let stored = FileTasksStore
            .load(&context.workspace, "task-1")
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Interrupted);
        assert_eq!(
            stored.attempts.last().expect("attempt").status,
            AttemptStatus::Interrupted
        );
        let untouched = FileTasksStore
            .load(&context.workspace, "task-2")
            .expect("load")
            .expect("record");
        assert_eq!(untouched.status, TaskStatus::Scheduled);
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Dependencies are recorded on a scheduled turn task and put it in
    /// Waiting; a now task cannot wait on another.
    #[tokio::test]
    async fn create_records_dependencies_and_waits() {
        let context = context("deps");
        let value = create_with(
            &Runs::empty(),
            &context,
            &json!({
                "intent": "Check",
                "trigger": {"every_seconds": 3600},
                "deps": ["task-a"]
            }),
        )
        .await
        .expect("scheduled");
        let id = value["task"].as_str().expect("id");
        let stored = FileTasksStore
            .load(&context.workspace, id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Waiting);
        assert_eq!(stored.deps, vec!["task-a".to_string()]);
        assert!(
            create_with(
                &Runs::empty(),
                &context,
                &json!({"intent": "x", "deps": ["a"]})
            )
            .await
            .is_err(),
            "a now task cannot wait on another"
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Retry starts a fresh attempt on a settled task.
    #[tokio::test]
    async fn retry_starts_a_new_attempt() {
        let context = context("retry");
        let fake = Fake::new();
        let runs = Runs::with(fake);
        let value = create_with(
            &runs,
            &context,
            &json!({"intent": "Review", "retry_policy": "idempotent"}),
        )
        .await
        .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        let mut record = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        record.status = TaskStatus::Failed;
        record.attempts.last_mut().expect("attempt").status = AttemptStatus::Failed;
        FileTasksStore
            .save(&context.workspace, &record)
            .expect("save");

        let report = retry_with(&runs, &context, &json!({"task": id.clone()}))
            .await
            .expect("retry");
        assert_eq!(report["state"], "running");
        let stored = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Running);
        assert_eq!(stored.attempts.len(), 2);
        assert_eq!(
            stored.attempts.last().expect("attempt").run.as_deref(),
            Some("run-2")
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A live attempt and an exhausted retry budget both refuse.
    #[tokio::test]
    async fn retry_refuses_while_live_or_exhausted() {
        let context = context("retry-refused");
        let runs = Runs::with(Fake::new());
        let value = create_with(&runs, &context, &json!({"intent": "Review"}))
            .await
            .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        assert!(
            retry_with(&runs, &context, &json!({"task": id.clone()}))
                .await
                .is_err(),
            "a live attempt cannot be retried"
        );

        let mut record = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        record.attempts.iter_mut().for_each(|attempt| {
            attempt.status = AttemptStatus::Failed;
        });
        record.attempts.push(Attempt::queued("a2", Tier::Turn));
        record.attempts.push(Attempt::queued("a3", Tier::Turn));
        record.attempts.iter_mut().for_each(|attempt| {
            attempt.status = AttemptStatus::Failed;
        });
        record.status = TaskStatus::Failed;
        FileTasksStore
            .save(&context.workspace, &record)
            .expect("save");
        let error = retry_with(&runs, &context, &json!({"task": id.clone()}))
            .await
            .expect_err("exhausted");
        assert!(format!("{error:?}").contains("all of its attempts"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A delegation records its input, and a retry re-delegates with it rather
    /// than refusing for want of the input.
    #[tokio::test]
    async fn delegation_input_is_recorded_and_retried() {
        let context = context("delegation-input");
        let mut task = Task::open(
            "task-d",
            "t",
            "i",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Now,
            Tier::Delegation,
            1,
        )
        .expect("task");
        task.target = Some("node-b".to_string());
        task.capability = Some("review".to_string());
        task.input = Some(json!({"path": "src"}));
        task.status = TaskStatus::Failed;
        let mut attempt = Attempt::queued("a1", Tier::Delegation);
        attempt.status = AttemptStatus::Failed;
        task.attempts.push(attempt);
        FileTasksStore
            .save(&context.workspace, &task)
            .expect("save");

        let status = status_with(&context, &json!({"task": "task-d"})).expect("status");
        assert_eq!(status["input"]["path"], "src");
        let error = retry_with(&Runs::empty(), &context, &json!({"task": "task-d"}))
            .await
            .expect_err("no node");
        assert!(
            format!("{error:?}").to_lowercase().contains("federation"),
            "the retry reaches the delegation instead of refusing for want of the input"
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// The record whose run ended settles immediately, and an every-trigger
    /// record returns to its schedule.
    #[test]
    fn settle_by_run_closes_the_record() {
        let context = context("settle");
        let task = Task::open(
            "task-1",
            "t",
            "i",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Now,
            Tier::Turn,
            1,
        )
        .expect("task");
        let mut attempt = Attempt::queued("a1", Tier::Turn);
        attempt.run = Some("run-1".to_string());
        attempt.status = AttemptStatus::Running;
        let mut task = task;
        task.status = TaskStatus::Running;
        task.attempts.push(attempt);
        FileTasksStore
            .save(&context.workspace, &task)
            .expect("save");

        assert!(crate::task_tools::settle_by_run(
            std::slice::from_ref(&context.workspace),
            "run-1",
            crate::extension::runs::RunState::Succeeded,
            Some("done".to_string()),
            None,
            7,
        ));
        let stored = FileTasksStore
            .load(&context.workspace, "task-1")
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Succeeded);
        let attempt = stored.attempts.last().expect("attempt");
        assert_eq!(attempt.status, AttemptStatus::Succeeded);
        assert_eq!(attempt.result.as_deref(), Some("done"));
        assert_eq!(attempt.usage.as_ref().map(|usage| usage.tokens), Some(7));

        let mut scheduled = Task::open(
            "task-2",
            "s",
            "i",
            Origin {
                session: "ses-1".to_string(),
                turn: 0,
            },
            Trigger::Every {
                every_seconds: 60,
                last_run_ms: 0,
            },
            Tier::Turn,
            1,
        )
        .expect("scheduled");
        let mut attempt = Attempt::queued("a2", Tier::Turn);
        attempt.run = Some("run-2".to_string());
        attempt.status = AttemptStatus::Running;
        scheduled.status = TaskStatus::Running;
        scheduled.attempts.push(attempt);
        FileTasksStore
            .save(&context.workspace, &scheduled)
            .expect("save");
        crate::task_tools::settle_by_run(
            std::slice::from_ref(&context.workspace),
            "run-2",
            crate::extension::runs::RunState::Failed,
            None,
            Some("boom".to_string()),
            0,
        );
        let stored = FileTasksStore
            .load(&context.workspace, "task-2")
            .expect("load")
            .expect("record");
        assert_eq!(stored.status, TaskStatus::Scheduled);
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// A job task starts a real process and settles through the same helper the
    /// tick uses.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_job_task_starts_and_settles() {
        let mut context = context("job");
        let jobs = std::sync::Arc::new(agent::jobs::JobRegistry::default());
        let bash = std::sync::Arc::new(
            agent::tools::BashTool::new("bash").with_jobs(jobs.clone(), "ses-1".to_string()),
        );
        context.bash = Some(bash as std::sync::Arc<dyn agent::tools::Tool>);
        context.jobs = Some(jobs.clone());
        let value = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "run", "tier": "job", "command": "echo hi"}),
        )
        .await
        .expect("created");
        let run = value["run"].as_str().expect("job id").to_string();
        for _ in 0..100 {
            if jobs
                .job(&run)
                .map(|job| job.status.is_settled())
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let job = jobs.job(&run).expect("the job exists");
        assert!(
            job.status.is_settled(),
            "the process finished: {:?}",
            job.status
        );
        let output = jobs
            .output_for(&run)
            .map(|output| output.text)
            .unwrap_or_default();
        assert!(
            output.contains("hi"),
            "the job captured its output: {output:?}"
        );
        let root = std::path::PathBuf::from(&context.workspace);
        assert!(crate::task_tools::settle_by_run(
            &[root],
            &run,
            crate::extension::runs::RunState::Succeeded,
            Some(output),
            None,
            0,
        ));
        let task_id = value["task"].as_str().expect("task");
        let stored = FileTasksStore
            .load(&context.workspace, task_id)
            .expect("load")
            .expect("record");
        assert!(
            stored
                .attempts
                .last()
                .and_then(|attempt| attempt.result.as_deref())
                .unwrap_or_default()
                .contains("hi"),
            "the attempt reports what the job printed"
        );
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// The default registry keeps the shipped caps, not a derived zero: a
    /// zero cap dropped every job the moment it was registered.
    #[test]
    fn a_default_registry_keeps_its_jobs() {
        use agent::tools::Tool as _;
        let jobs = std::sync::Arc::new(agent::jobs::JobRegistry::default());
        let bash = agent::tools::BashTool::new("bash").with_jobs(jobs.clone(), "ses-1".to_string());
        let response = bash
            .run(&json!({"command": "sleep 2", "run_in_background": true}))
            .expect("started");
        let id = response["job_id"].as_str().expect("id").to_string();
        assert!(jobs.job(&id).is_some(), "the registry holds it");
        jobs.kill(&id);
    }

    /// The graph tool validates the plan before it needs subagents.
    #[tokio::test]
    async fn graph_rejects_a_bad_plan_and_needs_subagents() {
        let context = context("graph");
        let duplicate = graph_with(
            &context,
            &json!({
                "tasks": [
                    {"id": "a", "objective": "x"},
                    {"id": "a", "objective": "y"}
                ]
            }),
        )
        .await
        .expect_err("duplicate");
        assert!(format!("{duplicate:?}").contains("duplicate"));

        let unknown = graph_with(
            &context,
            &json!({
                "tasks": [{"id": "a", "objective": "x", "depends_on": ["nope"]}]
            }),
        )
        .await;
        assert!(unknown.is_err(), "an unknown dependency is refused");

        let cycle = graph_with(
            &context,
            &json!({
                "tasks": [
                    {"id": "a", "objective": "x", "depends_on": ["b"]},
                    {"id": "b", "objective": "y", "depends_on": ["a"]}
                ]
            }),
        )
        .await;
        assert!(cycle.is_err(), "a cycle is refused");

        let no_subagents = graph_with(&context, &json!({"tasks": [{"id": "a", "objective": "x"}]}))
            .await
            .expect_err("no subagents");
        assert!(format!("{no_subagents:?}").contains("subagents are not available"));
    }
    /// The runtime verifier checks artifacts and commands deterministically;
    /// a missing artifact or a non-zero command fails, and an external
    /// approval contract is inconclusive rather than self-attested.
    #[tokio::test]
    async fn the_runtime_verifier_checks_artifacts_and_commands() {
        let root = std::env::temp_dir().join(format!("bsh-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let artifact = root.join("artifact.txt");
        std::fs::write(&artifact, "done").expect("artifact");
        let digest = file_sha256(&artifact).expect("digest");

        let passed = verify_contract(
            &VerificationContract::Artifact {
                path: "artifact.txt".to_string(),
                digest: Some(digest),
            },
            &root,
            None,
        )
        .await;
        assert_eq!(passed.status, VerificationStatus::Passed);
        assert_eq!(passed.verifier.as_deref(), Some("bsh-runtime"));
        assert!(!passed.evidence.is_empty());
        assert!(passed.is_verified());

        let missing = verify_contract(
            &VerificationContract::Artifact {
                path: "missing.txt".to_string(),
                digest: None,
            },
            &root,
            None,
        )
        .await;
        assert_eq!(missing.status, VerificationStatus::Failed);
        assert!(!missing.is_verified());

        #[cfg(unix)]
        {
            // A symlink inside the workspace that points outside it must not
            // widen the verifier's boundary (issue #418).
            let outside =
                std::env::temp_dir().join(format!("bsh-verify-outside-{}", std::process::id()));
            std::fs::create_dir_all(&outside).expect("outside");
            let secret = outside.join("secret.txt");
            std::fs::write(&secret, "secret").expect("secret");
            let link = root.join("escape.txt");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&secret, &link).expect("symlink");
            let escaped = verify_contract(
                &VerificationContract::Artifact {
                    path: "escape.txt".to_string(),
                    digest: None,
                },
                &root,
                None,
            )
            .await;
            assert_eq!(escaped.status, VerificationStatus::Failed);
            assert!(
                escaped.evidence[0].detail.as_deref() == Some("outside the workspace or missing")
            );
            let _ = std::fs::remove_dir_all(&outside);
        }

        let shell: VerificationShell = std::sync::Arc::new(|command: &str| match command {
            "exit 0" => Ok((0, String::new())),
            "exit 3" => Ok((3, String::new())),
            other => Err(format!("unexpected command {other}")),
        });
        let zero = verify_contract(
            &VerificationContract::Command {
                command: "exit 0".to_string(),
            },
            &root,
            Some(&shell),
        )
        .await;
        assert_eq!(zero.status, VerificationStatus::Passed, "{zero:?}");
        let non_zero = verify_contract(
            &VerificationContract::Command {
                command: "exit 3".to_string(),
            },
            &root,
            Some(&shell),
        )
        .await;
        assert_eq!(non_zero.status, VerificationStatus::Failed);
        let unsandboxed = verify_contract(
            &VerificationContract::Command {
                command: "exit 0".to_string(),
            },
            &root,
            None,
        )
        .await;
        assert_eq!(unsandboxed.status, VerificationStatus::Inconclusive);

        // A command that overruns the controlled shell's deadline is stopped
        // by that runtime, not left running behind a timed-out await. This is
        // the same BashTool path normal task execution uses (issue #418).
        use agent::tools::bash::BashToolConfig;
        use agent::Tool as _;
        let bash = agent::tools::BashTool::new("bash")
            .with_config(BashToolConfig {
                timeout_secs: 1,
                ..Default::default()
            })
            .with_workspace(root.to_str().expect("utf-8 workspace"));
        let timed: VerificationShell = std::sync::Arc::new(move |command: &str| {
            let response = bash
                .run(&json!({ "command": command }))
                .map_err(|error| format!("{error:?}"))?;
            let code = response
                .get("exit_code")
                .and_then(Value::as_i64)
                .unwrap_or(-1) as i32;
            Ok((code, String::new()))
        });
        let started = std::time::Instant::now();
        let timeout = verify_contract(
            &VerificationContract::Command {
                command: "sleep 5".to_string(),
            },
            &root,
            Some(&timed),
        )
        .await;
        assert_ne!(timeout.status, VerificationStatus::Passed);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "the verifier returned after the controlled timeout"
        );

        let review = verify_contract(&VerificationContract::Review, &root, None).await;
        assert_eq!(review.status, VerificationStatus::Inconclusive);
        assert!(!review.is_verified());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The task-side reliability scenarios are checked in and the corpus
    /// version is pinned, so the harness cannot silently shrink (issue #423).
    #[test]
    fn task_reliability_corpus_is_checked_in() {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/data/reliability_tasks.json"))
                .expect("corpus parses");
        assert_eq!(corpus["version"].as_u64(), Some(1));
        let ids: Vec<&str> = corpus["scenarios"]
            .as_array()
            .expect("scenarios")
            .iter()
            .map(|scenario| scenario["id"].as_str().expect("id"))
            .collect();
        assert_eq!(
            ids,
            vec![
                "contract_creation_persists",
                "attempt_binding_isolated",
                "sandbox_timeout_stops_command",
                "artifact_symlink_escape_refused",
            ]
        );
    }

    /// Non-idempotent and unknown-side-effect retries require explicit
    /// confirmation; idempotent retries do not (issue #430).
    #[tokio::test]
    async fn retry_policy_guards_repeated_side_effects() {
        let context = context("retry-policy");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(
            &runs,
            &context,
            &json!({"intent": "Deploy", "retry_policy": "non-idempotent"}),
        )
        .await
        .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-1").expect("run");
            snapshot.state = RunState::Succeeded;
        }
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let settled = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(settled.retry_policy, tasks::RetryPolicy::NonIdempotent);
        assert_eq!(
            settled
                .side_effects
                .values()
                .next()
                .map(|effect| effect.status),
            Some(tasks::SideEffectStatus::Completed)
        );

        let denied = retry_with(&runs, &context, &json!({"task": id.clone()}))
            .await
            .expect_err("unconfirmed non-idempotent retry refused");
        assert!(format!("{denied:?}").contains("non-idempotent"));

        retry_with(
            &runs,
            &context,
            &json!({"task": id.clone(), "confirm_side_effects": true}),
        )
        .await
        .expect("confirmed retry");

        let unknown = create_with(&runs, &context, &json!({"intent": "Unknown effect"}))
            .await
            .expect("created");
        let unknown_id = unknown["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            let snapshot = runs.get_mut("run-3").expect("third run");
            snapshot.state = RunState::Succeeded;
        }
        join_with(&runs, &context, &json!({"tasks": [unknown_id.clone()]}))
            .await
            .expect("join");
        let denied = retry_with(
            &runs,
            &context,
            &json!({"task": unknown_id.clone(), "confirm_side_effects": true}),
        )
        .await
        .expect_err("unknown effect needs a compensation path");
        assert!(format!("{denied:?}").contains("compensation"));
        retry_with(
            &runs,
            &context,
            &json!({
                "task": unknown_id,
                "confirm_side_effects": true,
                "compensation": "inspect the external record and roll back"
            }),
        )
        .await
        .expect("compensated retry");
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Every checked-in task scenario is executed through the real task paths
    /// (issue #431), not merely asserted from the fixture.
    #[tokio::test]
    async fn task_reliability_corpus_executes() {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/data/reliability_tasks.json"))
                .expect("corpus parses");
        let context = context("task-corpus");
        let fake = Fake::new();
        let runs = Runs::with(fake);
        for scenario in corpus["scenarios"].as_array().expect("scenarios") {
            match scenario["id"].as_str().expect("id") {
                "contract_creation_persists" => {
                    let value = create_with(
                        &runs,
                        &context,
                        &json!({
                            "intent": "corpus contract",
                            "retry_policy": "idempotent",
                            "verification_contract": {"kind": "artifact", "path": "artifact.txt"}
                        }),
                    )
                    .await
                    .expect("created");
                    let task = FileTasksStore
                        .load(&context.workspace, value["task"].as_str().expect("id"))
                        .expect("load")
                        .expect("record");
                    assert!(task.verification_contract.is_some());
                    assert_eq!(task.verification.status, VerificationStatus::Pending);
                }
                "attempt_binding_isolated" => {
                    let value = create_with(
                        &runs,
                        &context,
                        &json!({
                            "intent": "corpus binding",
                            "retry_policy": "idempotent",
                            "verification_contract": {"kind": "review"}
                        }),
                    )
                    .await
                    .expect("created");
                    let task = FileTasksStore
                        .load(&context.workspace, value["task"].as_str().expect("id"))
                        .expect("load")
                        .expect("record");
                    let attempt = task.attempts.last().expect("attempt");
                    assert_eq!(
                        attempt.verification.attempt_id.as_deref(),
                        Some(attempt.id.as_str())
                    );
                }
                "sandbox_timeout_stops_command" => {
                    let shell: VerificationShell = std::sync::Arc::new(|_| {
                        Err("controlled runtime stopped the command".into())
                    });
                    let report = verify_contract(
                        &VerificationContract::Command {
                            command: "sleep 5".to_string(),
                        },
                        &context.workspace,
                        Some(&shell),
                    )
                    .await;
                    assert_ne!(report.status, VerificationStatus::Passed);
                }
                "artifact_symlink_escape_refused" => {
                    #[cfg(unix)]
                    {
                        std::fs::write(context.workspace.join("artifact.txt"), "done")
                            .expect("artifact");
                        let link = context.workspace.join("escape.txt");
                        let _ = std::fs::remove_file(&link);
                        std::os::unix::fs::symlink("/etc/passwd", &link).expect("symlink");
                        let report = verify_contract(
                            &VerificationContract::Artifact {
                                path: "escape.txt".to_string(),
                                digest: None,
                            },
                            &context.workspace,
                            None,
                        )
                        .await;
                        assert_eq!(report.status, VerificationStatus::Failed);
                    }
                }
                other => panic!("unhandled corpus scenario {other}"),
            }
        }
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// Review/human approvals bind to one attempt, can be revoked/expired,
    /// and never verify a later retry (issue #433).
    #[tokio::test]
    async fn approvals_verify_only_their_attempt() {
        let context = context("approvals");
        let fake = Fake::new();
        let runs = Runs::with(fake.clone());
        let value = create_with(
            &runs,
            &context,
            &json!({
                "intent": "Approve the release",
                "retry_policy": "idempotent",
                "verification_contract": {"kind": "human"}
            }),
        )
        .await
        .expect("created");
        let id = value["task"].as_str().expect("id").to_string();
        {
            let mut runs = fake.runs.lock().expect("runs");
            runs.get_mut("run-1").expect("run").state = RunState::Succeeded;
        }
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(task.verification.status, VerificationStatus::Inconclusive);

        let error = approve_with(
            &context,
            &json!({"task": id, "approver": "model", "evidence": "x", "reference": "r"}),
        )
        .expect_err("model cannot self-approve");
        assert!(format!("{error:?}").contains("cannot approve"));

        let approved = approve_with(
            &context,
            &json!({
                "task": id,
                "approver": "alice",
                "decision": "approved",
                "scope": id,
                "evidence": "reviewed the artifact",
                "reference": "approval-1"
            }),
        )
        .expect("approval recorded");
        let attempt_one = approved["approvals"][0]["attempt_id"]
            .as_str()
            .expect("attempt")
            .to_string();
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(task.verification.status, VerificationStatus::Passed);
        assert!(task.verified_completion());

        retry_with(&runs, &context, &json!({"task": id.clone()}))
            .await
            .expect("retry");
        {
            let mut runs = fake.runs.lock().expect("runs");
            runs.get_mut("run-2").expect("second run").state = RunState::Succeeded;
        }
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(
            task.verification.status,
            VerificationStatus::Inconclusive,
            "attempt one's approval cannot verify attempt two"
        );
        let attempt_two = task.attempts.last().expect("attempt").id.clone();
        approve_with(
            &context,
            &json!({
                "task": id,
                "attempt_id": attempt_two,
                "approver": "alice",
                "decision": "approved",
                "evidence": "reviewed the retry",
                "reference": "approval-2"
            }),
        )
        .expect("second approval");
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(task.verification.status, VerificationStatus::Passed);
        assert_eq!(task.approvals.len(), 2, "audit history retained");

        approve_with(
            &context,
            &json!({"task": id, "revoke": true, "reference": "approval-2"}),
        )
        .expect("revoked");
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(task.verification.status, VerificationStatus::Inconclusive);
        assert!(
            task.approvals
                .iter()
                .any(|approval| approval.revoked_ms.is_some()),
            "revocation is audited"
        );

        approve_with(
            &context,
            &json!({
                "task": id,
                "attempt_id": task.attempts.last().expect("attempt").id,
                "approver": "alice",
                "decision": "approved",
                "evidence": "expired review",
                "reference": "approval-3",
                "expires_ms": 1
            }),
        )
        .expect("expired approval recorded");
        join_with(&runs, &context, &json!({"tasks": [id.clone()]}))
            .await
            .expect("join");
        let task = FileTasksStore
            .load(&context.workspace, &id)
            .expect("load")
            .expect("record");
        assert_eq!(
            task.verification.status,
            VerificationStatus::Inconclusive,
            "an expired approval cannot verify"
        );
        assert_eq!(task.approvals.len(), 3, "approval history survives restart");
        let _ = attempt_one;
        let _ = std::fs::remove_dir_all(&context.workspace);
    }

    /// An unknown id is a refusal, not an empty report.
    #[tokio::test]
    async fn join_refuses_unknown_ids() {
        let context = context("unknown");
        let error = join_with(&Runs::empty(), &context, &json!({"tasks": ["nope"]}))
            .await
            .expect_err("unknown");
        assert!(format!("{error:?}").contains("unknown task"));
        let _ = std::fs::remove_dir_all(&context.workspace);
    }
}
