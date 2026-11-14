//! The DeepSeek Harness host-entry bridge.
//!
//! This module is the one adapter seam for an IMPERATIVE package entry. Both
//! standards reach it: a DSH package host entry and any future imperative
//! Codex surface run through the same [`BridgeManifest`] ->
//! `BridgeSession` -> contribution path. There is deliberately no
//! second imperative path.
//!
//! # What runs
//!
//! A trusted package entry file is executed by Node in a subprocess that
//! speaks a line-delimited JSON protocol over stdio. The subprocess source is
//! the sibling asset `bridge/dsh-host-shim.js`, embedded with
//! [`include_str!`] and written to a scratch directory at each load, so the
//! host carries no runtime file dependency. Nothing runs at import time: the
//! child is started by [`BridgeSession::start`], and a package that is not
//! trusted never reaches this module at all (the caller applies the trust
//! gate).
//!
//! # The honest security story
//!
//! **A Node subprocess is not a security boundary.** The child runs with the
//! calling user privileges: it can read and write their files, open sockets,
//! and spawn its own children. The shim refuses the DSH *host services* a
//! plugin would use to reach the host own state (`ctx.fs`, `ctx.credentials`,
//! `ctx.settings`, `ctx.llm`, `ctx.subprocess`), but a plugin that imports
//! `node:fs` directly ignores those seams entirely.
//!
//! What actually protects the user:
//!
//! 1. **The trust gate.** Only a package the reader has explicitly trusted is
//!    executed. An untrusted package is parsed declaratively and its entry is
//!    refused with a named refusal; no process is started.
//! 2. **Path confinement.** The entry must resolve inside the package root, so
//!    a manifest cannot point the loader at `/bin/sh`.
//! 3. **Bounded resources.** The child output is capped ([`MAX_FRAME_BYTES`],
//!    [`MAX_OUTPUT_BYTES`]), every request has a deadline, and the child is
//!    killed when the session drops.
//! 4. **No host secret on the wire.** The protocol carries contribution
//!    metadata and tool arguments only; the host never sends a credential down
//!    it.
//!
//! A reader who wants a real boundary needs an OS sandbox (a container, a
//! `sandbox-exec` profile, a separate uid), which this crate does not build.
//! The limit is stated here rather than implied away.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// The shim source, embedded so a load needs no file on the host side.
pub const HOST_SHIM_SOURCE: &str = include_str!("bridge/dsh-host-shim.js");

/// Protocol revision this bridge speaks. It must match the shim own.
pub const BRIDGE_PROTOCOL: u32 = 1;

/// Longest a single protocol frame may be, on either side.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Longest stderr this bridge keeps when a child exits.
pub const MAX_OUTPUT_BYTES: usize = 4096;

/// Longest a host entry may run inside a load before it is killed.
///
/// Measured against the shipped packages: the DeepSeek Harness reference
/// gives a plugin fiber no explicit budget, but every shipped entry apply is
/// registration work. Twenty seconds is generous for that and still short
/// enough that a plugin which blocks on a network call does not hang a turn.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// Longest one bridged tool call may take.
pub const INVOKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Environment variable naming the interpreter the bridge runs the shim with.
pub const NODE_ENV_VAR: &str = "BOS_BRIDGE_NODE";
/// Why the bridge could not run a package entry, or refused to.
///
/// Every variant is a named refusal: a caller renders it, never swallows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeRefusal {
    /// No Node interpreter was found, so an imperative entry cannot be run.
    NoInterpreter,
    /// The entry file is missing or outside the package root.
    BadEntry {
        /// The path the manifest named.
        path: PathBuf,
        /// Why it was rejected.
        reason: String,
    },
    /// The child could not be started.
    Spawn {
        /// What failed to start.
        program: String,
        /// The OS error.
        reason: String,
    },
    /// The child exited before it answered.
    Exited {
        /// The exit status, when the OS reported one.
        status: Option<i32>,
        /// Whatever the child wrote to stderr, bounded.
        stderr: String,
    },
    /// The child answered with an error frame.
    Plugin {
        /// The message the shim reported.
        message: String,
    },
    /// The child did not answer within the deadline.
    Timeout {
        /// Which request timed out.
        request: &'static str,
        /// The deadline that expired.
        timeout: Duration,
    },
    /// The child sent a frame this bridge cannot read.
    Protocol {
        /// What was wrong.
        message: String,
    },
    /// The shim protocol revision is not this bridge own.
    Version {
        /// The revision the shim announced.
        found: u32,
        /// The revision this bridge speaks.
        expected: u32,
    },
}

impl std::fmt::Display for BridgeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeRefusal::NoInterpreter => write!(
                f,
                "no Node interpreter was found (looked for `{NODE_ENV_VAR}`, `node`, `nodejs`, and the DeepSeek Harness runtime), so the package JavaScript host entry cannot be executed"
            ),
            BridgeRefusal::BadEntry { path, reason } => write!(
                f,
                "the host entry {} was refused: {reason}",
                path.display()
            ),
            BridgeRefusal::Spawn { program, reason } => write!(
                f,
                "the Node interpreter {program} could not be started: {reason}"
            ),
            BridgeRefusal::Exited { status, stderr } => {
                write!(f, "the plugin process exited before answering")?;
                if let Some(status) = status {
                    write!(f, " (status {status})")?;
                }
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
            BridgeRefusal::Plugin { message } => {
                write!(f, "the plugin threw while loading: {message}")
            }
            BridgeRefusal::Timeout { request, timeout } => write!(
                f,
                "the plugin did not answer the {request} request within {}s and its process was killed",
                timeout.as_secs()
            ),
            BridgeRefusal::Protocol { message } => {
                write!(f, "the plugin process sent an unreadable frame: {message}")
            }
            BridgeRefusal::Version { found, expected } => write!(
                f,
                "the plugin bridge speaks protocol {found}, but this host speaks {expected}"
            ),
        }
    }
}

impl std::error::Error for BridgeRefusal {}
/// One contribution the running plugin announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeRegistration {
    /// The id the shim minted; the handle an invocation names.
    pub id: String,
    /// What was registered.
    pub kind: BridgeKind,
    /// The registration own name.
    pub name: String,
    /// The model-facing description, when the kind has one.
    pub description: String,
    /// A tool JSON Schema, when the kind is a tool.
    pub schema: Option<Value>,
    /// A command input hint, when the kind is a command.
    pub input_hint: Option<String>,
    /// A skill declared content, when the kind is a skill.
    pub content: Option<String>,
    /// The BOS hook stages a bridged event maps onto.
    pub stages: Vec<String>,
}

/// What a registration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeKind {
    /// A model-facing tool.
    Tool,
    /// A host command.
    Command,
    /// A runtime skill.
    Skill,
    /// A host-event listener that maps onto a BOS hook stage.
    Hook,
    /// A live panel card provider, the Cordis analog of the wasm panel export.
    Panel,
}

impl BridgeKind {
    /// Parse the shim `kind` discriminator.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "tool" => Some(BridgeKind::Tool),
            "command" => Some(BridgeKind::Command),
            "skill" => Some(BridgeKind::Skill),
            "hook" => Some(BridgeKind::Hook),
            "panel" => Some(BridgeKind::Panel),
            _ => None,
        }
    }

    /// The short name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            BridgeKind::Tool => "tool",
            BridgeKind::Command => "command",
            BridgeKind::Skill => "skill",
            BridgeKind::Hook => "hook",
            BridgeKind::Panel => "panel",
        }
    }
}

/// Everything one successful load produced.
#[derive(Debug, Clone, Default)]
pub struct BridgeManifest {
    /// The registrations the entry made, in the order it made them.
    pub registrations: Vec<BridgeRegistration>,
    /// Seams the plugin reached for that the bridge does not implement.
    pub refusals: Vec<String>,
    /// Log lines the plugin emitted while loading, bounded.
    pub logs: Vec<String>,
}

impl BridgeManifest {
    /// Consume the manifest and return its registrations.
    pub fn into_registrations(self) -> Vec<BridgeRegistration> {
        self.registrations
    }
}

/// How a bridge child is started and how long it may take.
#[derive(Debug, Clone)]
pub struct BridgeOptions {
    /// The interpreter to run, resolved already.
    pub interpreter: PathBuf,
    /// The package root; the entry must stay inside it.
    pub root: PathBuf,
    /// The entry file, inside the root.
    pub entry: PathBuf,
    /// The config object handed to the plugin `apply(ctx, config)`.
    pub config: Value,
    /// Deadline for the load request.
    pub load_timeout: Duration,
    /// Deadline for one invocation.
    pub invoke_timeout: Duration,
}
impl BridgeOptions {
    /// Create options with the default deadlines.
    pub fn new(
        interpreter: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        entry: impl Into<PathBuf>,
    ) -> Self {
        Self {
            interpreter: interpreter.into(),
            root: root.into(),
            entry: entry.into(),
            config: Value::Object(serde_json::Map::new()),
            load_timeout: LOAD_TIMEOUT,
            invoke_timeout: INVOKE_TIMEOUT,
        }
    }

    /// Set the config object handed to the plugin.
    pub fn with_config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }

    /// Set the load deadline.
    pub fn with_load_timeout(mut self, timeout: Duration) -> Self {
        self.load_timeout = timeout;
        self
    }

    /// Set the invocation deadline.
    pub fn with_invoke_timeout(mut self, timeout: Duration) -> Self {
        self.invoke_timeout = timeout;
        self
    }
}

/// Find a Node interpreter, in a fixed, documented order.
///
/// 1. `$BOS_BRIDGE_NODE`, when it names an existing file.
/// 2. `node` / `nodejs` on `PATH`.
/// 3. The DeepSeek Harness application own bundled runtime.
///
/// The search is a filesystem probe, never an execution: a host that finds
/// nothing records [`BridgeRefusal::NoInterpreter`] and starts no process.
pub fn find_interpreter() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(NODE_ENV_VAR) {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(path) = which("node").or_else(|| which("nodejs")) {
        return Some(path);
    }
    let bundled =
        PathBuf::from("/Applications/DeepSeek Harness.app/Contents/Resources/runtime/bin/node");
    bundled.is_file().then_some(bundled)
}

/// Resolve one executable name against `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // Windows appends the extension to the executable, so "node" on PATH is
    // "node.exe" and a bare join never found it: the cordis runtime reported
    // "no Node interpreter" on every Windows runner.
    #[cfg(windows)]
    let names = vec![name.to_string(), format!("{name}.exe")];
    #[cfg(not(windows))]
    let names = vec![name.to_string()];
    for directory in std::env::split_paths(&path) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether an interpreter is available at all.
pub fn bridge_available() -> bool {
    find_interpreter().is_some()
}
/// One bridge invocation failure.
#[derive(Debug, Clone)]
pub enum BridgeError {
    /// The plugin or protocol failed.
    Failed(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// A live plugin process and the registrations it announced.
/// Handler for a shim-to-host service call.
pub type HostCallHandler =
    std::sync::Arc<dyn Fn(&str, &str, Value) -> Result<Value, String> + Send + Sync>;

/// A live bridge subprocess plus its registrations and host-call handler.
pub struct BridgeSession {
    child: Child,
    /// The process-tree ownership boundary: a process group on Unix, a
    /// kill-on-close Job Object on Windows. A Cordis entry can spawn its own
    /// descendants through node:child_process, so killing only Node is not
    /// cleanup (issue #320).
    owner: hostkit::proc::TreeOwner,
    stdin: ChildStdin,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    /// The scratch directory holding the extracted shim; removed on drop.
    _scratch: tempfile::TempDir,
    options: BridgeOptions,
    /// Optional handler for shim-to-host service calls.
    host_call: Option<HostCallHandler>,
}

impl std::fmt::Debug for BridgeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSession")
            .field("entry", &self.options.entry)
            .field("pid", &self.child.id())
            .finish_non_exhaustive()
    }
}

impl BridgeSession {
    /// Start the shim and run the package entry, returning its manifest.
    pub fn start(options: BridgeOptions) -> Result<(Self, BridgeManifest), BridgeRefusal> {
        let entry = resolve_entry(&options.root, &options.entry)?;
        let scratch = tempfile::Builder::new()
            .prefix("bos-bridge-")
            .tempdir()
            .map_err(|error| BridgeRefusal::Spawn {
                program: "tempdir".to_string(),
                reason: error.to_string(),
            })?;
        let shim_path = scratch.path().join("dsh-host-shim.js");
        std::fs::write(&shim_path, HOST_SHIM_SOURCE).map_err(|error| BridgeRefusal::Spawn {
            program: "shim".to_string(),
            reason: error.to_string(),
        })?;

        let mut command = Command::new(&options.interpreter);
        command
            .arg("--no-warnings")
            .arg(&shim_path)
            .current_dir(&options.root)
            .env("NODE_ENV", "production")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The ownership boundary exists before extension code can run: a
        // fresh process group on Unix, a suspended-create/assign/resume Job
        // Object on Windows (issue #320).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
        let mut child = command.spawn().map_err(|error| BridgeRefusal::Spawn {
            program: options.interpreter.display().to_string(),
            reason: error.to_string(),
        })?;
        let owner = {
            let pid = child.id();
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                match hostkit::proc::adopt_suspended(pid, child.as_raw_handle(), None) {
                    Ok(owner) => owner,
                    Err(err) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(BridgeRefusal::Spawn {
                            program: options.interpreter.display().to_string(),
                            reason: format!("process-tree ownership failed: {err}"),
                        });
                    }
                }
            }
            #[cfg(not(windows))]
            {
                hostkit::proc::TreeOwner::from_pid(pid)
            }
        };
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = Some(BufReader::new(child.stdout.take().expect("piped stdout")));
        let mut session = BridgeSession {
            child,
            owner,
            stdin,
            stdout,
            next_id: 1,
            _scratch: scratch,
            options,
            host_call: None,
        };
        let manifest = session.load(&entry)?;
        Ok((session, manifest))
    }

    /// Install the handler for shim-to-host service calls.
    pub fn set_host_call(&mut self, handler: HostCallHandler) {
        self.host_call = Some(handler);
    }

    /// The child process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    /// Run the entry and collect its registrations.
    fn load(&mut self, entry: &Path) -> Result<BridgeManifest, BridgeRefusal> {
        let hello = self.read_frame(self.options.load_timeout, "handshake")?;
        match hello.get("type").and_then(Value::as_str) {
            Some("hello") => {
                let found = hello.get("protocol").and_then(Value::as_u64).unwrap_or(0) as u32;
                if found != BRIDGE_PROTOCOL {
                    self.kill();
                    return Err(BridgeRefusal::Version {
                        found,
                        expected: BRIDGE_PROTOCOL,
                    });
                }
            }
            _ => {
                self.kill();
                return Err(BridgeRefusal::Protocol {
                    message: "the process did not introduce itself".to_string(),
                });
            }
        }

        let id = self.mint();
        let frame = json!({
            "id": id,
            "method": "load",
            "params": {
                "entry": plain_path(entry),
                "root": plain_path(&self.options.root),
                "config": self.options.config,
            }
        });
        self.write_frame(&frame)?;

        let mut manifest = BridgeManifest::default();
        loop {
            let frame = match self.read_frame(self.options.load_timeout, "load") {
                Ok(frame) => frame,
                Err(error) => {
                    // A failed load is terminal. Nothing the plugin announced
                    // before it is kept: a partial load is the silent
                    // half-capability this substrate exists to prevent.
                    self.kill();
                    return Err(error);
                }
            };
            match frame.get("type").and_then(Value::as_str) {
                Some("register") => {
                    if let Some(registration) = parse_registration(&frame) {
                        manifest.registrations.push(registration);
                    } else {
                        manifest.refusals.push(format!(
                            "the plugin announced a registration this bridge could not read: {}",
                            bounded(&frame.to_string(), 200)
                        ));
                    }
                }
                Some("refuse") => {
                    let kind = frame
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("service");
                    let name = frame.get("name").and_then(Value::as_str).unwrap_or("");
                    let reason = frame
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("no reason reported");
                    manifest
                        .refusals
                        .push(format!("{reason} (reported as {kind}: {name})"));
                }
                Some("log") => {
                    let level = frame.get("level").and_then(Value::as_str).unwrap_or("info");
                    let text = frame.get("text").and_then(Value::as_str).unwrap_or("");
                    manifest
                        .logs
                        .push(bounded(&format!("[{level}] {text}"), 512));
                }
                Some("error") => {
                    self.kill();
                    return Err(BridgeRefusal::Plugin {
                        message: frame
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("the plugin reported an error")
                            .to_string(),
                    });
                }
                Some("result") => {
                    if frame.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if frame.get("ok").and_then(Value::as_bool) == Some(true) {
                        return Ok(manifest);
                    }
                    self.kill();
                    return Err(BridgeRefusal::Plugin {
                        message: frame
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("the plugin failed to load")
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
    }
    /// Call one registration the plugin announced.
    pub fn invoke(&mut self, registration: &str, input: Value) -> Result<Value, BridgeError> {
        self.invoke_with_timeout(registration, input, None, None)
    }

    /// Invoke with a caller deadline, capped by the bridge's own timeout.
    ///
    /// The workspace travels with the call, not the load: one bridge serves
    /// every turn of a session, and the shim hands it to the tool's execute
    /// as the second argument so a panel or tool can act on the open
    /// workspace without the model naming it.
    pub fn invoke_with_timeout(
        &mut self,
        registration: &str,
        input: Value,
        timeout: Option<Duration>,
        workspace: Option<&Path>,
    ) -> Result<Value, BridgeError> {
        let timeout = timeout
            .map(|timeout| timeout.min(self.options.invoke_timeout))
            .unwrap_or(self.options.invoke_timeout);
        let id = self.mint();
        let frame = json!({
            "id": id,
            "method": "invoke",
            "params": {
                "registration": registration,
                "input": input,
                "workspace": workspace.map(|path| path.display().to_string()),
            }
        });
        self.write_frame(&frame)
            .map_err(|error| BridgeError::Failed(error.to_string()))?;
        loop {
            let frame = self
                .read_frame(timeout, "invoke")
                .map_err(|error| BridgeError::Failed(error.to_string()))?;
            match frame.get("type").and_then(Value::as_str) {
                Some("result") => {
                    if frame.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if frame.get("ok").and_then(Value::as_bool) == Some(true) {
                        return Ok(frame
                            .get("value")
                            .and_then(|value| value.get("result"))
                            .cloned()
                            .unwrap_or(Value::Null));
                    }
                    return Err(BridgeError::Failed(
                        frame
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("the plugin call failed")
                            .to_string(),
                    ));
                }
                Some("host_call") => {
                    let call_id = frame.get("id").and_then(Value::as_u64);
                    let service = frame
                        .get("service")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let method = frame
                        .get("method_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let params = frame.get("params").cloned().unwrap_or(Value::Null);
                    let outcome = match &self.host_call {
                        Some(handler) => handler(service, method, params),
                        None => Err(format!("no host handler for {service}.{method}")),
                    };
                    let response = match outcome {
                        Ok(value) => json!({
                            "id": call_id,
                            "method": "host_result",
                            "params": { "ok": true, "value": value }
                        }),
                        Err(error) => json!({
                            "id": call_id,
                            "method": "host_result",
                            "params": { "ok": false, "error": error }
                        }),
                    };
                    self.write_frame(&response)
                        .map_err(|error| BridgeError::Failed(error.to_string()))?;
                    continue;
                }
                _ => continue,
            }
        }
    }

    /// Ask the child to exit, then make sure it did.
    pub fn shutdown(mut self) {
        let id = self.mint();
        let _ = self.write_frame(&json!({ "id": id, "method": "shutdown" }));
        let _ = self.read_frame(Duration::from_millis(500), "shutdown");
        self.kill();
    }

    /// Terminate the whole process tree and reap the root, so no descendant
    /// outlives the session. Idempotent: timeout, explicit shutdown and Drop
    /// all route here (issue #320).
    pub fn kill(&mut self) {
        let _ = self.owner.kill(std::time::Duration::from_millis(300));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn mint(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn write_frame(&mut self, frame: &Value) -> Result<(), BridgeRefusal> {
        let mut line = frame.to_string();
        if line.len() > MAX_FRAME_BYTES {
            return Err(BridgeRefusal::Protocol {
                message: format!("frame of {} bytes exceeds the limit", line.len()),
            });
        }
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|error| BridgeRefusal::Exited {
                status: self.child.try_wait().ok().flatten().and_then(|s| s.code()),
                stderr: error.to_string(),
            })
    }
    /// Read one frame, honouring a deadline.
    ///
    /// The read runs on a helper thread because a blocking pipe read has no
    /// timeout of its own. When the deadline expires the child is killed, which
    /// closes the pipe and releases the reader; the thread is then joined, so a
    /// timeout leaks no thread and no process.
    fn read_frame(
        &mut self,
        timeout: Duration,
        request: &'static str,
    ) -> Result<Value, BridgeRefusal> {
        let reader = self.stdout.take().ok_or_else(|| BridgeRefusal::Protocol {
            message: "the stdout handle was already taken".to_string(),
        })?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("bos-bridge-read".to_string())
            .spawn(move || {
                let mut reader = reader;
                let result = read_one_frame(&mut reader);
                let _ = sender.send(result);
                reader
            })
            .map_err(|error| BridgeRefusal::Protocol {
                message: format!("could not start the reader thread: {error}"),
            })?;

        let outcome = receiver.recv_timeout(timeout);
        match outcome {
            Ok(Ok(Some(value))) => {
                self.stdout = handle.join().ok();
                Ok(value)
            }
            Ok(Ok(None)) => {
                self.stdout = handle.join().ok();
                Err(self.exited())
            }
            Ok(Err(message)) => {
                self.stdout = handle.join().ok();
                Err(BridgeRefusal::Protocol { message })
            }
            Err(_) => {
                self.kill();
                self.stdout = handle.join().ok();
                Err(BridgeRefusal::Timeout { request, timeout })
            }
        }
    }

    fn exited(&mut self) -> BridgeRefusal {
        let status = self.child.try_wait().ok().flatten().and_then(|s| s.code());
        let stderr = self
            .child
            .stderr
            .take()
            .map(read_bounded_stderr)
            .unwrap_or_default();
        BridgeRefusal::Exited { status, stderr }
    }
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_one_frame(reader: &mut BufReader<ChildStdout>) -> Result<Option<Value>, String> {
    let mut line = String::new();
    let read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(format!("frame of {} bytes exceeds the limit", line.len()));
    }
    serde_json::from_str(line.trim())
        .map(Some)
        .map_err(|error| format!("{error}: {}", bounded(line.trim(), 200)))
}

/// Read at most a few kilobytes of a child stderr, for the refusal text.
fn read_bounded_stderr(stderr: std::process::ChildStderr) -> String {
    let mut reader = BufReader::new(stderr);
    let mut text = String::new();
    let mut line = String::new();
    let mut total = 0usize;
    while total < MAX_OUTPUT_BYTES {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                total += read;
                text.push_str(line.trim_end());
                text.push(' ');
            }
        }
    }
    bounded(text.trim(), 512)
}

/// Trim a string to a byte budget on a character boundary.
pub fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
/// Check that the entry stays inside the package root and exists.
/// A path string Node can resolve: Windows canonicalization returns the
/// verbatim device form (\\?\D:\...), which Node's URL conversion mangles
/// into a drive-scheme URL (issue #304). Strip the verbatim prefix back to the
/// ordinary drive/UNC form; containment is already proven by resolve_entry
/// and re-checked by the shim through realpathSync.
fn plain_path(path: &Path) -> String {
    strip_verbatim(&path.to_string_lossy())
}

/// Strip a Windows verbatim device prefix back to an ordinary path.
///
/// `\\?\D:\x` becomes `D:\x` and `\\?\UNC\server\share` becomes
/// `\\server\share`. A non-verbatim path is returned unchanged. Kept as a
/// string operation so it is testable on every host (issue #304).
fn strip_verbatim(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest);
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return rest.to_string();
    }
    text.to_string()
}

/// Resolve the package entry inside the canonical root, refusing traversal
/// and symlink escapes before the bridge loads it.
pub fn resolve_entry(root: &Path, entry: &Path) -> Result<PathBuf, BridgeRefusal> {
    // BOTH sides are canonicalised before the comparison. On macOS `/var` is a
    // symlink to `/private/var`, so a canonical entry under a non-canonical root
    // compares as OUTSIDE the package and every scratch-directory load fails this
    // check. The containment test must compare like with like.
    let canonical_root = root
        .canonicalize()
        .map_err(|error| BridgeRefusal::BadEntry {
            path: root.to_path_buf(),
            reason: error.to_string(),
        })?;
    let candidate = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        canonical_root.join(entry)
    };
    if !candidate.is_file() {
        return Err(BridgeRefusal::BadEntry {
            path: candidate,
            reason: "the file does not exist".to_string(),
        });
    }
    let canonical = candidate
        .canonicalize()
        .map_err(|error| BridgeRefusal::BadEntry {
            path: candidate.clone(),
            reason: error.to_string(),
        })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(BridgeRefusal::BadEntry {
            path: canonical,
            reason: "the entry leaves the package root".to_string(),
        });
    }
    Ok(canonical)
}

/// Read one registration frame.
fn parse_registration(frame: &Value) -> Option<BridgeRegistration> {
    let id = frame
        .get("registration")
        .and_then(Value::as_str)?
        .to_string();
    let kind = BridgeKind::parse(frame.get("kind").and_then(Value::as_str)?)?;
    let name = frame.get("name").and_then(Value::as_str)?.to_string();
    let description = frame
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let schema = match kind {
        BridgeKind::Tool => Some(
            frame
                .get("schema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
        ),
        _ => None,
    };
    let input_hint = frame
        .get("input")
        .and_then(|value| value.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let content = frame
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let stages = frame
        .get("stages")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(BridgeRegistration {
        id,
        kind,
        name,
        description,
        schema,
        input_hint,
        content,
        stages,
    })
}

/// Map a shim refusal kind onto the substrate refusal taxonomy.
/// A process-wide pool of live plugin bridge sessions, keyed by package id.
///
/// A bridged tool is only CALLABLE while the plugin's process is alive, so the
/// session cannot be owned by the manifest parse that produced it: a build reads
/// the package on every turn, and a fresh child per turn would multiply
/// processes and lose any state the plugin accumulated. The pool is therefore
/// keyed by `(id, root)` and REUSED: a second build for the same package finds
/// the running session instead of spawning another.
///
/// What invalidates an entry:
///
/// - the package is disabled or uninstalled (it is not asked for, so
///   [`BridgePool::retain`] drops it and the child exits);
/// - its path changes (a reinstall elsewhere is a different key);
/// - its trust is withdrawn (the build no longer asks for it);
/// - the process dies on its own, which the next call discovers.
///
/// An UNTRUSTED package never reaches this pool, because the loader refuses its
/// entry before a session is ever created.
#[derive(Default)]
pub struct BridgePool {
    entries: HashMap<String, PooledSession>,
}

struct PooledSession {
    /// The package root, so a repointed install is a different session.
    root: PathBuf,
    /// The live process backing this package's tools.
    session: Arc<Mutex<BridgeSession>>,
}

impl std::fmt::Debug for BridgePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgePool")
            .field("loaded", &self.loaded())
            .finish()
    }
}

impl BridgePool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// The ids of every package holding a live process.
    pub fn loaded(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.entries.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// The live session for one package, when one is running at this root.
    pub fn session(&self, id: &str, root: &Path) -> Option<Arc<Mutex<BridgeSession>>> {
        self.entries
            .get(id)
            .filter(|entry| entry.root == root)
            .map(|entry| Arc::clone(&entry.session))
    }

    /// How many plugin processes the pool holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pool holds no process.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adopt a session a load just produced, replacing any stale one.
    pub fn adopt(&mut self, id: &str, root: PathBuf, session: Arc<Mutex<BridgeSession>>) {
        let previous = self
            .entries
            .insert(id.to_string(), PooledSession { root, session });
        if let Some(previous) = previous {
            // A different root, or a re-run after a failure, replaces the old
            // child; dropping the last handle reaps it.
            shutdown_shared(previous.session);
        }
    }

    /// Drop every session whose id the predicate rejects, stopping its child.
    ///
    /// The caller passes the packages this build actually asked for, so a
    /// disabled, uninstalled, untrusted or moved package loses its process
    /// rather than lingering as an invisible child.
    pub fn retain(&mut self, keep: impl Fn(&str, &Path) -> bool) {
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter(|(id, entry)| !keep(id, &entry.root))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.stop(&id);
        }
    }

    /// Stop one package's process.
    pub fn stop(&mut self, id: &str) -> bool {
        match self.entries.remove(id) {
            Some(entry) => {
                shutdown_shared(entry.session);
                true
            }
            None => false,
        }
    }

    /// Stop every package's process.
    pub fn stop_all(&mut self) {
        for id in self.loaded() {
            self.stop(&id);
        }
    }
}

/// Shut a shared session down when this is the last handle to it.
///
/// A `BridgedTool` holds its own `Arc`, so a pool that has been dropped but
/// whose tools are still on an agent cannot force the child closed from here;
/// it drops the handle and lets `Drop` do it when the last user goes. That is
/// the correct ordering: a tool whose session was torn down under it would
/// fail on the next call, and an agent built this turn may still be running.
fn shutdown_shared(session: Arc<Mutex<BridgeSession>>) {
    if let Ok(session) = Arc::try_unwrap(session) {
        match session.into_inner() {
            Ok(session) => session.shutdown(),
            Err(poisoned) => poisoned.into_inner().shutdown(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::strip_verbatim;

    /// The verbatim device prefix Node mangles is stripped back to an ordinary
    /// drive/UNC path, and an ordinary path is unchanged (issue #304).
    #[test]
    fn verbatim_prefixes_become_ordinary_paths() {
        assert_eq!(strip_verbatim(r"\\?\D:\pkg\entry.js"), r"D:\pkg\entry.js");
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\entry.js"),
            r"\\server\share\entry.js"
        );
        assert_eq!(strip_verbatim(r"D:\pkg\entry.js"), r"D:\pkg\entry.js");
        assert_eq!(strip_verbatim("/unix/path/entry.js"), "/unix/path/entry.js");
        assert_eq!(
            strip_verbatim(r"C:\a dir with spaces\entry.js"),
            r"C:\a dir with spaces\entry.js"
        );
    }
}
