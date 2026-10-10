//! A bounded shell-command runner for the run panel.
//!
//! The panel is deliberately **not** a terminal: a PTY backend
//! (`portable-pty`) is not available in this build environment, so a command
//! runs to completion against pipes and its output arrives as plain lines —
//! no interactivity, no ANSI parsing, no color. Everything a run produces is
//! bounded: a line budget, a per-line character clamp, and a wall-clock
//! deadline that kills the child.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Default number of output lines one run may show before it is stopped.
pub(crate) const MAX_LINES: usize = 2_000;

/// Default wall-clock limit for one run.
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest line the panel keeps; the rest of the line is replaced by a marker.
pub(crate) const MAX_LINE_CHARS: usize = 4_096;

/// How many lines one run may show before it is stopped.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    max_lines: usize,
}

impl Budget {
    /// A budget of exactly `max_lines` lines.
    pub(crate) fn new(max_lines: usize) -> Self {
        Self { max_lines }
    }

    /// Whether the next line may still be shown.
    fn accepts(&self, shown: usize) -> bool {
        shown < self.max_lines
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new(MAX_LINES)
    }
}

/// One output line, forwarded to the webview as a `run-line` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunLine {
    /// Which stream the line came from: `"stdout"` or `"stderr"`.
    pub(crate) stream: &'static str,
    /// The text itself, already clamped and stripped of control characters.
    pub(crate) text: String,
}

/// What a run did once it stopped: the `run-finished` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunSummary {
    /// The process exit code, when the child exited on its own.
    pub(crate) exit_code: Option<i32>,
    /// Wall-clock milliseconds from spawn to the last line.
    pub(crate) duration_ms: u64,
    /// How many lines were shown.
    pub(crate) lines: usize,
    /// Whether the run hit a budget (line cap or deadline) and was stopped.
    pub(crate) truncated: bool,
    /// Set when the child could not be started or did not exit cleanly.
    pub(crate) error: Option<String>,
}

/// Check a command before it is spawned: non-empty after trimming, no NUL.
pub(crate) fn validate(command: &str) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("nothing to run".to_string());
    }
    if command.contains('\0') {
        return Err("a command may not contain NUL".to_string());
    }
    Ok(command.to_string())
}

/// Clamp one line for the panel: drop control characters (a progress bar's
/// carriage returns, stray escape bytes), keep tabs, and cut at `max`
/// characters, marking the cut when the line was longer.
pub(crate) fn clamp_line(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    if clean.chars().count() <= max {
        return clean;
    }
    let head: String = clean.chars().take(max).collect();
    format!("{head} …")
}

/// Run `command` under `sh -c` with `root` as the working directory, calling
/// `on_line` for every output line until the child exits, the budget is spent
/// or `timeout` expires — the last two kill the child.
///
/// Both pipes are read on their own threads into one channel, so a chatty
/// `stderr` cannot stall `stdout`: the panel sees the interleaving the OS
/// gave us, line by line. The caller reports the returned summary.
pub(crate) fn run_streaming<F>(
    command: &str,
    root: &Path,
    budget: Budget,
    timeout: Duration,
    mut on_line: F,
) -> RunSummary
where
    F: FnMut(RunLine),
{
    let started = Instant::now();
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return RunSummary {
                exit_code: None,
                duration_ms: 0,
                lines: 0,
                truncated: false,
                error: Some(format!("could not start: {err}")),
            };
        }
    };

    let (tx, rx) = mpsc::channel::<RunLine>();
    if let Some(pipe) = child.stdout.take() {
        pump(pipe, "stdout", tx.clone());
    }
    if let Some(pipe) = child.stderr.take() {
        pump(pipe, "stderr", tx.clone());
    }
    drop(tx);

    let mut shown = 0usize;
    let mut truncated = false;
    loop {
        match rx.recv_timeout(timeout.saturating_sub(started.elapsed())) {
            Ok(line) => {
                if !budget.accepts(shown) {
                    truncated = true;
                    let _ = child.kill();
                    break;
                }
                on_line(RunLine {
                    stream: line.stream,
                    text: clamp_line(&line.text, MAX_LINE_CHARS),
                });
                shown += 1;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                truncated = true;
                let _ = child.kill();
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // Reap the child, so a stopped run does not leave a zombie behind.
    let exit_code = child.wait().ok().and_then(|status| status.code());
    let error = match exit_code {
        Some(0) => None,
        Some(code) => Some(format!("exit code {code}")),
        None => Some("stopped before it finished".to_string()),
    };
    RunSummary {
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        lines: shown,
        truncated,
        error,
    }
}

/// Read one pipe to its end on its own thread, sending each line onward.
///
/// A line that is not valid UTF-8 ends *that* stream with a marker, rather
/// than dropping what follows silently.
fn pump<R>(pipe: R, stream: &'static str, tx: mpsc::Sender<RunLine>)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            match line {
                Ok(text) => {
                    if tx.send(RunLine { stream, text }).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = tx.send(RunLine {
                        stream,
                        text: "[output stopped: not valid UTF-8]".to_string(),
                    });
                    break;
                }
            }
        }
    });
}

/// Most output one job retains, in bytes. Older bytes are dropped from the
/// head as new output arrives, so a chatty job cannot grow without bound.
pub(crate) const MAX_JOB_BYTES: usize = 256 * 1024;

/// Default cap on how much of a job's output one `job_output` call returns.
pub(crate) const DEFAULT_JOB_READ_BYTES: usize = 64 * 1024;

/// Hard ceiling on how much of a job's output one read may return.
pub(crate) const MAX_JOB_READ_BYTES: usize = 1024 * 1024;

/// Most jobs the registry keeps before the oldest settled one is evicted.
pub(crate) const MAX_JOBS: usize = 64;

/// Where a job stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JobState {
    /// Still producing output.
    Running,
    /// Stopped on its own with no error.
    Finished,
    /// Stopped on its own and reported a failure.
    Failed,
    /// Stopped because the user asked it to.
    Interrupted,
}

impl JobState {
    /// Whether the job has stopped.
    pub(crate) fn is_settled(self) -> bool {
        !matches!(self, JobState::Running)
    }
}

/// A background job as the list reports it (no output body).
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct JobRecord {
    /// Stable identifier, generated when the job starts.
    pub(crate) id: String,
    /// Human label, e.g. the command or tool the job runs.
    pub(crate) label: String,
    /// Job family, e.g. `"shell"` or `"subagent"`.
    pub(crate) kind: String,
    /// Where the job stands.
    pub(crate) state: JobState,
    /// Unix seconds the job started.
    pub(crate) started_at: u64,
    /// Unix seconds the job settled, when it has.
    pub(crate) finished_at: Option<u64>,
    /// Bytes of output currently retained.
    pub(crate) bytes: usize,
    /// Bytes dropped from the head because of [`MAX_JOB_BYTES`].
    pub(crate) dropped_bytes: usize,
    /// Failure detail, when the job failed.
    pub(crate) error: Option<String>,
}

/// A bounded window onto one job's output.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct JobOutput {
    /// Job that was read.
    pub(crate) id: String,
    /// Where the job stood when it was read.
    pub(crate) state: JobState,
    /// The most recent retained output, at most the requested cap.
    pub(crate) text: String,
    /// Bytes returned in `text`.
    pub(crate) bytes: usize,
    /// Bytes of output retained for the job (may exceed `bytes`).
    pub(crate) total_bytes: usize,
    /// Whether the read (or retention) dropped anything.
    pub(crate) truncated: bool,
    /// Bytes dropped from the head of the read window.
    pub(crate) dropped_bytes: usize,
}

/// The stored job: its record plus the retained output body.
struct Job {
    record: JobRecord,
    output: String,
}

/// In-memory registry of background jobs with byte-bounded output.
///
/// The registry is deliberately small and process-local: it exists so the UI
/// can list what is running, interrupt it, and page through the tail of what it
/// produced, without ever holding an unbounded string.
#[derive(Default)]
pub(crate) struct Jobs {
    inner: std::sync::Mutex<JobsInner>,
}

#[derive(Default)]
struct JobsInner {
    jobs: Vec<Job>,
    seq: u64,
}

impl Jobs {
    /// An empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a new running job and return its record.
    pub(crate) fn start(&self, label: impl Into<String>, kind: impl Into<String>) -> JobRecord {
        let mut inner = self.inner.lock().unwrap();
        inner.seq += 1;
        let record = JobRecord {
            id: format!("job-{}-{}", std::process::id(), inner.seq),
            label: label.into(),
            kind: kind.into(),
            state: JobState::Running,
            started_at: crate::session::unix_now(),
            finished_at: None,
            bytes: 0,
            dropped_bytes: 0,
            error: None,
        };
        inner.jobs.push(Job {
            record: record.clone(),
            output: String::new(),
        });
        evict(&mut inner.jobs);
        record
    }

    /// Append output to a running job, keeping only the most recent bytes.
    pub(crate) fn push(&self, id: &str, text: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let job = inner
            .jobs
            .iter_mut()
            .find(|job| job.record.id == id)
            .ok_or_else(|| format!("unknown job '{id}'"))?;
        job.output.push_str(text);
        let dropped = trim_head(&mut job.output, MAX_JOB_BYTES);
        job.record.dropped_bytes += dropped;
        job.record.bytes = job.output.len();
        Ok(())
    }

    /// Settle a job. Unknown ids are ignored so a late finish cannot panic.
    pub(crate) fn finish(&self, id: &str, state: JobState, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(job) = inner.jobs.iter_mut().find(|job| job.record.id == id) {
            job.record.state = state;
            job.record.finished_at = Some(crate::session::unix_now());
            job.record.error = error;
            job.record.bytes = job.output.len();
        }
    }

    /// Interrupt a job by id, returning its updated record.
    ///
    /// An already-settled job is returned unchanged (interrupting something
    /// that has stopped is a no-op, not an error); an unknown id is a clean
    /// error.
    pub(crate) fn interrupt(&self, id: &str) -> Result<JobRecord, String> {
        let mut inner = self.inner.lock().unwrap();
        let job = inner
            .jobs
            .iter_mut()
            .find(|job| job.record.id == id)
            .ok_or_else(|| format!("unknown job '{id}'"))?;
        if !job.record.state.is_settled() {
            job.record.state = JobState::Interrupted;
            job.record.finished_at = Some(crate::session::unix_now());
        }
        Ok(job.record.clone())
    }

    /// Every job, newest first. Never includes output bodies.
    pub(crate) fn list(&self) -> Vec<JobRecord> {
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<JobRecord> = inner.jobs.iter().map(|job| job.record.clone()).collect();
        out.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        out
    }

    /// Read the tail of a job's output, bounded by `max_bytes`.
    pub(crate) fn output(&self, id: &str, max_bytes: Option<usize>) -> Result<JobOutput, String> {
        let inner = self.inner.lock().unwrap();
        let job = inner
            .jobs
            .iter()
            .find(|job| job.record.id == id)
            .ok_or_else(|| format!("unknown job '{id}'"))?;
        let cap = max_bytes
            .unwrap_or(DEFAULT_JOB_READ_BYTES)
            .min(MAX_JOB_READ_BYTES);
        let (text, cut) = clip_tail(&job.output, cap);
        Ok(JobOutput {
            id: job.record.id.clone(),
            state: job.record.state,
            bytes: text.len(),
            total_bytes: job.output.len(),
            truncated: cut || job.record.dropped_bytes > 0,
            dropped_bytes: job.record.dropped_bytes + (job.output.len() - text.len()),
            text,
        })
    }
}

/// Drop bytes from the head of `text` until it fits `max`, on a char boundary.
/// Returns how many bytes were dropped.
fn trim_head(text: &mut String, max: usize) -> usize {
    if text.len() <= max {
        return 0;
    }
    let mut cut = text.len() - max;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
    cut
}

/// Take the last `max` bytes of `text`, on a char boundary.
fn clip_tail(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_string(), false);
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

/// Keep the registry at [`MAX_JOBS`], preferring to drop settled jobs.
fn evict(jobs: &mut Vec<Job>) {
    while jobs.len() > MAX_JOBS {
        let victim = jobs
            .iter()
            .position(|job| job.record.state.is_settled())
            .unwrap_or(0);
        jobs.remove(victim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch directory the commands may run in.
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-gui-test-runs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// Collect every line a run produces, alongside its summary.
    fn run(command: &str, budget: Budget, timeout: Duration) -> (Vec<RunLine>, RunSummary) {
        let mut lines = Vec::new();
        let summary = run_streaming(command, &scratch(), budget, timeout, |line| {
            lines.push(line)
        });
        (lines, summary)
    }

    #[test]
    fn an_empty_command_is_refused() {
        let err = validate("   ").expect_err("blank must be refused");
        assert!(err.contains("nothing to run"), "{err}");
        assert!(validate("").is_err());
    }

    #[test]
    fn a_nul_byte_in_a_command_is_refused() {
        let err = validate("echo\0 hi").expect_err("NUL must be refused");
        assert!(err.contains("NUL"), "{err}");
        assert_eq!(validate(" echo hi ").unwrap(), "echo hi");
    }

    #[test]
    fn clamping_cuts_a_long_line_and_marks_the_cut() {
        let cut = clamp_line(&"x".repeat(50), 10);
        assert_eq!(cut.chars().count(), 12, "{cut}"); // 10 kept + " …"
        assert!(cut.ends_with(" …"), "{cut}");
        assert_eq!(clamp_line("short", 10), "short");
    }

    #[test]
    fn control_characters_are_dropped_but_tabs_survive() {
        // A progress bar's carriage returns and an escape sequence must not
        // reach the panel as raw bytes; a tab is part of the text.
        assert_eq!(clamp_line("a\tb\r\rc", 100), "a\tbc");
        assert_eq!(clamp_line("\u{1b}[31mred", 100), "[31mred");
    }

    #[test]
    fn stdout_and_stderr_both_arrive() {
        let (lines, summary) = run("echo out; echo err >&2", Budget::default(), RUN_TIMEOUT);
        let mut streams: Vec<&str> = lines.iter().map(|l| l.stream).collect();
        streams.sort_unstable();
        assert_eq!(streams, ["stderr", "stdout"]);
        let mut texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        texts.sort_unstable();
        assert_eq!(texts, ["err", "out"]);
        assert_eq!(summary.exit_code, Some(0));
        assert_eq!(summary.lines, 2, "{summary:?}");
        assert!(!summary.truncated && summary.error.is_none(), "{summary:?}");
    }

    #[test]
    fn a_failing_command_reports_its_exit_code() {
        let (lines, summary) = run("exit 7", Budget::default(), RUN_TIMEOUT);
        assert!(lines.is_empty());
        assert_eq!(summary.exit_code, Some(7));
        assert_eq!(summary.error.as_deref(), Some("exit code 7"));
        assert!(!summary.truncated);
    }

    #[test]
    fn the_line_budget_stops_a_chatty_run_and_reports_the_cut() {
        let (lines, summary) = run(
            "i=0; while [ $i -lt 500 ]; do echo \"line $i\"; i=$((i+1)); done",
            Budget::new(5),
            RUN_TIMEOUT,
        );
        assert_eq!(lines.len(), 5, "{:?}", lines.len());
        assert_eq!(summary.lines, 5, "{summary:?}");
        assert!(summary.truncated, "{summary:?}");
    }

    #[test]
    fn a_hung_command_is_stopped_at_the_deadline() {
        let started = Instant::now();
        let (lines, summary) = run("sleep 5", Budget::default(), Duration::from_millis(200));
        assert!(lines.is_empty());
        assert!(summary.truncated, "{summary:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "killed late");
        assert_eq!(summary.exit_code, None);
    }

    /* ---- M7: the job registry ---- */

    #[test]
    fn a_job_starts_running_and_settles() {
        let jobs = Jobs::new();
        let job = jobs.start("cargo test", "shell");
        assert_eq!(job.state, JobState::Running);
        assert_eq!(job.bytes, 0);
        assert_eq!(job.dropped_bytes, 0);
        assert!(job.finished_at.is_none());

        jobs.push(&job.id, "hello\n").expect("push");
        jobs.finish(&job.id, JobState::Finished, None);

        let listed = jobs.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, JobState::Finished);
        assert_eq!(listed[0].bytes, "hello\n".len());
        assert!(listed[0].finished_at.is_some());

        let read = jobs.output(&job.id, None).expect("read");
        assert_eq!(read.text, "hello\n");
        assert_eq!(read.total_bytes, 6);
        assert!(!read.truncated);
    }

    /// A chatty job's output is bounded, and the read says it was cut.
    #[test]
    fn job_output_is_bounded_with_a_truncation_flag() {
        let jobs = Jobs::new();
        let job = jobs.start("chatty", "shell");
        let chunk = format!("{}\n", "x".repeat(1023));
        for _ in 0..(MAX_JOB_BYTES / chunk.len() + 8) {
            jobs.push(&job.id, &chunk).expect("push");
        }
        let listed = jobs.list();
        assert!(
            listed[0].bytes <= MAX_JOB_BYTES,
            "retention must be bounded: {}",
            listed[0].bytes
        );
        assert!(listed[0].dropped_bytes > 0, "the head was dropped");

        let read = jobs.output(&job.id, None).expect("read");
        assert!(read.text.len() <= DEFAULT_JOB_READ_BYTES, "read is capped");
        assert!(read.truncated, "the read must report the cut");
        assert!(read.total_bytes > read.bytes);

        // A larger explicit cap returns more, still bounded by the hard ceiling.
        let wide = jobs
            .output(&job.id, Some(MAX_JOB_READ_BYTES * 8))
            .expect("read");
        assert!(wide.text.len() <= MAX_JOB_READ_BYTES);
        assert!(wide.text.len() >= read.text.len());
    }

    /// Interrupting an unknown job is a clean error, never a panic.
    #[test]
    fn interrupting_an_unknown_job_fails_cleanly() {
        let jobs = Jobs::new();
        let err = jobs
            .interrupt("job-does-not-exist")
            .expect_err("unknown job");
        assert!(err.contains("unknown job"), "{err}");
        assert!(jobs.output("job-does-not-exist", None).is_err());
        assert!(jobs.push("job-does-not-exist", "x").is_err());
        // A late finish for a job that never existed is silently ignored.
        jobs.finish("job-does-not-exist", JobState::Failed, None);
        assert!(jobs.list().is_empty());
    }

    /// Interrupting a running job settles it; a second interrupt is a no-op.
    #[test]
    fn interrupt_settles_a_running_job_once() {
        let jobs = Jobs::new();
        let job = jobs.start("long runner", "subagent");
        let stopped = jobs.interrupt(&job.id).expect("interrupt");
        assert_eq!(stopped.state, JobState::Interrupted);
        assert!(stopped.finished_at.is_some());
        let again = jobs.interrupt(&job.id).expect("second interrupt");
        assert_eq!(again.state, JobState::Interrupted);
        assert_eq!(again.finished_at, stopped.finished_at, "not re-stamped");
    }

    /// The registry evicts settled jobs rather than growing forever.
    #[test]
    fn the_registry_is_bounded() {
        let jobs = Jobs::new();
        for index in 0..(MAX_JOBS + 10) {
            let job = jobs.start(format!("job {index}"), "shell");
            jobs.finish(&job.id, JobState::Finished, None);
        }
        assert_eq!(jobs.list().len(), MAX_JOBS);
    }

    /// Tail clipping never splits a multi-byte character.
    #[test]
    fn tail_clipping_respects_char_boundaries() {
        let text = "记".repeat(100);
        let (clipped, cut) = clip_tail(&text, 51);
        assert!(cut);
        assert!(clipped.len() <= 51);
        assert!(clipped.chars().all(|c| c == '记'));
        let mut owned = text.clone();
        let dropped = trim_head(&mut owned, 51);
        assert!(dropped > 0);
        assert!(owned.len() <= 51);
        assert!(owned.chars().all(|c| c == '记'));
    }
}
