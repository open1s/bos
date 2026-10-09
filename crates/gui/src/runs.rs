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
}
