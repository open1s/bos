//! Peer-owned tasks: the durable queue a peer's node records and its shell drains.
//!
//! A federation node cannot run a model. What it can do is own the record of
//! work another node asked it to keep: validate the request, store it, answer
//! status queries, and mark a task due so the peer's shell can start the turn.
//! The node never starts a turn itself; the shell claims a due task, runs it,
//! and settles it through the same queue.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// When a peer-owned task runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TaskSchedule {
    /// Repeat every N seconds, never more often than once a minute.
    Every {
        /// The cadence.
        every_seconds: u64,
    },
    /// Run once at an absolute time.
    At {
        /// The instant, in unix milliseconds.
        at_ms: u64,
    },
}

impl TaskSchedule {
    /// The next run, given the last one and the record's creation time.
    pub fn next_run_ms(&self, last_run_ms: u64, created_ms: u64) -> u64 {
        match self {
            Self::Every { every_seconds } => {
                // A fresh task waits one interval, exactly like a local every
                // task created with its last run set to now.
                let base = if last_run_ms == 0 {
                    created_ms
                } else {
                    last_run_ms
                };
                base.saturating_add(every_seconds.saturating_mul(1_000))
            }
            Self::At { at_ms } => *at_ms,
        }
    }

    /// Whether the task runs more than once.
    pub fn is_repeating(&self) -> bool {
        matches!(self, Self::Every { .. })
    }
}

/// The lifecycle of a peer-owned task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedTaskState {
    /// Waiting for the clock (or the first claim).
    Queued,
    /// A shell claimed it and is running the turn.
    Running,
    /// The last run finished successfully.
    Succeeded,
    /// The last run failed.
    Failed,
    /// Cancelled by the origin.
    Cancelled,
}

/// One task a peer asked this node to own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedTask {
    /// Stable id chosen by the origin.
    pub id: String,
    /// The origin's node id, as the authenticated sender reported it.
    pub origin: String,
    /// The objective handed to the turn.
    pub objective: String,
    /// When it runs.
    pub schedule: TaskSchedule,
    /// Maximum steps for one run.
    pub budget_steps: u32,
    /// Maximum tokens for one run.
    pub budget_tokens: u64,
    /// Maximum wall clock for one run.
    pub deadline_ms: u64,
    /// The state.
    pub state: FederatedTaskState,
    /// The last time a shell claimed it, in unix milliseconds.
    pub last_run_ms: u64,
    /// When it should next run.
    pub next_run_ms: u64,
    /// The last bounded result.
    pub result: Option<String>,
    /// The last error.
    pub error: Option<String>,
    /// When it was recorded.
    pub created_ms: u64,
    /// When it last changed.
    pub updated_ms: u64,
}

/// Why a task request was refused.
#[derive(Debug, thiserror::Error)]
pub enum TaskQueueError {
    /// No task with that id.
    #[error("task {0} is unknown")]
    Unknown(String),
    /// Three hundredths.
    #[error("a task needs an objective")]
    EmptyObjective,
    /// A cadence faster than a minute.
    #[error("every_seconds must be at least 60")]
    TooFrequent,
    /// The id is already used.
    #[error("task {0} already exists")]
    Duplicate(String),
}

/// The durable queue.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskQueue {
    tasks: BTreeMap<String, FederatedTask>,
}

impl TaskQueue {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a task the origin asked this node to own.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        id: impl Into<String>,
        origin: impl Into<String>,
        objective: impl Into<String>,
        schedule: TaskSchedule,
        budget_steps: u32,
        budget_tokens: u64,
        deadline_ms: u64,
        now_ms: u64,
    ) -> Result<FederatedTask, TaskQueueError> {
        let id = id.into();
        let objective = objective.into();
        if objective.trim().is_empty() {
            return Err(TaskQueueError::EmptyObjective);
        }
        if let TaskSchedule::Every { every_seconds } = schedule {
            if every_seconds < 60 {
                return Err(TaskQueueError::TooFrequent);
            }
        }
        if self.tasks.contains_key(&id) {
            return Err(TaskQueueError::Duplicate(id));
        }
        let task = FederatedTask {
            id: id.clone(),
            origin: origin.into(),
            objective,
            schedule,
            budget_steps,
            budget_tokens,
            deadline_ms,
            state: FederatedTaskState::Queued,
            last_run_ms: 0,
            next_run_ms: schedule.next_run_ms(0, now_ms),
            result: None,
            error: None,
            created_ms: now_ms,
            updated_ms: now_ms,
        };
        self.tasks.insert(id, task.clone());
        Ok(task)
    }

    /// One task.
    pub fn get(&self, id: &str) -> Option<&FederatedTask> {
        self.tasks.get(id)
    }

    /// Every task.
    pub fn iter(&self) -> impl Iterator<Item = &FederatedTask> {
        self.tasks.values()
    }

    /// How many tasks the queue holds.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Claim the tasks that are due now. Each is marked Running so a second
    /// claim in the same tick cannot double-start it; the shell settles the
    /// ones it actually ran.
    pub fn claim_due(&mut self, now_ms: u64) -> Vec<FederatedTask> {
        let mut due = Vec::new();
        for task in self.tasks.values_mut() {
            if task.state != FederatedTaskState::Queued || task.next_run_ms > now_ms {
                continue;
            }
            task.state = FederatedTaskState::Running;
            task.last_run_ms = now_ms;
            task.updated_ms = now_ms;
            due.push(task.clone());
        }
        due
    }

    /// Settle a claimed task. A repeating task goes back to Queued with its
    /// next run; a one-shot keeps its terminal state.
    pub fn settle(
        &mut self,
        id: &str,
        ok: bool,
        result: Option<String>,
        error: Option<String>,
        now_ms: u64,
    ) -> Result<(), TaskQueueError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| TaskQueueError::Unknown(id.to_string()))?;
        task.updated_ms = now_ms;
        task.result = result;
        task.error = error;
        if task.schedule.is_repeating() {
            task.state = FederatedTaskState::Queued;
            task.next_run_ms = task.schedule.next_run_ms(now_ms, task.created_ms);
        } else {
            task.state = if ok {
                FederatedTaskState::Succeeded
            } else {
                FederatedTaskState::Failed
            };
        }
        Ok(())
    }

    /// Cancel a task. A task already settled cannot be cancelled.
    pub fn cancel(&mut self, id: &str, now_ms: u64) -> Result<(), TaskQueueError> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| TaskQueueError::Unknown(id.to_string()))?;
        if matches!(
            task.state,
            FederatedTaskState::Succeeded | FederatedTaskState::Cancelled
        ) {
            return Ok(());
        }
        task.state = FederatedTaskState::Cancelled;
        task.updated_ms = now_ms;
        Ok(())
    }

    /// Read the queue, resetting claims that outlived their process.
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(error) => return Err(error),
        };
        let mut queue: Self = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        for task in queue.tasks.values_mut() {
            if task.state == FederatedTaskState::Running {
                task.state = FederatedTaskState::Queued;
            }
        }
        Ok(queue)
    }

    /// Write the queue whole.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)
    }
}

/// A task the origin asks a peer to own.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskCreateRequest {
    /// The origin-chosen id.
    pub id: String,
    /// The objective handed to the peer's turn.
    pub objective: String,
    /// When it runs.
    pub schedule: TaskSchedule,
    /// Maximum steps for one run.
    pub budget_steps: u32,
    /// Maximum tokens for one run.
    pub budget_tokens: u64,
    /// Maximum wall clock for one run.
    pub deadline_ms: u64,
}

/// A status or cancel request.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskIdRequest {
    /// The task id.
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> TaskQueue {
        TaskQueue::new()
    }

    /// A created task is queued with its next run computed from the schedule.
    #[test]
    fn create_records_a_queued_task() {
        let mut queue = queue();
        let task = queue
            .create(
                "task-1",
                "node-origin",
                "Audit the repo",
                TaskSchedule::Every {
                    every_seconds: 3_600,
                },
                10,
                50_000,
                300_000,
                1_000,
            )
            .expect("created");
        assert_eq!(task.state, FederatedTaskState::Queued);
        assert_eq!(
            task.next_run_ms, 3_601_000,
            "a cadence waits one interval before its first run"
        );
        assert!(queue.claim_due(1_000).is_empty(), "not due yet");
        assert_eq!(queue.get("task-1").expect("stored"), &task);
    }

    /// An empty objective and a cadence under a minute are refused.
    #[test]
    fn create_refuses_invalid_requests() {
        let mut queue = queue();
        assert!(matches!(
            queue.create("a", "o", "  ", TaskSchedule::At { at_ms: 1 }, 1, 1, 1, 1),
            Err(TaskQueueError::EmptyObjective)
        ));
        assert!(matches!(
            queue.create(
                "b",
                "o",
                "x",
                TaskSchedule::Every { every_seconds: 5 },
                1,
                1,
                1,
                1
            ),
            Err(TaskQueueError::TooFrequent)
        ));
    }

    /// Claiming marks a due task Running and returns it; an early task waits.
    #[test]
    fn claim_due_takes_only_what_is_due() {
        let mut queue = queue();
        queue
            .create(
                "soon",
                "o",
                "x",
                TaskSchedule::At { at_ms: 1_000 },
                1,
                1,
                1,
                0,
            )
            .expect("soon");
        queue
            .create(
                "later",
                "o",
                "y",
                TaskSchedule::At { at_ms: 9_000 },
                1,
                1,
                1,
                0,
            )
            .expect("later");
        let due = queue.claim_due(1_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "soon");
        assert_eq!(
            queue.get("soon").expect("stored").state,
            FederatedTaskState::Running
        );
        assert_eq!(
            queue.get("later").expect("stored").state,
            FederatedTaskState::Queued
        );
        assert!(queue.claim_due(1_000).is_empty(), "a claim is taken once");
    }

    /// A repeating task returns to Queued with its next run; a one-shot keeps
    /// the terminal state.
    #[test]
    fn settle_reschedules_repeating_tasks() {
        let mut queue = queue();
        queue
            .create(
                "repeat",
                "o",
                "x",
                TaskSchedule::Every {
                    every_seconds: 3_600,
                },
                1,
                1,
                1,
                0,
            )
            .expect("repeat");
        let _ = queue.claim_due(0);
        queue
            .settle("repeat", true, Some("done".to_string()), None, 10_000)
            .expect("settled");
        let task = queue.get("repeat").expect("stored");
        assert_eq!(task.state, FederatedTaskState::Queued);
        assert_eq!(task.next_run_ms, 3_610_000);
        assert_eq!(task.result.as_deref(), Some("done"));

        queue
            .create("once", "o", "y", TaskSchedule::At { at_ms: 0 }, 1, 1, 1, 0)
            .expect("once");
        let _ = queue.claim_due(0);
        queue
            .settle("once", false, None, Some("nope".to_string()), 10_000)
            .expect("settled");
        assert_eq!(
            queue.get("once").expect("stored").state,
            FederatedTaskState::Failed
        );
    }

    /// Cancel is idempotent for a settled task and refuses an unknown one.
    #[test]
    fn cancel_marks_and_is_idempotent() {
        let mut queue = queue();
        queue
            .create("a", "o", "x", TaskSchedule::At { at_ms: 0 }, 1, 1, 1, 0)
            .expect("a");
        queue.cancel("a", 5).expect("cancelled");
        assert_eq!(
            queue.get("a").expect("stored").state,
            FederatedTaskState::Cancelled
        );
        queue.cancel("a", 6).expect("a second cancel is a no-op");
        assert!(matches!(
            queue.cancel("missing", 6),
            Err(TaskQueueError::Unknown(_))
        ));
    }

    /// A saved queue reloads whole, and a claim that outlived its process is
    /// reset to Queued.
    #[test]
    fn save_and_load_recover_a_claim() {
        let dir =
            std::env::temp_dir().join(format!("federation-task-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("tasks.json");
        let mut queue = queue();
        queue
            .create("a", "o", "x", TaskSchedule::At { at_ms: 0 }, 1, 1, 1, 0)
            .expect("a");
        let _ = queue.claim_due(0);
        queue.save(&path).expect("save");
        let loaded = TaskQueue::load(&path).expect("load");
        assert_eq!(
            loaded.get("a").expect("stored").state,
            FederatedTaskState::Queued,
            "a claim from a dead process is not trusted"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The schedule's next run is its own arithmetic.
    #[test]
    fn the_schedule_computes_the_next_run() {
        assert_eq!(
            TaskSchedule::Every { every_seconds: 60 }.next_run_ms(1_000, 0),
            61_000
        );
        assert_eq!(
            TaskSchedule::Every { every_seconds: 60 }.next_run_ms(0, 5_000),
            65_000,
            "a fresh task waits one interval from creation"
        );
        assert_eq!(TaskSchedule::At { at_ms: 7 }.next_run_ms(0, 0), 7);
    }
}
