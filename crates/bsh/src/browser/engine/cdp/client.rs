//! The engine process and the CDP client that drives it.
//!
//! # The shape
//!
//! Chromium is an EXTERNAL binary. This module starts it, learns its port, opens
//! one websocket and then hands the socket to a single dedicated thread: that
//! thread is the only reader and the only writer, so every request is written
//! in the order it was queued and every answer is correlated by its `id`
//! before it reaches the caller. Callers never touch the socket, so the GPUI
//! thread can never block on it.
//!
//! # Discovery, in the order it is tried
//!
//! 1. `BSH_CHROMIUM_BIN` - an explicit path, and a missing one is an ERROR that
//!    names the variable rather than silently falling through.
//! 2. Chrome/Chromium on `PATH` or in the standard application locations.
//!
//! # How the port is learned
//!
//! Chromium is launched with `--remote-debugging-port=0`; it atomically writes
//! the chosen port to `DevToolsActivePort` in its private profile.
//!
//! # What the endpoint can actually do
//!
//! The client uses the browser websocket and flattened target sessions so page
//! commands and Fetch events work with Chromium's standard CDP implementation.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the request seam must be quiet before a navigation counts as
/// network-idle.
///
/// MEASURED: the repo's own late-subresource proof starts its image 300 ms
/// after the load event, so the window has to outlast that start for the
/// completion stamp to reset it. 400 ms keeps a 100 ms margin there while
/// taking 100 ms off every settled navigation against the standard 500 ms
/// window, and the load event itself is stamped (see `handle_event`), so the
/// window can never elapse before the load it follows.
pub(crate) const NETWORK_IDLE: Duration = Duration::from_millis(400);
/// Requests that may still be in flight and count as idle.
pub(crate) const IDLE_IN_FLIGHT: u64 = 2;

use serde_json::{json, Value};

use super::super::reap;
use super::wire::{self, Incoming, Request, Socket};
use super::EngineError;

/// How long a launch may take before it is reported as a failed handshake.
///
/// V8 initialization plus the system font scan is measured at ~2-3 s on this
/// machine, so this is generous rather than tight: the cost of being wrong is a
/// pane that says "still starting" when the process is merely slow.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the read loop waits for one frame before doing its other work.
const READ_SLICE: Duration = Duration::from_millis(20);
/// The most a single `call` waits for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// The most stderr/stdout text kept for a failure message.
const LOG_CAP: usize = 8 * 1024;

/// A counter that makes every launch's scratch paths unique within one process.
///
/// The pid and nanosecond clock identify independent launches; the sequence
/// number covers parallel starts within the same clock tick.
static LAUNCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// State the socket thread publishes and the callers read.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Count of `Page.loadEventFired` events, monotonic.
    loads: AtomicU64,
    /// Count of `Page.domContentEventFired` events, monotonic.
    dom_contents: AtomicU64,
    /// Requests the `Fetch` seam paused.
    paused: AtomicU64,
    /// Requests the `Fetch` seam continued.
    continued: AtomicU64,
    /// Requests paused at the seam and not answered yet.
    inflight: AtomicU64,
    /// When the request seam last saw activity, as a monotonic instant.
    last_activity: Mutex<Option<Instant>>,
    /// Whether `Fetch.enable` succeeded, which is what makes the counter
    /// trustworthy. A build without the domain falls back to load + settle.
    fetch_ready: AtomicBool,
    /// Why the connection ended, when it did.
    closed: Mutex<Option<String>>,
    /// Count of DELIBERATE input - a key, a press or release, a wheel, a
    /// navigation, a resize - which is what wakes the frame pump immediately.
    ///
    /// Pointer MOTION is deliberately not counted. A hover repaints `:hover`,
    /// and the pump still sees that (a changed capture resets its quiet count),
    /// but a mouse crossing a pane arrives at ~60 Hz, and letting each move pull
    /// a fresh capture pinned the browser process at its capture ceiling and made
    /// every click and hover arrive late. The pane's pointer path forwards the
    /// move to the page; it does not spend a screenshot on it.
    intent: AtomicU64,
    /// Count of COALESCED hover wakes: a pointer that keeps moving gets at most
    /// one capture per [`HOVER_WAKE`], so `:hover` still lands quickly without
    /// spending a screenshot per motion event.
    hover: AtomicU64,
    /// When the last hover wake was sent, so motion is coalesced.
    hover_wake_at: Mutex<Option<Instant>>,
    /// Held while `intent` is bumped, so a wake cannot be lost between the
    /// pump's counter read and its wait.
    wake_gate: Mutex<()>,
    /// Woken by an intent bump, so a keystroke after an idle second is captured
    /// now instead of at the next idle probe.
    wake: Condvar,
    /// The page target this engine attached to, so discovery can tell a popup
    /// from the pane's own page.
    own_target: Mutex<Option<String>>,
    /// A page target the PAGE opened - `window.open`, a `target="_blank"` link -
    /// as (targetId, url). The pane has one view, so the pump adopts it: the
    /// extra target is closed and its URL navigates this one.
    popup: Mutex<Option<(String, String)>>,
}

impl Shared {
    /// Loads observed so far.
    pub(crate) fn loads(&self) -> u64 {
        self.loads.load(Ordering::Relaxed)
    }

    /// DOM content events observed so far.
    pub(crate) fn dom_contents(&self) -> u64 {
        self.dom_contents.load(Ordering::Relaxed)
    }

    /// Requests paused at the Fetch seam.
    #[cfg(test)]
    pub(crate) fn paused(&self) -> u64 {
        self.paused.load(Ordering::Relaxed)
    }

    /// Requests continued at the Fetch seam.
    #[cfg(test)]
    pub(crate) fn continued(&self) -> u64 {
        self.continued.load(Ordering::Relaxed)
    }

    /// Whether the request counter is trustworthy.
    pub(crate) fn fetch_ready(&self) -> bool {
        self.fetch_ready.load(Ordering::Relaxed)
    }

    /// Requests the seam has paused but not answered.
    pub(crate) fn inflight(&self) -> u64 {
        self.inflight.load(Ordering::Relaxed)
    }

    /// Mark one paused request as answered.
    fn settled(&self) {
        let _ = self
            .inflight
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |inflight| {
                Some(inflight.saturating_sub(1))
            });
        self.touch();
    }

    /// Note that the request seam just saw something.
    ///
    /// This is the NAVIGATION idle clock only; it is not input, so it does not
    /// wake the frame pump. A page that finishes a subresource is picked up by
    /// the pump's next probe.
    pub(crate) fn touch(&self) {
        if let Ok(mut slot) = self.last_activity.lock() {
            slot.replace(Instant::now());
        }
    }

    /// Record deliberate input and wake the frame pump.
    ///
    /// The counter and the wake travel together under [`Shared::wake_gate`], so
    /// an input that lands while the pump is deciding what to do cannot be
    /// missed: either the pump reads the new count or the wait sees the notify.
    fn note_intent(&self) {
        let _gate = self
            .wake_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.intent.fetch_add(1, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// Wake the frame pump for a hover, at most once per [`HOVER_WAKE`].
    ///
    /// ``:hover`` is the one thing a pointer move buys, and it must not cost a
    /// capture per event: MEASURED, a pointer crossing the pane sends a move
    /// every ~16 ms, and one screenshot each pinned the browser process at its
    /// capture ceiling. Coalescing keeps the highlight responsive (one probe per
    /// window) and leaves the page's own repaint to carry the rest.
    fn note_hover(&self) {
        if let Ok(mut at) = self.hover_wake_at.lock() {
            if at.is_some_and(|last| last.elapsed() < HOVER_WAKE) {
                return;
            }
            *at = Some(Instant::now());
        }
        let _gate = self
            .wake_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.hover.fetch_add(1, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// The hover stamp the frame pump reads.
    pub(crate) fn hover(&self) -> u64 {
        self.hover.load(Ordering::Relaxed)
    }

    /// The intent stamp the frame pump reads. See [`Shared::intent`].
    pub(crate) fn intent(&self) -> u64 {
        self.intent.load(Ordering::Relaxed)
    }

    /// Wait for the next intent bump, or for `timeout`.
    pub(crate) fn wait_intent(&self, timeout: Duration) {
        let Ok(gate) = self.wake_gate.lock() else {
            thread::sleep(timeout);
            return;
        };
        let _ = self.wake.wait_timeout(gate, timeout);
    }

    /// Wait until no request has been seen for `idle`, capped at `cap`.
    ///
    /// Returns false when the counter is unavailable (the Fetch domain was
    /// refused) or the cap expired, so the caller can fall back to load +
    /// settle instead of pretending the page went quiet.
    pub(crate) fn wait_network_idle(&self, idle: Duration, cap: Duration) -> bool {
        if !self.fetch_ready() {
            return false;
        }
        let deadline = Instant::now() + cap;
        loop {
            let quiet = self
                .last_activity
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .map(|last| last.elapsed() >= idle)
                .unwrap_or(true);
            if quiet && self.inflight() <= IDLE_IN_FLIGHT {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Why the connection ended, if it did.
    pub(crate) fn closed(&self) -> Option<String> {
        self.closed.lock().ok().and_then(|slot| slot.clone())
    }
}

/// How often a moving pointer may wake the pump.
///
/// Faster than the idle probe, slower than the input cadence: `:hover` reads as
/// "caught up" within a capture, and a sweep costs bounded probes instead of one
/// per motion event.
const HOVER_WAKE: Duration = Duration::from_millis(200);

/// Whether a command is pointer MOTION, which is coalesced rather than
/// deliberate.
fn is_hover(method: &str, params: &Value) -> bool {
    method == "Input.dispatchMouseEvent"
        && params.get("type").and_then(Value::as_str) == Some("mouseMoved")
}

/// One queued request for the socket thread.
struct Job {
    method: String,
    params: Value,
    session: Option<String>,
    reply: Sender<Result<Value, EngineError>>,
}

/// The engine process, its socket thread, and what the thread has seen.
pub(crate) struct Client {
    /// The port the engine bound.
    port: u16,
    /// The target id the browser advertises.
    target: String,
    /// The session every page command carries.
    session: Mutex<String>,
    /// The queue the socket thread drains.
    jobs: Sender<Job>,
    /// The thread's own latch, so `call` can refuse immediately when it died.
    alive: Arc<AtomicBool>,
    /// What the thread published.
    shared: Arc<Shared>,
    /// The child. Killed and reaped on drop.
    child: Mutex<Option<Child>>,
    /// The socket thread.
    thread: Mutex<Option<JoinHandle<()>>>,
    /// The ready file, removed on drop.
    ready_file: PathBuf,
    /// The engine's own profile directory, removed on drop.
    ///
    /// Private per launch: a browser with no profile of its own would keep the
    /// reader's cookies, and two tests would share one cache. Its NAME is the
    /// child's marker: it carries the argv tag the orphan reaper verifies.
    storage_dir: PathBuf,
    /// The pidfile that lets a later start reap this child if we die hard.
    ///
    /// Best effort: a data directory that cannot be written must not stop the
    /// browser from working, so a missing pidfile is a warning and this is
    /// None. The engine still runs; only crash reaping is lost.
    pidfile: Option<PathBuf>,
    /// The tail of the child's own output, for a failure message.
    log: Arc<Mutex<String>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Client")
            .field("port", &self.port)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Launch with an explicit data directory.
    ///
    /// Tests point this at a scratch directory so the pidfile bookkeeping never
    /// touches the reader's `~/.bos`. The reaping pass runs here, before the
    /// spawn, because this is the moment a crashed session's orphan can be told
    /// apart from the child about to exist.
    pub(crate) fn launch_at(
        viewport: (u32, u32),
        state_dir: &std::path::Path,
    ) -> Result<Self, EngineError> {
        let reaped = reap::reap_orphans(state_dir, &reap::SystemProcesses);
        if reaped.reaped_any() {
            eprintln!(
                "bsh: reaped {} orphaned browser engine process(es): {:?}",
                reaped.reaped.len(),
                reaped.reaped
            );
        }
        let binary = discover_binary()?;
        // The sandboxed launch first: it is the one that should guard a
        // process rendering untrusted content. A process that already learned
        // its environment cannot host that sandbox goes straight to the mode
        // that works, so the reader waits once rather than on every pane.
        let preferred = if SANDBOX_BROKEN.load(Ordering::Relaxed) {
            SandboxMode::Relaxed
        } else {
            SandboxMode::Default
        };
        match Self::launch_once(&binary, viewport, state_dir, preferred) {
            Ok(engine) => Ok(engine),
            Err(first) => {
                // Chromium's own sandbox needs privileges a container, a CI
                // runner or a sandboxed harness does not give it. MEASURED
                // there: "sandbox initialization failed: Operation not
                // permitted", the GPU process exits with code 6, and the CDP
                // websocket is reset - so the pane would show nothing but a
                // closed-socket sentence. Retrying without the flags keeps the
                // pane WORKING where the outer sandbox is the boundary that
                // already applies, instead of dying with a correct refusal.
                eprintln!(
                    "bsh: the browser engine did not start with its own sandbox ({}); retrying without it",
                    first.message()
                );
                SANDBOX_BROKEN.store(true, Ordering::Relaxed);
                Self::launch_once(&binary, viewport, state_dir, SandboxMode::Relaxed).inspect_err(
                    |second| {
                        eprintln!(
                            "bsh: the browser engine did not start without its sandbox either ({})",
                            second.message()
                        );
                    },
                )
            }
        }
    }

    /// One launch attempt under one [`SandboxMode`].
    fn launch_once(
        binary: &Path,
        viewport: (u32, u32),
        state_dir: &std::path::Path,
        sandbox: SandboxMode,
    ) -> Result<Self, EngineError> {
        let marker = reap::new_marker(LAUNCH_SEQ.fetch_add(1, Ordering::Relaxed));
        let storage_dir = std::env::temp_dir().join(&marker);
        let ready_file = storage_dir.join("DevToolsActivePort");
        let log = Arc::new(Mutex::new(String::new()));
        let mut child = spawn_engine(binary, &storage_dir, sandbox)?;
        // Record the child BEFORE the handshake: a crash during startup is
        // exactly the case a later pass has to be able to reap.
        let pidfile = match reap::write_pidfile(state_dir, child.id(), &marker) {
            Ok(path) => Some(path),
            Err(error) => {
                // Crash bookkeeping is not a reason to deny the browser.
                eprintln!(
                    "bsh: the browser engine's pidfile could not be written ({error}); a crashed session's child will not be reaped by marker"
                );
                None
            }
        };
        let output = child
            .stdout
            .take()
            .map(|stream| drain(stream, log.clone()))
            .into_iter()
            .chain(child.stderr.take().map(|stream| drain(stream, log.clone())));
        let drainers: Vec<JoinHandle<()>> = output.collect();

        let port = match wait_for_port(&mut child, &ready_file, &log, STARTUP_TIMEOUT) {
            Ok(port) => port,
            Err(error) => {
                cleanup_failed_launch(&mut child, &ready_file, &storage_dir, pidfile.as_deref());
                return Err(error);
            }
        };
        let shared = Arc::new(Shared::default());
        let alive = Arc::new(AtomicBool::new(true));
        let endpoint = match browser_endpoint(port) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                cleanup_failed_launch(&mut child, &ready_file, &storage_dir, pidfile.as_deref());
                return Err(EngineError::Handshake(error));
            }
        };
        let socket = match Socket::connect(&endpoint, CALL_TIMEOUT) {
            Ok(socket) => socket,
            Err(error) => {
                cleanup_failed_launch(&mut child, &ready_file, &storage_dir, pidfile.as_deref());
                return Err(EngineError::Handshake(format!(
                    "the websocket at {endpoint} could not be opened: {error}"
                )));
            }
        };
        let (jobs, queue) = mpsc::channel::<Job>();
        let thread_shared = shared.clone();
        let thread_alive = alive.clone();
        let thread = match thread::Builder::new()
            .name(String::from("bsh-cdp"))
            .spawn(move || run_socket(socket, queue, thread_shared, thread_alive))
        {
            Ok(thread) => thread,
            Err(error) => {
                cleanup_failed_launch(&mut child, &ready_file, &storage_dir, pidfile.as_deref());
                return Err(EngineError::Spawn(format!(
                    "the CDP thread did not start: {error}"
                )));
            }
        };

        let mut client = Self {
            port,
            target: String::new(),
            session: Mutex::new(String::new()),
            jobs,
            alive,
            shared,
            child: Mutex::new(Some(child)),
            thread: Mutex::new(Some(thread)),
            ready_file,
            storage_dir,
            pidfile,
            log,
        };
        // The drain threads end when the child's pipes close; nothing joins
        // them, and a detached reader cannot outlive the process it reads.
        drop(drainers);

        if let Err(error) = client.bootstrap(viewport) {
            // The engine's own output is often the only thing that names a
            // CDP refusal, so it travels with the failure.
            let tail = client.log();
            client.shutdown();
            return Err(match error {
                EngineError::Handshake(message) => {
                    let trimmed = tail.trim();
                    if trimmed.is_empty() {
                        EngineError::Handshake(message)
                    } else {
                        EngineError::Handshake(format!(
                            "{message} (the engine said: {})",
                            wire::truncate(trimmed, 300)
                        ))
                    }
                }
                other => other,
            });
        }
        Ok(client)
    }

    /// Find a page target, bind a session and enable the domains.
    fn bootstrap(&mut self, viewport: (u32, u32)) -> Result<(), EngineError> {
        let target = discover_target(self.port)?;
        // Chromium starts one blank page with the headless process. Reuse it
        // so this process stays a single-page renderer and page state remains
        // isolated to the browser context owned by this pane.
        let attached = self.call_browser(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| {
                EngineError::Handshake(format!(
                    "Target.attachToTarget answered without a session for page {target}"
                ))
            })?;
        if let Ok(mut slot) = self.shared.own_target.lock() {
            *slot = Some(target.clone());
        }
        self.target = target;
        if let Ok(mut slot) = self.session.lock() {
            *slot = session.clone();
        }
        // Watch for page targets this pane did not create. A search result is
        // usually a `target="_blank"` link, and without this the page it opens
        // renders in a window nobody can see, so the click looks dead.
        let _ = self.call_browser("Target.setDiscoverTargets", json!({ "discover": true }));
        self.call("Page.enable", json!({}))?;
        self.call("Runtime.enable", json!({}))?;
        // Observe page requests for the navigation idle clock. Every paused
        // request is continued immediately; browser page traffic stays direct.
        self.call(
            "Fetch.enable",
            json!({ "patterns": [{ "urlPattern": "*" }] }),
        )?;
        // The counter is only meaningful once the domain is on. A build that
        // refuses it keeps load + settle as its "navigated".
        self.shared.fetch_ready.store(true, Ordering::Relaxed);
        self.shared.touch();
        // The launch starts at 1x: the pane's geometry tick applies the
        // window's own scale within its first resize window, and the launch
        // path has no window to read it from.
        self.resize(viewport, 1.0)?;
        Ok(())
    }

    /// The session every page command carries.
    pub(crate) fn session(&self) -> String {
        self.session
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    /// The target the engine created.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    /// The port the engine bound.
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// What the socket thread has seen.
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// The intent stamp the frame pump reads. See [`Shared::intent`].
    pub(crate) fn intent(&self) -> u64 {
        self.shared.intent()
    }

    /// The hover stamp the frame pump reads. See [`Shared::hover`].
    pub(crate) fn hover(&self) -> u64 {
        self.shared.hover()
    }

    /// Wait for the next deliberate input, or for `timeout`.
    pub(crate) fn wait_intent(&self, timeout: Duration) {
        self.shared.wait_intent(timeout);
    }

    /// The (targetId, url) of a page the page itself opened, once.
    pub(crate) fn take_popup(&self) -> Option<(String, String)> {
        self.shared
            .popup
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    /// The tail of the child's own output, for a failure sentence.
    pub(crate) fn log(&self) -> String {
        let text = self.log.lock().map(|slot| slot.clone()).unwrap_or_default();
        let start = text.len().saturating_sub(1200);
        text[start..].to_string()
    }

    /// Call a page method, with the bound session.
    pub(crate) fn call(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        let session = Some(self.session());
        self.dispatch(method, params, session)
    }

    /// Call a browser-level method (Target.*, Browser.*), with no session.
    pub(crate) fn call_browser(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        self.dispatch(method, params, None)
    }

    /// Queue one request and return the channel its answer will arrive on.
    ///
    /// The PUMP uses this instead of [`Client::dispatch`], because a capture
    /// round trip is a queue wait inside the engine and not work: MEASURED at
    /// the pane's real size, one capture at a time costs ~50 ms and does not
    /// shrink with the format, the scale or the region, while two in flight
    /// cost ~25 ms each and eight cost ~10 ms each. The pump therefore keeps
    /// more than one capture outstanding and is never idle across the wait.
    ///
    /// The caller owns the receiver, so a request it no longer wants can be
    /// dropped without disturbing the ones it kept.
    ///
    /// Unlike [`Client::dispatch`], this never notes intent or hover: the
    /// pump's captures are the EFFECT of input, not input themselves, and
    /// counting them would wake the pump for its own probe. Any future caller
    /// that sends deliberate input through here must call
    /// [`Shared::note_intent`] itself.
    pub(crate) fn dispatch_async(
        &self,
        method: &str,
        params: Value,
        session: Option<String>,
    ) -> Result<Receiver<Result<Value, EngineError>>, EngineError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(EngineError::Closed(
                self.shared
                    .closed()
                    .unwrap_or_else(|| String::from("the engine's socket stopped")),
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Job {
                method: method.to_string(),
                params,
                session,
                reply: tx,
            })
            .map_err(|_| {
                EngineError::Closed(
                    self.shared
                        .closed()
                        .unwrap_or_else(|| String::from("the engine's socket stopped")),
                )
            })?;
        Ok(rx)
    }

    /// Queue one request and wait, bounded, for its answer.
    pub(crate) fn dispatch(
        &self,
        method: &str,
        params: Value,
        session: Option<String>,
    ) -> Result<Value, EngineError> {
        // Deliberate input may repaint the page: a navigate, a key, a press, a
        // resize. The pump's own probe is excepted; pointer MOTION is coalesced
        // instead - see [`Shared::intent`] and [`Shared::hover`].
        if method != "Page.captureScreenshot" {
            if is_hover(method, &params) {
                self.shared.note_hover();
            } else {
                self.shared.note_intent();
            }
        }
        if !self.alive.load(Ordering::Relaxed) {
            return Err(EngineError::Closed(
                self.shared
                    .closed()
                    .unwrap_or_else(|| String::from("the engine's socket stopped")),
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.jobs
            .send(Job {
                method: method.to_string(),
                params,
                session,
                reply: tx,
            })
            .map_err(|_| {
                EngineError::Closed(
                    self.shared
                        .closed()
                        .unwrap_or_else(|| String::from("the engine's socket stopped")),
                )
            })?;
        match rx.recv_timeout(CALL_TIMEOUT) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => match error {
                EngineError::Rpc { error, .. } if error.unsupported() => {
                    Err(EngineError::Unsupported {
                        method: method.to_string(),
                        error,
                    })
                }
                other => Err(other),
            },
            Err(RecvTimeoutError::Timeout) => Err(EngineError::Timeout(format!(
                "the engine did not answer {method} within {} s",
                CALL_TIMEOUT.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(EngineError::Closed(
                self.shared
                    .closed()
                    .unwrap_or_else(|| format!("the engine dropped {method}")),
            )),
        }
    }

    /// Queue a request whose answer is not needed.
    ///
    /// Pointer MOVES are the hot path: waiting for a reply would block the GPUI
    /// thread on every hover, and a move has no value to return. The socket
    /// thread still writes jobs in queue order, so a move that precedes a press
    /// cannot overtake it, and a closed engine is reported as a named error.
    pub(crate) fn notify(&self, method: &str, params: Value) -> Result<(), EngineError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(EngineError::Closed(
                self.shared
                    .closed()
                    .unwrap_or_else(|| String::from("the engine's socket stopped")),
            ));
        }
        // A fire-and-forget event is input exactly like a call: a keystroke, a
        // wheel or a press repaints, so the pump wakes for it without this path
        // paying for a reply. Hover motion is coalesced.
        if is_hover(method, &params) {
            self.shared.note_hover();
        } else {
            self.shared.note_intent();
        }
        let (reply, _unread) = mpsc::channel();
        let queued = self.jobs.send(Job {
            method: method.to_string(),
            params,
            session: Some(self.session()),
            reply,
        });
        queued.map_err(|_| {
            EngineError::Closed(
                self.shared
                    .closed()
                    .unwrap_or_else(|| String::from("the engine's socket stopped")),
            )
        })
    }

    /// Resize the page viewport, which is also the capture size.
    ///
    /// `scale` is the WINDOW's device scale factor, and the capture must match
    /// it: a 1x capture drawn into a 2x display's pixels is upsampled and the
    /// page reads blurry (MEASURED: dsf 1 captures 800x600 for a 800x600 point
    /// viewport, which the GPU stretches over 1600x1200 device pixels; dsf 2
    /// captures 1600x1200 and draws one texture pixel per screen pixel).
    pub(crate) fn resize(&self, viewport: (u32, u32), scale: f32) -> Result<(), EngineError> {
        let (width, height) = (viewport.0.clamp(320, 4096), viewport.1.clamp(240, 4096));
        let scale = if (0.5..=4.0).contains(&scale) {
            scale
        } else {
            1.0
        };
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": scale,
                "mobile": false,
            }),
        )
        .map(|_| ())
    }

    /// Wait until a load event newer than `above` arrives, or the deadline.
    pub(crate) fn wait_load(&self, above: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.loads() > above {
                return true;
            }
            if !self.alive.load(Ordering::Relaxed) {
                return false;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Wait until a DOM content event newer than `above` arrives, or the deadline.
    pub(crate) fn wait_dom_content(&self, above: u64, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.shared.dom_contents() > above {
                return true;
            }
            if !self.alive.load(Ordering::Relaxed) {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Close the page target, then stop everything.
    pub(crate) fn shutdown(&mut self) {
        if self.alive.swap(false, Ordering::Relaxed) {
            // The socket thread exits on its own once the channel closes and
            // it sees `alive` down; dropping the sender is what closes it.
            let _ = self.call_browser("Target.closeTarget", json!({ "targetId": self.target }));
        }
        if let Some(thread) = self.thread.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = thread.join();
        }
        if let Some(mut child) = self.child.lock().ok().and_then(|mut slot| slot.take()) {
            kill_and_wait(&mut child);
        }
        remove_file(&self.ready_file);
        if let Some(pidfile) = &self.pidfile {
            let _ = std::fs::remove_file(pidfile);
        }
        let _ = std::fs::remove_dir_all(&self.storage_dir);
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The socket thread: one reader, one writer, in one place.
fn run_socket(
    mut socket: Socket,
    jobs: Receiver<Job>,
    shared: Arc<Shared>,
    alive: Arc<AtomicBool>,
) {
    let mut next_id = 1u64;
    let mut pending: std::collections::HashMap<u64, Sender<Result<Value, EngineError>>> =
        std::collections::HashMap::new();
    loop {
        // 1. Write everything that is queued. Sending is what the caller's
        //    call waits on, so this runs before every read.
        while let Ok(job) = jobs.try_recv() {
            let id = next_id;
            next_id += 1;
            let request = Request {
                id,
                method: job.method.clone(),
                params: job.params,
                session: job.session,
            };
            if let Err(error) = socket.send(&request.encode()) {
                let _ = job.reply.send(Err(EngineError::Transport(error)));
                alive.store(false, Ordering::Relaxed);
                break;
            }
            pending.insert(id, job.reply);
        }
        if !alive.load(Ordering::Relaxed) {
            break;
        }
        // 2. Read one frame, bounded. A quiet socket is not an error.
        match socket.read(READ_SLICE) {
            Ok(None) => {}
            Ok(Some(text)) => match wire::decode(&text) {
                Ok(Incoming::Response { id, result, error }) => {
                    if let Some(reply) = pending.remove(&id) {
                        let outcome = match error {
                            Some(error) => Err(EngineError::Rpc {
                                method: String::new(),
                                error,
                            }),
                            None => Ok(result),
                        };
                        let _ = reply.send(outcome);
                    }
                }
                Ok(Incoming::Event {
                    method,
                    params,
                    session,
                }) => {
                    handle_event(&mut socket, &method, &params, session.as_deref(), &shared);
                }
                Err(error) => {
                    if let Ok(mut slot) = shared.closed.lock() {
                        slot.replace(error);
                    }
                    break;
                }
            },
            Err(error) => {
                if let Ok(mut slot) = shared.closed.lock() {
                    slot.replace(error);
                }
                break;
            }
        }
    }
    alive.store(false, Ordering::Relaxed);
    let reason = shared
        .closed()
        .unwrap_or_else(|| String::from("the engine's socket stopped"));
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err(EngineError::Closed(reason.clone())));
    }
    socket.close();
}

/// Route one event.
fn handle_event(
    socket: &mut Socket,
    method: &str,
    params: &Value,
    event_session: Option<&str>,
    shared: &Shared,
) {
    match method {
        "Page.domContentEventFired" => {
            shared.dom_contents.fetch_add(1, Ordering::Relaxed);
            shared.touch();
        }
        "Target.targetCreated" | "Target.targetInfoChanged" => {
            let Some(info) = params.get("targetInfo") else {
                return;
            };
            if info.get("type").and_then(Value::as_str) != Some("page") {
                return;
            }
            let Some(target) = info.get("targetId").and_then(Value::as_str) else {
                return;
            };
            // Discovery reports the pane's OWN page too; it is not a popup.
            let own = shared.own_target.lock().ok().and_then(|slot| slot.clone());
            if own.as_deref() == Some(target) {
                return;
            }
            // A window is created on `about:blank` and navigated after, so the
            // URL only names the page on the LATER report.
            let url = info.get("url").and_then(Value::as_str).unwrap_or_default();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return;
            }
            if let Ok(mut slot) = shared.popup.lock() {
                slot.replace((target.to_string(), url.to_string()));
            }
        }
        "Target.targetDestroyed" => {
            let Some(target) = params.get("targetId").and_then(Value::as_str) else {
                return;
            };
            if let Ok(mut slot) = shared.popup.lock() {
                if slot.as_ref().is_some_and(|(id, _)| id == target) {
                    slot.take();
                }
            }
        }
        "Page.loadEventFired" => {
            shared.loads.fetch_add(1, Ordering::Relaxed);
            // The load event is network activity too: stamping it means the
            // idle window can never elapse BEFORE the load it follows, so a
            // quiet window that started with the document's own fetch cannot
            // race past a subresource whose stamp lands just after load.
            shared.touch();
        }
        "Fetch.requestPaused" => {
            shared.paused.fetch_add(1, Ordering::Relaxed);
            shared.touch();
            let Some(request_id) = params.get("requestId").and_then(Value::as_str) else {
                return;
            };
            shared.continued.fetch_add(1, Ordering::Relaxed);
            shared.settled();
            // Observation, NOT a gate: the browser is direct, so every paused
            // request is continued. The seam is kept because its pause is the
            // navigation idle clock (`shared.touch`), which is what makes
            // `wait_network_idle` real.
            let mut text = json!({
                "id": 0,
                "method": "Fetch.continueRequest",
                "params": { "requestId": request_id },
            });
            if let Some(session) = event_session {
                text["sessionId"] = Value::String(session.to_string());
            }
            let text = text.to_string();
            let _ = socket.send(&text);
        }
        _ => {}
    }
}

/// The browser endpoint's websocket URL, from `/json/version`.
fn browser_endpoint(port: u16) -> Result<String, String> {
    let value = http_get(port, "/json/version")?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| String::from("/json/version answered without a webSocketDebuggerUrl"))
}

/// The first page target `/json/list` advertises.
fn discover_target(port: u16) -> Result<String, EngineError> {
    let value = http_get(port, "/json/list").map_err(EngineError::Handshake)?;
    let list = value.as_array().ok_or_else(|| {
        EngineError::Handshake(String::from(
            "/json/list answered with something that is not a list",
        ))
    })?;
    list.iter()
        .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .and_then(|target| target.get("id").and_then(Value::as_str))
        .map(String::from)
        .ok_or_else(|| EngineError::Handshake(String::from("/json/list named no page target")))
}

/// One tiny HTTP GET, for the two discovery endpoints.
///
/// Include the port in Host for compatibility with Chromium's DevTools server.
fn http_get(port: u16, path: &str) -> Result<Value, String> {
    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .map_err(|error| format!("the engine's port {port} did not resolve: {error}"))?
        .next()
        .ok_or_else(|| format!("the engine's port {port} has no address"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|error| {
        format!("the engine's HTTP endpoint at {port} refused the connection: {error}")
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("the engine's HTTP endpoint could not be tuned: {error}"))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    let mut stream = stream;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("the engine's HTTP endpoint refused the request: {error}"))?;
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|error| format!("the engine's HTTP endpoint did not answer: {error}"))?;
    if !status_line.contains(" 200 ") {
        let status = status_line.trim();
        return Err(format!("the engine answered {path} with {status}"));
    }
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| format!("the engine's HTTP headers did not finish: {error}"))?;
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = content_length
        .filter(|length| *length <= 1_048_576)
        .ok_or_else(|| {
            format!("the engine's HTTP answer to {path} has no usable Content-Length")
        })?;
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("the engine's HTTP body for {path} was incomplete: {error}"))?;
    let body = String::from_utf8_lossy(&body);
    serde_json::from_str(&body).map_err(|error| {
        format!(
            "the engine's answer to {path} is not JSON ({error}): {}",
            wire::truncate(&body, 200)
        )
    })
}

/// The binary to launch, or a failure that names every place it looked.
pub(crate) fn discover_binary() -> Result<PathBuf, EngineError> {
    let mut searched = Vec::new();
    if let Some(explicit) = std::env::var_os("BSH_CHROMIUM_BIN") {
        let path = PathBuf::from(explicit);
        searched.push(format!("BSH_CHROMIUM_BIN={}", path.display()));
        if path.is_file() {
            return Ok(path);
        }
        return Err(EngineError::MissingBinary {
            searched: searched.join(", "),
            hint: String::from(
                "BSH_CHROMIUM_BIN names a path that is not a file. Point it at Chrome or Chromium.",
            ),
        });
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        searched.push(format!("PATH:{name}"));
        if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())
        }) {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    for path in [
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
    ] {
        searched.push(path.display().to_string());
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(EngineError::MissingBinary {
        searched: searched.join(", "),
        hint: format!(
            "Install Google Chrome or Chromium, or set BSH_CHROMIUM_BIN to its executable. Looked in: {}.",
            searched.join(", ")
        ),
    })
}

/// Start the process with the measured argument set.
/// The engine never runs under the turn's heel sandbox.
///
/// It is a DIRECT child of bsh, not a consumer of a
/// [`agent::sandbox::SandboxSpec`], and its tool results say
/// `sandbox: "none"`. What this scrub closes is the one way heel could still
/// reach it: a bsh started from a sandboxed shell inherits heel's environment,
/// and the proxy variables heel sets would route the engine's traffic through
/// it, while its private TMPDIR points at a scratch the engine was never
/// granted. Removing the SAME reserved list the sandbox sets - one list, two
/// readers - is what makes "no heel for the browser" true there too.
fn scrub_sandbox_env(command: &mut Command) {
    for key in agent::sandbox::RESERVED_ENV {
        command.env_remove(key);
    }
}

/// Whether this process has proved Chromium's own sandbox cannot initialize.
///
/// Set by the first launch that had to fall back, so the NEXT pane in the same
/// process does not pay the doomed attempt again: the sandboxed launch costs a
/// failed spawn, a handshake and its cleanup (MEASURED ~1.5 s) before the retry
/// even starts.
static SANDBOX_BROKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How much of Chromium's own process sandbox to keep.
///
/// [`SandboxMode::Default`] is the normal launch and the one that should
/// guard a process rendering untrusted web content. [`SandboxMode::Relaxed`]
/// exists for the environments that cannot give Chromium what its sandbox
/// needs - a container, a CI runner, a sandboxed harness - where the
/// alternative is a pane that cannot start at all. It is only ever a RETRY,
/// never the first attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SandboxMode {
    /// Chromium keeps its own sandbox.
    Default,
    /// Chromium's sandbox flags are relaxed.
    Relaxed,
}

impl SandboxMode {
    /// The extra Chromium flags this mode adds.
    fn args(self) -> &'static [&'static str] {
        match self {
            SandboxMode::Default => &[],
            // `--disable-dev-shm-usage` rides along because the container case
            // that breaks the sandbox usually has a tiny `/dev/shm` too, and
            // Chromium crashes on a full one.
            SandboxMode::Relaxed => &[
                "--no-sandbox",
                "--disable-gpu-sandbox",
                "--disable-dev-shm-usage",
            ],
        }
    }
}

fn spawn_engine(
    binary: &Path,
    storage_dir: &Path,
    sandbox: SandboxMode,
) -> Result<Child, EngineError> {
    std::fs::create_dir_all(storage_dir).map_err(|error| {
        EngineError::Spawn(format!(
            "the engine's profile directory {} could not be created: {error}",
            storage_dir.display()
        ))
    })?;
    let mut command = Command::new(binary);
    command
        .arg("--headless=new")
        .arg("--remote-debugging-port=0")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--remote-allow-origins=*")
        .arg(format!("--user-data-dir={}", storage_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg("--disable-background-networking")
        .arg("--no-proxy-server")
        .arg("--autoplay-policy=no-user-gesture-required");
    for flag in sandbox.args() {
        command.arg(flag);
    }
    command
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // No heel: not wrapped, and none of its environment either.
    scrub_sandbox_env(&mut command);
    command.spawn().map_err(|error| {
        let _ = std::fs::remove_dir_all(storage_dir);
        EngineError::Spawn(format!(
            "{} could not be started: {error}",
            binary.display()
        ))
    })
}

/// Read DevToolsActivePort until Chromium publishes its selected port.
fn wait_for_port(
    child: &mut Child,
    ready_file: &Path,
    log: &Arc<Mutex<String>>,
    timeout: Duration,
) -> Result<u16, EngineError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(ready_file) {
            if let Some(port) = text
                .lines()
                .next()
                .and_then(|line| line.parse::<u16>().ok())
                .filter(|port| *port > 0)
            {
                return Ok(port);
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(EngineError::Startup {
                status: status.to_string(),
                log: tail(log),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(EngineError::Startup {
        status: String::from("still starting"),
        log: tail(log),
    })
}

/// The tail of a captured log.
fn tail(log: &Arc<Mutex<String>>) -> String {
    let text = log.lock().map(|slot| slot.clone()).unwrap_or_default();
    let start = text.len().saturating_sub(1200);
    text[start..].to_string()
}

/// Copy a child pipe into the bounded log.
fn drain(mut stream: impl Read + Send + 'static, log: Arc<Mutex<String>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut slot) = log.lock() {
                        slot.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        if slot.len() > LOG_CAP {
                            let cut = slot.len() - LOG_CAP;
                            slot.drain(..cut);
                        }
                    }
                }
            }
        }
    })
}

/// Kill and reap, so neither an orphan nor a zombie is left.
fn kill_and_wait(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Remove all per-launch state after startup failed before Client owned it.
fn cleanup_failed_launch(
    child: &mut Child,
    ready_file: &Path,
    storage_dir: &Path,
    pidfile: Option<&Path>,
) {
    kill_and_wait(child);
    remove_file(ready_file);
    if let Some(pidfile) = pidfile {
        remove_file(pidfile);
    }
    let _ = std::fs::remove_dir_all(storage_dir);
}

/// Remove a scratch file, ignoring the ways that can fail.
fn remove_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Where this build keeps its engine pidfiles.
///
/// The harness data directory, not a temp directory: a pidfile is crash
/// bookkeeping that has to survive a reboot's /tmp cleanup as long as the bsh
/// installation does.
pub(crate) fn default_state_dir() -> PathBuf {
    crate::settings::dir().join("browser")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hover_is_coalesced_and_every_other_command_is_deliberate_input() {
        let move_event = json!({ "type": "mouseMoved", "button": "none", "buttons": 0 });
        assert!(
            is_hover("Input.dispatchMouseEvent", &move_event),
            "a hover must be coalesced: it arrives at ~60 Hz"
        );
        for kind in ["mousePressed", "mouseReleased", "mouseWheel"] {
            let event = json!({ "type": kind });
            assert!(
                !is_hover("Input.dispatchMouseEvent", &event),
                "{kind} is the reader's intent"
            );
        }
        assert!(!is_hover(
            "Input.dispatchKeyEvent",
            &json!({ "type": "char" })
        ));
        assert!(!is_hover("Input.insertText", &json!({ "text": "a" })));
        assert!(!is_hover(
            "Page.navigate",
            &json!({ "url": "https://example.com" })
        ));
    }

    #[test]
    fn the_engine_launch_removes_the_sandbox_environment() {
        let mut command = Command::new("chromium");
        for key in agent::sandbox::RESERVED_ENV {
            command.env(key, "heel-value");
        }
        command.env("KEEP_ME", "1");
        scrub_sandbox_env(&mut command);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for key in agent::sandbox::RESERVED_ENV {
            assert!(
                envs.iter()
                    .any(|(name, value)| name == key && value.is_none()),
                "{key} must be removed from the engine's environment"
            );
        }
        assert!(
            envs.iter()
                .any(|(name, value)| name == "KEEP_ME" && value.as_deref() == Some("1")),
            "unrelated variables must survive: {envs:?}"
        );
    }

    #[test]
    fn the_default_launch_is_sandboxed_and_only_the_retry_relaxes() {
        // The first attempt must keep Chromium's own sandbox: it renders
        // untrusted web content. A regression that relaxes the DEFAULT would
        // silently drop that guard on every desktop.
        assert!(SandboxMode::Default.args().is_empty());
        // The retry relaxes precisely what a constrained host needs, and
        // includes the /dev/shm flag because the environments that break the
        // sandbox usually have a tiny one.
        let relaxed = SandboxMode::Relaxed.args();
        assert!(relaxed.contains(&"--no-sandbox"));
        assert!(relaxed.contains(&"--disable-gpu-sandbox"));
        assert!(relaxed.contains(&"--disable-dev-shm-usage"));
    }

    #[test]
    fn browser_markers_are_unique_even_in_one_tick() {
        let first = reap::new_marker(LAUNCH_SEQ.fetch_add(1, Ordering::Relaxed));
        let second = reap::new_marker(LAUNCH_SEQ.fetch_add(1, Ordering::Relaxed));
        assert_ne!(first, second);
        assert!(first.starts_with("bsh-browser-"));
    }
}
