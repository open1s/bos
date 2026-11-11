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

use crate::compensation::{CompensationQueue, PendingCompensation};
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
    /// The acceptance verdict, once evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_result: Option<AcceptanceReport>,
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
    /// When the task reached a terminal state, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    /// Default artifact retention for this task, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_retention_ms: Option<u64>,
    /// The remote acknowledgement of compensation, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation_receipt: Option<Value>,
}

/// The evaluator that decides a task's acceptance criterion.
pub const ACCEPTANCE_EVALUATOR: &str = "bsh.acceptance/1";

/// What an acceptance evaluation decided, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceReport {
    /// The evaluator's own identity, so a verdict is attributable.
    pub evaluator: String,
    /// The criterion exactly as evaluated.
    pub criterion: String,
    /// Whether the result was accepted.
    pub accepted: bool,
    /// Why it was rejected, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Evaluate TaskNode.acceptance against a result.
///
/// The vocabulary is deliberately small and deterministic, so a task is never
/// marked complete by an opaque string nobody evaluates:
///
/// - non_empty: the result's text is not blank
/// - json_object: the result is a JSON object
/// - json_array: the result is a JSON array
/// - `contains:<text>`: the result's text contains the text
/// - `equals:<text>`: the result's text equals the text
/// - `sha256:<hex>`: the result's text hashes to the hex digest
///
/// Anything else is rejected as malformed. A criterion that cannot be evaluated
/// must never read as success.
pub fn evaluate_acceptance(criterion: &str, result: &Value) -> AcceptanceReport {
    let criterion = criterion.trim();
    let text = result
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| result.to_string());
    let verdict = if criterion.is_empty() {
        Err("the criterion is empty".to_string())
    } else {
        match criterion {
            "non_empty" => {
                if text.trim().is_empty() {
                    Err("the result is empty".to_string())
                } else {
                    Ok(())
                }
            }
            "json_object" => {
                if result.is_object() {
                    Ok(())
                } else {
                    Err("the result is not a JSON object".to_string())
                }
            }
            "json_array" => {
                if result.is_array() {
                    Ok(())
                } else {
                    Err("the result is not a JSON array".to_string())
                }
            }
            _ => {
                if let Some(needle) = criterion.strip_prefix("contains:") {
                    if text.contains(needle) {
                        Ok(())
                    } else {
                        Err(format!("the result does not contain {needle:?}"))
                    }
                } else if let Some(expected) = criterion.strip_prefix("equals:") {
                    if text == expected {
                        Ok(())
                    } else {
                        Err("the result does not equal the expectation".to_string())
                    }
                } else if let Some(expected) = criterion.strip_prefix("sha256:") {
                    let actual = sha256_hex(text.as_bytes());
                    if actual.eq_ignore_ascii_case(expected.trim()) {
                        Ok(())
                    } else {
                        Err(format!("the result hashes to {actual}"))
                    }
                } else {
                    Err(format!("unknown acceptance criterion {criterion:?}"))
                }
            }
        }
    };
    AcceptanceReport {
        evaluator: ACCEPTANCE_EVALUATOR.to_string(),
        criterion: criterion.to_string(),
        accepted: verdict.is_ok(),
        detail: verdict.err(),
    }
}

/// The lower-case hex SHA-256 of the bytes.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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
    /// Retain this artifact for this many milliseconds after the task finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_ms: Option<u64>,
}

impl ArtifactRef {
    /// A reference with no integrity or size metadata.
    pub fn new(name: impl Into<String>, uri: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uri: uri.into(),
            sha256: None,
            bytes: None,
            retention_ms: None,
        }
    }

    /// Set this artifact's retention window.
    pub fn with_retention(mut self, retention_ms: u64) -> Self {
        self.retention_ms = Some(retention_ms);
        self
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
                acceptance_result: None,
                artifacts: Vec::new(),
                compensation: None,
                compensated: false,
                compensation_error: None,
                finished_at_ms: None,
                artifact_retention_ms: None,
                compensation_receipt: None,
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
        receipt: Option<Value>,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.compensated = true;
        task.compensation_error = error;
        task.compensation_receipt = receipt;
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
        task.finished_at_ms = Some(capability::unix_ms());
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
        task.finished_at_ms = Some(capability::unix_ms());
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
                task.finished_at_ms = Some(capability::unix_ms());
                self.revision += 1;
            }
            self.cancel_dependents(id);
        }
        running.len()
    }

    /// Set the default artifact retention window for one task.
    pub fn set_artifact_retention(
        &mut self,
        id: &TaskNodeId,
        retention_ms: u64,
    ) -> Result<(), CollaborationError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| CollaborationError::UnknownTask { id: id.clone() })?;
        task.artifact_retention_ms = Some(retention_ms);
        self.revision += 1;
        Ok(())
    }

    /// Drop artifact references whose retention window has elapsed.
    ///
    /// Results are kept; only the reference (not the host's stored bytes) is
    /// removed. An artifact with no retention and no task default is kept.
    pub fn prune_artifacts(&mut self, now_ms: u64) -> usize {
        self.take_expired_artifacts(now_ms).len()
    }

    /// The artifact references whose retention elapsed, without removing them.
    pub fn expired_artifacts(&self, now_ms: u64) -> Vec<ArtifactRef> {
        let mut expired = Vec::new();
        for task in self.tasks.values() {
            let Some(finished) = task.finished_at_ms else {
                continue;
            };
            let default = task.artifact_retention_ms;
            for artifact in &task.artifacts {
                let retention = artifact.retention_ms.or(default);
                if retention.is_some_and(|retention| finished.saturating_add(retention) <= now_ms) {
                    expired.push(artifact.clone());
                }
            }
        }
        expired
    }

    /// Remove and return the artifact references whose retention elapsed.
    ///
    /// A caller that has a peer artifact store can send these to
    /// `FederationNode::cleanup_artifacts` instead of dropping them silently.
    pub fn take_expired_artifacts(&mut self, now_ms: u64) -> Vec<ArtifactRef> {
        let mut removed = Vec::new();
        for task in self.tasks.values_mut() {
            let Some(finished) = task.finished_at_ms else {
                continue;
            };
            let default = task.artifact_retention_ms;
            task.artifacts.retain(|artifact| {
                let retention = artifact.retention_ms.or(default);
                let keep =
                    retention.is_none_or(|retention| finished.saturating_add(retention) > now_ms);
                if !keep {
                    removed.push(artifact.clone());
                }
                keep
            });
        }
        if !removed.is_empty() {
            self.revision += 1;
        }
        removed
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

    /// Evidence that compensation happened, stored for reconciliation.
    fn compensation_evidence(&self, _task: &TaskNode) -> Option<Value> {
        None
    }

    /// Whether this executor can delete artifact bytes, not just references.
    fn supports_cleanup(&self) -> bool {
        false
    }

    /// Remove the bytes behind expired references. Only called when supported.
    async fn cleanup_artifacts(&self, _artifacts: &[ArtifactRef]) -> Result<(), String> {
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

/// One task whose stored compensation receipt disagrees with the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptMismatch {
    /// The task whose receipt was checked.
    pub task_id: String,
    /// The state the receipt recorded.
    pub expected: String,
    /// The state the target reports now, when it answered.
    pub actual: Option<String>,
    /// A transport or missing-receipt error.
    pub error: Option<String>,
}

/// A node-level auditor that checks task receipts across targets.
pub struct ReceiptAuditor {
    node: Arc<crate::service::FederationNode>,
    context: capability::InvocationContext,
}

impl ReceiptAuditor {
    /// Build an auditor for one node and invocation context.
    pub fn new(
        node: Arc<crate::service::FederationNode>,
        context: capability::InvocationContext,
    ) -> Self {
        Self { node, context }
    }

    /// Compare every task's stored receipt with its target's live record.
    pub async fn audit(&self, tasks: &[TaskNode]) -> Vec<ReceiptMismatch> {
        let mut mismatches = Vec::new();
        for task in tasks {
            let Some(receipt) = &task.compensation_receipt else {
                continue;
            };
            let expected = receipt
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let Some(target) = receipt
                .get("target")
                .and_then(Value::as_str)
                .and_then(|target| capability::NodeId::new(target).ok())
            else {
                mismatches.push(ReceiptMismatch {
                    task_id: task.id.to_string(),
                    expected,
                    actual: None,
                    error: Some("the receipt names no target".to_string()),
                });
                continue;
            };
            let Some(delegation) = receipt.get("delegation").and_then(Value::as_str) else {
                mismatches.push(ReceiptMismatch {
                    task_id: task.id.to_string(),
                    expected,
                    actual: None,
                    error: Some("the receipt names no delegation".to_string()),
                });
                continue;
            };
            match self
                .node
                .delegation_status(&target, &DelegationId::new(delegation), &self.context)
                .await
            {
                Ok(record) => {
                    let actual = format!("{:?}", record.state);
                    if actual != expected {
                        mismatches.push(ReceiptMismatch {
                            task_id: task.id.to_string(),
                            expected,
                            actual: Some(actual),
                            error: None,
                        });
                    }
                }
                Err(error) => mismatches.push(ReceiptMismatch {
                    task_id: task.id.to_string(),
                    expected,
                    actual: None,
                    error: Some(error.to_string()),
                }),
            }
        }
        mismatches
    }
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
    receipt: std::sync::Mutex<Option<Value>>,
    compensation_queue: Option<std::path::PathBuf>,
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
            receipt: std::sync::Mutex::new(None),
            compensation_queue: None,
        }
    }

    /// Ask the target to run this JSON result contract.
    pub fn with_contract(mut self, contract: impl Into<String>) -> Self {
        self.contract = Some(contract.into());
        self
    }

    /// Persist failed compensations here and retry them later.
    pub fn with_compensation_queue(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.compensation_queue = Some(path.into());
        self
    }

    /// Compare a stored compensation receipt with the target's live record.
    pub async fn receipt_matches_remote(&self, task: &TaskNode) -> Result<bool, String> {
        let receipt = task
            .compensation_receipt
            .as_ref()
            .ok_or_else(|| "the task has no compensation receipt".to_string())?;
        let delegation = receipt
            .get("delegation")
            .and_then(Value::as_str)
            .ok_or_else(|| "the receipt names no delegation".to_string())?;
        let expected = receipt
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| "the receipt has no state".to_string())?;
        let record = self
            .node
            .delegation_status(&self.target, &DelegationId::new(delegation), &self.context)
            .await
            .map_err(|error| error.to_string())?;
        Ok(format!("{:?}", record.state) == expected)
    }

    /// Check every task receipt against its target in one pass.
    pub async fn audit_receipts(&self, tasks: &[TaskNode]) -> Vec<ReceiptMismatch> {
        let mut mismatches = Vec::new();
        for task in tasks {
            let Some(receipt) = &task.compensation_receipt else {
                continue;
            };
            let expected = receipt
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let Some(delegation) = receipt.get("delegation").and_then(Value::as_str) else {
                mismatches.push(ReceiptMismatch {
                    task_id: task.id.to_string(),
                    expected,
                    actual: None,
                    error: Some("the receipt names no delegation".to_string()),
                });
                continue;
            };
            match self
                .node
                .delegation_status(&self.target, &DelegationId::new(delegation), &self.context)
                .await
            {
                Ok(record) => {
                    let actual = format!("{:?}", record.state);
                    if actual != expected {
                        mismatches.push(ReceiptMismatch {
                            task_id: task.id.to_string(),
                            expected,
                            actual: Some(actual),
                            error: None,
                        });
                    }
                }
                Err(error) => mismatches.push(ReceiptMismatch {
                    task_id: task.id.to_string(),
                    expected,
                    actual: None,
                    error: Some(error.to_string()),
                }),
            }
        }
        mismatches
    }

    /// Retry every queued compensation addressed to this executor's target.
    pub async fn reconcile_compensations(&self) -> Result<usize, String> {
        let Some(path) = &self.compensation_queue else {
            return Ok(0);
        };
        let mut queue = CompensationQueue::load(path);
        let now = capability::unix_ms();
        let pending: Vec<(String, String)> = queue
            .due_entries(now)
            .into_iter()
            .filter(|entry| entry.target == self.target.to_string())
            .map(|entry| (entry.delegation_id, entry.task_id))
            .collect();
        let mut settled = 0;
        for (delegation, task_id) in pending {
            let id = DelegationId::new(delegation.clone());
            match self
                .node
                .cancel_delegation(&self.target, &id, &self.context)
                .await
            {
                Ok(_) => {
                    queue.remove(&delegation);
                    settled += 1;
                }
                Err(error) => {
                    queue.record_failure(&delegation, error.to_string(), capability::unix_ms())
                }
            }
            let _ = task_id;
        }
        queue.save()?;
        Ok(settled)
    }

    /// Retry due compensations on a background interval.
    pub fn spawn_reconciler(self: Arc<Self>, every: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if let Err(error) = self.reconcile_compensations().await {
                    tracing::warn!("collaboration: compensation reconciliation failed: {error}");
                }
                tokio::time::sleep(every).await;
            }
        })
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
            operations: Vec::new(),
            resources: Vec::new(),
            deadline_ms,
            allow_sub_delegation: false,
            result_contract: self.contract.clone(),
            trace_id: None,
            span_id: None,
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
        let result = self
            .node
            .cancel_delegation(&self.target, &id, &self.context)
            .await;
        if let Some(path) = &self.compensation_queue {
            let mut queue = CompensationQueue::load(path);
            match &result {
                Ok(_) => {
                    queue.remove(&id.to_string());
                }
                Err(error) => queue.enqueue(PendingCompensation {
                    task_id: task.id.to_string(),
                    delegation_id: id.to_string(),
                    target: self.target.to_string(),
                    attempts: 0,
                    last_error: Some(error.to_string()),
                    updated_at_ms: capability::unix_ms(),
                    next_attempt_ms: capability::unix_ms(),
                }),
            }
            if let Err(error) = queue.save() {
                tracing::warn!("collaboration: compensation queue was not saved: {error}");
            }
        }
        let record = result.map_err(|error| error.to_string())?;
        let receipt = serde_json::json!({
            "kind": "delegation_cancel",
            "target": self.target.to_string(),
            "delegation": record.request.id.to_string(),
            "state": format!("{:?}", record.state),
        });
        if let Ok(mut slot) = self.receipt.lock() {
            *slot = Some(receipt);
        }
        Ok(())
    }

    fn compensation_evidence(&self, _task: &TaskNode) -> Option<Value> {
        self.receipt.lock().ok().and_then(|slot| slot.clone())
    }

    fn supports_cleanup(&self) -> bool {
        true
    }

    async fn cleanup_artifacts(&self, artifacts: &[ArtifactRef]) -> Result<(), String> {
        for chunk in artifacts.chunks(crate::artifacts::MAX_CLEANUP_ARTIFACTS) {
            let request = crate::artifacts::ArtifactCleanupRequest::new(
                format!("task-cleanup-{}", capability::unix_ms()),
                chunk.to_vec(),
            );
            let response = self
                .node
                .cleanup_artifacts(&self.target, &request, &self.context)
                .await
                .map_err(|error| error.to_string())?;
            if !response.failed.is_empty() {
                return Err(format!(
                    "{} artifact(s) were not removed",
                    response.failed.len()
                ));
            }
        }
        Ok(())
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
                    // A successful executor return is not completion: when the
                    // task names an acceptance criterion, evaluate it first and
                    // keep the output even when it is rejected.
                    let task = self.graph.tasks.get(&id).cloned();
                    let evaluated = task
                        .as_ref()
                        .and_then(|task| task.acceptance.as_ref())
                        .map(|criterion| evaluate_acceptance(criterion, &result.result));
                    if let Some(report) = &evaluated {
                        if let Some(node) = self.graph.tasks.get_mut(&id) {
                            node.result = Some(result.result.clone());
                            node.acceptance_result = Some(report.clone());
                        }
                        self.graph.revision = self.graph.revision.saturating_add(1);
                        if !report.accepted {
                            let reason = format!(
                                "acceptance rejected: {}",
                                report.detail.clone().unwrap_or_default()
                            );
                            let final_failure = self.graph.fail(&id, reason)?;
                            if final_failure {
                                if let Some(task) = task {
                                    if task.compensation.is_some() {
                                        let compensation = executor.compensate(&task).await;
                                        let receipt = if compensation.is_ok() {
                                            executor.compensation_evidence(&task)
                                        } else {
                                            None
                                        };
                                        self.graph.set_compensated(
                                            &id,
                                            compensation.err(),
                                            receipt,
                                        )?;
                                    }
                                }
                            }
                            self.persist()?;
                            continue;
                        }
                    }
                    self.graph.succeed(&id, result)?;
                    if evaluated.is_some() {
                        self.persist()?;
                    }
                }
                Ok((id, Err(error))) => {
                    let task = self.graph.tasks.get(&id).cloned();
                    if self.graph.fail(&id, error)? {
                        if let Some(task) = task {
                            if task.compensation.is_some() {
                                let compensation = executor.compensate(&task).await;
                                let receipt = if compensation.is_ok() {
                                    executor.compensation_evidence(&task)
                                } else {
                                    None
                                };
                                self.graph
                                    .set_compensated(&id, compensation.err(), receipt)?;
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
        if executor.supports_cleanup() {
            let now = capability::unix_ms();
            let expired = self.graph.expired_artifacts(now);
            if !expired.is_empty() {
                match executor.cleanup_artifacts(&expired).await {
                    Ok(()) => {
                        self.graph.prune_artifacts(now);
                        self.persist()?;
                    }
                    Err(error) => {
                        tracing::warn!("collaboration: artifact cleanup failed: {error}");
                    }
                }
            }
        }
        Ok(self.graph.progress())
    }
}
