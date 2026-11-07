//! Durable task-graph orchestration above delegation.
//!
//! Delegation is the primitive: one bounded request to one Node. This module
//! adds the layer above it — a dependency graph, deterministic ready order,
//! bounded parallel execution, persisted transitions, and restart recovery.
//! A task graph belongs to one Node; it never merges another Node's authority
//! or state, and a failed dependency cancels only the work that depended on it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::delegation::DelegationId;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The task-graph schema this build writes.
pub const COLLABORATION_SCHEMA: u32 = 1;

/// A durable task identity inside one graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskNodeId(String);

impl TaskNodeId {
    /// Build a task id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskNodeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The state of one task in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    /// Not started; dependencies may still be running.
    Pending,
    /// Executing.
    Running,
    /// Finished with a result.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled, or skipped because a dependency failed.
    Cancelled,
}

/// One node of a collaboration graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskNode {
    /// Stable id.
    pub id: TaskNodeId,
    /// What the task is for.
    pub objective: String,
    /// Tasks that must succeed before this one becomes ready.
    #[serde(default)]
    pub depends_on: Vec<TaskNodeId>,
    /// Current state.
    pub state: TaskState,
    /// The result once succeeded.
    pub result: Option<Value>,
    /// The failure or cancellation reason.
    pub error: Option<String>,
    /// How many attempts have started.
    #[serde(default)]
    pub attempts: u32,
    /// Maximum attempts before the task is failed for good.
    #[serde(default = "one_attempt")]
    pub max_attempts: u32,
    /// The acceptance criterion the host will evaluate on the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    /// Artifacts produced by the successful attempt.
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
    /// What to do when the task fails for good.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<String>,
    /// Whether compensation ran.
    #[serde(default)]
    pub compensated: bool,
    /// Why compensation failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_error: Option<String>,
}

fn one_attempt() -> u32 {
    1
}

/// One artifact a task produced or consumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Display name.
    pub name: String,
    /// URI or path the host resolves; it is a reference, not the bytes.
    pub uri: String,
    /// Hex SHA-256 of the artifact, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl ArtifactRef {
    /// A reference with no integrity or size metadata.
    pub fn new(name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uri: uri.into(),
            sha256: None,
            bytes: None,
        }
    }
}

/// The result of one executed task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOutcome {
    /// The bounded result value.
    pub result: Value,
    /// Artifacts the task produced.
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

impl TaskOutcome {
    /// A result with no artifacts.
    pub fn value(result: Value) -> Self {
        Self {
            result,
            artifacts: Vec::new(),
        }
    }
}

impl TaskNode {
    /// Whether all dependencies are satisfied for this task.
    pub fn ready(&self, tasks: &BTreeMap<TaskNodeId, TaskNode>) -> bool {
        self.state == TaskState::Pending
            && self.depends_on.iter().all(|dependency| {
                tasks
                    .get(dependency)
                    .is_some_and(|task| task.state == TaskState::Succeeded)
            })
    }
}

/// A versioned, serializable task graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskGraph {
    /// Schema version; only COLLABORATION_SCHEMA is accepted.
    pub schema: u32,
    /// Mutation counter, for a writer deciding whether its file is stale.
    #[serde(default)]
    pub revision: u64,
    /// Tasks, keyed by id.
    pub tasks: BTreeMap<TaskNodeId, TaskNode>,
}

impl Default for TaskGraph {
    fn default() -> Self {
        Self {
            schema: COLLABORATION_SCHEMA,
            revision: 0,
            tasks: BTreeMap::new(),
        }
    }
}

impl TaskGraph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pending task. Dependencies are validated when the graph is run or
    /// serialized, so a forward reference is legal during construction.
    pub fn add(
        &mut self,
        id: impl Into<TaskNodeId>,
        objective: impl Into<String>,
        depends_on: Vec<TaskNodeId>,
    ) -> Result<(), CollaborationError> {
        let id = id.into();
        if self.tasks.contains_key(&id) {
            return Err(CollaborationError::Duplicate { id });
        }
        let mut depends_on = depends_on;
        depends_on.sort();
        depends_on.dedup();
        self.tasks.insert(
            id.clone(),
            TaskNode {
                id,
                objective: objective.into(),
                depends_on,
                state: TaskState::Pending,
                result: None,
                error: None,
                attempts: 0,
                max_attempts: 1,
                acceptance: None,
                artifacts: Vec::new(),
                compensation: None,
                compensated: false,
                compensation_error: None,
            },
        );
        self.revision += 1;
        Ok(())
    }

    /// Set the retry ceiling for one task.
    pub fn set_max_attempts(
        &mut self,
        id: &TaskNodeId,
        max_attempts: u32,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.max_attempts = max_attempts.max(1);
        self.revision += 1;
        Ok(())
    }

    /// Attach a compensation instruction to one task.
    pub fn set_compensation(
        &mut self,
        id: &TaskNodeId,
        compensation: impl Into<String>,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.compensation = Some(compensation.into());
        self.revision += 1;
        Ok(())
    }

    /// Record the outcome of a compensation attempt.
    pub fn set_compensated(
        &mut self,
        id: &TaskNodeId,
        error: Option<String>,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.compensated = true;
        task.compensation_error = error;
        self.revision += 1;
        Ok(())
    }

    /// Attach an acceptance criterion to one task.
    pub fn set_acceptance(
        &mut self,
        id: &TaskNodeId,
        acceptance: impl Into<String>,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.acceptance = Some(acceptance.into());
        self.revision += 1;
        Ok(())
    }

    /// Reject unknown dependencies and cycles.
    pub fn validate(&self) -> Result<(), CollaborationError> {
        if self.schema != COLLABORATION_SCHEMA {
            return Err(CollaborationError::Schema(self.schema));
        }
        for task in self.tasks.values() {
            for dependency in &task.depends_on {
                if !self.tasks.contains_key(dependency) {
                    return Err(CollaborationError::UnknownDependency {
                        task: task.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
        let mut remaining: BTreeMap<&TaskNodeId, BTreeSet<&TaskNodeId>> = self
            .tasks
            .values()
            .map(|task| (&task.id, task.depends_on.iter().collect()))
            .collect();
        while let Some((id, _)) = remaining
            .iter()
            .find(|(_, dependencies)| dependencies.is_empty())
            .map(|(id, _)| (*id, ()))
        {
            remaining.remove(id);
            for dependencies in remaining.values_mut() {
                dependencies.remove(id);
            }
        }
        if !remaining.is_empty() {
            return Err(CollaborationError::Cycle {
                tasks: remaining.into_keys().cloned().collect(),
            });
        }
        Ok(())
    }

    /// The deterministic topological order.
    pub fn order(&self) -> Result<Vec<TaskNodeId>, CollaborationError> {
        self.validate()?;
        let mut remaining: BTreeMap<TaskNodeId, BTreeSet<TaskNodeId>> = self
            .tasks
            .values()
            .map(|task| (task.id.clone(), task.depends_on.iter().cloned().collect()))
            .collect();
        let mut ready: BTreeSet<TaskNodeId> = remaining
            .iter()
            .filter(|(_, dependencies)| dependencies.is_empty())
            .map(|(id, _)| id.clone())
            .collect();
        let mut order = Vec::with_capacity(self.tasks.len());
        while let Some(id) = ready.pop_first() {
            for dependencies in remaining.values_mut() {
                dependencies.remove(&id);
            }
            remaining.remove(&id);
            ready.extend(
                remaining
                    .iter()
                    .filter(|(_, dependencies)| dependencies.is_empty())
                    .map(|(id, _)| id.clone()),
            );
            order.push(id);
        }
        Ok(order)
    }

    /// Pending tasks whose dependencies all succeeded, in id order.
    pub fn ready(&self) -> Vec<TaskNodeId> {
        self.tasks
            .values()
            .filter(|task| task.ready(&self.tasks))
            .map(|task| task.id.clone())
            .collect()
    }

    /// Mark one ready task running.
    pub fn start(&mut self, id: &TaskNodeId) -> Result<(), CollaborationError> {
        let ready = self.ready();
        if !ready.iter().any(|candidate| candidate == id) {
            return Err(CollaborationError::NotReady { id: id.clone() });
        }
        if let Some(task) = self.tasks.get_mut(id) {
            task.state = TaskState::Running;
            task.attempts = task.attempts.saturating_add(1);
            task.error = None;
            self.revision += 1;
        }
        Ok(())
    }

    /// Mark a running task succeeded and store its bounded result.
    pub fn succeed(
        &mut self,
        id: &TaskNodeId,
        outcome: TaskOutcome,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        if task.state != TaskState::Running {
            return Err(CollaborationError::InvalidState {
                id: id.clone(),
                state: task.state,
            });
        }
        task.state = TaskState::Succeeded;
        task.result = Some(outcome.result);
        task.artifacts = outcome.artifacts;
        task.error = None;
        self.revision += 1;
        Ok(())
    }

    /// Mark a running task failed and cancel every pending dependent.
    pub fn fail(
        &mut self,
        id: &TaskNodeId,
        reason: impl Into<String>,
    ) -> Result<bool, CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        if task.state != TaskState::Running {
            return Err(CollaborationError::InvalidState {
                id: id.clone(),
                state: task.state,
            });
        }
        let reason = reason.into();
        if task.attempts < task.max_attempts {
            task.state = TaskState::Pending;
            task.error = Some(reason);
            self.revision += 1;
            return Ok(false);
        }
        task.state = TaskState::Failed;
        task.error = Some(reason);
        self.revision += 1;
        self.cancel_dependents(id);
        Ok(true)
    }

    /// Cancel a task and every pending task that depends on it.
    pub fn cancel(&mut self, id: &TaskNodeId) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        match task.state {
            TaskState::Pending | TaskState::Running => {
                task.state = TaskState::Cancelled;
                task.error = Some("cancelled".to_string());
                self.revision += 1;
                self.cancel_dependents(id);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn cancel_dependents(&mut self, id: &TaskNodeId) {
        let mut stack = vec![id.clone()];
        while let Some(dependency) = stack.pop() {
            let dependents: Vec<TaskNodeId> = self
                .tasks
                .values()
                .filter(|task| {
                    task.state == TaskState::Pending && task.depends_on.contains(&dependency)
                })
                .map(|task| task.id.clone())
                .collect();
            for dependent in dependents {
                if let Some(task) = self.tasks.get_mut(&dependent) {
                    task.state = TaskState::Cancelled;
                    task.error = Some(format!("dependency {dependency} did not succeed"));
                    self.revision += 1;
                }
                stack.push(dependent);
            }
        }
    }

    /// Mark tasks left Running by a restart as Failed with an indeterminate
    /// reason, then cancel their pending dependents. Idempotent.
    pub fn recover(&mut self) -> usize {
        let running: Vec<TaskNodeId> = self
            .tasks
            .values()
            .filter(|task| task.state == TaskState::Running)
            .map(|task| task.id.clone())
            .collect();
        for id in &running {
            if let Some(task) = self.tasks.get_mut(id) {
                task.state = TaskState::Failed;
                task.error =
                    Some("interrupted by a Node restart; the outcome is indeterminate".to_string());
                self.revision += 1;
            }
            self.cancel_dependents(id);
        }
        running.len()
    }

    /// Whether every task is terminal.
    pub fn is_complete(&self) -> bool {
        self.tasks.values().all(|task| {
            matches!(
                task.state,
                TaskState::Succeeded | TaskState::Failed | TaskState::Cancelled
            )
        })
    }

    /// Counts by state.
    pub fn progress(&self) -> TaskProgress {
        let mut progress = TaskProgress::default();
        for task in self.tasks.values() {
            match task.state {
                TaskState::Pending => progress.pending += 1,
                TaskState::Running => progress.running += 1,
                TaskState::Succeeded => progress.succeeded += 1,
                TaskState::Failed => progress.failed += 1,
                TaskState::Cancelled => progress.cancelled += 1,
            }
        }
        progress
    }

    /// Serialize the graph.
    pub fn to_json(&self) -> std::io::Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Read a graph from a file; a missing file is empty.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut graph: Self = serde_json::from_str(&text)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                graph.recover();
                Ok(graph)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(error) => Err(error),
        }
    }

    /// Write the graph atomically, private to the account.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, self.to_json()?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path)
    }
}

/// State counts for a graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProgress {
    /// Not started.
    pub pending: usize,
    /// Running.
    pub running: usize,
    /// Succeeded.
    pub succeeded: usize,
    /// Failed.
    pub failed: usize,
    /// Cancelled or skipped.
    pub cancelled: usize,
}

/// The executor a graph runs each ready task through.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Perform one task and return its bounded result and artifacts.
    async fn execute(&self, task: &TaskNode) -> Result<TaskOutcome, String>;

    /// Undo or settle side effects after a final failure. The default does
    /// nothing; a host that performs external side effects should override it.
    async fn compensate(&self, _task: &TaskNode) -> Result<(), String> {
        Ok(())
    }
}

/// Why a graph operation was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CollaborationError {
    /// Unsupported schema.
    #[error("unsupported collaboration schema {0}")]
    Schema(u32),
    /// A duplicated task id.
    #[error("task {id} already exists")]
    Duplicate {
        /// The id.
        id: TaskNodeId,
    },
    /// A dependency names no task.
    #[error("task {task} depends on unknown {dependency}")]
    UnknownDependency {
        /// The task.
        task: TaskNodeId,
        /// The missing dependency.
        dependency: TaskNodeId,
    },
    /// A cycle remains.
    #[error("collaboration cycle among {tasks:?}")]
    Cycle {
        /// Remaining tasks.
        tasks: Vec<TaskNodeId>,
    },
    /// The task is unknown.
    #[error("task {id} is unknown")]
    UnknownTask {
        /// The id.
        id: TaskNodeId,
    },
    /// The task is not ready to start.
    #[error("task {id} is not ready")]
    NotReady {
        /// The id.
        id: TaskNodeId,
    },
    /// The task is in the wrong state for the transition.
    #[error("task {id} is {state:?}")]
    InvalidState {
        /// The id.
        id: TaskNodeId,
        /// The state.
        state: TaskState,
    },
    /// The graph file could not be read or written.
    #[error("collaboration storage failed: {0}")]
    Storage(String),
}

/// A task executor that delegates each task through a FederationNode.
///
/// The executor submits the task as a delegation, polls its authoritative
/// status, and maps a terminal record back to a task outcome. A structured
/// result contract asks the target to run one capability; a prose contract
/// leaves the task queued, and the poll eventually times out.
pub struct DelegationExecutor {
    node: Arc<crate::service::FederationNode>,
    target: capability::NodeId,
    capabilities: Vec<capability::CapabilityId>,
    contract: Option<String>,
    context: capability::InvocationContext,
    poll: Duration,
}

impl DelegationExecutor {
    /// Build an executor for one target and one delegated capability scope.
    pub fn new(
        node: Arc<crate::service::FederationNode>,
        target: capability::NodeId,
        capabilities: Vec<capability::CapabilityId>,
        context: capability::InvocationContext,
    ) -> Self {
        Self {
            node,
            target,
            capabilities,
            contract: None,
            context,
            poll: Duration::from_millis(25),
        }
    }

    /// Ask the target to run this JSON result contract.
    pub fn with_contract(mut self, contract: impl Into<String>) -> Self {
        self.contract = Some(contract.into());
        self
    }

    /// Change the status poll interval.
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll.max(Duration::from_millis(5));
        self
    }

    fn request_id(task: &TaskNode) -> DelegationId {
        DelegationId::new(format!("task-{}-{}", task.id, task.attempts))
    }
}

#[async_trait]
impl TaskExecutor for DelegationExecutor {
    async fn execute(&self, task: &TaskNode) -> Result<TaskOutcome, String> {
        let id = Self::request_id(task);
        let deadline_ms = self.context.deadline.map(|deadline| {
            capability::unix_ms().saturating_add(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis() as u64,
            )
        });
        let request = crate::delegation::DelegationRequest {
            id: id.clone(),
            parent: None,
            target: self.target.clone(),
            objective: task.objective.clone(),
            capabilities: self.capabilities.clone(),
            deadline_ms,
            allow_sub_delegation: false,
            result_contract: self.contract.clone(),
        };
        let accepted = self
            .node
            .delegate(&self.target, request, &self.context)
            .await
            .map_err(|error| error.to_string())?;
        let mut state = accepted.state;
        let deadline = self
            .context
            .deadline
            .unwrap_or_else(|| std::time::Instant::now() + Duration::from_secs(30));
        loop {
            if state.is_terminal() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "delegation {id} did not finish before the deadline"
                ));
            }
            tokio::time::sleep(self.poll).await;
            let record = self
                .node
                .delegation_status(&self.target, &id, &self.context)
                .await
                .map_err(|error| error.to_string())?;
            state = record.state;
            if state == crate::delegation::DelegationState::Succeeded {
                return Ok(TaskOutcome {
                    result: record.result.unwrap_or(Value::Null),
                    artifacts: Vec::new(),
                });
            }
            if state == crate::delegation::DelegationState::Failed
                || state == crate::delegation::DelegationState::Cancelled
                || state == crate::delegation::DelegationState::Rejected
            {
                return Err(record
                    .error
                    .unwrap_or_else(|| format!("delegation {id} ended {state:?}")));
            }
        }
        Err(format!("delegation {id} ended {state:?}"))
    }

    async fn compensate(&self, task: &TaskNode) -> Result<(), String> {
        let id = Self::request_id(task);
        self.node
            .cancel_delegation(&self.target, &id, &self.context)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// A bounded, deterministic runner for a TaskGraph.
pub struct Orchestrator {
    graph: TaskGraph,
    path: Option<std::path::PathBuf>,
}

impl Orchestrator {
    /// A runner over an in-memory graph.
    pub fn new(graph: TaskGraph) -> Self {
        Self { graph, path: None }
    }

    /// A runner that persists every transition to a file.
    pub fn with_persistence(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The graph.
    pub fn graph(&self) -> &TaskGraph {
        &self.graph
    }

    /// Mutable graph access, for a host that advances a task itself.
    pub fn graph_mut(&mut self) -> &mut TaskGraph {
        &mut self.graph
    }

    fn persist(&self) -> Result<(), CollaborationError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        self.graph
            .save(path)
            .map_err(|error| CollaborationError::Storage(error.to_string()))
    }

    /// Run ready tasks with bounded concurrency until the graph is quiescent.
    ///
    /// A task failure marks its pending dependents cancelled; a panic in one
    /// executor becomes a failed task rather than aborting the graph.
    pub async fn run(
        &mut self,
        executor: Arc<dyn TaskExecutor>,
        concurrency: usize,
    ) -> Result<TaskProgress, CollaborationError> {
        self.graph.validate()?;
        self.persist()?;
        let mut joins = tokio::task::JoinSet::new();
        loop {
            while joins.len() < concurrency.max(1) {
                let Some(id) = self.graph.ready().into_iter().next() else {
                    break;
                };
                self.graph.start(&id)?;
                self.persist()?;
                let Some(task) = self.graph.tasks.get(&id).cloned() else {
                    continue;
                };
                let executor = Arc::clone(&executor);
                joins.spawn(async move {
                    let result = executor.execute(&task).await;
                    (id, result)
                });
            }
            let Some(joined) = joins.join_next().await else {
                break;
            };
            match joined {
                Ok((id, Ok(result))) => {
                    self.graph.succeed(&id, result)?;
                }
                Ok((id, Err(error))) => {
                    let task = self.graph.tasks.get(&id).cloned();
                    if self.graph.fail(&id, error)? {
                        if let Some(task) = task {
                            if task.compensation.is_some() {
                                let compensation = executor.compensate(&task).await;
                                self.graph.set_compensated(&id, compensation.err())?;
                            }
                        }
                    }
                }
                Err(error) => {
                    // A panicking executor leaves no id in the JoinError, so
                    // every running task is failed conservatively.
                    let running: Vec<TaskNodeId> = self
                        .graph
                        .tasks
                        .values()
                        .filter(|task| task.state == TaskState::Running)
                        .map(|task| task.id.clone())
                        .collect();
                    for id in running {
                        self.graph
                            .fail(&id, format!("executor task failed: {error}"))?;
                    }
                }
            }
            self.persist()?;
        }
        Ok(self.graph.progress())
    }
}
