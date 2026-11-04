//! The Model Context Protocol runtime.
//!
//! MCP is an adapter, not the platform's internal protocol. A package selects
//! kind = "mcp"; the host starts the server, performs initialize and
//! notifications/initialized, lists tools/resources/prompts, and maps them onto
//! BSH tool and context/prompt contributions. The MCP types never reach the
//! Agent API: only tool descriptors and text contributions do.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::contract::{CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::declarative::{DeclarativeContributions, TextContribution};
use ferris_core::capability::{CallScope, Capability, PolicyEngine};
use ferris_core::events::{EventBus, EventFilter, Subscription};
use ferris_core::manifest::{is_allowlisted_command, validate_relative_path, Manifest};
use ferris_core::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, TrustLevel, HOST_VERSION,
};

/// The protocol version this adapter sends.
const MCP_PROTOCOL: &str = "2024-11-05";

/// The handshake deadline.
const HANDSHAKE_MS: u64 = 10_000;

/// The largest one-line response.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// The largest number of each contribution kind read from one server.
const MAX_ITEMS: usize = 256;

/// The largest number of catalog pages one discovery may read.
const MAX_CATALOG_PAGES: usize = 64;

/// The aggregate byte ceiling for one discovered catalog.
const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;

/// The optional capabilities the server advertised during initialize.
#[derive(Default, Clone)]
struct ServerCapabilities {
    resources: bool,
    prompts: bool,
}

impl ServerCapabilities {
    fn from_initialize(value: &Value) -> Self {
        let capabilities = value.get("capabilities");
        Self {
            resources: capabilities
                .and_then(|caps| caps.get("resources"))
                .is_some(),
            prompts: capabilities.and_then(|caps| caps.get("prompts")).is_some(),
        }
    }
}

fn catalog_size(descriptor: &ToolDescriptor) -> usize {
    descriptor.id.as_str().len()
        + descriptor.description.len()
        + descriptor.input_schema.to_string().len()
}

fn catalog_limit(extension: &ExtensionId, bytes: usize) -> ExtensionError {
    ExtensionError::Limit {
        extension: extension.to_string(),
        message: format!("MCP catalog is {bytes} bytes; the ceiling is {MAX_CATALOG_BYTES}"),
    }
}

/// One write queued for the MCP writer thread.
struct WriteRequest {
    line: Vec<u8>,
    done: std::sync::mpsc::SyncSender<std::io::Result<()>>,
}

/// One live MCP server.
/// One spawned MCP server: a std child on Unix, or an AppContainer child on
/// Windows. The runtime only needs id, piped stdio, kill and wait.
enum McpChild {
    #[cfg_attr(windows, allow(dead_code))]
    Std(Child),
    #[cfg(windows)]
    Sandbox(heel::Child),
}

impl McpChild {
    #[cfg_attr(windows, allow(dead_code))]
    fn id(&self) -> u32 {
        match self {
            Self::Std(child) => child.id(),
            #[cfg(windows)]
            Self::Sandbox(child) => child.id(),
        }
    }

    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        match self {
            Self::Std(child) => child
                .stdin
                .take()
                .map(|stdin| Box::new(stdin) as Box<dyn std::io::Write + Send>),
            #[cfg(windows)]
            Self::Sandbox(child) => child
                .take_stdin()
                .map(|stdin| Box::new(stdin) as Box<dyn std::io::Write + Send>),
        }
    }

    fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match self {
            Self::Std(child) => child
                .stdout
                .take()
                .map(|stdout| Box::new(stdout) as Box<dyn std::io::Read + Send>),
            #[cfg(windows)]
            Self::Sandbox(child) => child
                .take_stdout()
                .map(|stdout| Box::new(stdout) as Box<dyn std::io::Read + Send>),
        }
    }

    fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
        match self {
            Self::Std(child) => child
                .stderr
                .take()
                .map(|stderr| Box::new(stderr) as Box<dyn std::io::Read + Send>),
            #[cfg(windows)]
            Self::Sandbox(child) => child
                .take_stderr()
                .map(|stderr| Box::new(stderr) as Box<dyn std::io::Read + Send>),
        }
    }

    fn kill(&mut self) {
        match self {
            Self::Std(child) => {
                let _ = child.kill();
            }
            #[cfg(windows)]
            Self::Sandbox(child) => {
                let _ = child.kill();
            }
        }
    }

    fn wait(&mut self) {
        match self {
            Self::Std(child) => {
                let _ = child.wait();
            }
            #[cfg(windows)]
            Self::Sandbox(child) => {
                let _ = futures_lite::future::block_on(child.wait());
            }
        }
    }
}

struct Session {
    child: Arc<Mutex<McpChild>>,
    /// A dedicated writer thread, so a child that stops reading stdin cannot
    /// block the caller (or an executor worker) in write_all.
    writer: std::sync::mpsc::SyncSender<WriteRequest>,
    lines: Receiver<Vec<u8>>,
    next_id: u64,
    /// Optional capabilities the server advertised at initialize time.
    capabilities: ServerCapabilities,
    /// Set when the server announces a catalog list change.
    catalog_dirty: Arc<AtomicBool>,
}

impl Session {
    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
    ) -> Result<Value, ExtensionError> {
        self.request_with_progress(method, params, timeout_ms, None)
    }

    fn request_with_progress(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        progress: Option<&tokio::sync::mpsc::Sender<Value>>,
    ) -> Result<Value, ExtensionError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_vec(&request).map_err(|error| ExtensionError::Protocol {
            extension: "mcp".to_string(),
            message: error.to_string(),
        })?;
        line.push(b'\n');
        self.send_line(line, timeout_ms)?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ExtensionError::Timeout {
                    extension: "mcp".to_string(),
                    operation: method.to_string(),
                    millis: timeout_ms,
                });
            }
            let bytes = self
                .lines
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => ExtensionError::Timeout {
                        extension: "mcp".to_string(),
                        operation: method.to_string(),
                        millis: timeout_ms,
                    },
                    RecvTimeoutError::Disconnected => ExtensionError::Protocol {
                        extension: "mcp".to_string(),
                        message: "server closed stdout".to_string(),
                    },
                })?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|error| ExtensionError::Protocol {
                    extension: "mcp".to_string(),
                    message: format!("response is not JSON: {error}"),
                })?;
            if value
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|method| method.ends_with("/list_changed"))
            {
                self.catalog_dirty.store(true, Ordering::SeqCst);
            }
            if let Some(result) = validate_response(&value, id, progress)? {
                return Ok(result);
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), ExtensionError> {
        let line = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .map_err(|error| ExtensionError::Protocol {
            extension: "mcp".to_string(),
            message: error.to_string(),
        })?;
        let mut line = line;
        line.push(b'\n');
        self.send_line(line, HANDSHAKE_MS)
    }

    /// Queue one line for the writer thread and wait, under the call deadline,
    /// for it to be written. A child that stops reading makes the queue fill;
    /// send_timeout then fails instead of blocking indefinitely.
    fn send_line(&mut self, line: Vec<u8>, timeout_ms: u64) -> Result<(), ExtensionError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
        let (done, result) = sync_channel(0);
        let mut request = WriteRequest { line, done };
        loop {
            match self.writer.try_send(request) {
                Ok(()) => break,
                Err(std::sync::mpsc::TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        self.close();
                        return Err(ExtensionError::Timeout {
                            extension: "mcp".to_string(),
                            operation: "write".to_string(),
                            millis: timeout_ms,
                        });
                    }
                    request = returned;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    self.close();
                    return Err(protocol_error("MCP writer thread stopped"));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match result.recv_timeout(remaining) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.close();
                Err(ExtensionError::Io {
                    path: PathBuf::from("<mcp stdin>"),
                    source: error,
                })
            }
            Err(RecvTimeoutError::Timeout) => {
                self.close();
                Err(ExtensionError::Timeout {
                    extension: "mcp".to_string(),
                    operation: "write".to_string(),
                    millis: timeout_ms,
                })
            }
            Err(RecvTimeoutError::Disconnected) => Err(protocol_error("MCP writer thread stopped")),
        }
    }

    /// Drain queued notifications outside a request and mark a list change.
    fn drain_notifications(&mut self) {
        while let Ok(bytes) = self.lines.try_recv() {
            if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                if value
                    .get("method")
                    .and_then(Value::as_str)
                    .is_some_and(|method| method.ends_with("/list_changed"))
                {
                    self.catalog_dirty.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    fn close(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            kill_process_group(&child);
            child.kill();
            child.wait();
        }
    }
}

/// Validate one inbound frame against the JSON-RPC envelope this client sent.
///
/// Returns the matching response's `result`, or `None` for a notification or
/// a late response to another id. Anything that claims to answer this id but
/// does not have exactly one well-formed outcome is a protocol error, so a
/// malformed server cannot turn a missing result into a silent `null`.
fn validate_response(
    value: &Value,
    expected_id: u64,
    progress: Option<&tokio::sync::mpsc::Sender<Value>>,
) -> Result<Option<Value>, ExtensionError> {
    let object = value
        .as_object()
        .ok_or_else(|| protocol_error("response is not a JSON object"))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(protocol_error("response is missing jsonrpc \"2.0\""));
    }
    let frame_id = match object.get("id") {
        None => None,
        Some(Value::Number(number)) => Some(
            number
                .as_u64()
                .ok_or_else(|| protocol_error("response id is not an integer"))?,
        ),
        Some(_) => return Err(protocol_error("response id is not an integer")),
    };
    if frame_id != Some(expected_id) {
        if let Some(sender) = progress {
            if object.get("method").and_then(Value::as_str) == Some("notifications/progress") {
                if let Some(params) = object.get("params") {
                    let _ = sender.try_send(params.clone());
                }
            }
        }
        // A notification has no id. A response for a different id is usually a
        // timed-out call's late result; neither answers this request.
        return Ok(None);
    }
    if object.contains_key("method") {
        return Err(protocol_error("response carries a method field"));
    }
    match (object.get("result"), object.get("error")) {
        (Some(result), None) => Ok(Some(result.clone())),
        (None, Some(error)) => {
            let code = error.get("code").and_then(Value::as_i64);
            let message = error.get("message").and_then(Value::as_str);
            let (Some(code), Some(message)) = (code, message) else {
                return Err(protocol_error(
                    "error object lacks an integer code or a string message",
                ));
            };
            Err(ExtensionError::Remote {
                extension: "mcp".to_string(),
                message: format!("JSON-RPC error {code}: {message}"),
            })
        }
        (Some(_), Some(_)) => Err(protocol_error("response has both result and error")),
        (None, None) => Err(protocol_error("response has neither result nor error")),
    }
}

/// A protocol violation attributed to the MCP transport.
fn protocol_error(message: impl Into<String>) -> ExtensionError {
    ExtensionError::Protocol {
        extension: "mcp".to_string(),
        message: message.into(),
    }
}

/// One line read under a hard byte ceiling.
enum LimitedLine {
    /// A complete, in-ceiling line.
    Line(Vec<u8>),
    /// The line exceeded the ceiling; the reader consumed through its newline,
    /// so the next call starts at the following line.
    TooLong,
    /// End of stream.
    Eof,
}

/// Read a newline-terminated line without ever buffering past the ceiling.
///
/// `read_until` grows the buffer as it reads, so a server that never emits a
/// newline could make the host allocate without bound before the size check
/// ran. This reader checks the ceiling against the available slice and only
/// copies the bytes it will keep.
fn read_limited_line_sync<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<LimitedLine> {
    let mut line = Vec::new();
    loop {
        let (consumed, complete, eof, too_long) = {
            let buffer = reader.fill_buf()?;
            let room = limit.saturating_sub(line.len());
            if buffer.is_empty() {
                (0, false, true, false)
            } else if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                let take = index + 1;
                if take > room {
                    line.extend_from_slice(&buffer[..room]);
                    (take, true, false, true)
                } else {
                    line.extend_from_slice(&buffer[..take]);
                    (take, true, false, false)
                }
            } else if buffer.len() > room {
                line.extend_from_slice(&buffer[..room]);
                (buffer.len(), false, false, true)
            } else {
                line.extend_from_slice(buffer);
                (buffer.len(), false, false, false)
            }
        };
        if consumed > 0 {
            reader.consume(consumed);
        }
        if too_long {
            // A complete line was already consumed through its newline; only
            // an unterminated tail still needs discarding.
            if !complete {
                discard_rest_of_line_sync(reader)?;
            }
            return Ok(LimitedLine::TooLong);
        }
        if eof {
            return Ok(if line.is_empty() {
                LimitedLine::Eof
            } else {
                LimitedLine::Line(line)
            });
        }
        if complete {
            return Ok(LimitedLine::Line(line));
        }
    }
}

/// Consume the remainder of an over-long line through its newline.
fn discard_rest_of_line_sync<R: BufRead>(reader: &mut R) -> std::io::Result<()> {
    loop {
        let (consumed, done) = {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(());
            }
            match buffer.iter().position(|byte| *byte == b'\n') {
                Some(index) => (index + 1, true),
                None => (buffer.len(), false),
            }
        };
        reader.consume(consumed);
        if done {
            return Ok(());
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            kill_process_group(&child);
            child.kill();
            child.wait();
        }
    }
}

/// Terminate one child through the standard-library process handle.
///
/// Keeping this operation behind a small helper makes the cancellation path
/// independent of Unix-only pid signalling and directly testable on Windows.
fn terminate_child_handle(child: &Arc<Mutex<McpChild>>) {
    let mut child = child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    kill_process_group(&child);
    child.kill();
}

/// Signal the child's process group, not just the child.
///
/// stdio limits only configure the rlimits; the process-group setup happens in
/// the shared pre_exec hook, so a server that spawns a helper would leave the
/// helper alive when only the leader is killed.
#[cfg(unix)]
fn kill_process_group(child: &McpChild) {
    let pid = child.id();
    if pid != 0 {
        // SAFETY: pid is the group leader configured in pre_exec; killpg on a
        // non-leader fails with ESRCH and is harmless.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_child: &McpChild) {}

/// The state one MCP server package owns.
///
/// Split out behind an `Arc` so a call can run on the blocking pool: the server
/// speaks synchronous stdio, and awaiting it on an executor thread both blocks
/// that thread and prevents the caller's deadline and cancel from firing.
struct McpCore {
    extension: ExtensionId,
    root: PathBuf,
    entry: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    config: String,
    max_execution_ms: u64,
    max_output_bytes: u64,
    memory_bytes: u64,
    cpu_seconds: u64,
    /// The process.execute grant this runtime spawns under.
    policy: Option<(Arc<PolicyEngine>, TrustLevel)>,
    #[allow(dead_code)]
    events: Option<Arc<EventBus>>,
    subscription: Mutex<Option<Subscription>>,
    /// contribution id -> the name the server itself uses.
    wire: Mutex<BTreeMap<String, String>>,
    catalog: Mutex<Option<Catalog>>,
    session: Mutex<Option<Session>>,
    /// A separately lockable child handle, so cancellation never waits for the
    /// synchronous session exchange mutex. Child::kill is portable across OSes.
    child: Mutex<Option<Arc<Mutex<McpChild>>>>,
    /// RSS sampler bound to the child's start-time identity (#38 / #109).
    #[cfg(unix)]
    watchdog: Mutex<Option<crate::process::MemoryWatchdog>>,
    /// The AppContainer every server child is launched into. Created on first
    /// spawn; a failure to create it is fatal, never a direct-exec fallback.
    /// heel's job object carries the memory ceiling and kill-on-close.
    #[cfg(windows)]
    sandbox: Mutex<Option<Arc<crate::windows_sandbox::WindowsSandbox>>>,
    cancelled: AtomicBool,
    stopped: AtomicBool,
    /// Set by notifications/*/list_changed; the next catalog read rediscovers.
    catalog_dirty: Arc<AtomicBool>,
}

/// The MCP runtime for one server package.
pub struct McpRuntime {
    core: Arc<McpCore>,
    /// Serializes calls so one invocation cannot reset another's cancellation.
    call_gate: tokio::sync::Mutex<()>,
}

struct Catalog {
    tools: Vec<ToolDescriptor>,
    contexts: Vec<TextContribution>,
    prompts: Vec<TextContribution>,
}

impl McpRuntime {
    /// Prepare an MCP runtime without starting the server.
    pub fn new(
        manifest: &Manifest,
        root: &Path,
        config: String,
        policy: Arc<PolicyEngine>,
        trust: TrustLevel,
        events: Arc<EventBus>,
    ) -> Result<Self, ExtensionError> {
        let subscription = if manifest.contributes.events.is_empty() {
            None
        } else {
            EventFilter::from_topics(&manifest.contributes.events)
                .ok()
                .map(|filter| events.subscribe(filter, 64))
                .transpose()?
        };
        Ok(Self {
            core: Arc::new(McpCore::new(
                manifest,
                root,
                config,
                policy,
                trust,
                events,
                subscription,
            )?),
            call_gate: tokio::sync::Mutex::new(()),
        })
    }

    /// The tool descriptors the server advertised.
    pub fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.core.descriptors()
    }

    /// Context and prompt contributions discovered from resources/prompts.
    pub fn contributions(&self) -> Result<DeclarativeContributions, ExtensionError> {
        self.core.contributions()
    }

    /// Ask the server to stop; the session dies with the runtime.
    pub fn stop(&self) {
        self.core.cancelled.store(true, Ordering::SeqCst);
        self.core.kill_child();
    }
}

impl McpCore {
    fn ensure_running(&self) -> Result<(), ExtensionError> {
        if self.stopped.load(Ordering::SeqCst) {
            Err(self.cancellation_error())
        } else {
            Ok(())
        }
    }

    fn new(
        manifest: &Manifest,
        root: &Path,
        config: String,
        policy: Arc<PolicyEngine>,
        trust: TrustLevel,
        events: Arc<EventBus>,
        subscription: Option<Subscription>,
    ) -> Result<Self, ExtensionError> {
        if manifest.runtime.kind != RuntimeKind::Mcp {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the MCP runtime needs runtime.kind = \"mcp\"".to_string(),
            });
        }
        let entry = manifest
            .runtime
            .entry
            .clone()
            .ok_or_else(|| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the MCP runtime needs runtime.entry".to_string(),
            })?;
        if !is_allowlisted_command(&entry) {
            validate_relative_path(&entry).map_err(|reason| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: reason,
            })?;
        }
        if let Some(key) = manifest
            .runtime
            .env
            .keys()
            .find(|key| ferris_core::manifest::is_dangerous_env(key))
        {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("runtime.env may not set {key}"),
            });
        }
        Ok(Self {
            extension: manifest.id().clone(),
            root: root.to_path_buf(),
            entry,
            args: manifest.runtime.args.clone(),
            env: manifest.runtime.env.clone(),
            config,
            max_execution_ms: manifest.resources.execution_ms(),
            max_output_bytes: manifest.resources.output_bytes(),
            memory_bytes: manifest.resources.memory_mb().saturating_mul(1024 * 1024),
            cpu_seconds: (manifest.resources.execution_ms() / 1000).max(1) + 2,
            policy: Some((policy, trust)),
            events: Some(events),
            subscription: Mutex::new(subscription),
            wire: Mutex::new(BTreeMap::new()),
            catalog: Mutex::new(None),
            session: Mutex::new(None),
            child: Mutex::new(None),
            #[cfg(unix)]
            watchdog: Mutex::new(None),
            #[cfg(windows)]
            sandbox: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            catalog_dirty: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Start the server, handshake, and list its contributions.
    pub fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        let catalog = self.catalog()?;
        Ok(catalog
            .as_ref()
            .expect("catalog was just built")
            .tools
            .clone())
    }

    /// Context and prompt contributions discovered from resources/prompts.
    pub fn contributions(&self) -> Result<DeclarativeContributions, ExtensionError> {
        let catalog = self.catalog()?;
        let catalog = catalog.as_ref().expect("catalog was just built");
        Ok(DeclarativeContributions {
            contexts: catalog.contexts.clone(),
            prompts: catalog.prompts.clone(),
            ..DeclarativeContributions::default()
        })
    }

    fn catalog(&self) -> Result<std::sync::MutexGuard<'_, Option<Catalog>>, ExtensionError> {
        self.ensure_running()?;
        // A list_changed notification that arrived while the runtime was idle
        // is still queued; drain it before deciding whether the cache is fresh.
        if let Ok(mut slot) = self.session.lock() {
            if let Some(session) = slot.as_mut() {
                session.drain_notifications();
            }
        }
        let mut catalog = self.catalog.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP catalog lock is poisoned".to_string(),
        })?;
        if self.catalog_dirty.swap(false, Ordering::SeqCst) {
            *catalog = None;
        }
        if catalog.is_none() {
            *catalog = Some(self.discover()?);
        }
        Ok(catalog)
    }

    fn discover(&self) -> Result<Catalog, ExtensionError> {
        // Reuse the live session when there is one: a list_changed refresh must
        // re-read the catalog from the running server, not spawn a second one.
        let existing = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let mut session = match existing {
            Some(session) => session,
            None => self.start()?,
        };
        let discovered = self.read_catalog(&mut session);
        match discovered {
            Ok(catalog) => {
                if let Ok(mut slot) = self.session.lock() {
                    *slot = Some(session);
                }
                Ok(catalog)
            }
            Err(error) => {
                session.close();
                Err(error)
            }
        }
    }

    /// Start with two bounded retries; a failed handshake has no tool side
    /// effect, so retrying the transport is safe. Delays are 100ms and 250ms.
    fn start_with_retry(&self) -> Result<Session, ExtensionError> {
        let mut last = None;
        for delay in [0_u64, 100, 250] {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(self.cancellation_error());
            }
            if delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
            match self.start() {
                Ok(session) => return Ok(session),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a start attempt ran"))
    }

    #[cfg(windows)]
    fn sandbox(
        &self,
        program: &Path,
    ) -> Result<Arc<crate::windows_sandbox::WindowsSandbox>, ExtensionError> {
        let mut slot = self
            .sandbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(sandbox) = slot.as_ref() {
            return Ok(Arc::clone(sandbox));
        }
        let sandbox = Arc::new(
            crate::windows_sandbox::WindowsSandbox::new_blocking(
                &self.root,
                None,
                None,
                self.memory_bytes,
                program,
            )
            .map_err(|message| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message,
            })?,
        );
        *slot = Some(Arc::clone(&sandbox));
        Ok(sandbox)
    }

    fn start(&self) -> Result<Session, ExtensionError> {
        self.ensure_running()?;
        let program = if is_allowlisted_command(&self.entry) {
            PathBuf::from(&self.entry)
        } else {
            let relative =
                validate_relative_path(&self.entry).map_err(|reason| ExtensionError::Manifest {
                    path: self.root.join("extension.toml"),
                    message: reason,
                })?;
            let path = self.root.join(relative);
            let canonical_root = self
                .root
                .canonicalize()
                .map_err(|source| ExtensionError::Io {
                    path: self.root.clone(),
                    source,
                })?;
            let canonical = path.canonicalize().map_err(|source| ExtensionError::Io {
                path: path.clone(),
                source,
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ExtensionError::Corrupt {
                    path,
                    message: "MCP entry escapes the package root".to_string(),
                });
            }
            canonical
        };
        // The MCP runtime spawns a child exactly like the process runtime, so
        // it must pass the same gate: an explicit process.execute grant, scoped
        // to this exact program and argument vector. Without this a package
        // that simply omitted the capability from its manifest ran arbitrary
        // commands with the reader's privileges.
        if let Some((policy, trust)) = &self.policy {
            policy.authorize_call(
                self.extension.as_str(),
                *trust,
                Capability::ProcessExecute,
                &CallScope {
                    program: Some(self.entry.clone()),
                    args: Some(self.args.clone()),
                    ..CallScope::default()
                },
            )?;
        }
        // MCP is also an arbitrary package-controlled child process. Reuse
        // the platform sandbox used by ProcessRuntime; a missing sandbox binary
        // or failed profile must never fall back to direct execution.
        let mut command = if cfg!(target_os = "linux") {
            let mut command = Command::new("bwrap");
            command.args(crate::process::linux_bwrap_plan(
                &self.root, None, None, &program, &self.args,
            ));
            command
        } else if cfg!(target_os = "macos") {
            let mut command = Command::new("/usr/bin/sandbox-exec");
            command
                .arg("-p")
                .arg(crate::process::sandbox_profile(&self.root, None, None))
                .arg(&program)
                // The profile and the program are sandbox-exec's own arguments;
                // the package's argument vector belongs to the program it execs.
                // Passing only the program ran it with no arguments, so an MCP
                // server whose entry needs a script path never answered.
                .args(&self.args);
            command
        } else {
            let mut command = Command::new(&program);
            command.args(&self.args);
            command
        };
        // Apply the same Unix RLIMIT and process-group ceilings as
        // ProcessRuntime. Windows remains a documented limitation until
        // Job Object enforcement is implemented.
        crate::process::configure_std_process_limits(
            &mut command,
            self.memory_bytes,
            self.cpu_seconds,
        );
        command
            .current_dir(&self.root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        #[cfg(windows)]
        let _ = command;
        #[cfg(not(windows))]
        let mut child = McpChild::Std(command.spawn().map_err(|source| ExtensionError::Io {
            path: program,
            source,
        })?);
        #[cfg(windows)]
        let mut child = {
            let sandbox = self.sandbox(&program)?;
            let mut envs: Vec<(String, String)> = Vec::new();
            for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
                if let Some(value) = std::env::var_os(key) {
                    envs.push((key.to_string(), value.to_string_lossy().into_owned()));
                }
            }
            for (key, value) in &self.env {
                envs.push((key.clone(), value.clone()));
            }
            let entry = program.to_string_lossy().into_owned();
            let child = futures_lite::future::block_on(
                sandbox.spawn(&entry, &self.args, &self.root, &envs),
            )
            .map_err(|message| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message,
            })?;
            McpChild::Sandbox(child)
        };
        let stdin = child.take_stdin().ok_or_else(|| ExtensionError::Protocol {
            extension: self.extension.to_string(),
            message: "MCP stdin was not piped".to_string(),
        })?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "MCP stdout was not piped".to_string(),
            })?;
        let stderr = child.take_stderr();
        let child = self.register_child(child)?;
        if let Some(stderr) = stderr {
            let extension = self.extension.to_string();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    match read_limited_line_sync(&mut reader, MAX_LINE_BYTES) {
                        Ok(LimitedLine::Line(bytes)) => {
                            let line = String::from_utf8_lossy(&bytes);
                            log::debug!(target: "bsh::extension", "{extension} mcp stderr: {line}");
                        }
                        Ok(LimitedLine::TooLong) => {
                            // Keep draining: stopping here would leave the pipe
                            // full and turn every later write into a failure.
                            log::warn!(
                                target: "bsh::extension",
                                "{extension} mcp stderr line exceeded {MAX_LINE_BYTES} bytes; discarding it"
                            );
                        }
                        Ok(LimitedLine::Eof) | Err(_) => break,
                    }
                }
            });
        }
        let (sender, lines) = sync_channel(64);
        let extension = self.extension.to_string();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_limited_line_sync(&mut reader, MAX_LINE_BYTES) {
                    Ok(LimitedLine::Line(buffer)) => {
                        if sender.send(buffer).is_err() {
                            break;
                        }
                    }
                    Ok(LimitedLine::TooLong) => {
                        // The line was consumed through its newline, so the
                        // next read can resynchronize instead of dying here.
                        log::warn!(
                            target: "bsh::extension",
                            "{extension} mcp line exceeded {MAX_LINE_BYTES} bytes; discarding it"
                        );
                    }
                    Ok(LimitedLine::Eof) | Err(_) => break,
                }
            }
        });
        let (writer, writer_rx) = sync_channel::<WriteRequest>(8);
        std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(request) = writer_rx.recv() {
                let result = stdin.write_all(&request.line).and_then(|()| stdin.flush());
                let failed = result.is_err();
                let _ = request.done.send(result);
                if failed {
                    break;
                }
            }
        });
        let mut session = Session {
            child,
            writer,
            lines,
            next_id: 1,
            capabilities: ServerCapabilities::default(),
            catalog_dirty: Arc::clone(&self.catalog_dirty),
        };
        let initialized = session.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL,
                "capabilities": {},
                "clientInfo": { "name": "bsh", "version": HOST_VERSION },
                "config": self.config,
            }),
            HANDSHAKE_MS,
        )?;
        let protocol = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if protocol != MCP_PROTOCOL {
            session.close();
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: format!(
                    "MCP server selected unsupported protocolVersion {protocol:?}; this host speaks {MCP_PROTOCOL}"
                ),
            });
        }
        session.capabilities = ServerCapabilities::from_initialize(&initialized);
        session.notify("notifications/initialized", Value::Null)?;
        Ok(session)
    }

    fn read_catalog(&self, session: &mut Session) -> Result<Catalog, ExtensionError> {
        let timeout = self.max_execution_ms;
        let mut budget = 0usize;
        let mut tools = Vec::new();
        let mut wire = self.wire.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP wire-name lock is poisoned".to_string(),
        })?;
        let mut cursor = Value::Null;
        let mut seen = std::collections::HashSet::new();
        let mut pages = 0usize;
        let mut raw = 0usize;
        loop {
            pages += 1;
            if pages > MAX_CATALOG_PAGES {
                return Err(protocol_error(format!(
                    "tools/list exceeded {MAX_CATALOG_PAGES} pages"
                )));
            }
            let result = session.request(
                "tools/list",
                if cursor.is_null() {
                    json!({})
                } else {
                    json!({ "cursor": cursor })
                },
                timeout,
            )?;
            if let Some(list) = result.get("tools").and_then(Value::as_array) {
                for tool in list {
                    // The ceiling counts raw items, not only accepted ones: a
                    // server that sends unusable names must not loop forever.
                    raw += 1;
                    if raw > MAX_ITEMS {
                        break;
                    }
                    let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
                    let id = match ContributionId::parse(sanitize_id(name)) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    if wire.contains_key(id.as_str()) {
                        log::warn!(
                            target: "bsh::extension",
                            "{}: MCP tools {:?} and {:?} share the id {:?}; the later one is ignored",
                            self.extension,
                            wire.get(id.as_str()).map(String::as_str).unwrap_or_default(),
                            name,
                            id
                        );
                        continue;
                    }
                    wire.insert(id.as_str().to_string(), name.to_string());
                    let descriptor = ToolDescriptor {
                        id,
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or(name)
                            .to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type":"object","properties":{}})),
                        capabilities: Vec::new(),
                    };
                    budget = budget.saturating_add(catalog_size(&descriptor));
                    if budget > MAX_CATALOG_BYTES {
                        return Err(catalog_limit(&self.extension, budget));
                    }
                    tools.push(descriptor);
                }
            }
            cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() || raw >= MAX_ITEMS {
                break;
            }
            if !seen.insert(cursor.to_string()) {
                return Err(protocol_error("tools/list repeated a cursor"));
            }
        }
        drop(wire);

        let mut contexts = Vec::new();
        // Only ask for resources/prompts the server declared. A server that
        // omits the capability gets no optional request, so a method-not-found
        // cannot masquerade as a discovery failure.
        if session.capabilities.resources {
            let mut cursor = Value::Null;
            let mut seen = std::collections::HashSet::new();
            let mut pages = 0usize;
            let mut raw = 0usize;
            loop {
                pages += 1;
                if pages > MAX_CATALOG_PAGES {
                    return Err(protocol_error(format!(
                        "resources/list exceeded {MAX_CATALOG_PAGES} pages"
                    )));
                }
                // A failure here is reported: the server advertised resources,
                // so a partial catalog is an error, not an empty feature.
                let result = session.request(
                    "resources/list",
                    if cursor.is_null() {
                        json!({})
                    } else {
                        json!({ "cursor": cursor })
                    },
                    timeout,
                )?;
                if let Some(list) = result.get("resources").and_then(Value::as_array) {
                    for resource in list {
                        raw += 1;
                        if raw > MAX_ITEMS {
                            break;
                        }
                        let uri = resource
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let name = resource.get("name").and_then(Value::as_str).unwrap_or(uri);
                        let Some(id) = ContributionId::parse(sanitize_id(name)).ok() else {
                            continue;
                        };
                        match session.request("resources/read", json!({ "uri": uri }), timeout) {
                            Ok(result) => {
                                let text = result
                                    .get("contents")
                                    .and_then(Value::as_array)
                                    .and_then(|contents| contents.first())
                                    .and_then(|content| content.get("text"))
                                    .and_then(Value::as_str);
                                match text {
                                    Some(text) => {
                                        let text: String = text.chars().take(64 * 1024).collect();
                                        budget = budget.saturating_add(text.len());
                                        if budget > MAX_CATALOG_BYTES {
                                            return Err(catalog_limit(&self.extension, budget));
                                        }
                                        contexts.push(TextContribution {
                                            id,
                                            description: resource
                                                .get("description")
                                                .and_then(Value::as_str)
                                                .unwrap_or(uri)
                                                .to_string(),
                                            text,
                                            scope: Vec::new(),
                                        });
                                    }
                                    None => log::warn!(
                                        target: "bsh::extension",
                                        "{}: resources/read for {uri:?} returned no text",
                                        self.extension
                                    ),
                                }
                            }
                            Err(error) => log::warn!(
                                target: "bsh::extension",
                                "{}: resources/read for {uri:?} failed: {error}",
                                self.extension
                            ),
                        }
                    }
                }
                cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
                if cursor.is_null() || raw >= MAX_ITEMS {
                    break;
                }
                if !seen.insert(cursor.to_string()) {
                    return Err(protocol_error("resources/list repeated a cursor"));
                }
            }
        }

        let mut prompts = Vec::new();
        if session.capabilities.prompts {
            let mut cursor = Value::Null;
            let mut seen = std::collections::HashSet::new();
            let mut pages = 0usize;
            let mut raw = 0usize;
            loop {
                pages += 1;
                if pages > MAX_CATALOG_PAGES {
                    return Err(protocol_error(format!(
                        "prompts/list exceeded {MAX_CATALOG_PAGES} pages"
                    )));
                }
                let result = session.request(
                    "prompts/list",
                    if cursor.is_null() {
                        json!({})
                    } else {
                        json!({ "cursor": cursor })
                    },
                    timeout,
                )?;
                if let Some(list) = result.get("prompts").and_then(Value::as_array) {
                    for prompt in list {
                        raw += 1;
                        if raw > MAX_ITEMS {
                            break;
                        }
                        let name = prompt
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let Some(id) = ContributionId::parse(sanitize_id(name)).ok() else {
                            continue;
                        };
                        match session.request("prompts/get", json!({ "name": name }), timeout) {
                            Ok(result) => {
                                let text = result.get("messages").and_then(Value::as_array).map(
                                    |messages| {
                                        messages
                                            .iter()
                                            .filter_map(|message| {
                                                message
                                                    .get("content")?
                                                    .get("text")?
                                                    .as_str()
                                                    .map(str::to_string)
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n\n")
                                    },
                                );
                                match text {
                                    Some(text) => {
                                        let text: String = text.chars().take(64 * 1024).collect();
                                        budget = budget.saturating_add(text.len());
                                        if budget > MAX_CATALOG_BYTES {
                                            return Err(catalog_limit(&self.extension, budget));
                                        }
                                        prompts.push(TextContribution {
                                            id,
                                            description: prompt
                                                .get("description")
                                                .and_then(Value::as_str)
                                                .unwrap_or(name)
                                                .to_string(),
                                            text,
                                            scope: Vec::new(),
                                        });
                                    }
                                    None => log::warn!(
                                        target: "bsh::extension",
                                        "{}: prompts/get for {name:?} returned no messages",
                                        self.extension
                                    ),
                                }
                            }
                            Err(error) => log::warn!(
                                target: "bsh::extension",
                                "{}: prompts/get for {name:?} failed: {error}",
                                self.extension
                            ),
                        }
                    }
                }
                cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
                if cursor.is_null() || raw >= MAX_ITEMS {
                    break;
                }
                if !seen.insert(cursor.to_string()) {
                    return Err(protocol_error("prompts/list repeated a cursor"));
                }
            }
        }

        Ok(Catalog {
            tools,
            contexts,
            prompts,
        })
    }

    /// The name the server itself uses for a contribution id.
    fn wire_name(&self, id: &str) -> String {
        self.wire
            .lock()
            .ok()
            .and_then(|wire| wire.get(id).cloned())
            .unwrap_or_else(|| id.to_string())
    }

    /// Kill the live child without waiting for the session lock.
    ///
    /// The child handle has its own mutex, independent of the session mutex
    /// held during blocking stdio exchanges. Child::kill uses the native
    /// process-termination API on both Unix and Windows.
    fn kill_child(&self) {
        let child = self
            .child
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned();
        if let Some(child) = child {
            terminate_child_handle(&child);
        }
        self.stop_watchdog();
    }

    /// Start an RSS sampler bound to this child, replacing any previous one.
    #[cfg(unix)]
    fn start_watchdog(&self, pid: u32) {
        self.stop_watchdog();
        let identity = crate::process::child_identity(pid);
        let watchdog = crate::process::spawn_memory_watchdog(pid, self.memory_bytes, identity);
        if let Ok(mut slot) = self.watchdog.lock() {
            *slot = Some(watchdog);
        }
    }

    /// Cancel the sampler before the child is reaped.
    fn stop_watchdog(&self) {
        #[cfg(unix)]
        if let Ok(mut slot) = self.watchdog.lock() {
            if let Some(watchdog) = slot.take() {
                watchdog.stop();
            }
        }
    }

    fn cancellation_error(&self) -> ExtensionError {
        ExtensionError::Runtime {
            extension: self.extension.to_string(),
            message: "MCP call cancelled".to_string(),
        }
    }

    /// Register a spawned child and close the spawn/cancel race. If cancellation
    /// arrived before the child handle became visible, terminate and reap it
    /// before allowing the caller to proceed.
    fn register_child(&self, child: McpChild) -> Result<Arc<Mutex<McpChild>>, ExtensionError> {
        #[cfg(unix)]
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        {
            let mut slot = self
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *slot = Some(Arc::clone(&child));
        }
        if self.stopped.load(Ordering::SeqCst) || self.cancelled.load(Ordering::SeqCst) {
            terminate_child_handle(&child);
            if let Ok(mut process) = child.lock() {
                process.wait();
            }
            self.stop_watchdog();
            let mut slot = self
                .child
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &child))
            {
                *slot = None;
            }
            return Err(self.cancellation_error());
        }
        #[cfg(unix)]
        if pid != 0 {
            self.start_watchdog(pid);
        }
        Ok(child)
    }

    /// One tool call over the session, blocking this thread.
    fn call_tool_blocking(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<tokio::sync::mpsc::Sender<Value>>,
        timeout_ms: u64,
    ) -> Result<Value, ExtensionError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(self.cancellation_error());
        }
        let wire = self.wire_name(name);
        let mut guard = self.session.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP session lock is poisoned".to_string(),
        })?;
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(self.cancellation_error());
        }
        if guard.is_none() {
            *guard = Some(self.start_with_retry()?);
        }
        let session = guard.as_mut().expect("session");
        let result = session.request_with_progress(
            "tools/call",
            json!({ "name": wire, "arguments": arguments }),
            timeout_ms,
            progress.as_ref(),
        );
        match result {
            Ok(value) => {
                let bytes = serde_json::to_vec(&value)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0);
                if bytes as u64 > self.max_output_bytes {
                    *guard = None;
                    return Err(ExtensionError::Limit {
                        extension: self.extension.to_string(),
                        message: format!(
                            "MCP result is {bytes} bytes; the ceiling is {}",
                            self.max_output_bytes
                        ),
                    });
                }
                let is_error = value
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if is_error {
                    let message = value
                        .get("content")
                        .and_then(Value::as_array)
                        .and_then(|content| content.first())
                        .and_then(|content| content.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("MCP tool failed")
                        .to_string();
                    return Err(ExtensionError::Runtime {
                        extension: self.extension.to_string(),
                        message,
                    });
                }
                Ok(value)
            }
            // A well-formed JSON-RPC error leaves the pipe in a known state, so
            // the next call keeps the session and the server's in-process state.
            Err(error @ ExtensionError::Remote { .. }) => Err(error),
            Err(error) => {
                *guard = None;
                Err(error)
            }
        }
    }

    /// Deliver queued host events as `bsh/events/event` notifications.
    ///
    /// The server is started on demand, so a package whose server is not
    /// running yet simply drops the queued events instead of paying a spawn
    /// just to deliver a notification.
    fn pump(&self) {
        let mut queued = Vec::new();
        if let Ok(slot) = self.subscription.lock() {
            if let Some(subscription) = slot.as_ref() {
                while let Some(event) = subscription.try_recv() {
                    queued.push(event);
                }
            }
        }
        if queued.is_empty() {
            return;
        }
        let Ok(mut guard) = self.session.lock() else {
            return;
        };
        let Some(session) = guard.as_mut() else {
            return;
        };
        for event in queued {
            let Ok(params) = serde_json::to_value(&event) else {
                continue;
            };
            let _ = session.notify("bsh/events/event", params);
        }
    }
}

fn sanitize_id(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '_'
            {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized
        .trim_matches(|character| character == '.' || character == '_' || character == '-')
        .to_string();
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed
    }
}

#[async_trait]
impl Runtime for McpRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Mcp
    }

    fn declarative(&self) -> DeclarativeContributions {
        self.contributions().unwrap_or_default()
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        // Discovery performs a process spawn, handshake and synchronous stdio
        // exchange. Running it inline on an executor worker starved the
        // reactor; the blocking pool keeps the worker free.
        self.discovery_off_worker().await
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        // Stop new calls and terminate an active exchange before waiting for
        // the session mutex the blocking worker holds.
        self.stop();
        let _gate = self.call_gate.lock().await;
        self.deactivate_locked()
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.core.stopped.store(true, Ordering::SeqCst);
        self.stop();
        // Wait for an active blocking exchange to observe cancellation before
        // closing and clearing its session. New calls fail after taking this gate.
        let _gate = self.call_gate.lock().await;
        self.deactivate_locked()
    }

    async fn cancel(&self) -> Result<(), ExtensionError> {
        // No session lock: a running call holds it for the whole stdio
        // exchange, and the separate child handle makes Stop effective.
        self.stop();
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.tools_off_worker().await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.call(name, arguments, context, None).await
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.call(name, arguments, context, None)
            .await
            .map(ToolResult::from_value)
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::Sender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.call(name, arguments, context, progress)
            .await
            .map(ToolResult::from_value)
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        // Share the lifecycle gate with calls and shutdown: do not drain queued
        // events while shutdown is terminalizing the session, and do not let a
        // pump start after shutdown has acquired the gate.
        let _gate = self.call_gate.lock().await;
        self.core.ensure_running()?;
        self.core.pump();
        Ok(())
    }
}

impl McpRuntime {
    /// Run blocking catalog discovery on the blocking pool.
    async fn discovery_off_worker(&self) -> Result<(), ExtensionError> {
        let core = Arc::clone(&self.core);
        let extension = self.core.extension.to_string();
        tokio::task::spawn_blocking(move || core.descriptors().map(|_| ()))
            .await
            .map_err(|error| ExtensionError::Runtime {
                extension,
                message: format!("MCP discovery task failed: {error}"),
            })?
    }

    /// Run blocking catalog discovery on the blocking pool and return tools.
    async fn tools_off_worker(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        let core = Arc::clone(&self.core);
        let extension = self.core.extension.to_string();
        tokio::task::spawn_blocking(move || core.descriptors())
            .await
            .map_err(|error| ExtensionError::Runtime {
                extension,
                message: format!("MCP discovery task failed: {error}"),
            })?
    }

    /// Close the session and invalidate the catalog. The caller owns the
    /// lifecycle gate, so no exchange can start while this runs.
    fn deactivate_locked(&self) -> Result<(), ExtensionError> {
        let mut slot = self
            .core
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut session) = slot.take() {
            session.close();
        }
        if let Ok(mut child) = self.core.child.lock() {
            *child = None;
        }
        // The catalog describes a server that is now gone; a later activate
        // must discover again instead of reporting success from the cache.
        if let Ok(mut catalog) = self.core.catalog.lock() {
            *catalog = None;
        }
        if !self.core.stopped.load(Ordering::SeqCst) {
            self.core.cancelled.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    /// Run one call on the blocking pool under the caller's deadline.
    ///
    /// The MCP transport is synchronous stdio: running it inline on an executor
    /// thread starved the runtime and made the caller's timeout and cancel
    /// unreachable. `spawn_blocking` keeps the exchange off the executor, and
    /// on a deadline the child is killed so the blocked read returns now rather
    /// than when the server chooses to answer.
    async fn call(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::Sender<Value>>,
    ) -> Result<Value, ExtensionError> {
        let _gate = self.call_gate.lock().await;
        self.core.ensure_running()?;
        let deadline = context.timeout_ms.max(1).min(self.core.max_execution_ms);
        // Reset only after this invocation owns the gate. A queued call can
        // therefore never clear cancellation for the active invocation.
        self.core.cancelled.store(false, Ordering::SeqCst);
        let core = Arc::clone(&self.core);
        let name = name.to_string();
        let handle = tokio::task::spawn_blocking(move || {
            core.call_tool_blocking(&name, arguments, progress, deadline)
        });
        match wait_for_blocking_call(
            handle,
            Duration::from_millis(deadline.saturating_add(100)),
            || self.stop(),
        )
        .await
        {
            BlockingCallWait::TimedOut => Err(ExtensionError::Timeout {
                extension: self.core.extension.to_string(),
                operation: "mcp tools/call".to_string(),
                millis: deadline,
            }),
            BlockingCallWait::Completed(Err(error)) => Err(ExtensionError::Runtime {
                extension: self.core.extension.to_string(),
                message: format!("MCP call task failed: {error}"),
            }),
            BlockingCallWait::Completed(Ok(result)) => result,
        }
    }
}
enum BlockingCallWait<T> {
    Completed(Result<T, tokio::task::JoinError>),
    TimedOut,
}

/// Wait for a blocking MCP exchange without detaching it on deadline.
///
/// Dropping a JoinHandle detaches its blocking task. The timeout path therefore
/// cancels the child and then joins the worker before its caller can release the
/// lifecycle gate.
async fn wait_for_blocking_call<T: Send + 'static>(
    mut handle: tokio::task::JoinHandle<T>,
    timeout: Duration,
    on_timeout: impl FnOnce(),
) -> BlockingCallWait<T> {
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(result) => BlockingCallWait::Completed(result),
        Err(_) => {
            on_timeout();
            let _ = handle.await;
            BlockingCallWait::TimedOut
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timed_out_worker_keeps_call_gate_held_until_worker_exits() {
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let caller_gate = Arc::clone(&gate);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (timed_out_tx, timed_out_rx) = tokio::sync::oneshot::channel();

        let caller = tokio::spawn(async move {
            let _gate = caller_gate.lock().await;
            let worker = tokio::task::spawn_blocking(move || {
                let _ = started_tx.send(());
                release_rx.recv().expect("test releases blocking worker");
            });
            wait_for_blocking_call(worker, Duration::from_millis(10), move || {
                let _ = timed_out_tx.send(());
            })
            .await
        });

        started_rx.await.expect("blocking worker started");
        timed_out_rx.await.expect("deadline callback ran");

        let waiter_gate = Arc::clone(&gate);
        let (waiting_tx, waiting_rx) = tokio::sync::oneshot::channel();
        let mut waiter = tokio::spawn(async move {
            let _ = waiting_tx.send(());
            let _gate = waiter_gate.lock().await;
        });
        waiting_rx
            .await
            .expect("second call is waiting for the gate");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiter)
                .await
                .is_err(),
            "the next call must not acquire the gate while the timed-out worker is alive"
        );

        release_tx.send(()).expect("release blocking worker");
        assert!(matches!(
            caller.await.expect("caller task"),
            BlockingCallWait::TimedOut
        ));
        waiter
            .await
            .expect("second call acquires the gate after worker exit");
    }

    #[cfg(unix)]
    #[test]
    fn child_handle_termination_is_portable() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let child = command.spawn().expect("long-running child");
        let child = Arc::new(Mutex::new(McpChild::Std(child)));
        terminate_child_handle(&child);
        // wait() blocks until the killed child is reaped; a regression that
        // failed to terminate would hang this test rather than pass it.
        child.lock().expect("child lock").wait();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_linux_mcp_child_cannot_write_outside_its_package_root() {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("CI runner HOME is set");
        let root = home.join(format!("bsh-mcp-sandbox-package-{}", std::process::id()));
        let probe = home.join(format!("bsh-mcp-sandbox-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&probe);
        std::fs::create_dir_all(&root).expect("package root");
        let server = r#"import json, os, sys, resource
probe = os.path.join(os.environ["HOME"], "bsh-mcp-sandbox-probe-PID")
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    rid = request.get("id")
    if method == "initialize":
        result = {"protocolVersion": "2024-11-05", "capabilities": {"resources": {}, "prompts": {}}, "serverInfo": {"name": "sandbox-test", "version": "1"}}
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        result = {"tools": [{"name": "probe", "description": "Probe filesystem isolation", "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        try:
            with open(probe, "w", encoding="utf-8") as handle:
                handle.write("escaped")
            text = "wrote"
        except OSError:
            text = "blocked"
        memory = resource.getrlimit(resource.RLIMIT_AS)[0]
        cpu = resource.getrlimit(resource.RLIMIT_CPU)[0]
        nofile = resource.getrlimit(resource.RLIMIT_NOFILE)[0]
        text = f"{text}|{memory}|{cpu}|{nofile}"
        result = {"content": [{"type": "text", "text": text}], "isError": False}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unsupported"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": rid, "result": result}), flush=True)
"#.replace("PID", &std::process::id().to_string());
        std::fs::write(root.join("server.py"), server).expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"
[extension]
id = "com.example.mcp-sandbox"
name = "MCP Sandbox"
version = "1.0.0"
description = "Linux MCP sandbox integration test"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "mcp"
entry = "python3"
args = ["server.py"]
[capabilities]
required = ["process.execute"]
"#,
            &root.join("extension.toml"),
        )
        .expect("manifest");
        let policy = Arc::new(
            PolicyEngine::open(
                root.join("grants.toml"),
                root.join("audit.jsonl"),
                ferris_core::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        policy
            .grant(
                manifest.id().as_str(),
                Capability::ProcessExecute,
                ferris_core::capability::GrantScope::Program("python3".to_string()),
                None,
            )
            .expect("grant");
        // A Stop received after scheduling but before the blocking worker
        // begins must not be cleared by the worker.
        let cancelled_runtime = McpRuntime::new(
            &manifest,
            &root,
            "{}".to_string(),
            Arc::clone(&policy),
            TrustLevel::Local,
            EventBus::new(),
        )
        .expect("cancel test runtime");
        cancelled_runtime
            .core
            .cancelled
            .store(true, Ordering::SeqCst);
        let cancelled =
            cancelled_runtime
                .core
                .call_tool_blocking("echo", json!({"x": 1}), None, 5000);
        assert!(matches!(
            cancelled,
            Err(ExtensionError::Runtime { message, .. }) if message.contains("cancelled")
        ));
        assert!(
            cancelled_runtime
                .core
                .child
                .lock()
                .expect("child slot")
                .is_none(),
            "a pre-cancelled call must not spawn a child"
        );

        // Cancellation that races with Child registration must kill and reap
        // the just-spawned process before start() can continue.
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            command
        };
        let child = command.spawn().expect("long-running child");
        assert!(cancelled_runtime
            .core
            .register_child(McpChild::Std(child))
            .is_err());
        assert!(
            cancelled_runtime
                .core
                .child
                .lock()
                .expect("child slot")
                .is_none(),
            "a child registered after cancellation must be reaped and cleared"
        );

        // Terminal shutdown is distinct from a recoverable cancellation:
        // even when the transient cancellation flag is clear, a late child
        // registration must be rejected and reaped.
        cancelled_runtime
            .core
            .cancelled
            .store(false, Ordering::SeqCst);
        cancelled_runtime.core.stopped.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
            command
        };
        let child = command.spawn().expect("late child");
        assert!(cancelled_runtime
            .core
            .register_child(McpChild::Std(child))
            .is_err());
        assert!(
            cancelled_runtime
                .core
                .child
                .lock()
                .expect("child slot")
                .is_none(),
            "a child registered after terminal shutdown must be reaped and cleared"
        );

        let runtime = McpRuntime::new(
            &manifest,
            &root,
            "{}".to_string(),
            policy,
            TrustLevel::Local,
            EventBus::new(),
        )
        .expect("runtime");
        let tools = runtime.descriptors().expect("MCP inventory");
        assert!(tools.iter().any(|tool| tool.id.as_str() == "probe"));
        let context = CallContext::new(manifest.id().clone(), "sandbox", None, 5000);
        let result = runtime
            .call_tool("probe", json!({}), &context)
            .await
            .expect("sandboxed MCP call");
        let text = result["content"][0]["text"]
            .as_str()
            .expect("MCP probe returns text");
        let values: Vec<&str> = text.split('|').collect();
        assert_eq!(values.first(), Some(&"blocked"));
        assert_eq!(
            values.get(1).and_then(|value| value.parse::<u64>().ok()),
            Some(256 * 1024 * 1024),
            "MCP child must inherit RLIMIT_AS"
        );
        assert_eq!(
            values.get(2),
            Some(&"-1"),
            "RLIMIT_CPU stays unlimited: max_execution_ms is a per-call deadline, not a lifetime CPU budget"
        );
        assert_eq!(
            values.get(3).and_then(|value| value.parse::<u64>().ok()),
            Some(256),
            "MCP child must inherit RLIMIT_NOFILE"
        );
        assert!(!probe.exists(), "MCP must not write into the host HOME");
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_server_maps_tools_resources_and_prompts() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/mcp-echo")
            .canonicalize()
            .expect("the MCP example exists");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let scratch = std::env::temp_dir().join(format!("bsh-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch");
        let policy = Arc::new(
            PolicyEngine::open(
                scratch.join("grants.toml"),
                scratch.join("audit.jsonl"),
                ferris_core::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        policy
            .grant(
                "com.example.mcp-echo",
                Capability::ProcessExecute,
                ferris_core::capability::GrantScope::Program("python3".to_string()),
                None,
            )
            .expect("grant");
        let runtime = McpRuntime::new(
            &manifest,
            &root,
            "{}".to_string(),
            policy,
            TrustLevel::Local,
            EventBus::new(),
        )
        .expect("runtime");
        let tools = runtime.descriptors().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id.as_str(), "echo");
        let contributions = runtime.contributions().expect("contributions");
        assert_eq!(contributions.contexts.len(), 1);
        assert_eq!(contributions.contexts[0].text, "MCP resource text.");
        assert_eq!(contributions.prompts.len(), 1);
        assert_eq!(contributions.prompts[0].text, "Say hello from MCP.");
        let context = CallContext::new(
            ExtensionId::parse("com.example.mcp-echo").expect("id"),
            "test",
            None,
            5000,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
        let result = runtime
            .call_tool_progress("echo", serde_json::json!({"x": 1}), &context, Some(sender))
            .await
            .expect("call");
        assert_eq!(result.to_value(), serde_json::json!("{\"x\": 1}"));
        let progress = receiver.recv().await.expect("one progress notification");
        assert_eq!(progress["progress"], 1);
        runtime.shutdown().await.expect("shutdown");

        // Shutdown is terminal: no public discovery or invocation surface may
        // lazily recreate the MCP server after its child and session are gone.
        assert!(runtime.descriptors().is_err());
        assert!(runtime.contributions().is_err());
        assert!(runtime.activate().await.is_err());
        assert!(runtime.pump().await.is_err());
        assert!(runtime
            .call_tool("echo", serde_json::json!({"x": 2}), &context)
            .await
            .is_err());
        runtime
            .shutdown()
            .await
            .expect("repeated shutdown is idempotent");
    }

    #[tokio::test]
    async fn deactivation_and_reactivation_start_a_fresh_server() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/mcp-echo")
            .canonicalize()
            .expect("the MCP example exists");
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
        let scratch =
            std::env::temp_dir().join(format!("bsh-mcp-reactivate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).expect("scratch");
        let policy = Arc::new(
            PolicyEngine::open(
                scratch.join("grants.toml"),
                scratch.join("audit.jsonl"),
                ferris_core::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        policy
            .grant(
                "com.example.mcp-echo",
                Capability::ProcessExecute,
                ferris_core::capability::GrantScope::Program("python3".to_string()),
                None,
            )
            .expect("grant");
        let runtime = McpRuntime::new(
            &manifest,
            &root,
            "{}".to_string(),
            policy,
            TrustLevel::Local,
            EventBus::new(),
        )
        .expect("runtime");
        runtime.activate().await.expect("first activation");
        assert!(runtime.core.catalog.lock().expect("catalog").is_some());
        runtime.deactivate().await.expect("deactivate");
        assert!(
            runtime.core.catalog.lock().expect("catalog").is_none(),
            "deactivation must invalidate the cached catalog"
        );
        assert!(runtime.core.session.lock().expect("session").is_none());
        runtime
            .activate()
            .await
            .expect("reactivation starts a fresh server");
        assert!(
            runtime.core.catalog.lock().expect("catalog").is_some(),
            "reactivation must re-run discovery"
        );
        let context = CallContext::new(
            ExtensionId::parse("com.example.mcp-echo").expect("id"),
            "test",
            None,
            5000,
        );
        runtime
            .call_tool("echo", serde_json::json!({"x": 1}), &context)
            .await
            .expect("the restarted server answers");
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&scratch);
    }
    #[test]
    fn a_malformed_response_envelope_is_rejected() {
        let rejected = |value: Value| {
            validate_response(&value, 7, None)
                .expect_err("malformed envelope")
                .to_string()
        };
        assert!(rejected(json!({"id": 7, "result": {}})).contains("jsonrpc"));
        assert!(rejected(json!({"jsonrpc": "1.0", "id": 7, "result": {}})).contains("jsonrpc"));
        assert!(rejected(json!({"jsonrpc": "2.0", "id": "7", "result": {}})).contains("integer"));
        assert!(rejected(json!({"jsonrpc": "2.0", "id": 7})).contains("neither"));
        assert!(rejected(json!({
            "jsonrpc": "2.0", "id": 7, "result": {},
            "error": {"code": 1, "message": "both"}
        }))
        .contains("both"));
        assert!(rejected(json!({"jsonrpc": "2.0", "id": 7, "error": {}})).contains("error object"));
        assert!(rejected(json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list", "result": {}
        }))
        .contains("method"));
    }

    #[test]
    fn a_valid_result_and_a_remote_error_are_distinguished() {
        let result = validate_response(
            &json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": 1}}),
            7,
            None,
        )
        .expect("valid result");
        assert_eq!(result, Some(json!({"ok": 1})));
        assert_eq!(
            validate_response(&json!({"jsonrpc": "2.0", "id": 7, "result": null}), 7, None)
                .expect("null is a result"),
            Some(Value::Null)
        );
        assert_eq!(
            validate_response(&json!({"jsonrpc": "2.0", "id": 8, "result": {}}), 7, None)
                .expect("a late response is not a failure"),
            None
        );
        let error = validate_response(
            &json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "nope"}}),
            7,
            None,
        )
        .expect_err("remote error");
        assert!(error.to_string().contains("-32601"), "{error}");
    }

    #[test]
    fn the_mcp_line_reader_bounds_a_line_before_it_allocates() {
        let mut reader = std::io::Cursor::new(vec![b'a'; 64]);
        assert!(matches!(
            read_limited_line_sync(&mut reader, 16),
            Ok(LimitedLine::TooLong)
        ));
        assert!(matches!(
            read_limited_line_sync(&mut reader, 16),
            Ok(LimitedLine::Eof)
        ));
    }

    #[test]
    fn the_mcp_line_reader_resynchronizes_after_an_over_long_line() {
        let mut data = vec![b'x'; 64];
        data.push(b'\n');
        data.extend_from_slice(b"{\"ok\":true}\n");
        let mut reader = std::io::Cursor::new(data);
        assert!(matches!(
            read_limited_line_sync(&mut reader, 16),
            Ok(LimitedLine::TooLong)
        ));
        match read_limited_line_sync(&mut reader, 16) {
            Ok(LimitedLine::Line(line)) => assert_eq!(line, b"{\"ok\":true}\n"),
            other => panic!(
                "the next line must be readable, got {}",
                matches!(other, Ok(LimitedLine::Eof))
            ),
        }
    }

    #[test]
    fn the_mcp_line_reader_keeps_non_utf8_stderr() {
        let mut reader = std::io::Cursor::new(vec![0xff, 0xfe, b'\n']);
        match read_limited_line_sync(&mut reader, 16) {
            Ok(LimitedLine::Line(line)) => {
                assert_eq!(
                    String::from_utf8_lossy(&line).trim_end(),
                    "\u{fffd}\u{fffd}"
                );
            }
            _ => panic!("a non-UTF-8 line must still be read"),
        }
    }
}
