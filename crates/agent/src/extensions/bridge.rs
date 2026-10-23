//! The DeepSeek Harness host-entry bridge.
//!
//! This module is the one adapter seam for an IMPERATIVE package entry. Both
//! standards reach it: a DSH package host entry and any future imperative
//! Codex surface run through the same [`BridgeManifest`] ->
//! [`BridgeSession`] -> [`Contribution`] path. There is deliberately no
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
//!    refused with a named [`Refusal`]; no process is started.
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

use react::tool::ToolError;

use super::{Contribution, ExtensionError, ExtensionStandard, Refusal, RefusedFeature};

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
}

impl BridgeKind {
    /// Parse the shim `kind` discriminator.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "tool" => Some(BridgeKind::Tool),
            "command" => Some(BridgeKind::Command),
            "skill" => Some(BridgeKind::Skill),
            "hook" => Some(BridgeKind::Hook),
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
        }
    }
}

/// Everything one successful load produced.
#[derive(Debug, Clone, Default)]
pub struct BridgeManifest {
    /// The registrations the entry made, in the order it made them.
    pub registrations: Vec<BridgeRegistration>,
    /// Seams the plugin reached for that the bridge does not implement.
    pub refusals: Vec<Refusal>,
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
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Whether an interpreter is available at all.
pub fn bridge_available() -> bool {
    find_interpreter().is_some()
}
/// A live plugin process and the registrations it announced.
pub struct BridgeSession {
    child: Child,
    stdin: ChildStdin,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    /// The scratch directory holding the extracted shim; removed on drop.
    _scratch: tempfile::TempDir,
    options: BridgeOptions,
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

        let mut child = Command::new(&options.interpreter)
            .arg("--no-warnings")
            .arg(&shim_path)
            .current_dir(&options.root)
            .env("NODE_ENV", "production")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| BridgeRefusal::Spawn {
                program: options.interpreter.display().to_string(),
                reason: error.to_string(),
            })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = Some(BufReader::new(child.stdout.take().expect("piped stdout")));
        let mut session = BridgeSession {
            child,
            stdin,
            stdout,
            next_id: 1,
            _scratch: scratch,
            options,
        };
        let manifest = session.load(&entry)?;
        Ok((session, manifest))
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
                "entry": entry.to_string_lossy(),
                "root": self.options.root.to_string_lossy(),
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
                        manifest.refusals.push(Refusal::new(
                            self.package_id(),
                            RefusedFeature::ImperativeHostEntry,
                            Some(entry.to_path_buf()),
                            format!(
                                "the plugin announced a registration this bridge could not read: {}",
                                bounded(&frame.to_string(), 200)
                            ),
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
                    manifest.refusals.push(Refusal::new(
                        self.package_id(),
                        refused_feature_for(kind),
                        Some(entry.to_path_buf()),
                        format!("{reason} (reported as {kind}: {name})"),
                    ));
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
    pub fn invoke(&mut self, registration: &str, input: Value) -> Result<Value, ToolError> {
        let id = self.mint();
        let frame = json!({
            "id": id,
            "method": "invoke",
            "params": { "registration": registration, "input": input }
        });
        self.write_frame(&frame)
            .map_err(|error| ToolError::Failed(error.to_string()))?;
        loop {
            let frame = self
                .read_frame(self.options.invoke_timeout, "invoke")
                .map_err(|error| ToolError::Failed(error.to_string()))?;
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
                    return Err(ToolError::Failed(
                        frame
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("the plugin call failed")
                            .to_string(),
                    ));
                }
                _ => continue,
            }
        }
    }

    /// The package id the child refusals are attributed to.
    fn package_id(&self) -> String {
        self.options
            .root
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("plugin")
            .to_string()
    }

    /// Ask the child to exit, then make sure it did.
    pub fn shutdown(mut self) {
        let id = self.mint();
        let _ = self.write_frame(&json!({ "id": id, "method": "shutdown" }));
        let _ = self.read_frame(Duration::from_millis(500), "shutdown");
        self.kill();
    }

    /// Kill the child and reap it, so no process outlives the session.
    pub fn kill(&mut self) {
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
fn refused_feature_for(kind: &str) -> RefusedFeature {
    match kind {
        "mcp" => RefusedFeature::McpServer,
        "app" => RefusedFeature::ExternalApp,
        "slots" => RefusedFeature::ImperativeClientEntry,
        _ => RefusedFeature::ImperativeHostEntry,
    }
}
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
/// A [`react::tool::Tool`] backed by a live bridge registration.
///
/// The tool is CALLABLE: its `run` sends one invocation frame and returns what
/// the plugin `execute` returned. The session is shared behind a mutex because
/// one plugin registrations all speak on one stdio pipe, so two concurrent
/// calls serialise rather than interleaving frames.
pub struct BridgedTool {
    name: String,
    description: String,
    schema: Value,
    registration: String,
    session: Arc<Mutex<BridgeSession>>,
}

impl BridgedTool {
    /// Create a tool over one registration of a running session.
    pub fn new(registration: &BridgeRegistration, session: Arc<Mutex<BridgeSession>>) -> Self {
        Self {
            name: registration.name.clone(),
            description: registration.description.clone(),
            schema: registration
                .schema
                .clone()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
            registration: registration.id.clone(),
            session,
        }
    }
}

impl react::tool::Tool for BridgedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn category(&self) -> String {
        "extension".to_string()
    }

    fn json_schema(&self) -> Value {
        self.schema.clone()
    }

    fn run(&self, input: &Value) -> Result<Value, ToolError> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| ToolError::Failed("the plugin session lock was poisoned".to_string()))?;
        session.invoke(&self.registration, input.clone())
    }
}

/// Turn a bridge manifest into substrate contributions.
///
/// This is the mapping table in code: a bridged registration and a declarative
/// file land in the SAME [`Contribution`] variants, so the registry, the agent
/// install, and the refusal list have one implementation.
pub fn contributions_from(
    manifest: &BridgeManifest,
    session: Arc<Mutex<BridgeSession>>,
) -> (Vec<Contribution>, Vec<Refusal>) {
    let mut contributions = Vec::new();
    let mut refusals = manifest.refusals.clone();
    for registration in &manifest.registrations {
        match registration.kind {
            BridgeKind::Tool => {
                contributions.push(Contribution::Tool(Arc::new(BridgedTool::new(
                    registration,
                    Arc::clone(&session),
                ))));
            }
            BridgeKind::Command => {
                contributions.push(Contribution::Command(crate::plugins::PluginCommand::new(
                    registration.name.clone(),
                    registration.description.clone(),
                )));
            }
            BridgeKind::Skill => {
                let reason = if registration.content.is_some() {
                    format!(
                        "the plugin registered runtime skill {}; this host registers skills from a SKILL.md path, and the registry has no carrier for an in-memory body",
                        registration.name
                    )
                } else {
                    format!(
                        "the plugin registered skill {} with no content, so there is no document to register",
                        registration.name
                    )
                };
                refusals.push(Refusal::new(
                    &registration.name,
                    RefusedFeature::UnsupportedContribution,
                    None,
                    reason,
                ));
            }
            BridgeKind::Hook => {
                refusals.push(Refusal::new(
                    &registration.name,
                    RefusedFeature::UnsupportedContribution,
                    None,
                    format!(
                        "the plugin listens for {} (stages: {}); this host hook seam takes an in-process AgentPlugin, so a bridged listener is recorded and not yet dispatched",
                        registration.name,
                        registration.stages.join(", ")
                    ),
                ));
            }
        }
    }
    (contributions, refusals)
}
/// A registry of live bridge sessions, one per package, keyed by package id.
#[derive(Default)]
pub struct BridgeRegistry {
    sessions: HashMap<String, Arc<Mutex<BridgeSession>>>,
    manifests: HashMap<String, BridgeManifest>,
}

impl std::fmt::Debug for BridgeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeRegistry")
            .field("loaded", &self.loaded())
            .finish()
    }
}

impl BridgeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load one package entry and keep its session alive.
    pub fn load(
        &mut self,
        id: &str,
        options: BridgeOptions,
    ) -> Result<&BridgeManifest, BridgeRefusal> {
        let (session, manifest) = BridgeSession::start(options)?;
        self.sessions
            .insert(id.to_string(), Arc::new(Mutex::new(session)));
        self.manifests.insert(id.to_string(), manifest);
        Ok(self.manifests.get(id).expect("just inserted"))
    }

    /// The session for one package, when it is loaded.
    pub fn session(&self, id: &str) -> Option<Arc<Mutex<BridgeSession>>> {
        self.sessions.get(id).cloned()
    }

    /// The manifest for one package, when it is loaded.
    pub fn manifest(&self, id: &str) -> Option<&BridgeManifest> {
        self.manifests.get(id)
    }

    /// The ids of every loaded package.
    pub fn loaded(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.sessions.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Unload one package: the child exits before this returns.
    pub fn unload(&mut self, id: &str) -> bool {
        self.manifests.remove(id);
        match self.sessions.remove(id) {
            Some(session) => {
                if let Ok(session) = Arc::try_unwrap(session) {
                    match session.into_inner() {
                        Ok(session) => session.shutdown(),
                        Err(poisoned) => poisoned.into_inner().shutdown(),
                    }
                }
                true
            }
            None => false,
        }
    }

    /// Unload every package.
    pub fn unload_all(&mut self) {
        for id in self.loaded() {
            self.unload(&id);
        }
    }
}

/// One extension bridged contributions, with the session that backs them.
pub struct BridgedExtension {
    /// The live session; keep it alive for as long as the tools must work.
    pub session: Arc<Mutex<BridgeSession>>,
    /// The contributions, ready to register.
    pub contributions: Vec<Contribution>,
    /// The refusals the bridge and the plugin recorded.
    pub refusals: Vec<Refusal>,
    /// What the plugin logged while loading.
    pub logs: Vec<String>,
}

/// Load a package entry and lower it onto contributions.
///
/// The single entry point both standards use. `root` is the package directory,
/// `entry` the manifest-declared path, and `standard` the standard being read
/// (kept so a future Codex imperative surface reuses this function unchanged).
pub fn load_entry(
    root: &Path,
    entry: &Path,
    standard: ExtensionStandard,
    config: Value,
) -> Result<BridgedExtension, BridgeRefusal> {
    let _ = standard;
    let interpreter = find_interpreter().ok_or(BridgeRefusal::NoInterpreter)?;
    let options = BridgeOptions::new(interpreter, root, entry).with_config(config);
    let (session, manifest) = BridgeSession::start(options)?;
    let session = Arc::new(Mutex::new(session));
    let (contributions, refusals) = contributions_from(&manifest, Arc::clone(&session));
    Ok(BridgedExtension {
        session,
        contributions,
        refusals,
        logs: manifest.logs.clone(),
    })
}

/// Turn a [`BridgeRefusal`] into the substrate named refusal.
pub fn refusal_for(extension: &str, entry: &Path, refusal: &BridgeRefusal) -> Refusal {
    Refusal::new(
        extension,
        RefusedFeature::ImperativeHostEntry,
        Some(entry.to_path_buf()),
        format!(
            "the host entry was not executed in the DeepSeek Harness bridge: {refusal}. Nothing it would have registered was loaded."
        ),
    )
}

/// Turn a [`BridgeRefusal`] into an [`ExtensionError`].
pub fn error_for(extension: &str, entry: &Path, refusal: BridgeRefusal) -> ExtensionError {
    ExtensionError::Refused(refusal_for(extension, entry, &refusal))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_outside_the_package_is_refused_before_any_process() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let root = scratch.path().join("pkg");
        std::fs::create_dir_all(&root).expect("pkg");
        let outside = scratch.path().join("outside.js");
        std::fs::write(&outside, "module.exports = {}").expect("write");
        let error = resolve_entry(&root, &outside).expect_err("outside is refused");
        match error {
            BridgeRefusal::BadEntry { reason, .. } => {
                assert!(reason.contains("leaves the package"), "{reason}");
            }
            other => panic!("expected BadEntry, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_entry_is_refused() {
        let scratch = tempfile::tempdir().expect("tempdir");
        let error = resolve_entry(scratch.path(), Path::new("nope.js")).expect_err("missing");
        assert!(matches!(error, BridgeRefusal::BadEntry { .. }));
    }

    #[test]
    fn the_protocol_revision_is_pinned() {
        assert_eq!(BRIDGE_PROTOCOL, 1);
        assert!(HOST_SHIM_SOURCE.contains("const PROTOCOL = 1"));
    }

    #[test]
    fn bounded_never_splits_a_character() {
        let text = "ααααα";
        assert_eq!(bounded(text, 100), text);
        let trimmed = bounded(text, 3);
        assert!(trimmed.ends_with('…'));
        assert!(trimmed.len() <= 3 + '…'.len_utf8());
    }
}
