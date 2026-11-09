//! Durable task records: the tasks extension's storage seam.
//!
//! One JSON file per task at <workspace>/.bsh/tasks/<id>.json, beside the
//! sessions the tasks belong to. The directory is the database: a task that
//! fails to parse is skipped rather than taking the board down, and a save is
//! written to a temporary file and renamed so a crash cannot leave half a
//! record.
//!
//! The record is the platform's, not the UI's: it is what the scheduler, the
//! model tools and the board all agree on.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::extension::runs::{RunSnapshot, RunState};

/// Where a task runs. A mirrored peer-owned task is not executed here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum Host {
    /// A task this node owns and schedules.
    #[default]
    Local,
    /// A task another node owns; this record is a mirror plus a command channel.
    Peer {
        /// The owning node id.
        node: String,
    },
}

/// The execution tier an attempt binds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    /// A turn in the origin session.
    Turn,
    /// A child run inside a turn; illegal on a scheduled task.
    Subagent,
    /// A background job (a command).
    Job,
    /// A delegation to a peer node.
    Delegation,
    /// A task another node owns and schedules; this record is a mirror.
    PeerTask,
}

/// When a task fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum Trigger {
    /// Fire as soon as capacity allows.
    Now,
    /// Fire once at an absolute time, in unix milliseconds.
    At {
        /// The instant to fire.
        at_ms: u64,
    },
    /// Fire every N seconds, remembering when it last ran.
    Every {
        /// The cadence.
        every_seconds: u64,
        /// The last firing, in unix milliseconds; zero means never.
        last_run_ms: u64,
    },
}

impl Trigger {
    /// Whether this trigger can ever fire without an interactive turn.
    pub(crate) fn is_scheduled(&self) -> bool {
        !matches!(self, Self::Now)
    }

    /// When the task should fire next, in unix milliseconds.
    pub(crate) fn next_fire_ms(&self) -> Option<u64> {
        match self {
            Self::Now => None,
            Self::At { at_ms } => Some(*at_ms),
            Self::Every {
                every_seconds,
                last_run_ms,
            } => Some(last_run_ms.saturating_add(every_seconds.saturating_mul(1_000))),
        }
    }
}

/// Why a task construct was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A subagent only exists inside a turn, so a scheduled task cannot bind it.
    SubagentNeedsATurn,
    /// A peer task is owned by the peer's clock, so it needs a schedule.
    PeerTaskNeedsASchedule,
}

impl Refusal {
    /// The reader-facing reason, one sentence.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::SubagentNeedsATurn => {
                "a subagent runs inside a turn, so a scheduled task cannot bind one"
            }
            Self::PeerTaskNeedsASchedule => {
                "a peer task is owned by the peer's clock, so it needs a schedule"
            }
        }
    }
}

impl Tier {
    /// Refuse a tier that cannot fire from a clock.
    pub(crate) fn legal_for_trigger(self, trigger: Trigger) -> Result<(), Refusal> {
        if matches!(self, Self::Subagent) && trigger.is_scheduled() {
            return Err(Refusal::SubagentNeedsATurn);
        }
        if matches!(self, Self::PeerTask) && !trigger.is_scheduled() {
            return Err(Refusal::PeerTaskNeedsASchedule);
        }
        Ok(())
    }
}

/// The lifecycle of a task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskStatus {
    /// Waiting for its next trigger; an every or at task between firings.
    Scheduled,
    /// Waiting for a resource; the reason lives on the attempt.
    Queued,
    /// An attempt is live.
    Running,
    /// Waiting for dependencies.
    Waiting,
    /// Waiting for a human approval.
    Blocked,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Stopped by the reader or the model.
    Cancelled,
    /// The process died while the attempt ran.
    Interrupted,
    /// A peer-owned task or delegation whose peer is unreachable.
    Unknown,
}

impl TaskStatus {
    /// Whether the task will not change again without a new attempt.
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Whether a scheduler should consider the task done for now.
    pub(crate) fn is_settled(&self) -> bool {
        self.is_terminal() || matches!(self, Self::Unknown)
    }

    /// The board section this status belongs to.
    pub(crate) fn section(&self) -> BoardSectionKind {
        match self {
            Self::Scheduled => BoardSectionKind::Scheduled,
            Self::Queued | Self::Waiting => BoardSectionKind::Queued,
            Self::Running | Self::Blocked | Self::Unknown => BoardSectionKind::Running,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Interrupted => {
                BoardSectionKind::History
            }
        }
    }
}

/// The lifecycle of one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttemptStatus {
    /// Reserved but not started (for example, waiting for a slot).
    Queued,
    /// Live.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Stopped deliberately.
    Cancelled,
    /// The process died.
    Interrupted,
    /// Waiting for a human approval.
    Blocked,
    /// The peer could not be reached.
    Unknown,
}

/// What a finished attempt consumed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Usage {
    /// Model tokens, when the tier has a model.
    pub(crate) tokens: u64,
    /// Steps the run took.
    pub(crate) steps: u32,
    /// Wall clock in milliseconds.
    pub(crate) wall_ms: u64,
}

impl Usage {
    /// Whether every counter is zero.
    pub(crate) fn is_zero(&self) -> bool {
        self.tokens == 0 && self.steps == 0 && self.wall_ms == 0
    }
}

/// The caps a run must respect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Budget {
    /// Maximum steps.
    pub(crate) max_steps: u32,
    /// Maximum model tokens.
    pub(crate) max_tokens: u64,
    /// Maximum wall clock in milliseconds.
    pub(crate) deadline_ms: u64,
}

impl Default for Budget {
    fn default() -> Self {
        // The approved defaults: 20 steps, 200k tokens, 10 minutes.
        Self {
            max_steps: 20,
            max_tokens: 200_000,
            deadline_ms: 600_000,
        }
    }
}

impl Budget {
    /// The approved smaller defaults for cadence attempts.
    pub(crate) fn cadence() -> Self {
        Self {
            max_steps: 5,
            max_tokens: 50_000,
            deadline_ms: 300_000,
        }
    }
}

/// Whether an interrupted or failed task is retried automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetryMode {
    /// The reader or the model retries.
    Manual,
    /// The scheduler retries while budget and attempts remain.
    Auto,
}

/// The retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retry {
    /// Manual or automatic.
    pub(crate) mode: RetryMode,
    /// The cap on attempts, including the first.
    pub(crate) max_attempts: u32,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            mode: RetryMode::Manual,
            max_attempts: 3,
        }
    }
}

/// Where a task came from: the session and the turn that asked for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    /// The origin session.
    pub(crate) session: String,
    /// The origin turn index, zero when unknown.
    pub(crate) turn: u64,
}

/// One execution try of a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Attempt {
    /// Stable attempt id.
    pub(crate) id: String,
    /// The tier this attempt binds to.
    pub(crate) tier: Tier,
    /// The tier object's id once started: subagent, job, delegation or turn.
    pub(crate) run: Option<String>,
    /// The attempt's state.
    pub(crate) status: AttemptStatus,
    /// When it started, in unix milliseconds.
    pub(crate) started_ms: Option<u64>,
    /// When it ended, in unix milliseconds.
    pub(crate) ended_ms: Option<u64>,
    /// The bounded result text.
    pub(crate) result: Option<String>,
    /// The usage, once settled.
    pub(crate) usage: Option<Usage>,
    /// The error or refusal reason.
    pub(crate) error: Option<String>,
    /// When the harness delivered this attempt to the origin session.
    pub(crate) delivered_ms: Option<u64>,
    /// The exact-change set this attempt produced, when review is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) change_set_id: Option<String>,
}

impl Attempt {
    /// A queued attempt for a tier.
    pub(crate) fn queued(id: impl Into<String>, tier: Tier) -> Self {
        Self {
            id: id.into(),
            tier,
            run: None,
            status: AttemptStatus::Queued,
            started_ms: None,
            ended_ms: None,
            result: None,
            usage: None,
            error: None,
            delivered_ms: None,
            change_set_id: None,
        }
    }
}

/// One durable task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Task {
    /// Stable id; the store file is named after it.
    pub(crate) id: String,
    /// A short label for the board.
    pub(crate) title: String,
    /// The full prompt or objective.
    pub(crate) intent: String,
    /// Who executes it.
    #[serde(default)]
    pub(crate) host: Host,
    /// An optional parent task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
    /// Where it came from.
    pub(crate) origin: Origin,
    /// When it fires.
    pub(crate) trigger: Trigger,
    /// The attempt tier.
    pub(crate) tier: Tier,
    /// For a delegation: the peer node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    /// For a delegation: an optional capability allowlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capability: Option<String>,
    /// For a job: the command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    /// For a delegation: the capability input, so a retry can repeat it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<serde_json::Value>,
    /// Tasks that must succeed first.
    #[serde(default)]
    pub(crate) deps: Vec<String>,
    /// The caps for each attempt.
    #[serde(default)]
    pub(crate) budget: Budget,
    /// The retry policy.
    #[serde(default)]
    pub(crate) retry: Retry,
    /// The task's state.
    pub(crate) status: TaskStatus,
    /// Every attempt, oldest first.
    #[serde(default)]
    pub(crate) attempts: Vec<Attempt>,
    /// When the harness armed a resume for this task, in unix milliseconds.
    ///
    /// Set when a join returns with the task still running: the harness will
    /// deliver one trigger turn when it settles, and clear this in the same
    /// write, which is the delivery-once guarantee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resume_armed_ms: Option<u64>,
    /// When a peer mirror's owner was last asked, in unix milliseconds.
    #[serde(default)]
    pub(crate) mirror_checked_ms: u64,
    /// When it was created, in unix milliseconds.
    pub(crate) created_ms: u64,
    /// When it last changed, in unix milliseconds.
    pub(crate) updated_ms: u64,
}

impl Task {
    /// The active attempt, when one exists and is not settled.
    pub(crate) fn active_attempt(&self) -> Option<&Attempt> {
        self.attempts
            .iter()
            .rev()
            .find(|attempt| !attempt.is_settled())
    }

    /// Whether the task has attempts left under its retry policy.
    pub(crate) fn may_retry(&self) -> bool {
        (self.attempts.len() as u32) < self.retry.max_attempts
    }

    /// Open a task, validating the tier against the trigger and taking the
    /// approved budget for that kind.
    pub(crate) fn open(
        id: impl Into<String>,
        title: impl Into<String>,
        intent: impl Into<String>,
        origin: Origin,
        trigger: Trigger,
        tier: Tier,
        now_ms: u64,
    ) -> Result<Self, Refusal> {
        tier.legal_for_trigger(trigger)?;
        let scheduled = trigger.is_scheduled();
        Ok(Self {
            id: id.into(),
            title: title.into(),
            intent: intent.into(),
            host: Host::Local,
            parent: None,
            origin,
            trigger,
            tier,
            target: None,
            capability: None,
            command: None,
            input: None,
            deps: Vec::new(),
            budget: if scheduled {
                Budget::cadence()
            } else {
                Budget::default()
            },
            retry: Retry::default(),
            status: if scheduled {
                TaskStatus::Scheduled
            } else {
                TaskStatus::Queued
            },
            attempts: Vec::new(),
            resume_armed_ms: None,
            mirror_checked_ms: 0,
            created_ms: now_ms,
            updated_ms: now_ms,
        })
    }
}

impl Attempt {
    /// Whether this attempt has settled.
    pub(crate) fn is_settled(&self) -> bool {
        matches!(
            self.status,
            AttemptStatus::Succeeded
                | AttemptStatus::Failed
                | AttemptStatus::Cancelled
                | AttemptStatus::Interrupted
        )
    }
}

/// The board's sections, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoardSectionKind {
    /// Live attempts, including blocked and unknown ones.
    Running,
    /// Waiting for a resource or a dependency.
    Queued,
    /// Between firings.
    Scheduled,
    /// Settled.
    History,
}

/// One row the board draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoardRow {
    /// The task.
    pub(crate) id: String,
    /// The label.
    pub(crate) title: String,
    /// The attempt tier.
    pub(crate) tier: Tier,
    /// The task state.
    pub(crate) status: TaskStatus,
    /// Who executes it.
    pub(crate) host: Host,
    /// The origin session.
    pub(crate) session: String,
    /// When it was created, in unix milliseconds.
    pub(crate) created_ms: u64,
    /// The next scheduled fire, for at or every.
    pub(crate) next_fire_ms: Option<u64>,
    /// How many attempts have been made.
    pub(crate) attempts: u32,
}

/// One section and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BoardSection {
    /// Which section.
    pub(crate) kind: BoardSectionKind,
    /// Its rows.
    pub(crate) rows: Vec<BoardRow>,
}

/// The whole board: the four sections, always present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Board {
    /// Running, Queued, Scheduled, History.
    pub(crate) sections: Vec<BoardSection>,
}

/// Project records onto the board. History is newest first; the live sections
/// are oldest first so a long wait does not jump the queue visually.
pub(crate) fn board(tasks: &[Task]) -> Board {
    let mut running: Vec<BoardRow> = Vec::new();
    let mut queued: Vec<BoardRow> = Vec::new();
    let mut scheduled: Vec<BoardRow> = Vec::new();
    let mut history: Vec<BoardRow> = Vec::new();
    for task in tasks {
        let row = BoardRow {
            id: task.id.clone(),
            title: task.title.clone(),
            tier: task.tier,
            status: task.status,
            host: task.host.clone(),
            session: task.origin.session.clone(),
            created_ms: task.created_ms,
            next_fire_ms: task.trigger.next_fire_ms(),
            attempts: task.attempts.len() as u32,
        };
        match task.status.section() {
            BoardSectionKind::Running => running.push(row),
            BoardSectionKind::Queued => queued.push(row),
            BoardSectionKind::Scheduled => scheduled.push(row),
            BoardSectionKind::History => history.push(row),
        }
    }
    let by_time = |a: &BoardRow, b: &BoardRow| {
        a.created_ms
            .cmp(&b.created_ms)
            .then_with(|| a.id.cmp(&b.id))
    };
    running.sort_by(by_time);
    queued.sort_by(by_time);
    scheduled.sort_by(by_time);
    history.sort_by(|a, b| by_time(b, a));
    Board {
        sections: vec![
            BoardSection {
                kind: BoardSectionKind::Running,
                rows: running,
            },
            BoardSection {
                kind: BoardSectionKind::Queued,
                rows: queued,
            },
            BoardSection {
                kind: BoardSectionKind::Scheduled,
                rows: scheduled,
            },
            BoardSection {
                kind: BoardSectionKind::History,
                rows: history,
            },
        ],
    }
}

/// One row of a join report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JoinRow {
    /// The task.
    pub(crate) task: String,
    /// The run its latest attempt names, once started.
    pub(crate) run: Option<String>,
    /// The run's observed state.
    pub(crate) state: Option<RunState>,
    /// The bounded result.
    pub(crate) result: Option<String>,
    /// The error or refusal reason.
    pub(crate) error: Option<String>,
}

/// What a join returns: every row, and whether every run settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JoinReport {
    /// One row per task, in the order asked for.
    pub(crate) rows: Vec<JoinRow>,
    /// Whether every task's run settled before the timeout.
    pub(crate) settled: bool,
}

/// Poll the runs named by each task's latest attempt until all settle or the
/// timeout passes. The wait is a sleep, so the caller's thread is never held.
///
/// The observer is a function, not a single registry: a turn run lives in the
/// host.runs service while a subagent run lives in the subagent registry, and
/// the caller is the one that knows both.
pub(crate) async fn join<F>(observe: F, tasks: &[Task], timeout_ms: u64) -> JoinReport
where
    F: Fn(&str) -> Option<RunSnapshot>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let rows: Vec<JoinRow> = tasks
            .iter()
            .map(|task| {
                let run = task.attempts.last().and_then(|attempt| attempt.run.clone());
                let snapshot = run.as_deref().and_then(&observe);
                JoinRow {
                    task: task.id.clone(),
                    run,
                    state: snapshot.as_ref().map(|run| run.state),
                    result: snapshot.as_ref().and_then(|run| run.result.clone()),
                    error: snapshot.as_ref().and_then(|run| run.error.clone()),
                }
            })
            .collect();
        let settled = rows
            .iter()
            .all(|row| row.state.is_some_and(|state| state.is_settled()));
        if settled || std::time::Instant::now() >= deadline {
            return JoinReport { rows, settled };
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A task id unique within this process and millisecond.
pub(crate) fn new_id(now_ms: u64) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("task-{now_ms}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A task store for one workspace root.
pub(crate) trait TasksStore: Send + Sync {
    /// Every task that parses under the root's task directory.
    fn list(&self, root: &Path) -> io::Result<Vec<Task>>;
    /// One task, or None when it does not exist.
    fn load(&self, root: &Path, id: &str) -> io::Result<Option<Task>>;
    /// Write one task, atomically.
    fn save(&self, root: &Path, task: &Task) -> io::Result<()>;
    /// Remove one task; a missing file is not an error.
    fn delete(&self, root: &Path, id: &str) -> io::Result<()>;
}

/// The file-backed store: one JSON document per task.
pub(crate) struct FileTasksStore;

impl FileTasksStore {
    /// The task directory for a workspace root.
    pub(crate) fn dir(root: &Path) -> PathBuf {
        root.join(".bsh").join("tasks")
    }

    /// The document path for one task id.
    pub(crate) fn path(root: &Path, id: &str) -> io::Result<PathBuf> {
        if id.is_empty() || id == "." || id == ".." || id.contains('/') || id.contains('\\') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a task id cannot name a path",
            ));
        }
        Ok(Self::dir(root).join(format!("{id}.json")))
    }
}

impl TasksStore for FileTasksStore {
    fn list(&self, root: &Path) -> io::Result<Vec<Task>> {
        let dir = Self::dir(root);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut tasks = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Task>(&bytes).ok())
            {
                Some(task) => tasks.push(task),
                None => log::warn!("skipping unreadable task record {}", path.display()),
            }
        }
        tasks.sort_by(|left, right| {
            left.created_ms
                .cmp(&right.created_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(tasks)
    }

    fn load(&self, root: &Path, id: &str) -> io::Result<Option<Task>> {
        let path = Self::path(root, id)?;
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn save(&self, root: &Path, task: &Task) -> io::Result<()> {
        let path = Self::path(root, &task.id)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(task)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, &path)
    }

    fn delete(&self, root: &Path, id: &str) -> io::Result<()> {
        let path = Self::path(root, id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bsh-tasks-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch workspace");
        dir
    }

    fn task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: "Review the diff".to_string(),
            intent: "Review the working copy and report issues.".to_string(),
            host: Host::Local,
            parent: None,
            origin: Origin {
                session: "ses-1".to_string(),
                turn: 3,
            },
            trigger: Trigger::Now,
            tier: Tier::Subagent,
            target: None,
            capability: None,
            command: None,
            input: None,
            deps: Vec::new(),
            budget: Budget::default(),
            retry: Retry::default(),
            status: TaskStatus::Queued,
            attempts: vec![Attempt::queued("att-1", Tier::Subagent)],
            resume_armed_ms: None,
            mirror_checked_ms: 0,
            created_ms: 1_000,
            updated_ms: 1_000,
        }
    }

    /// A saved task reads back whole.
    #[test]
    fn a_task_round_trips() {
        let root = workspace("roundtrip");
        let store = FileTasksStore;
        let record = task("task-1");
        store.save(&root, &record).expect("save");
        let listed = store.list(&root).expect("list");
        assert_eq!(listed, vec![record]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An unreadable record is skipped, not fatal.
    #[test]
    fn a_damaged_record_is_skipped() {
        let root = workspace("damaged");
        let store = FileTasksStore;
        store.save(&root, &task("task-good")).expect("save");
        let damaged = FileTasksStore::dir(&root).join("task-bad.json");
        std::fs::write(&damaged, b"{ this is not a task").expect("write");
        let listed = store.list(&root).expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "task-good");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Delete removes one record and tolerates a missing one.
    #[test]
    fn delete_is_idempotent() {
        let root = workspace("delete");
        let store = FileTasksStore;
        store.save(&root, &task("task-1")).expect("save");
        store.delete(&root, "task-1").expect("delete");
        assert!(store.list(&root).expect("list").is_empty());
        store.delete(&root, "task-1").expect("delete again");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A task id cannot escape the directory.
    #[test]
    fn a_task_id_cannot_name_a_path() {
        let root = workspace("path");
        assert!(FileTasksStore::path(&root, "../escape").is_err());
        assert!(FileTasksStore::path(&root, "a/b").is_err());
        assert!(FileTasksStore::path(&root, "").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The approved defaults are the record's defaults.
    #[test]
    fn the_defaults_are_the_approved_ones() {
        let budget = Budget::default();
        assert_eq!(budget.max_steps, 20);
        assert_eq!(budget.max_tokens, 200_000);
        assert_eq!(budget.deadline_ms, 600_000);
        let cadence = Budget::cadence();
        assert_eq!(cadence.max_steps, 5);
        assert_eq!(cadence.max_tokens, 50_000);
        assert_eq!(Retry::default().mode, RetryMode::Manual);
    }

    /// A missing directory is an empty board, not an error.
    #[test]
    fn an_absent_directory_is_empty() {
        let root = workspace("absent");
        assert!(FileTasksStore.list(&root).expect("list").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Settled and active attempts are classified, and a task retries only
    /// while the policy allows.
    #[test]
    fn lifecycle_helpers_agree_with_the_statuses() {
        assert!(TaskStatus::Succeeded.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Unknown.is_settled());
        let mut record = task("task-1");
        assert!(record.active_attempt().is_some());
        assert!(record.may_retry());
        record.attempts[0].status = AttemptStatus::Succeeded;
        assert!(record.active_attempt().is_none());
        record
            .attempts
            .push(Attempt::queued("att-2", Tier::Subagent));
        record
            .attempts
            .push(Attempt::queued("att-3", Tier::Subagent));
        assert!(!record.may_retry(), "three attempts reach the cap");
    }

    /// A subagent cannot be scheduled; a scheduled task takes the cadence
    /// budget, and an immediate one takes the default.
    #[test]
    fn open_refuses_a_subagent_on_a_clock() {
        let origin = Origin {
            session: "ses-1".to_string(),
            turn: 1,
        };
        let refused = Task::open(
            "t",
            "title",
            "intent",
            origin.clone(),
            Trigger::Every {
                every_seconds: 60,
                last_run_ms: 0,
            },
            Tier::Subagent,
            1,
        );
        assert_eq!(refused.unwrap_err(), Refusal::SubagentNeedsATurn);

        let immediate = Task::open(
            "task-now",
            "title",
            "intent",
            origin.clone(),
            Trigger::Now,
            Tier::Subagent,
            1,
        )
        .expect("an immediate subagent task is legal");
        assert_eq!(immediate.status, TaskStatus::Queued);
        assert_eq!(immediate.budget, Budget::default());

        let scheduled_job = Task::open(
            "task-job",
            "title",
            "intent",
            origin.clone(),
            Trigger::Every {
                every_seconds: 60,
                last_run_ms: 5,
            },
            Tier::Job,
            10,
        )
        .expect("a scheduled job is legal");
        assert_eq!(scheduled_job.status, TaskStatus::Scheduled);
        let cadence = Task::open(
            "task-every",
            "title",
            "intent",
            origin,
            Trigger::Every {
                every_seconds: 60,
                last_run_ms: 5,
            },
            Tier::Turn,
            10,
        )
        .expect("a scheduled turn is legal");
        assert_eq!(cadence.status, TaskStatus::Scheduled);
        assert_eq!(cadence.budget, Budget::cadence());
        assert_eq!(
            cadence.trigger.next_fire_ms(),
            Some(60_005),
            "the next fire is last run plus the cadence"
        );
    }

    /// The board groups by state and orders live rows oldest first and
    /// history newest first.
    #[test]
    fn the_board_groups_and_orders() {
        let origin = Origin {
            session: "ses-1".to_string(),
            turn: 1,
        };
        let mut a = Task::open(
            "a",
            "A",
            "intent",
            origin.clone(),
            Trigger::Now,
            Tier::Subagent,
            10,
        )
        .expect("a");
        a.status = TaskStatus::Running;
        let b = Task::open(
            "b",
            "B",
            "intent",
            origin.clone(),
            Trigger::Now,
            Tier::Job,
            20,
        )
        .expect("b");
        let c = Task::open(
            "c",
            "C",
            "intent",
            origin.clone(),
            Trigger::Every {
                every_seconds: 60,
                last_run_ms: 0,
            },
            Tier::Turn,
            30,
        )
        .expect("c");
        let mut d = Task::open(
            "d",
            "D",
            "intent",
            origin.clone(),
            Trigger::Now,
            Tier::Job,
            40,
        )
        .expect("d");
        d.status = TaskStatus::Succeeded;
        let mut e = Task::open("e", "E", "intent", origin, Trigger::Now, Tier::Job, 50).expect("e");
        e.status = TaskStatus::Failed;

        let board = board(&[a, b, c, d, e]);
        assert_eq!(board.sections.len(), 4);
        assert_eq!(board.sections[0].kind, BoardSectionKind::Running);
        assert_eq!(board.sections[0].rows[0].id, "a");
        assert_eq!(board.sections[1].kind, BoardSectionKind::Queued);
        assert_eq!(board.sections[1].rows[0].id, "b");
        assert_eq!(board.sections[2].kind, BoardSectionKind::Scheduled);
        assert_eq!(board.sections[2].rows[0].id, "c");
        assert_eq!(board.sections[2].rows[0].next_fire_ms, Some(60_000));
        assert_eq!(board.sections[3].kind, BoardSectionKind::History);
        assert_eq!(board.sections[3].rows[0].id, "e", "history is newest first");
        assert_eq!(board.sections[3].rows[1].id, "d");
        assert!(board.sections[1].rows[0].attempts == 0);
    }

    /// The wire shape stays stable for the fields the board and scheduler read.
    #[test]
    fn the_json_shape_is_stable() {
        let record = task("task-1");
        let value = serde_json::to_value(&record).expect("json");
        assert_eq!(value["tier"], "subagent");
        assert_eq!(value["status"], "queued");
        assert_eq!(value["trigger"]["kind"], "now");
        assert_eq!(value["host"]["kind"], "local");
        let every = Trigger::Every {
            every_seconds: 60,
            last_run_ms: 0,
        };
        let value = serde_json::to_value(every).expect("json");
        assert_eq!(value["kind"], "every");
        assert_eq!(value["every_seconds"], 60);
    }

    /// New ids stay unique within a millisecond.
    #[test]
    fn new_ids_are_unique() {
        let first = new_id(5);
        let second = new_id(5);
        assert_ne!(first, second);
        assert!(first.starts_with("task-5-"), "got {first}");
    }

    /// The join reports every row, does not claim settling early, and settles
    /// once the runs do.
    #[tokio::test]
    async fn join_reports_each_run_and_settles() {
        use std::collections::BTreeMap;
        use std::sync::Mutex;

        use crate::extension::runs::{RunHandle, RunSnapshot, RunSpec, Runs, RunsService};

        struct Fake {
            snapshots: Mutex<BTreeMap<String, RunSnapshot>>,
        }

        impl RunsService for Fake {
            fn start(&self, _spec: RunSpec) -> Result<RunHandle, String> {
                Err("not used".to_string())
            }

            fn observe(&self, id: &str) -> Option<RunSnapshot> {
                self.snapshots.lock().ok()?.get(id).cloned()
            }

            fn cancel(&self, _id: &str) -> Result<(), String> {
                Ok(())
            }
        }

        let snapshot = |id: &str, state: RunState, result: Option<&str>| RunSnapshot {
            id: id.to_string(),
            state,
            result: result.map(str::to_string),
            tokens: 0,
            steps: 0,
            error: None,
        };
        let fake = std::sync::Arc::new(Fake {
            snapshots: Mutex::new(BTreeMap::from([
                (
                    "run-1".to_string(),
                    snapshot("run-1", RunState::Running, None),
                ),
                (
                    "run-2".to_string(),
                    snapshot("run-2", RunState::Running, None),
                ),
            ])),
        });
        let runs = Runs::with(fake.clone());

        let origin = Origin {
            session: "ses-1".to_string(),
            turn: 1,
        };
        let mut first = Task::open(
            "t1",
            "T1",
            "intent",
            origin.clone(),
            Trigger::Now,
            Tier::Subagent,
            1,
        )
        .expect("t1");
        first.attempts.push(Attempt {
            run: Some("run-1".to_string()),
            status: AttemptStatus::Running,
            ..Attempt::queued("a1", Tier::Subagent)
        });
        let mut second =
            Task::open("t2", "T2", "intent", origin, Trigger::Now, Tier::Job, 2).expect("t2");
        second.attempts.push(Attempt {
            run: Some("run-2".to_string()),
            status: AttemptStatus::Running,
            ..Attempt::queued("a2", Tier::Job)
        });
        let mut waiting = Task::open(
            "t3",
            "T3",
            "intent",
            Origin {
                session: "ses-1".to_string(),
                turn: 1,
            },
            Trigger::Now,
            Tier::Job,
            3,
        )
        .expect("t3");
        waiting.attempts.clear();

        let report = join(|id| runs.observe(id), &[first.clone(), second.clone()], 0).await;
        assert_eq!(report.rows.len(), 2);
        assert!(!report.settled, "live runs are not settled");
        assert_eq!(report.rows[0].run.as_deref(), Some("run-1"));
        assert_eq!(report.rows[0].state, Some(RunState::Running));

        {
            let mut snapshots = fake.snapshots.lock().expect("snapshots");
            snapshots.insert(
                "run-1".to_string(),
                snapshot("run-1", RunState::Succeeded, Some("done one")),
            );
            snapshots.insert(
                "run-2".to_string(),
                snapshot("run-2", RunState::Failed, None),
            );
        }
        let report = join(|id| runs.observe(id), &[first, second, waiting], 0).await;
        assert!(!report.settled, "a task with no run is not settled");
        assert_eq!(report.rows[0].state, Some(RunState::Succeeded));
        assert_eq!(report.rows[0].result.as_deref(), Some("done one"));
        assert_eq!(report.rows[1].state, Some(RunState::Failed));
        assert_eq!(report.rows[2].state, None);
    }
}
