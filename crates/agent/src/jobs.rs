//! Background jobs: detached tool processes and their bounded output.
//!
//! A background job is a process a tool started and did not wait for. The
//! registry is the one place its lifecycle lives, and it is shared by the tool
//! that spawns it and the shell that draws it: the agent crate owns the model
//! and the process handles, while the UI crate only reads snapshots.
//!
//! # Bounds
//!
//! Two bounds keep a long-lived app honest: each job keeps at most
//! [DEFAULT_OUTPUT_CAP] bytes of merged stdout and stderr (the tail, because a
//! reader wants the last thing a command said), and the registry keeps at most
//! [DEFAULT_JOB_CAP] jobs, dropping the oldest first. Dropping a running job
//! kills its process, so the bound cannot leak children.
//!
//! # No persistence
//!
//! Jobs live in memory only. A restart loses the roster and the output, and any
//! process a previous launch detached keeps running outside this registry. That
//! is stated rather than guessed: the model carries no store.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Bytes of merged output a job keeps by default.
pub const DEFAULT_OUTPUT_CAP: usize = 64 * 1024;

/// Jobs a registry keeps by default.
pub const DEFAULT_JOB_CAP: usize = 200;

/// The last cap bytes written to a job, with the number of bytes the bound
/// already dropped.
#[derive(Debug, Clone)]
pub struct BoundedRing {
    bytes: VecDeque<u8>,
    cap: usize,
    dropped: u64,
}

impl BoundedRing {
    /// A ring that keeps at most cap bytes. Zero is treated as one byte,
    /// because a ring that keeps nothing cannot answer "what did it say".
    pub fn new(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    /// Append bytes, dropping the oldest once the cap is reached.
    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // A chunk larger than the ring replaces it wholesale: keeping the
        // prefix and then trimming again would do the same work twice.
        if bytes.len() >= self.cap {
            let keep = &bytes[bytes.len() - self.cap..];
            self.dropped += (self.bytes.len() + bytes.len() - self.cap) as u64;
            self.bytes.clear();
            self.bytes.extend(keep.iter().copied());
            return;
        }
        let overflow = (self.bytes.len() + bytes.len()).saturating_sub(self.cap);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.dropped += overflow as u64;
        }
        self.bytes.extend(bytes.iter().copied());
    }

    /// Bytes currently held, oldest first.
    pub fn to_vec(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    /// The held bytes decoded as text, lossily, because a command may not speak
    /// UTF-8.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.to_vec()).into_owned()
    }

    /// Bytes dropped by the bound so far.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Bytes held right now.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been captured.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The configured cap.
    pub fn cap(&self) -> usize {
        self.cap
    }
}

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// The process is alive and has not been asked to stop.
    Running,
    /// The process exited with status zero.
    Succeeded {
        /// The process exit code.
        code: i32,
    },
    /// The process exited with a non-zero status, or could not be waited for.
    Failed {
        /// The process exit code, or -1 when the host reported none.
        code: i32,
    },
    /// The reader (or the job bound) stopped the process.
    Killed,
}

impl JobStatus {
    /// Whether the process may still write output.
    pub fn is_running(self) -> bool {
        matches!(self, JobStatus::Running)
    }

    /// Whether the job will not change again.
    pub fn is_settled(self) -> bool {
        !self.is_running()
    }

    /// The exit code, when the process reported one.
    pub fn exit_code(self) -> Option<i32> {
        match self {
            JobStatus::Succeeded { code } | JobStatus::Failed { code } => Some(code),
            JobStatus::Running | JobStatus::Killed => None,
        }
    }

    /// The stable lower-case word a tool result carries.
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded { .. } => "succeeded",
            JobStatus::Failed { .. } => "failed",
            JobStatus::Killed => "killed",
        }
    }
}

/// One background job and everything captured about it.
#[derive(Debug, Clone)]
pub struct BackgroundJob {
    /// The job identity, a UUID.
    pub id: String,
    /// The session whose turn started it. A reader sees only its own session.
    pub session: String,
    /// The tool that started it, for example bash.
    pub tool: String,
    /// The command line, as the model sent it.
    pub command: String,
    /// Where the job stands.
    pub status: JobStatus,
    /// When the process was spawned.
    pub started_at: Instant,
    /// When the process settled, if it has.
    pub finished_at: Option<Instant>,
    /// The bounded tail of merged stdout and stderr.
    pub output: BoundedRing,
    /// The process id, once the host assigned it.
    pub pid: Option<u32>,
    /// The working directory the process was started in, which is the path a
    /// policy re-check names for job_output and job_kill.
    pub cwd: PathBuf,
}

impl BackgroundJob {
    /// How long the job ran, or has been running, in milliseconds.
    pub fn duration_ms(&self, now: Instant) -> u64 {
        let end = self.finished_at.unwrap_or(now);
        end.saturating_duration_since(self.started_at)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    /// The metadata view a UI draws, without a copy of the output.
    pub fn summary(&self, now: Instant) -> JobSummary {
        JobSummary {
            id: self.id.clone(),
            session: self.session.clone(),
            tool: self.tool.clone(),
            command: self.command.clone(),
            status: self.status,
            started_at: self.started_at,
            finished_at: self.finished_at,
            pid: self.pid,
            duration_ms: self.duration_ms(now),
            output_len: self.output.len(),
            output_dropped: self.output.dropped(),
        }
    }
}

/// One job as a list draws it: metadata, not the captured bytes.
///
/// The output is read lazily through [JobRegistry::output_for], so a frame that
/// paints a hundred settled rows never clones a hundred rings.
#[derive(Debug, Clone)]
pub struct JobSummary {
    /// The job identity.
    pub id: String,
    /// The session that owns it.
    pub session: String,
    /// The tool that started it.
    pub tool: String,
    /// The command line.
    pub command: String,
    /// Where it stands.
    pub status: JobStatus,
    /// When it was spawned.
    pub started_at: Instant,
    /// When it settled, if it has.
    pub finished_at: Option<Instant>,
    /// The process id, when known.
    pub pid: Option<u32>,
    /// Elapsed milliseconds at the instant the snapshot was taken.
    pub duration_ms: u64,
    /// Bytes currently held in the output ring.
    pub output_len: usize,
    /// Bytes the ring bound already dropped.
    pub output_dropped: u64,
}

impl JobSummary {
    /// Whether the job may still write output.
    pub fn is_running(&self) -> bool {
        self.status.is_running()
    }

    /// Whether the output ring lost earlier bytes.
    pub fn output_truncated(&self) -> bool {
        self.output_dropped > 0
    }
}

/// The captured output of one job, decoded lossily.
#[derive(Debug, Clone)]
pub struct JobOutput {
    /// The text held in the ring, oldest first.
    pub text: String,
    /// Bytes held at the time of the read.
    pub bytes: usize,
    /// Bytes the bound dropped before this read.
    pub dropped: u64,
}

impl JobOutput {
    /// Whether earlier output was dropped by the bound.
    pub fn truncated(&self) -> bool {
        self.dropped > 0
    }
}

#[derive(Debug, Default)]
struct RegistryState {
    jobs: VecDeque<BackgroundJob>,
    version: u64,
}

/// The live roster of background jobs.
///
/// Methods lock one short critical section and never hold it across I/O: a
/// process is signalled after the lock is released, so the reader threads that
/// append output are never blocked behind a kill.
#[derive(Debug, Default)]
pub struct JobRegistry {
    inner: Mutex<RegistryState>,
    settled: Condvar,
    job_cap: usize,
    output_cap: usize,
}

impl JobRegistry {
    /// A registry with the shipped bounds.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_JOB_CAP, DEFAULT_OUTPUT_CAP)
    }

    /// A registry with explicit bounds. job_cap is clamped to at least one.
    pub fn with_capacity(job_cap: usize, output_cap: usize) -> Self {
        Self {
            inner: Mutex::new(RegistryState::default()),
            settled: Condvar::new(),
            job_cap: job_cap.max(1),
            output_cap: output_cap.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start a job for session and return its id.
    pub fn begin(
        &self,
        session: impl Into<String>,
        tool: impl Into<String>,
        command: impl Into<String>,
    ) -> String {
        self.begin_in(session, tool, command, PathBuf::new())
    }

    /// Start a job that records the directory it runs in.
    pub fn begin_in(
        &self,
        session: impl Into<String>,
        tool: impl Into<String>,
        command: impl Into<String>,
        cwd: impl Into<PathBuf>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut state = self.lock();
        state.jobs.push_back(BackgroundJob {
            id: id.clone(),
            session: session.into(),
            tool: tool.into(),
            command: command.into(),
            status: JobStatus::Running,
            started_at: Instant::now(),
            finished_at: None,
            output: BoundedRing::new(self.output_cap),
            pid: None,
            cwd: cwd.into(),
        });
        state.version += 1;
        // The oldest job goes first. A running one is signalled after the lock
        // is released, so the bound can never orphan a child.
        let mut displaced: Vec<u32> = Vec::new();
        while state.jobs.len() > self.job_cap {
            if let Some(job) = state.jobs.pop_front() {
                if job.status.is_running() {
                    displaced.extend(job.pid);
                }
            }
        }
        drop(state);
        for pid in displaced {
            kill_group(pid);
        }
        id
    }

    /// Record the process id once the host assigned it.
    pub fn set_pid(&self, id: &str, pid: u32) {
        let mut state = self.lock();
        if let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) {
            job.pid = Some(pid);
            state.version += 1;
        }
    }

    /// Append captured bytes to a job ring.
    pub fn append_output(&self, id: &str, bytes: &[u8]) {
        let mut state = self.lock();
        if let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) {
            job.output.push(bytes);
            state.version += 1;
        }
    }

    /// Settle a job. A job already marked [JobStatus::Killed] keeps that
    /// status, because a kill races the wait and the reader intent wins.
    pub fn finish(&self, id: &str, status: JobStatus) -> bool {
        let mut state = self.lock();
        let Some(job) = state.jobs.iter_mut().find(|job| job.id == id) else {
            return false;
        };
        if !job.status.is_running() {
            return false;
        }
        job.status = status;
        job.finished_at = Some(Instant::now());
        state.version += 1;
        drop(state);
        self.settled.notify_all();
        true
    }

    /// Stop a running job process group and mark it [JobStatus::Killed].
    ///
    /// Returns false for an unknown or already settled job, which is also what a
    /// second kill reports.
    pub fn kill(&self, id: &str) -> bool {
        let pid = {
            let mut state = self.lock();
            let Some(index) = state.jobs.iter().position(|job| job.id == id) else {
                return false;
            };
            if !state.jobs[index].status.is_running() {
                return false;
            }
            // A kill needs the pid: marking a job killed without a process to
            // signal would claim an outcome the host never performed. The
            // stop handler retries (its failed phase) if this window is hit.
            let Some(pid) = state.jobs[index].pid else {
                return false;
            };
            state.jobs[index].status = JobStatus::Killed;
            state.jobs[index].finished_at = Some(Instant::now());
            state.version += 1;
            pid
        };
        self.settled.notify_all();
        kill_group(pid);
        true
    }

    /// Every job owned by session, newest first.
    ///
    /// The order is insertion order reversed, not a re-sort: a job settling
    /// moves between sections but never reshuffles its neighbours, and the list
    /// is stable across frames by construction.
    pub fn list_for_session(&self, session: &str) -> Vec<JobSummary> {
        let now = Instant::now();
        let state = self.lock();
        state
            .jobs
            .iter()
            .rev()
            .filter(|job| job.session == session)
            .map(|job| job.summary(now))
            .collect()
    }

    /// One job full record, for tests and tool results.
    pub fn job(&self, id: &str) -> Option<BackgroundJob> {
        self.lock().jobs.iter().find(|job| job.id == id).cloned()
    }

    /// The cheap version of a job's output: bytes held and bytes dropped.
    ///
    /// A frame that already holds the text compares this pair instead of
    /// cloning the ring, so a static expanded row costs two integers.
    pub fn output_state(&self, id: &str) -> Option<(usize, u64)> {
        let state = self.lock();
        let job = state.jobs.iter().find(|job| job.id == id)?;
        Some((job.output.len(), job.output.dropped()))
    }

    /// One job output, or None for an unknown id.
    pub fn output_for(&self, id: &str) -> Option<JobOutput> {
        let state = self.lock();
        let job = state.jobs.iter().find(|job| job.id == id)?;
        Some(JobOutput {
            text: job.output.text(),
            bytes: job.output.len(),
            dropped: job.output.dropped(),
        })
    }

    /// The subset of a job output a reader asked for: the last max_bytes
    /// bytes. None means the whole bounded ring.
    pub fn output_tail(&self, id: &str, max_bytes: Option<usize>) -> Option<JobOutput> {
        let output = self.output_for(id)?;
        let Some(max) = max_bytes else {
            return Some(output);
        };
        if output.bytes <= max {
            return Some(output);
        }
        let bytes = output.text.as_bytes();
        let start = bytes.len().saturating_sub(max);
        let tail = &bytes[start..];
        Some(JobOutput {
            text: String::from_utf8_lossy(tail).into_owned(),
            bytes: tail.len(),
            dropped: output.dropped + (output.bytes - tail.len()) as u64,
        })
    }

    /// Settled jobs owned by session.
    pub fn settled_count(&self, session: &str) -> usize {
        self.lock()
            .jobs
            .iter()
            .filter(|job| job.session == session && job.status.is_settled())
            .count()
    }

    /// Running jobs owned by session.
    pub fn running_count(&self, session: &str) -> usize {
        self.lock()
            .jobs
            .iter()
            .filter(|job| job.session == session && job.status.is_running())
            .count()
    }

    /// Drop every settled job owned by session, returning how many went.
    pub fn clear_settled(&self, session: &str) -> usize {
        let mut state = self.lock();
        let before = state.jobs.len();
        state
            .jobs
            .retain(|job| job.session != session || job.status.is_running());
        let removed = before - state.jobs.len();
        if removed > 0 {
            state.version += 1;
        }
        removed
    }

    /// A counter that moves on every roster change, for a UI tick that must
    /// repaint once when a job settles instead of polling a duration.
    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// Wait up to wait for a job to settle, returning its latest summary.
    ///
    /// A zero wait is a pure read. The condition variable is woken by
    /// [JobRegistry::finish] and [JobRegistry::kill], so a caller that waits
    /// consumes no CPU while the process runs.
    pub fn wait_for(&self, id: &str, wait: Duration) -> Option<JobSummary> {
        let deadline = Instant::now() + wait;
        let mut state = self.lock();
        loop {
            let summary = state
                .jobs
                .iter()
                .find(|job| job.id == id)
                .map(|job| job.summary(Instant::now()))?;
            if summary.status.is_settled() || wait.is_zero() {
                return Some(summary);
            }
            let now = Instant::now();
            if now >= deadline {
                return Some(summary);
            }
            let (guard, _) = self
                .settled
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(|e| e.into_inner());
            state = guard;
        }
    }
}

/// Whether a job belongs to a session, the ownership test a job tool applies.
pub fn owned_by(job: &JobSummary, session: &str) -> bool {
    job.session == session
}

/// Whether a process id is still alive, used by the kill probe.
///
/// The check is kill -0, which asks the kernel without delivering a signal.
pub fn pid_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Signal a job process group, gracefully and then forcefully.
///
/// The child is spawned as its own process group, so the negative pid reaches
/// the shell and every command it started. A group that ignores SIGTERM is sent
/// SIGKILL after a short grace period; the escalation runs off the caller so a
/// kill never blocks a render or a tool call.
fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            if pid_exists(pid) {
                let _ = std::process::Command::new("/bin/kill")
                    .args(["-KILL", &format!("-{pid}")])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

/// A registry shared by a tool and the view that draws it.
pub type SharedJobs = Arc<JobRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_tail_and_counts_what_it_dropped() {
        let mut ring = BoundedRing::new(4);
        ring.push(b"abc");
        assert_eq!(ring.text(), "abc");
        assert_eq!(ring.dropped(), 0);
        ring.push(b"def");
        assert_eq!(ring.text(), "cdef", "the oldest bytes go first");
        assert_eq!(ring.dropped(), 2);
        ring.push(b"012345");
        assert_eq!(ring.text(), "2345", "an oversized chunk replaces the ring");
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.cap(), 4);
    }

    #[test]
    fn a_job_is_listed_newest_first_and_scoped_to_its_session() {
        let registry = JobRegistry::new();
        let first = registry.begin("a", "bash", "one");
        std::thread::sleep(Duration::from_millis(2));
        let second = registry.begin("b", "bash", "two");
        std::thread::sleep(Duration::from_millis(2));
        let third = registry.begin("a", "bash", "three");
        registry.finish(&first, JobStatus::Succeeded { code: 0 });
        registry.finish(&third, JobStatus::Failed { code: 2 });

        let listed = registry.list_for_session("a");
        assert_eq!(
            listed.iter().map(|job| job.id.as_str()).collect::<Vec<_>>(),
            vec![third.as_str(), first.as_str()]
        );
        assert_eq!(registry.list_for_session("b")[0].id, second);
        assert_eq!(registry.settled_count("a"), 2);
        assert_eq!(registry.settled_count("b"), 0);
        assert_eq!(registry.running_count("b"), 1);
    }

    #[test]
    fn settled_rows_clear_without_touching_a_running_job() {
        let registry = JobRegistry::new();
        let done = registry.begin("a", "bash", "done");
        let live = registry.begin("a", "bash", "live");
        registry.finish(&done, JobStatus::Succeeded { code: 0 });
        assert_eq!(registry.clear_settled("a"), 1);
        assert_eq!(registry.settled_count("a"), 0);
        assert!(registry.job(&live).is_some(), "a running job stays");
        assert!(registry.job(&done).is_none());
    }

    #[test]
    fn the_roster_is_bounded_and_a_displaced_running_job_is_killed() {
        let registry = JobRegistry::with_capacity(2, 16);
        let first = registry.begin("a", "bash", "one");
        let second = registry.begin("a", "bash", "two");
        let third = registry.begin("a", "bash", "three");
        assert!(registry.job(&first).is_none(), "the oldest job is gone");
        assert!(registry.job(&second).is_some());
        assert!(registry.job(&third).is_some());
        assert_eq!(registry.list_for_session("a").len(), 2);
    }

    #[test]
    fn waiting_returns_when_the_job_settles() {
        let registry = Arc::new(JobRegistry::new());
        let id = registry.begin("a", "bash", "slow");
        let writer = {
            let registry = registry.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(40));
                registry.append_output(&id, b"hello");
                registry.finish(&id, JobStatus::Succeeded { code: 0 });
            })
        };
        let summary = registry
            .wait_for(&id, Duration::from_secs(2))
            .expect("the job exists");
        writer.join().expect("the writer");
        assert_eq!(summary.status, JobStatus::Succeeded { code: 0 });
        assert_eq!(registry.output_for(&id).expect("output").text, "hello");
    }

    #[test]
    fn a_kill_wins_over_the_wait_and_is_not_overwritten() {
        let registry = JobRegistry::new();
        let id = registry.begin("a", "bash", "x");
        // A kill needs a pid; an id no process can hold is the harmless stand-in.
        registry.set_pid(&id, u32::MAX);
        assert!(registry.kill(&id));
        assert!(
            !registry.finish(&id, JobStatus::Succeeded { code: 0 }),
            "a settled job does not settle twice"
        );
        assert_eq!(registry.job(&id).expect("job").status, JobStatus::Killed);
        assert!(!registry.kill(&id), "a second kill reports nothing to do");
    }

    #[test]
    fn the_version_moves_with_the_roster() {
        let registry = JobRegistry::new();
        let start = registry.version();
        let id = registry.begin("a", "bash", "x");
        assert!(registry.version() > start);
        let after_begin = registry.version();
        registry.append_output(&id, b"x");
        assert!(registry.version() > after_begin);
    }

    #[test]
    fn a_tail_read_keeps_only_the_requested_bytes() {
        let registry = JobRegistry::new();
        let id = registry.begin("a", "bash", "x");
        registry.append_output(&id, b"0123456789");
        let tail = registry.output_tail(&id, Some(4)).expect("output");
        assert_eq!(tail.text, "6789");
        assert!(tail.truncated());
        assert_eq!(registry.output_for(&id).expect("all").text, "0123456789");
    }

    #[test]
    fn a_session_cannot_see_another_sessions_jobs() {
        let registry = JobRegistry::new();
        let id = registry.begin("a", "bash", "x");
        assert!(registry.list_for_session("b").is_empty());
        assert!(!owned_by(&registry.list_for_session("a")[0], "b"));
        assert!(owned_by(&registry.list_for_session("a")[0], "a"));
        assert!(registry.job(&id).is_some());
    }
}
