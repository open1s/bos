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
    self, Attempt, AttemptStatus, FileTasksStore, Host, Origin, Task, TaskStatus, TasksStore, Tier,
    Trigger,
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
}

/// Attach the tools to an agent.
pub(crate) fn attach(agent: &mut Agent, context: TaskToolsContext) {
    attach_create(agent, context.clone());
    attach_join(agent, context.clone());
    attach_cancel(agent, context);
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
                    "enum": ["turn", "subagent", "delegation", "peer-task"],
                    "description": "How the task runs. turn starts a turn in this session; subagent delegates to a child agent; delegation sends the objective to a peer node for one capability; peer-task asks a peer to own and schedule the task on its own clock."
                },
                "target": {
                    "type": "string",
                    "description": "For a delegation: the peer node id, as federation_peers reports it."
                },
                "capability": {
                    "type": "string",
                    "description": "For a delegation: the capability the peer runs, as its grants report it."
                },
                "input": {
                    "type": "object",
                    "description": "For a delegation: the capability's own input object."
                },
                "trigger": {
                    "type": "object",
                    "description": "Omit to run now. every_seconds repeats the task; at_ms runs it once at an absolute unix time in milliseconds.",
                    "properties": {
                        "every_seconds": { "type": "integer", "minimum": 60 },
                        "at_ms": { "type": "integer", "minimum": 0 }
                    }
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
                return Err(ToolError::Failed(
                    "stopping a job task is not wired yet".to_string(),
                ));
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
    let tier = match args.get("tier").and_then(Value::as_str).unwrap_or("turn") {
        "turn" => Tier::Turn,
        "subagent" => Tier::Subagent,
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
    let id = tasks::new_id(now);
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
    if !matches!(trigger, Trigger::Now) {
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "state": "scheduled",
            "next_fire_ms": task.trigger.next_fire_ms(),
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
        task.status = TaskStatus::Running;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "run": run,
            "state": "running",
        }));
    }
    if tier == Tier::Delegation {
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
        let Some(capability) = args
            .get("capability")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|capability| !capability.is_empty())
        else {
            return Err(ToolError::InvalidInput(
                "a delegated task needs a capability; the peer executes one exposed capability"
                    .to_string(),
            ));
        };
        let payload = args.get("input").cloned().unwrap_or(Value::Null);
        let record =
            crate::federation_tools::delegate_record(target, intent, Some(capability), payload)
                .await?;
        let run = record.request.id.to_string();
        task.target = Some(target.to_string());
        task.capability = Some(capability.to_string());
        let mut attempt = Attempt::queued(format!("{id}-1"), Tier::Delegation);
        attempt.run = Some(run.clone());
        attempt.status = AttemptStatus::Running;
        attempt.started_ms = Some(now);
        task.attempts.push(attempt);
        task.status = TaskStatus::Running;
        task.updated_ms = now;
        FileTasksStore
            .save(&context.workspace, &task)
            .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
        return Ok(json!({
            "task": id,
            "run": run,
            "state": "running",
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
    task.status = TaskStatus::Running;
    task.updated_ms = now;
    FileTasksStore
        .save(&context.workspace, &task)
        .map_err(|error| ToolError::Failed(format!("the task could not be saved: {error}")))?;
    Ok(json!({
        "task": id,
        "run": run.id,
        "state": "running",
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
    };
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

/// The local mirror status for a peer task state.
fn mirror_status(state: federation::FederatedTaskState) -> TaskStatus {
    match state {
        federation::FederatedTaskState::Queued => TaskStatus::Scheduled,
        federation::FederatedTaskState::Running => TaskStatus::Running,
        federation::FederatedTaskState::Succeeded => TaskStatus::Succeeded,
        federation::FederatedTaskState::Failed => TaskStatus::Failed,
        federation::FederatedTaskState::Cancelled => TaskStatus::Cancelled,
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
            &json!({"intent": "Review", "tier": "job"}),
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

        let missing_capability = create_with(
            &Runs::empty(),
            &context,
            &json!({"intent": "Audit", "tier": "delegation", "target": "node-b"}),
        )
        .await
        .expect_err("no capability");
        assert!(format!("{missing_capability:?}").contains("capability"));

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
