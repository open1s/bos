//! The engine process and the CDP client that drives it.
//!
//! # The shape
//!
//! Obscura is an EXTERNAL binary. This module starts it, learns its port, opens
//! one websocket and then hands the socket to a single dedicated thread: that
//! thread is the only reader and the only writer, so every request is written
//! in the order it was queued and every answer is correlated by its `id`
//! before it reaches the caller. Callers never touch the socket, so the GPUI
//! thread can never block on it.
//!
//! # Discovery, in the order it is tried
//!
//! 1. `BSH_OBSCURA_BIN` - an explicit path, and a missing one is an ERROR that
//!    names the variable rather than silently falling through.
//! 2. `obscura` on `PATH`.
//! 3. `target/obscura/obscura` in this checkout - the documented development
//!    location, which `.gitignore` keeps out of the tree because the release
//!    tarball is 67 MB and is not vendored.
//!
//! # How the port is learned
//!
//! The engine is launched with `--port 0` (the kernel picks a free port) and
//! `--ready-file`, which Obscura writes ATOMICALLY once it is actually
//! listening: `{"pid":..,"host":..,"port":..,"websocket_path":..}`. This is
//! more robust than scraping the banner, and it still works with `--quiet`.
//! The banner is parsed as a fallback (`CDP server: ws://...`), so a build
//! without `--ready-file` is not a hard failure.
//!
//! # What the endpoint can actually do
//!
//! MEASURED on obscura 0.2.5: `/json/list` advertises a page target whose own
//! websocket (`/devtools/page/page-1`) answers EVERY command with
//! `{"code":-32601,"message":"No page for session"}` - before and after a
//! page exists. The working route is the BROWSER endpoint
//! (`/devtools/browser`) plus `Target.createTarget`/`Target.attachToTarget`,
//! whose `sessionId` every page command must carry. `discover_target` therefore
//! uses `/json/list` to find the target ID and then speaks on the browser
//! socket; the page-endpoint URL is deliberately not used.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the request seam must be quiet before a navigation counts as
/// network-idle. This is the standard 500 ms idle window: a page that starts a
/// subresource just after its load event resets it, which is the whole point.
pub(crate) const NETWORK_IDLE: Duration = Duration::from_millis(500);
/// Requests that may still be in flight and count as idle. The Fetch pause is
/// emitted after the fetch in obscura 0.2.5, so this is a ceiling rather than an
/// exact count.
pub(crate) const IDLE_IN_FLIGHT: u64 = 2;

use serde_json::{json, Value};

use super::super::gate::Gate;
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

/// A counter that makes every launch's scratch paths unique WITHIN one process.
///
/// MEASURED: a ready-file name built from pid + nanoseconds alone collides when
/// two tests launch an engine in parallel (same clock tick), and the second
/// engine then reads the first one's port - which looks like a CDP socket reset
/// and is a path collision.
static LAUNCH_SEQ: AtomicU64 = AtomicU64::new(0);

/// A policy refusal seen at the engine's own request seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The URL the engine asked for.
    pub(crate) url: String,
    /// The rule that refused, when the policy named one.
    pub(crate) rule: Option<String>,
    /// The policy's own prose.
    pub(crate) message: String,
}

/// State the socket thread publishes and the callers read.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Count of `Page.loadEventFired` events, monotonic.
    loads: AtomicU64,
    /// Requests the `Fetch` seam paused.
    paused: AtomicU64,
    /// Requests the `Fetch` seam continued.
    continued: AtomicU64,
    /// Requests the `Fetch` seam failed.
    failed: AtomicU64,
    /// Requests paused at the seam and not answered yet.
    inflight: AtomicU64,
    /// When the request seam last saw activity, as a monotonic instant.
    last_activity: Mutex<Option<Instant>>,
    /// Whether `Fetch.enable` succeeded, which is what makes the counter
    /// trustworthy. A build without the domain falls back to load + settle.
    fetch_ready: AtomicBool,
    /// The most recent policy refusals seen at the seam.
    refusals: Mutex<Vec<Refusal>>,
    /// Why the connection ended, when it did.
    closed: Mutex<Option<String>>,
    /// The session id Obscura attached, learned from the event.
    session: Mutex<Option<String>>,
}

impl Shared {
    /// Loads observed so far.
    pub(crate) fn loads(&self) -> u64 {
        self.loads.load(Ordering::Relaxed)
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

    /// Requests failed at the Fetch seam.
    #[cfg(test)]
    pub(crate) fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
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
    pub(crate) fn touch(&self) {
        if let Ok(mut slot) = self.last_activity.lock() {
            slot.replace(Instant::now());
        }
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

    /// The refusals this engine's request seam has recorded.
    pub(crate) fn refusals(&self) -> Vec<Refusal> {
        self.refusals
            .lock()
            .map(|list| list.clone())
            .unwrap_or_default()
    }

    /// Why the connection ended, if it did.
    pub(crate) fn closed(&self) -> Option<String> {
        self.closed.lock().ok().and_then(|slot| slot.clone())
    }

    /// The session Obscura attached.
    pub(crate) fn session(&self) -> Option<String> {
        self.session.lock().ok().and_then(|slot| slot.clone())
    }

    fn record_refusal(&self, refusal: Refusal) {
        if let Ok(mut list) = self.refusals.lock() {
            list.push(refusal);
            if list.len() > 64 {
                list.remove(0);
            }
        }
    }
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
    /// The target id from `Target.createTarget`.
    target: String,
    /// The session every page command carries.
    session: Mutex<String>,
    /// The queue the socket thread drains.
    jobs: Sender<Job>,
    /// The thread's own latch, so `call` can refuse immediately when it died.
    alive: Arc<AtomicBool>,
    /// What the thread published.
    shared: Arc<Shared>,
    /// The policy seam the process was started with.
    gate: Arc<Gate>,
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
        gate: Arc<Gate>,
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
        let suffix = unique_suffix();
        let marker = reap::new_marker(LAUNCH_SEQ.load(Ordering::Relaxed));
        let ready_file = std::env::temp_dir().join(format!("bsh-obscura-{suffix}.json"));
        let storage_dir = std::env::temp_dir().join(&marker);
        let log = Arc::new(Mutex::new(String::new()));
        let mut child = spawn_engine(&binary, &ready_file, &storage_dir, gate.port())?;
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
                kill_and_wait(&mut child);
                remove_file(&ready_file);
                return Err(error);
            }
        };
        let shared = Arc::new(Shared::default());
        let alive = Arc::new(AtomicBool::new(true));
        let endpoint = match browser_endpoint(port) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                kill_and_wait(&mut child);
                remove_file(&ready_file);
                return Err(EngineError::Handshake(error));
            }
        };
        let socket = match Socket::connect(&endpoint, CALL_TIMEOUT) {
            Ok(socket) => socket,
            Err(error) => {
                kill_and_wait(&mut child);
                remove_file(&ready_file);
                return Err(EngineError::Handshake(format!(
                    "the websocket at {endpoint} could not be opened: {error}"
                )));
            }
        };
        let (jobs, queue) = mpsc::channel::<Job>();
        let thread_shared = shared.clone();
        let thread_alive = alive.clone();
        let thread_gate = gate.clone();
        let thread = thread::Builder::new()
            .name(String::from("bsh-cdp"))
            .spawn(move || run_socket(socket, queue, thread_shared, thread_gate, thread_alive))
            .map_err(|error| {
                EngineError::Spawn(format!("the CDP thread did not start: {error}"))
            })?;

        let mut client = Self {
            port,
            target: String::new(),
            session: Mutex::new(String::new()),
            jobs,
            alive,
            shared,
            gate,
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
        // Attaching to the discovered target is tried first, because that is
        // the contract /json/list advertises. In obscura 0.2.5 that target
        // does not exist in the engine until Target.createTarget runs, so a
        // refusal is expected and the create path is the measured one.
        let attached = self
            .call_browser(
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
            )
            .ok()
            .and_then(|result| {
                result
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(String::from)
            });
        let (target, session) = match attached {
            Some(session) => (target, session),
            None => {
                let created =
                    self.call_browser("Target.createTarget", json!({ "url": "about:blank" }))?;
                let target = created
                    .get("targetId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        EngineError::Handshake(String::from(
                            "Target.createTarget answered without a targetId",
                        ))
                    })?
                    .to_string();
                // The session arrives as a Target.attachedToTarget event,
                // which may already have been read by the socket thread.
                let session = self.wait_session(Duration::from_secs(5)).ok_or_else(|| {
                    EngineError::Handshake(format!(
                        "the engine created target {target} but never attached a session"
                    ))
                })?;
                (target, session)
            }
        };
        self.target = target;
        if let Ok(mut slot) = self.session.lock() {
            *slot = session.clone();
        }
        if let Ok(mut slot) = self.shared.session.lock() {
            slot.replace(session);
        }

        self.call("Page.enable", json!({}))?;
        self.call("Runtime.enable", json!({}))?;
        // The request seam. Interception in 0.2.5 happens AFTER the engine's
        // own fetch (docs/bsh-browser-substrate.md records the measurement),
        // so this is delivery control and observation; the SOCKET guarantee is
        // the loopback gate the process was launched with.
        self.call(
            "Fetch.enable",
            json!({ "patterns": [{ "urlPattern": "*" }] }),
        )?;
        // The counter is only meaningful once the domain is on. A build that
        // refuses it keeps load + settle as its "navigated".
        self.shared.fetch_ready.store(true, Ordering::Relaxed);
        self.shared.touch();
        self.resize(viewport)?;
        Ok(())
    }

    /// Wait for the attached-session event the socket thread records.
    fn wait_session(&self, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(session) = self.shared.session() {
                return Some(session);
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
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

    /// The policy seam this process was launched with.
    pub(crate) fn gate(&self) -> &Arc<Gate> {
        &self.gate
    }

    /// What the socket thread has seen.
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        &self.shared
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

    /// Queue one request and wait, bounded, for its answer.
    pub(crate) fn dispatch(
        &self,
        method: &str,
        params: Value,
        session: Option<String>,
    ) -> Result<Value, EngineError> {
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
        let (reply, _unread) = mpsc::channel();
        self.jobs
            .send(Job {
                method: method.to_string(),
                params,
                session: Some(self.session()),
                reply,
            })
            .map_err(|_| {
                EngineError::Closed(
                    self.shared
                        .closed()
                        .unwrap_or_else(|| String::from("the engine's socket stopped")),
                )
            })
    }

    /// Resize the page viewport, which is also the capture size.
    pub(crate) fn resize(&self, viewport: (u32, u32)) -> Result<(), EngineError> {
        let (width, height) = (viewport.0.clamp(320, 4096), viewport.1.clamp(240, 4096));
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": width,
                "height": height,
                "deviceScaleFactor": 1,
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
    gate: Arc<Gate>,
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
                Ok(Incoming::Event { method, params }) => {
                    handle_event(&mut socket, &method, &params, &shared, &gate);
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
fn handle_event(socket: &mut Socket, method: &str, params: &Value, shared: &Shared, gate: &Gate) {
    match method {
        "Page.loadEventFired" => {
            shared.loads.fetch_add(1, Ordering::Relaxed);
        }
        "Target.attachedToTarget" => {
            if let Some(session) = params.get("sessionId").and_then(Value::as_str) {
                if let Ok(mut slot) = shared.session.lock() {
                    slot.replace(session.to_string());
                }
            }
        }
        "Fetch.requestPaused" => {
            shared.paused.fetch_add(1, Ordering::Relaxed);
            let request_id = params.get("requestId").and_then(Value::as_str);
            if request_id.is_some() {
                shared.inflight.fetch_add(1, Ordering::Relaxed);
            }
            shared.touch();
            let url = params
                .get("request")
                .and_then(|request| request.get("url"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let denied = gate.authorize_url(url);
            let (method, payload) = match (denied, request_id) {
                (Ok(()), Some(request_id)) => {
                    shared.continued.fetch_add(1, Ordering::Relaxed);
                    shared.settled();
                    ("Fetch.continueRequest", json!({ "requestId": request_id }))
                }
                (Err(denial), Some(request_id)) => {
                    shared.failed.fetch_add(1, Ordering::Relaxed);
                    shared.settled();
                    shared.record_refusal(Refusal {
                        url: denial.url,
                        rule: denial.rule,
                        message: denial.message,
                    });
                    (
                        "Fetch.failRequest",
                        json!({ "requestId": request_id, "errorReason": "BlockedByClient" }),
                    )
                }
                (_, None) => return,
            };
            // Events are answered without correlation; the reply is ignored.
            let text = json!({ "id": 0, "method": method, "params": payload }).to_string();
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
/// The `Host` header carries the PORT: Obscura answers 403 to a request whose
/// Host omits it (measured), so a client that sends a bare host would fail at
/// discovery with a refusal that looks like a policy decision.
fn http_get(port: u16, path: &str) -> Result<Value, String> {
    let addr: SocketAddr = format!("127.0.0.1:{port}")
        .to_socket_addrs()
        .map_err(|error| format!("the engine's port {port} did not resolve: {error}"))?
        .next()
        .ok_or_else(|| format!("the engine's port {port} has no address"))?;
    let mut stream =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|error| {
            format!("the engine's HTTP endpoint at {port} refused the connection: {error}")
        })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| format!("the engine's HTTP endpoint could not be tuned: {error}"))?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("the engine's HTTP endpoint refused the request: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("the engine's HTTP endpoint did not answer: {error}"))?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| {
        format!(
            "the engine's HTTP answer had no body: {}",
            wire::truncate(&text, 200)
        )
    })?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(format!("the engine answered {path} with {status}"));
    }
    serde_json::from_str(body).map_err(|error| {
        format!(
            "the engine's answer to {path} is not JSON ({error}): {}",
            wire::truncate(body, 200)
        )
    })
}

/// The binary to launch, or a failure that names every place it looked.
pub(crate) fn discover_binary() -> Result<PathBuf, EngineError> {
    let mut searched = Vec::new();
    if let Some(explicit) = std::env::var_os("BSH_OBSCURA_BIN") {
        let path = PathBuf::from(explicit);
        searched.push(format!("BSH_OBSCURA_BIN={}", path.display()));
        if path.is_file() {
            return Ok(path);
        }
        return Err(EngineError::MissingBinary {
            searched: searched.join(", "),
            hint: String::from(
                "BSH_OBSCURA_BIN names a path that is not a file. Point it at the extracted obscura binary.",
            ),
        });
    }
    searched.push(String::from("PATH"));
    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("obscura"))
            .find(|candidate| candidate.is_file())
    }) {
        return Ok(path);
    }
    let dev = dev_binary();
    searched.push(dev.display().to_string());
    if dev.is_file() {
        return Ok(dev);
    }
    Err(EngineError::MissingBinary {
        searched: searched.join(", "),
        hint: format!(
            "Install Obscura 0.2.5 (release obscura-aarch64-macos.tar.gz from https://github.com/h4ckf0r0day/obscura/releases), \
             extract it, and put obscura on PATH, copy it to {}, or set BSH_OBSCURA_BIN.",
            dev.display()
        ),
    })
}

/// Where a development build is looked for: this checkout's `target/obscura`.
pub(crate) fn dev_binary() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("obscura")
        .join("obscura")
}

/// Start the process with the measured argument set.
/// The engine never runs under the turn's heel sandbox.
///
/// It is a DIRECT child of bsh with its own egress gate, not a consumer of a
/// [`agent::sandbox::SandboxSpec`], and its tool results say
/// `sandbox: "none"`. What this scrub closes is the one way heel could still
/// reach it: a bsh started from a sandboxed shell inherits heel's environment,
/// and heel's proxy would sit in front of (or beside) the browser's own gate
/// while its private TMPDIR points at a scratch the engine was never granted.
/// Removing the SAME reserved list the sandbox sets - one list, two readers -
/// is what makes "no heel for the browser" true there too.
fn scrub_sandbox_env(command: &mut Command) {
    for key in agent::sandbox::RESERVED_ENV {
        command.env_remove(key);
    }
}

fn spawn_engine(
    binary: &Path,
    ready_file: &Path,
    storage_dir: &Path,
    gate_port: u16,
) -> Result<Child, EngineError> {
    std::fs::create_dir_all(storage_dir).map_err(|error| {
        EngineError::Spawn(format!(
            "the engine's profile directory {} could not be created: {error}",
            storage_dir.display()
        ))
    })?;
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg("--port")
        .arg("0")
        .arg("--ready-file")
        .arg(ready_file)
        .arg("--storage-dir")
        .arg(storage_dir)
        .arg("--allow-private-network")
        .arg("--proxy")
        .arg(format!("http://127.0.0.1:{gate_port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // No heel: not wrapped, and none of its environment either.
    scrub_sandbox_env(&mut command);
    // The system fonts, so CJK is not tofu. MEASURED: without --font-dir,
    // Chinese renders as boxes; with it the same page renders Chinese,
    // Japanese and Korean.
    for directory in font_dirs() {
        command.arg("--font-dir").arg(directory);
    }
    command.spawn().map_err(|error| {
        EngineError::Spawn(format!(
            "{} could not be started: {error}",
            binary.display()
        ))
    })
}

/// The font directories that exist on this host, in the order they are tried.
///
/// `BSH_OBSCURA_FONT_DIRS` overrides the list (colon separated; `none` turns
/// the scan off). The defaults are the measured CJK fix on macOS.
pub(crate) fn font_dirs() -> Vec<PathBuf> {
    if let Some(configured) = std::env::var_os("BSH_OBSCURA_FONT_DIRS") {
        let text = configured.to_string_lossy().to_string();
        if text == "none" {
            return Vec::new();
        }
        return text
            .split(':')
            .filter(|part| !part.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .collect();
    }
    let mut candidates = vec![
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/System/Library/Fonts/Supplemental"),
        PathBuf::from("/Library/Fonts"),
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ];
    candidates.retain(|path| path.is_dir());
    candidates
}

/// Read the ready file until it says which port was bound.
fn wait_for_port(
    child: &mut Child,
    ready_file: &Path,
    log: &Arc<Mutex<String>>,
    timeout: Duration,
) -> Result<u16, EngineError> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(ready_file) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(port) = value.get("port").and_then(Value::as_u64) {
                    if port > 0 && port <= u16::MAX as u64 {
                        return Ok(port as u16);
                    }
                }
            }
        }
        if let Some(port) = banner_port(log) {
            return Ok(port);
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

/// The port from the engine's own banner, when it printed one.
fn banner_port(log: &Arc<Mutex<String>>) -> Option<u16> {
    let text = log.lock().ok()?.clone();
    let marker = "CDP server: ws://";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let authority = rest.split('/').next()?;
    let port = authority.rsplit_once(':')?.1.trim();
    port.parse().ok()
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

/// A suffix no other launch in this process can collide with.
fn unique_suffix() -> String {
    let seq = LAUNCH_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}-{seq}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_parser_reads_the_measured_line() {
        let log = Arc::new(Mutex::new(String::from(
            "\n  Headless Browser v0.2.5\n  CDP server: ws://127.0.0.1:63853/devtools/browser\n",
        )));
        assert_eq!(banner_port(&log), Some(63853));
        assert_eq!(banner_port(&Arc::new(Mutex::new(String::new()))), None);
    }

    #[test]
    fn the_dev_location_is_inside_this_checkout() {
        let dev = dev_binary();
        assert!(dev.ends_with("target/obscura/obscura"), "{}", dev.display());
        assert!(dev.is_absolute(), "{}", dev.display());
    }

    #[test]
    fn the_engine_launch_removes_the_sandbox_environment() {
        let mut command = Command::new("obscura");
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
    fn a_launch_suffix_is_unique_even_in_one_tick() {
        let first = unique_suffix();
        let second = unique_suffix();
        assert_ne!(first, second);
        assert!(first.starts_with(&std::process::id().to_string()));
    }
}
