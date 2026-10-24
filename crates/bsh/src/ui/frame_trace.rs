//! An off-by-default per-frame trace for flicker and relayout reports.
//!
//! A report that says "the pane keeps re-rendering" is not diagnosable from a
//! screenshot: the frame count, the geometry and the reason the frame was
//! requested all live inside the view. This module writes exactly that, one
//! bounded line per painted frame, when the reader asks for it:
//!
//!     BSH_FRAME_TRACE=/tmp/bsh-frames.log ./target/debug/bsh
//!
//! With the variable unset the shell keeps None and the render path only
//! matches that option: no file, no buffer, no allocation. The environment is
//! read ONCE, when the shell is assembled, never on a frame.
//!
//! The value 1 (or an empty value) writes to $TMPDIR/bsh-frames-PID.log and
//! prints the path on stderr; any other value is the path. Output stops after
//! DEFAULT_CAP lines with one final notice, so a trace left running cannot
//! fill the disk.

use std::io::Write as _;

/// The facts of one painted frame, derived from the view that painted it.
pub(crate) struct FrameFacts<'a> {
    /// The shell's monotonic frame number.
    pub(crate) frame: u64,
    /// The active tab, as a stable word: chat, trajectory or audit.
    pub(crate) tab: &'a str,
    /// Whether any session is streaming (the running line / composer state).
    pub(crate) streaming: bool,
    /// The transcript scroller's offset (negative going down).
    pub(crate) offset: f32,
    /// The transcript scroller's maximum offset (a positive magnitude).
    pub(crate) max: f32,
    /// The scrollable content height: the maximum offset plus the viewport.
    pub(crate) content: f32,
    /// The transcript's child count (the timestamp row plus the messages).
    pub(crate) rows: usize,
}

/// How many lines one trace writes before it stops, plus a final notice.
pub(crate) const DEFAULT_CAP: usize = 20_000;

/// One open frame trace.
pub(crate) struct FrameTrace {
    file: std::fs::File,
    previous: std::time::Instant,
    lines: usize,
    cap: usize,
    /// The last instrumented request that asked for the frame, if any. Reset
    /// after every line, so a stale reason is never attributed to a click.
    source: Option<&'static str>,
}

impl FrameTrace {
    /// Open a trace from the environment, or None when it is not requested.
    ///
    /// The environment is read here and only here. A failed open reports its
    /// reason on stderr and disables the trace rather than failing the launch.
    pub(crate) fn from_env() -> Option<Self> {
        Self::from_value(std::env::var_os("BSH_FRAME_TRACE"))
    }

    /// The value-shaped half of [Self::from_env], so a test can pin both
    /// branches without touching the process environment.
    pub(crate) fn from_value(value: Option<std::ffi::OsString>) -> Option<Self> {
        let value = value?;
        let text = value.to_string_lossy();
        let path = if text.is_empty() || text == "1" {
            std::env::temp_dir().join(format!("bsh-frames-{}.log", std::process::id()))
        } else {
            std::path::PathBuf::from(text.as_ref())
        };
        match Self::open(&path) {
            Ok(trace) => {
                eprintln!("bsh: frame trace writing to {}", path.display());
                Some(trace)
            }
            Err(error) => {
                eprintln!(
                    "bsh: frame trace could not open {}: {error}",
                    path.display()
                );
                None
            }
        }
    }

    /// Open a trace at an explicit path, with the production line cap.
    pub(crate) fn open(path: &std::path::Path) -> std::io::Result<Self> {
        Self::open_with_cap(path, DEFAULT_CAP)
    }

    /// Open a trace at an explicit path with a chosen cap (tests use a small
    /// one to prove the bound without 20,000 writes).
    pub(crate) fn open_with_cap(path: &std::path::Path, cap: usize) -> std::io::Result<Self> {
        let file = std::fs::File::create(path)?;
        Ok(Self {
            file,
            previous: std::time::Instant::now(),
            lines: 0,
            cap: cap.max(1),
            source: None,
        })
    }

    /// Name the request that asked for the next frame, when it is known.
    pub(crate) fn mark(&mut self, source: &'static str) {
        self.source = Some(source);
    }

    /// Write one line for this frame, or the bounded-cap notice once.
    ///
    /// A write error is ignored: the shell is not allowed to fail a frame
    /// because a diagnostic file went away. The line is a single
    /// space-separated record with fixed keys, so a report can be cut/pasted
    /// and grepped.
    pub(crate) fn record(&mut self, facts: FrameFacts<'_>) {
        if self.lines >= self.cap {
            if self.lines == self.cap {
                let _ = writeln!(
                    self.file,
                    "frame_trace capped at {} lines (BSH_FRAME_TRACE); rerun to start a new file",
                    self.cap
                );
                let _ = self.file.flush();
                self.lines += 1;
            }
            return;
        }
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.previous).as_secs_f64() * 1_000.0;
        let source = self.source.take().unwrap_or("unknown");
        let _ = writeln!(
            self.file,
            "frame={} dt_ms={dt:.1} tab={} streaming={} offset={:.1} max={:.1} content={:.1} rows={} source={source}",
            facts.frame,
            facts.tab,
            if facts.streaming { "yes" } else { "no" },
            facts.offset,
            facts.max,
            facts.content,
            facts.rows,
        );
        let _ = self.file.flush();
        self.previous = now;
        self.lines += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch path unique to one test process and label, removed on entry so
    /// a rerun never appends to an old trace.
    fn scratch(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bsh-frame-trace-{label}-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        dir
    }

    fn facts(frame: u64) -> FrameFacts<'static> {
        FrameFacts {
            frame,
            tab: "chat",
            streaming: true,
            offset: -120.0,
            max: 480.0,
            content: 1_300.0,
            rows: 7,
        }
    }

    /// Off by default is a value branch, not a guess: None opens nothing.
    #[test]
    fn an_unset_environment_opens_no_trace() {
        assert!(FrameTrace::from_value(None).is_none());
    }

    /// Every frame writes exactly one line with every documented key, and the
    /// line names the request that asked for the frame when it is known.
    #[test]
    fn one_frame_writes_one_labelled_line() {
        let path = scratch("one-line");
        {
            let mut trace = FrameTrace::open(&path).expect("the trace opens");
            trace.mark("elapsed-clock");
            trace.record(facts(1));
            trace.record(facts(2));
        }
        let text = std::fs::read_to_string(&path).expect("the trace is readable");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one line per frame: {text}");
        assert!(
            lines[0].starts_with("frame=1 dt_ms="),
            "the line opens with the frame and its delta: {}",
            lines[0]
        );
        for key in [
            " tab=chat ",
            " streaming=yes ",
            " offset=-120.0 ",
            " max=480.0 ",
            " content=1300.0 ",
            " rows=7 ",
            " source=elapsed-clock",
        ] {
            assert!(
                lines[0].contains(key),
                "the line carries {key:?}: {}",
                lines[0]
            );
        }
        assert!(
            lines[1].ends_with(" source=unknown"),
            "a reason is never reused by the next frame: {}",
            lines[1]
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The trace is bounded: a cap of three writes three frames and one final
    /// notice, then nothing more.
    #[test]
    fn a_trace_stops_at_its_cap() {
        let path = scratch("cap");
        {
            let mut trace = FrameTrace::open_with_cap(&path, 3).expect("the trace opens");
            for frame in 0..10 {
                trace.record(facts(frame));
            }
        }
        let text = std::fs::read_to_string(&path).expect("the trace is readable");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "three frames and one notice: {text}");
        assert!(
            lines[3].contains("capped at 3 lines"),
            "the notice says the trace stopped: {}",
            lines[3]
        );
        let _ = std::fs::remove_file(&path);
    }
}
