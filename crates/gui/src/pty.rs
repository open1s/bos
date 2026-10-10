//! PTY sessions: a real interactive shell behind the run panel.
//!
//! [`crate::runs`] answers "run this command and show bounded output". A
//! terminal is a different promise: the process stays alive, input goes back
//! in, and what it prints is whatever the shell decides — progress bars,
//! prompts without a newline, colour. This module keeps those sessions on
//! the Rust side (in a [`PtyRegistry`] managed by Tauri) so a webview reload
//! re-attaches to the same live shell instead of killing it.
//!
//! The webview still never sees raw escape sequences: a streaming
//! [`AnsiFilter`] on the pump thread removes them before an event leaves
//! here, so the panel's textContent-only rule keeps holding. A session keeps
//! a capped tail of what it printed, which is what a re-attach replays.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// How many bytes of output a session remembers for a re-attach replay.
const TAIL_BYTES: usize = 64 * 1024;
/// How many chunks the tail remembers at most.
const TAIL_ENTRIES: usize = 400;
/// Read buffer for the pump thread.
const PUMP_BUF: usize = 8192;
/// Largest accepted single input chunk from the panel.
pub const MAX_INPUT_BYTES: usize = 16 * 128;
/// Default window size before the panel knows better.
pub const DEFAULT_COLS: u16 = 120;
/// Default rows before the panel knows better.
pub const DEFAULT_ROWS: u16 = 40;

/// Sentinel in the exit code: the child is still running.
const RUNNING: i32 = -1;
/// Sentinel: the child died on a signal, so there is no exit code.
const SIGNALED: i32 = -2;

/// What a session reports while it runs.
///
/// Emitted through the sink given to [`PtyRegistry::start`]; the app turns
/// each variant into its webview event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyEvent {
    /// One read of output, ANSI sequences already stripped.
    Output {
        /// The session it came from.
        session_id: String,
        /// The text of this read.
        text: String,
    },
    /// The child exited (or was killed).
    Exit {
        /// The session that ended.
        session_id: String,
        /// Its exit code, or `None` when it died on a signal.
        code: Option<i32>,
    },
}

/// Streaming ANSI/escape filter.
///
/// Terminal output is byte soup with embedded sequences: SGR colour, cursor
/// moves, OSC window titles, device queries. The webview must only ever get
/// printable text, so every read passes through here. It is streaming —
/// a sequence split across two reads stays pending until it completes —
/// and it fails open: an escape that never completes is dropped after
/// [`MAX_PENDING`] bytes so a garbage stream cannot grow memory.
#[derive(Debug, Default)]
pub struct AnsiFilter {
    /// Bytes of an incomplete sequence held back from the output.
    pending: Vec<u8>,
}

/// Longest incomplete escape sequence held back before it is given up on.
const MAX_PENDING: usize = 64;

impl AnsiFilter {
    /// New filter with nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one read of raw bytes; returns the printable text to show now.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let mut out = String::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            if b == 0x1b {
                match skip_escape(&data[i..]) {
                    Skip::Consumed(n) => i += n,
                    Skip::Incomplete => {
                        // Hold the tail back until its sequence completes.
                        self.pending.extend_from_slice(&data[i..]);
                        break;
                    }
                }
                continue;
            }
            // Control bytes other than tab/newline carry no text; drop them
            // (bell, carriage return of a progress bar, NUL).
            if b == b'\t' || b == b'\n' || !b.is_ascii_control() {
                let rest = &data[i..];
                let len = utf8_char_len(b);
                match std::str::from_utf8(&rest[..len.min(rest.len())]) {
                    Ok(s) => out.push_str(s),
                    Err(_) => out.push('\u{fffd}'),
                }
            }
            i += 1;
        }
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
        }
        out
    }
}

/// How an escape sequence at the start of `data` ends.
enum Skip {
    /// These bytes are sequence, not text.
    Consumed(usize),
    /// The sequence is cut short; wait for more bytes.
    Incomplete,
}

/// Byte length of the UTF-8 character starting with `first`.
fn utf8_char_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// Classify one escape sequence starting at `data[0] == 0x1b`.
fn skip_escape(data: &[u8]) -> Skip {
    match data.get(1) {
        None => Skip::Incomplete,
        // CSI: ESC [ … final byte in 0x40..=0x7e.
        Some(b'[') => {
            let mut i = 2;
            while i < data.len() {
                let b = data[i];
                if (0x40..=0x7e).contains(&b) {
                    return Skip::Consumed(i + 1);
                }
                i += 1;
            }
            Skip::Incomplete
        }
        // OSC: ESC ] … terminated by BEL or ST (ESC \).
        Some(b']') => {
            let mut i = 2;
            while i < data.len() {
                if data[i] == 0x07 {
                    return Skip::Consumed(i + 1);
                }
                if data[i] == 0x1b && data.get(i + 1) == Some(&b'\\') {
                    return Skip::Consumed(i + 2);
                }
                i += 1;
            }
            Skip::Incomplete
        }
        // Two-byte escapes (charset selection, reset) and anything else:
        // consume the ESC and the byte after it.
        Some(_) => Skip::Consumed(2),
    }
}

/// What a start or re-attach reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyReport {
    /// True when this call spawned a new process (false = re-attached).
    pub fresh: bool,
    /// The shell program the session runs.
    pub shell: String,
    /// What the session printed so far (a re-attach replays this).
    pub tail: Vec<String>,
    /// The live session's exit code; always `None` for a live session.
    pub exited: Option<i32>,
}

/// One live session.
struct Live {
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// [`RUNNING`] while alive, the exit code after, [`SIGNALED`] if killed.
    exited: Arc<AtomicI32>,
    tail: Arc<Mutex<Tail>>,
    shell: String,
}

/// Ring of recent output kept for a re-attach replay.
#[derive(Debug, Default)]
struct Tail {
    /// Recent chunks, oldest first.
    chunks: VecDeque<String>,
    /// Bytes currently in `chunks`.
    bytes: usize,
}

impl Tail {
    /// Remember one chunk, evicting from the front when over budget.
    fn push(&mut self, text: &str) {
        self.chunks.push_back(text.to_string());
        self.bytes += text.len();
        while self.bytes > TAIL_BYTES || self.chunks.len() > TAIL_ENTRIES {
            match self.chunks.pop_front() {
                Some(gone) => self.bytes -= gone.len(),
                None => break,
            }
        }
    }
}

/// All live sessions, keyed by the panel's session id.
///
/// Managed by Tauri next to the app state; a webview reload re-attaches
/// through [`PtyRegistry::start`] instead of spawning a second shell.
#[derive(Default)]
pub struct PtyRegistry {
    /// Sessions by id.
    sessions: Mutex<std::collections::HashMap<String, Arc<Live>>>,
}

impl PtyRegistry {
    /// Lock the session map, treating a poisoned lock as a hard error.
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, std::collections::HashMap<String, Arc<Live>>>, String>
    {
        self.sessions
            .lock()
            .map_err(|_| "session registry lock poisoned".to_string())
    }

    /// Start a session or re-attach to the one already running under `id`.
    ///
    /// `shell` is a program to run; when absent `$SHELL` is used, then
    /// `/bin/sh`. `cols`/`rows` size the pseudo-terminal (clamped). The
    /// `sink` receives every [`PtyEvent`] until the session exits.
    pub fn start(
        &self,
        id: &str,
        shell: Option<&str>,
        cwd: &Path,
        cols: u16,
        rows: u16,
        sink: Arc<dyn Fn(PtyEvent) + Send + Sync>,
    ) -> Result<PtyReport, String> {
        let program = resolve_shell(shell)?;
        let cols = cols.clamp(20, 500);
        let rows = rows.clamp(5, 200);

        // Re-attach first: a reload must not spawn a second shell.
        {
            let sessions = self.lock()?;
            if let Some(live) = sessions.get(id) {
                if live.exited.load(Ordering::SeqCst) == RUNNING {
                    let tail = live
                        .tail
                        .lock()
                        .map_err(|_| "session tail lock poisoned".to_string())?;
                    return Ok(PtyReport {
                        fresh: false,
                        shell: live.shell.clone(),
                        tail: tail.chunks.iter().cloned().collect(),
                        exited: None,
                    });
                }
            }
        }

        let live = spawn_session(id, &program, cwd, cols, rows, sink)?;
        let mut sessions = self.lock()?;
        sessions.insert(id.to_string(), Arc::clone(&live));
        let tail = live
            .tail
            .lock()
            .map_err(|_| "session tail lock poisoned".to_string())?
            .chunks
            .iter()
            .cloned()
            .collect();
        Ok(PtyReport {
            fresh: true,
            shell: program,
            tail,
            exited: None,
        })
    }

    /// Send raw input to a session (a line the user typed, a Ctrl-C byte).
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        if data.len() > MAX_INPUT_BYTES {
            return Err("input too long".to_string());
        }
        let live = {
            let sessions = self.lock()?;
            Arc::clone(
                sessions
                    .get(id)
                    .ok_or_else(|| "no such session".to_string())?,
            )
        };
        if live.exited.load(Ordering::SeqCst) != RUNNING {
            return Err("session has exited".to_string());
        }
        let mut writer = live
            .writer
            .lock()
            .map_err(|_| "session writer lock poisoned".to_string())?;
        writer
            .write_all(data.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|err| format!("write failed: {err}"))
    }

    /// Resize a session's window (clamped to a sane range).
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let live = {
            let sessions = self.lock()?;
            Arc::clone(
                sessions
                    .get(id)
                    .ok_or_else(|| "no such session".to_string())?,
            )
        };
        if live.exited.load(Ordering::SeqCst) != RUNNING {
            return Err("session has exited".to_string());
        }
        let master = live
            .master
            .lock()
            .map_err(|_| "session master lock poisoned".to_string())?;
        master
            .resize(portable_pty::PtySize {
                rows: rows.clamp(5, 200),
                cols: cols.clamp(20, 500),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| format!("resize failed: {err}"))
    }

    /// Kill a session's process; its tail stays until the id is reused.
    pub fn kill(&self, id: &str) -> Result<(), String> {
        let live = {
            let sessions = self.lock()?;
            Arc::clone(
                sessions
                    .get(id)
                    .ok_or_else(|| "no such session".to_string())?,
            )
        };
        let mut killer = live
            .killer
            .lock()
            .map_err(|_| "session killer lock poisoned".to_string())?;
        killer.kill().map_err(|err| format!("kill failed: {err}"))
    }

    /// Whether a session id currently has a (possibly exited) record.
    /// Registry reads otherwise go through `start`'s report, so this exists
    /// for the tests that probe unknown ids.
    #[cfg(test)]
    pub fn contains(&self, id: &str) -> Result<bool, String> {
        let sessions = self.lock()?;
        Ok(sessions.contains_key(id))
    }
}

/// Pick the shell program: explicit, else `$SHELL`, else `/bin/sh`.
fn resolve_shell(shell: Option<&str>) -> Result<String, String> {
    if let Some(program) = shell.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(program.to_string());
    }
    match std::env::var("SHELL") {
        Ok(shell) if !shell.trim().is_empty() => Ok(shell),
        _ => Ok("/bin/sh".to_string()),
    }
}

/// Spawn one interactive session and start its pump and wait threads.
fn spawn_session(
    id: &str,
    program: &str,
    cwd: &Path,
    cols: u16,
    rows: u16,
    sink: Arc<dyn Fn(PtyEvent) + Send + Sync>,
) -> Result<Arc<Live>, String> {
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| format!("openpty failed: {err}"))?;
    let mut cmd = portable_pty::CommandBuilder::new(program);
    cmd.cwd(cwd);
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| format!("spawn failed: {err}"))?;
    // Closing our slave copy lets the child see EOF when it is the last user.
    drop(pair.slave);
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| format!("reader clone failed: {err}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| format!("writer failed: {err}"))?;
    let killer = child.clone_killer();
    let master = pair.master;

    let session_id = id.to_string();
    let tail = Arc::new(Mutex::new(Tail::default()));
    let exited = Arc::new(AtomicI32::new(RUNNING));

    // Pump: raw bytes in, filtered text out, until EOF.
    let wait_sink = Arc::clone(&sink);
    let pump_id = session_id.clone();
    let pump_tail = Arc::clone(&tail);
    let pump_exited = Arc::clone(&exited);
    std::thread::spawn(move || {
        pump(reader, pump_id, sink, pump_tail, pump_exited);
    });

    // Wait: reap the child, record the code, announce the end once.
    let wait_id = session_id.clone();
    let wait_exited = Arc::clone(&exited);
    std::thread::spawn(move || {
        let code = reap(child);
        wait_exited.store(code.unwrap_or(SIGNALED), Ordering::SeqCst);
        wait_sink(PtyEvent::Exit {
            session_id: wait_id,
            code,
        });
    });

    Ok(Arc::new(Live {
        master: Mutex::new(master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
        exited,
        tail,
        shell: program.to_string(),
    }))
}

/// Read until EOF, filtering and forwarding; flushes a trailing partial line.
fn pump(
    mut reader: Box<dyn Read + Send>,
    session_id: String,
    sink: Arc<dyn Fn(PtyEvent) + Send + Sync>,
    tail: Arc<Mutex<Tail>>,
    _exited: Arc<AtomicI32>,
) {
    let mut filter = AnsiFilter::new();
    let mut buf = vec![0u8; PUMP_BUF];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            // A killed PTY read errors out; that is the end of the stream.
            Err(_) => break,
        };
        let text = filter.push(&buf[..n]);
        if text.is_empty() {
            continue;
        }
        if let Ok(mut tail) = tail.lock() {
            tail.push(&text);
        }
        sink(PtyEvent::Output {
            session_id: session_id.clone(),
            text,
        });
    }
}

/// Wait for the child; `None` when it died on a signal rather than exiting.
fn reap(mut child: Box<dyn portable_pty::Child + Send + Sync>) -> Option<i32> {
    match child.wait() {
        Ok(status) if status.signal().is_none() => Some(status.exit_code() as i32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver};

    /// A sink that hands every event to a channel for the test to read.
    fn recording_sink() -> (Arc<dyn Fn(PtyEvent) + Send + Sync>, Receiver<PtyEvent>) {
        let (tx, rx) = channel();
        // `Sender` is not `Sync`; behind the lock it can live in a shared sink.
        let tx = Mutex::new(tx);
        let sink: Arc<dyn Fn(PtyEvent) + Send + Sync> = Arc::new(move |ev| {
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(ev);
            }
        });
        (sink, rx)
    }

    /// Wait for the next event, failing rather than hanging the suite.
    fn next_event(rx: &Receiver<PtyEvent>, label: &str) -> PtyEvent {
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
    }

    /// Collect output until the exit event, returning everything seen.
    fn drain_until_exit(rx: &Receiver<PtyEvent>) -> (String, Option<i32>) {
        let mut text = String::new();
        loop {
            match next_event(rx, "session events") {
                PtyEvent::Output { text: chunk, .. } => text.push_str(&chunk),
                PtyEvent::Exit { code, .. } => return (text, code),
            }
        }
    }

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("bos-gui-pty-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    /// Whether this environment can allocate a PTY at all. The sandbox this
    /// crate is developed in refuses `/dev/ptmx` outright (`EPERM`), which no
    /// code under test can fix — so the spawning tests stand down, loudly, on
    /// that specific refusal and run in full on a normal dev machine.
    fn pty_spawnable() -> bool {
        let probe = portable_pty::native_pty_system().openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        });
        match probe {
            Ok(pair) => {
                drop(pair);
                true
            }
            Err(err) => {
                eprintln!("[pty] spawning tests stand down here: {err}");
                false
            }
        }
    }

    #[test]
    fn ansi_filter_strips_colour_titles_and_moves() {
        let mut f = AnsiFilter::new();
        let out = f.push(b"\x1b[1;31mred\x1b[0m plain\r\n\x1b]0;my title\x07done");
        assert_eq!(out, "red plain\ndone");
        let mut f = AnsiFilter::new();
        assert_eq!(f.push(b"a\x1b[2Kb"), "ab");
    }

    #[test]
    fn ansi_filter_holds_an_incomplete_sequence_until_it_finishes() {
        let mut f = AnsiFilter::new();
        assert_eq!(f.push(b"hi\x1b[3"), "hi");
        assert_eq!(f.push(b"1mx"), "x");
        // An escape that never completes is dropped, not accumulated forever.
        let mut f = AnsiFilter::new();
        for _ in 0..(MAX_PENDING * 2) {
            let _ = f.push(b"\x1b[1");
        }
        assert_eq!(f.pending.len(), 0);
    }

    #[test]
    fn a_session_prints_and_exits_with_its_code() {
        if !pty_spawnable() {
            return;
        }
        let hub = PtyRegistry::default();
        let root = temp_root("exit");
        let (sink, rx) = recording_sink();
        let report = hub
            .start("s1", Some("/bin/sh"), &root, 80, 24, sink)
            .expect("start");
        assert!(report.fresh);
        // The shell is interactive and stays up; drive it to output and exit.
        hub.write("s1", "printf 'hello-from-pty\\n'\nexit 7\n")
            .expect("write");
        let (text, code) = drain_until_exit(&rx);
        assert!(text.contains("hello-from-pty"), "saw {text:?}");
        assert_eq!(code, Some(7));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reattach_reports_the_live_session_and_its_tail() {
        if !pty_spawnable() {
            return;
        }
        let hub = PtyRegistry::default();
        let root = temp_root("reattach");
        let (sink, rx) = recording_sink();
        let (tx_done, rx_done) = channel();
        let tx_done = Mutex::new(tx_done);
        let root2 = root.clone();
        let sink_done: Arc<dyn Fn(PtyEvent) + Send + Sync> = {
            let sink = Arc::clone(&sink);
            Arc::new(move |ev: PtyEvent| {
                sink(ev.clone());
                if matches!(ev, PtyEvent::Exit { .. }) {
                    if let Ok(tx) = tx_done.lock() {
                        let _ = tx.send(());
                    }
                }
            })
        };
        hub.start("s2", Some("/bin/sh"), &root2, 80, 24, sink_done)
            .expect("start");
        hub.write("s2", "printf 'tail-marker\\n'\n").expect("write");
        // Wait until the marker was seen so the tail surely contains it.
        loop {
            if let Ok(PtyEvent::Output { text, .. }) =
                rx.recv_timeout(std::time::Duration::from_secs(10))
            {
                if text.contains("tail-marker") {
                    break;
                }
            } else {
                panic!("timed out waiting for the marker");
            }
        }
        // The same id re-attaches instead of spawning a second shell.
        let again = hub
            .start("s2", None, &root, 80, 24, Arc::new(|_ev: PtyEvent| {}))
            .expect("reattach");
        assert!(!again.fresh);
        assert!(
            again.tail.concat().contains("tail-marker"),
            "tail: {:?}",
            again.tail
        );
        hub.kill("s2").expect("kill");
        rx_done
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("exit after kill");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dead_sessions_id_can_be_started_again() {
        if !pty_spawnable() {
            return;
        }
        let hub = PtyRegistry::default();
        let root = temp_root("restart");
        let (sink, rx) = recording_sink();
        hub.start("s3", Some("/bin/sh"), &root, 80, 24, sink)
            .expect("start");
        hub.write("s3", "exit 0\n").expect("write");
        let (_, code) = drain_until_exit(&rx);
        assert_eq!(code, Some(0));
        // Writing to the dead session fails loudly; starting again succeeds.
        assert!(hub.write("s3", "x\n").is_err());
        let (sink2, _rx2) = recording_sink();
        let report = hub
            .start("s3", Some("/bin/sh"), &root, 80, 24, sink2)
            .expect("restart");
        assert!(report.fresh);
        hub.kill("s3").expect("kill");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_ids_and_oversized_input_are_rejected() {
        let hub = PtyRegistry::default();
        assert!(hub.write("nope", "x\n").is_err());
        assert!(hub.kill("nope").is_err());
        assert!(hub.resize("nope", 80, 24).is_err());
        assert!(!hub.contains("nope").expect("contains"));
        let big = "x".repeat(MAX_INPUT_BYTES + 1);
        // The length guard runs before the session lookup, so no session is
        // needed to prove the cap.
        assert!(hub.write("nope", &big).is_err());
    }

    #[test]
    fn the_shell_defaults_to_she_then_bin_sh() {
        // An explicit shell wins over the environment.
        std::env::set_var("SHELL", "/bin/zsh");
        assert_eq!(
            resolve_shell(Some("/bin/bash")).expect("explicit"),
            "/bin/bash"
        );
        assert_eq!(resolve_shell(None).expect("env"), "/bin/zsh");
        std::env::remove_var("SHELL");
        assert_eq!(resolve_shell(Some("  ")).expect("blank"), "/bin/sh");
    }
}
