//! The external-process runtime: JSON-RPC 2.0 over stdin/stdout.
//!
//! A process package is the right shape for a language service, an MCP-style
//! server or an integration with dependencies that cannot live in wasm. The
//! host starts the executable with a cleared environment, a package-root
//! working directory, piped stdio and a deadline; it never goes through a
//! shell. The wire is one JSON object per line, with an explicit initialize
//! handshake and a tools/call request whose result is the tool's JSON value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::{load_tool_descriptors, CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::extension::capability::{CallScope, Capability, PolicyEngine};
use crate::extension::events::{EventBus, EventFilter, ExtensionEvent, Subscription};
use crate::extension::manifest::{is_allowlisted_command, validate_relative_path, Manifest};
use crate::extension::types::{ExtensionError, ExtensionId, RuntimeKind, TrustLevel, HOST_VERSION};

/// The protocol generation a process must speak.
pub(crate) const PROTOCOL_VERSION: &str = "1.0";

/// The largest one-line request or response.
pub(crate) const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// The initialize handshake deadline.
pub(crate) const HANDSHAKE_MS: u64 = 10_000;

/// The shutdown deadline.
pub(crate) const SHUTDOWN_MS: u64 = 1_000;

/// One live child process.
struct ProcessSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    token: String,
}

impl ProcessSession {
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        extension: &ExtensionId,
    ) -> Result<Value, ExtensionError> {
        self.request_with_progress(method, params, timeout_ms, extension, None)
            .await
    }

    async fn request_with_progress(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        extension: &ExtensionId,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<Value>>,
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
            extension: extension.to_string(),
            message: error.to_string(),
        })?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: format!(
                    "request is {} bytes; the line ceiling is {MAX_LINE_BYTES}",
                    line.len()
                ),
            });
        }
        line.push(b'\n');
        let stdin = &mut self.stdin;
        let stdout = &mut self.stdout;
        let deadline = Duration::from_millis(timeout_ms.max(1));
        let exchange = async {
            stdin
                .write_all(&line)
                .await
                .map_err(|source| ExtensionError::Io {
                    path: PathBuf::from("<process stdin>"),
                    source,
                })?;
            stdin.flush().await.map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })?;
            loop {
                let Some(response) = read_limited_line(stdout, MAX_LINE_BYTES, extension).await?
                else {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "process closed stdout before answering".to_string(),
                    });
                };
                let line = response.trim_ascii();
                let value: Value =
                    serde_json::from_slice(line).map_err(|error| ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: format!("response is not JSON: {error}"),
                    })?;
                if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "response is not JSON-RPC 2.0".to_string(),
                    });
                }
                if json_depth_ok(&value, 0).is_err() {
                    return Err(ExtensionError::Protocol {
                        extension: extension.to_string(),
                        message: "response nests deeper than 32 levels".to_string(),
                    });
                }
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    // A notification or an earlier response. Progress
                    // notifications are forwarded; everything else is skipped.
                    if let Some(sender) = progress {
                        if value.get("method").and_then(Value::as_str) == Some("$/progress") {
                            if let Some(params) = value.get("params") {
                                let _ = sender.send(params.clone());
                            }
                        }
                    }
                    continue;
                }
                if let Some(error) = value.get("error") {
                    return Err(ExtensionError::Runtime {
                        extension: extension.to_string(),
                        message: jsonrpc_error_message(error),
                    });
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        };
        match tokio::time::timeout(deadline, exchange).await {
            Ok(result) => result,
            Err(_) => Err(ExtensionError::Timeout {
                extension: extension.to_string(),
                operation: method.to_string(),
                millis: timeout_ms,
            }),
        }
    }

    async fn notify(
        &mut self,
        method: &str,
        params: Value,
        extension: &ExtensionId,
    ) -> Result<(), ExtensionError> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_vec(&notification).map_err(|error| ExtensionError::Protocol {
                extension: extension.to_string(),
                message: error.to_string(),
            })?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: "event notification exceeds the line ceiling".to_string(),
            });
        }
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|source| ExtensionError::Io {
                path: PathBuf::from("<process stdin>"),
                source,
            })
    }
}

/// Resident memory of one child, from procfs on Linux or ps on macOS.
fn process_rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = text.split_whitespace().next()?.parse().ok()?;
        return Some(pages.saturating_mul(4096));
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let kb: u64 = text.trim().parse().ok()?;
        return Some(kb.saturating_mul(1024));
    }
    #[allow(unreachable_code)]
    None
}

/// A bubblewrap plan that gives the child a private namespace, a read-only
/// root, and writable binds only for the package, workspace, data and temp.
///
/// Kept platform-independent so the argument plan is testable on macOS even
/// though it is only executed on Linux.
fn linux_bwrap_plan(
    root: &Path,
    workspace: Option<&Path>,
    data_root: Option<&Path>,
    program: &Path,
    args: &[String],
) -> Vec<String> {
    let mut plan = vec![
        "--die-with-parent".to_string(),
        "--unshare-all".to_string(),
        "--share-net".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--tmpfs".to_string(),
        "/tmp".to_string(),
    ];
    let mut writable: Vec<&Path> = vec![root];
    if let Some(workspace) = workspace {
        writable.push(workspace);
    }
    if let Some(data_root) = data_root {
        writable.push(data_root);
    }
    for path in writable {
        plan.push("--bind".to_string());
        plan.push(path.display().to_string());
        plan.push(path.display().to_string());
    }
    plan.push("--chdir".to_string());
    plan.push(root.display().to_string());
    plan.push(program.display().to_string());
    plan.extend(args.iter().cloned());
    plan
}

/// A macOS profile that allows reads everywhere and writes only inside the
/// package, the open workspace, temp and device nodes.
///
/// This is a filesystem profile, not a network sandbox: the process protocol
/// has no host network bridge, so network policy for a process package remains
/// the capability decision that authorized spawning it.
fn sandbox_profile(root: &Path, workspace: Option<&Path>, data_root: Option<&Path>) -> String {
    let mut profile = "(version 1)(allow default)(deny file-write*)(allow file-write*".to_string();
    let mut paths: Vec<PathBuf> = vec![
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/dev"),
        PathBuf::from("/private/dev"),
    ];
    if let Some(workspace) = workspace {
        paths.push(
            workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.to_path_buf()),
        );
    }
    if let Some(data_root) = data_root {
        paths.push(
            data_root
                .canonicalize()
                .unwrap_or_else(|_| data_root.to_path_buf()),
        );
    }
    for path in paths {
        let escaped = path.display().to_string().replace('"', "\\\"");
        profile.push_str(&format!(" (subpath \"{escaped}\")"));
    }
    profile.push(')');
    profile
}

/// Apply the manifest's resource ceilings in the child, before exec.
///
/// RLIMIT_AS bounds the address space, RLIMIT_CPU is the wall-independent CPU
/// backstop behind the host's own wall-clock timeout, and RLIMIT_NOFILE keeps a
/// package from exhausting the process's descriptor table. Failures are
/// ignored: a platform that refuses a limit still gets the host-side timeout.
fn configure_process_limits(command: &mut Command, memory: u64, cpu: u64) {
    #[cfg(unix)]
    {
        // SAFETY: pre_exec runs in the forked child before exec. The closure
        // only calls async-signal-safe setrlimit/setpgid and captures integers.
        unsafe {
            command.pre_exec(move || {
                let memory = memory.max(16 * 1024 * 1024) as libc::rlim_t;
                let address = libc::rlimit {
                    rlim_cur: memory,
                    rlim_max: memory,
                };
                let _ = libc::setrlimit(libc::RLIMIT_AS, &address);
                let cpu = cpu.max(1) as libc::rlim_t;
                let cpu_limit = libc::rlimit {
                    rlim_cur: cpu,
                    rlim_max: cpu,
                };
                let _ = libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit);
                let descriptors = libc::rlimit {
                    rlim_cur: 256,
                    rlim_max: 256,
                };
                let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &descriptors);
                let _ = libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (command, memory, cpu);
    }
}

fn jsonrpc_error_message(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    format!("JSON-RPC error {code}: {message}")
}

/// Read one newline-terminated line with a byte ceiling.
/// Reject a JSON value nested more than 32 levels deep.
fn json_depth_ok(value: &Value, depth: usize) -> Result<(), ()> {
    if depth > 32 {
        return Err(());
    }
    match value {
        Value::Array(items) => items
            .iter()
            .try_for_each(|item| json_depth_ok(item, depth + 1)),
        Value::Object(map) => map
            .values()
            .try_for_each(|item| json_depth_ok(item, depth + 1)),
        _ => Ok(()),
    }
}

async fn read_limited_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
    extension: &ExtensionId,
) -> Result<Option<Vec<u8>>, ExtensionError> {
    let mut line = Vec::new();
    loop {
        let mut consumed = 0usize;
        let mut complete = false;
        let mut eof = false;
        {
            let buffer = reader
                .fill_buf()
                .await
                .map_err(|source| ExtensionError::Io {
                    path: PathBuf::from("<process stdout>"),
                    source,
                })?;
            if buffer.is_empty() {
                eof = true;
            } else if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
                line.extend_from_slice(&buffer[..=index]);
                consumed = index + 1;
                complete = true;
            } else {
                line.extend_from_slice(buffer);
                consumed = buffer.len();
            }
        }
        if consumed > 0 {
            reader.consume(consumed);
        }
        if eof {
            if line.is_empty() {
                return Ok(None);
            }
            if line.len() > limit {
                return Err(ExtensionError::Limit {
                    extension: extension.to_string(),
                    message: format!("process line exceeds {limit} bytes"),
                });
            }
            return Ok(Some(line));
        }
        if complete {
            if line.len() > limit {
                return Err(ExtensionError::Limit {
                    extension: extension.to_string(),
                    message: format!("process line exceeds {limit} bytes"),
                });
            }
            return Ok(Some(line));
        }
        if line.len() > limit {
            return Err(ExtensionError::Limit {
                extension: extension.to_string(),
                message: format!("process line exceeds {limit} bytes"),
            });
        }
    }
}

/// The process runtime for one package.
pub(crate) struct ProcessRuntime {
    extension: ExtensionId,
    root: PathBuf,
    entry: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    tools: Vec<ToolDescriptor>,
    max_execution_ms: u64,
    max_output_bytes: u64,
    memory_bytes: u64,
    cpu_seconds: u64,
    config: String,
    workspace: Option<PathBuf>,
    data_root: Option<PathBuf>,
    policy: Option<(Arc<PolicyEngine>, TrustLevel)>,
    sandbox: bool,
    child_pid: AtomicU32,
    session: Mutex<Option<ProcessSession>>,
    subscriptions: std::sync::Mutex<Vec<Subscription>>,
}

impl ProcessRuntime {
    /// Prepare a process runtime. This validates the descriptor files and the
    /// entry's path shape but does not spawn; the first call does.
    pub(crate) fn new(manifest: &Manifest, root: &Path) -> Result<Self, ExtensionError> {
        if manifest.runtime.kind != RuntimeKind::Process {
            return Err(ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the process runtime needs runtime.kind = \"process\"".to_string(),
            });
        }
        let entry = manifest
            .runtime
            .entry
            .clone()
            .ok_or_else(|| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the process runtime needs runtime.entry".to_string(),
            })?;
        if !is_allowlisted_command(&entry) {
            validate_relative_path(&entry).map_err(|reason| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: format!("runtime.entry: {reason}"),
            })?;
        }
        let tools = load_tool_descriptors(root, manifest)?;
        Ok(Self {
            extension: manifest.id().clone(),
            root: root.to_path_buf(),
            entry,
            args: manifest.runtime.args.clone(),
            env: manifest.runtime.env.clone(),
            tools,
            max_execution_ms: manifest.resources.execution_ms(),
            max_output_bytes: manifest.resources.output_bytes(),
            memory_bytes: manifest.resources.memory_mb().saturating_mul(1024 * 1024),
            cpu_seconds: (manifest.resources.execution_ms() / 1000).max(1) + 2,
            config: "{}".to_string(),
            workspace: None,
            data_root: None,
            policy: None,
            sandbox: cfg!(target_os = "macos"),
            child_pid: AtomicU32::new(0),
            session: Mutex::new(None),
            subscriptions: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The tool descriptors, without spawning the process.
    pub(crate) fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.clone()
    }

    /// Attach the effective configuration sent in the initialize handshake.
    pub(crate) fn with_config(mut self, config: String) -> Self {
        self.config = config;
        self
    }

    /// Attach the policy engine and trust level used for per-spawn checks.
    pub(crate) fn with_policy(mut self, policy: Arc<PolicyEngine>, trust: TrustLevel) -> Self {
        self.policy = Some((policy, trust));
        self
    }

    /// Attach the workspace a sandboxed child may write inside.
    pub(crate) fn with_workspace(mut self, workspace: Option<PathBuf>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Attach the package data namespace a sandboxed child may write inside.
    pub(crate) fn with_data_root(mut self, data_root: Option<PathBuf>) -> Self {
        self.data_root = data_root;
        self
    }

    /// Disable or enable the macOS filesystem profile (tests may opt out).
    pub(crate) fn with_sandbox(mut self, sandbox: bool) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// Subscribe the process to the topics the manifest declares.
    pub(crate) fn with_events(self, events: Arc<EventBus>, topics: &[String]) -> Self {
        let mut subscriptions = Vec::new();
        for topic in topics {
            if let Ok(filter) = EventFilter::from_topics(std::slice::from_ref(topic)) {
                subscriptions.push(events.subscribe(filter, 64));
            }
        }
        if let Ok(mut slot) = self.subscriptions.lock() {
            *slot = subscriptions;
        }
        self
    }

    async fn call_tool_inner(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<Value, ExtensionError> {
        if !self.tools.iter().any(|tool| tool.id.as_str() == name) {
            return Err(ExtensionError::NotFound {
                id: format!("{}::{name}", self.extension),
            });
        }
        let wait = Duration::from_millis(context.timeout_ms.max(1));
        let mut guard = match tokio::time::timeout(wait, self.session.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                return Err(ExtensionError::Timeout {
                    extension: self.extension.to_string(),
                    operation: format!("{name} (waiting for the process)"),
                    millis: context.timeout_ms,
                });
            }
        };
        if guard.is_none() {
            *guard = Some(self.start_with_retry().await?);
        }
        let session = guard.as_mut().expect("session was just started");
        let result = session
            .request_with_progress(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                    "correlationId": context.correlation_id,
                }),
                context.timeout_ms.max(1).min(self.max_execution_ms),
                &self.extension,
                progress,
            )
            .await;
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
                            "tool result is {bytes} bytes; the ceiling is {}",
                            self.max_output_bytes
                        ),
                    });
                }
                Ok(value)
            }
            Err(error) => {
                // A timed-out or broken exchange leaves the pipe in an unknown
                // state, so the next call starts a fresh process instead of
                // reading a response that belongs to the previous call.
                *guard = None;
                Err(error)
            }
        }
    }

    /// Start with two bounded retries; a failed handshake leaves no tool side
    /// effect, so retrying the transport is safe. Delays are 100ms and 250ms.
    async fn start_with_retry(&self) -> Result<ProcessSession, ExtensionError> {
        let mut last = None;
        for delay in [0_u64, 100, 250] {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            match self.start().await {
                Ok(session) => return Ok(session),
                Err(error) => last = Some(error),
            }
        }
        Err(last.expect("a start attempt ran"))
    }

    async fn start(&self) -> Result<ProcessSession, ExtensionError> {
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
                    message: "process entry escapes the package root".to_string(),
                });
            }
            if !canonical.is_file() {
                return Err(ExtensionError::Manifest {
                    path: canonical,
                    message: "process entry is not a file".to_string(),
                });
            }
            canonical
        };
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
        let token = uuid::Uuid::new_v4().to_string();
        let mut wrapped = false;
        let mut command = if self.sandbox && cfg!(target_os = "linux") {
            wrapped = true;
            let mut command = Command::new("bwrap");
            command.args(linux_bwrap_plan(
                &self.root,
                self.workspace.as_deref(),
                self.data_root.as_deref(),
                &program,
                &self.args,
            ));
            command
        } else if self.sandbox && cfg!(target_os = "macos") {
            let mut command = Command::new("/usr/bin/sandbox-exec");
            command
                .arg("-p")
                .arg(sandbox_profile(
                    &self.root,
                    self.workspace.as_deref(),
                    self.data_root.as_deref(),
                ))
                .arg(&program);
            command
        } else {
            Command::new(&program)
        };
        configure_process_limits(&mut command, self.memory_bytes, self.cpu_seconds);
        if !wrapped {
            command.args(&self.args);
        }
        command
            .current_dir(&self.root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.env("BSH_IPC_TOKEN", &token);
        for key in ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, value) in &self.env {
            command.env(key, value);
        }
        let mut child = command.spawn().map_err(|source| ExtensionError::Io {
            path: program.clone(),
            source,
        })?;
        if let Some(pid) = child.id() {
            self.child_pid.store(pid, Ordering::Relaxed);
        }
        let stdin = child.stdin.take().ok_or_else(|| ExtensionError::Protocol {
            extension: self.extension.to_string(),
            message: "child stdin was not piped".to_string(),
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "child stdout was not piped".to_string(),
            })?;
        if let Some(stderr) = child.stderr.take() {
            let extension = self.extension.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                while let Ok(Some(line)) =
                    read_limited_line(&mut reader, 64 * 1024, &extension).await
                {
                    log::debug!(
                        target: "bsh::extension",
                        "{} stderr: {}",
                        extension,
                        String::from_utf8_lossy(&line).trim_end()
                    );
                }
            });
        }
        let mut session = ProcessSession {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            token: token.clone(),
        };
        let initialized = session
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "extensionId": self.extension.as_str(),
                    "host": { "name": "bsh", "version": HOST_VERSION },
                    "config": self.config,
                    "token": session.token.clone(),
                }),
                HANDSHAKE_MS,
                &self.extension,
            )
            .await?;
        let version = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !version.starts_with("1.") {
            let _ = session.child.start_kill();
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: format!(
                    "process speaks protocol {version:?}; this host requires {PROTOCOL_VERSION}"
                ),
            });
        }
        let echoed = initialized
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if echoed != session.token.as_str() {
            let _ = session.child.start_kill();
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "process did not echo the handshake token".to_string(),
            });
        }
        Ok(session)
    }

    async fn stop(&self) {
        let mut guard = self.session.lock().await;
        let Some(mut session) = guard.take() else {
            return;
        };
        let _ = tokio::time::timeout(
            Duration::from_millis(SHUTDOWN_MS),
            session.request("shutdown", json!({}), SHUTDOWN_MS, &self.extension),
        )
        .await;
        let _ = session.child.start_kill();
        let _ = session.child.wait().await;
        self.child_pid.store(0, Ordering::Relaxed);
    }
}

#[async_trait]
impl Runtime for ProcessRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Process
    }

    fn memory_limit(&self) -> Option<u64> {
        Some(self.memory_bytes)
    }

    fn memory_used(&self) -> Option<u64> {
        let pid = self.child_pid.load(Ordering::Relaxed);
        (pid > 0).then(|| process_rss_bytes(pid)).flatten()
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        // The descriptors are the contract; the process starts on the first
        // call so an enabled package costs no memory until it is used.
        Ok(())
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        self.stop().await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.stop().await;
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        self.call_tool_inner(name, arguments, context, None).await
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool_inner(name, arguments, context, None)
            .await
            .map(ToolResult::from_value)
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool_inner(name, arguments, context, progress.as_ref())
            .await
            .map(ToolResult::from_value)
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        let events: Vec<ExtensionEvent> = match self.subscriptions.lock() {
            Ok(subscriptions) => subscriptions
                .iter()
                .filter_map(Subscription::try_recv)
                .collect(),
            Err(_) => Vec::new(),
        };
        if events.is_empty() {
            return Ok(());
        }
        let mut guard = self.session.lock().await;
        let Some(session) = guard.as_mut() else {
            return Ok(());
        };
        for event in events {
            session
                .notify("events/event", json!({ "event": event }), &self.extension)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn the_linux_plan_binds_only_writable_roots_and_appends_the_command() {
        let plan = linux_bwrap_plan(
            Path::new("/pkg"),
            Some(Path::new("/work")),
            Some(Path::new("/data")),
            Path::new("/pkg/server"),
            &["--flag".to_string()],
        );
        assert!(plan.contains(&"--unshare-all".to_string()));
        assert_eq!(
            plan.iter().filter(|item| item.as_str() == "--bind").count(),
            3
        );
        assert_eq!(plan[plan.len() - 2], "/pkg/server");
        assert_eq!(plan[plan.len() - 1], "--flag");
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "bsh-ext-process-{label}-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(path.join("tools")).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn manifest() -> Manifest {
        Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "process"
entry = "sh"
args = ["server.sh"]

[capabilities]
required = ["process.execute"]

[contributes]
tools = ["echo"]

[resources]
max_execution_ms = 2000
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest")
    }

    #[test]
    fn deeply_nested_json_is_refused() {
        let mut value = json!(0);
        for _ in 0..40 {
            value = json!([value]);
        }
        assert!(json_depth_ok(&value, 0).is_err());
        assert!(json_depth_ok(&json!({"ok": [1, 2, 3]}), 0).is_ok());
    }

    #[tokio::test]
    async fn read_line_is_bounded_and_line_oriented() {
        let mut reader = BufReader::new(&b"a\nbb\ncc"[..]);
        let id = ExtensionId::parse("com.example.a").expect("id");
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"a\n".to_vec())
        );
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"bb\n".to_vec())
        );
        assert_eq!(
            read_limited_line(&mut reader, 16, &id).await.expect("line"),
            Some(b"cc".to_vec())
        );
        assert!(read_limited_line(&mut reader, 16, &id)
            .await
            .expect("eof")
            .is_none());
        let mut short = BufReader::new(&b"aaaaaaaaaaaaaaaaaaaa\n"[..]);
        assert!(read_limited_line(&mut short, 8, &id).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_declared_event_is_forwarded_to_the_process() {
        let scratch = Scratch::new("events");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'tools/call'*) echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
    *'events/event'*) echo "$line" >> events.log ;;
  esac
done
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"

[extension]
id = "com.example.events"
name = "Events"
version = "1.0.0"
description = "A process event subscriber"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "process"
entry = "sh"
args = ["server.sh"]

[capabilities]
required = ["process.execute"]

[contributes]
tools = ["echo"]
events = ["workspace.changed"]
"#,
            Path::new("extension.toml"),
        )
        .expect("manifest");
        let bus = EventBus::new();
        let runtime = ProcessRuntime::new(&manifest, &scratch.0)
            .expect("runtime")
            .with_events(Arc::clone(&bus), &manifest.contributes.events);
        runtime.activate().await.expect("activate");
        let context = CallContext::new(manifest.id().clone(), "events", None, 2000);
        runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect("first call starts the process");
        let event = ExtensionEvent::new("bsh", "workspace.changed", json!({"root": "/tmp"}))
            .expect("event");
        bus.publish(&event).expect("publish");
        runtime.pump().await.expect("pump");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let logged = std::fs::read_to_string(scratch.0.join("events.log")).expect("events log");
        assert!(logged.contains("workspace.changed"), "{logged}");
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_macos_sandbox_denies_a_write_outside_the_package() {
        let scratch = Scratch::new("sandbox");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        let probe = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join(format!("bsh-ext-sandbox-probe-{}", std::process::id()));
        let _ = std::fs::remove_file(&probe);
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*)
      if echo x > "$HOME/bsh-ext-sandbox-probe-PID" 2>/dev/null; then
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":false}}'
      else
        echo '{"jsonrpc":"2.0","id":2,"result":{"blocked":true}}'
      fi
      ;;
  esac
done
"#
            .replace("PID", &std::process::id().to_string()),
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0)
            .expect("runtime")
            .with_sandbox(true);
        let context = CallContext::new(manifest().id().clone(), "sandbox", None, 2000);
        let result = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect("call");
        assert_eq!(result["blocked"], true);
        assert!(!probe.exists());
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_calls_share_one_child_without_exhaustion() {
        let scratch = Scratch::new("concurrent");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.py"),
            r#"import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {"protocolVersion": "1.0", "token": request.get("params", {}).get("token")}
    elif method == "tools/call":
        result = {"ok": True}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
"#,
        )
        .expect("server");
        let manifest = Manifest::parse(
            r#"
schema_version = "1.0"
[extension]
id = "com.example.process"
name = "Process"
version = "1.0.0"
description = "A process package"
[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"
[runtime]
kind = "process"
entry = "python3"
args = ["server.py"]
[capabilities]
required = ["process.execute"]
[contributes]
tools = ["echo"]
[resources]
max_execution_ms = 2000
"#,
            &scratch.0.join("extension.toml"),
        )
        .expect("manifest");
        let runtime =
            std::sync::Arc::new(ProcessRuntime::new(&manifest, &scratch.0).expect("runtime"));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let runtime = std::sync::Arc::clone(&runtime);
            let id = manifest.id().clone();
            tasks.push(tokio::spawn(async move {
                let context = CallContext::new(id, "concurrent", None, 3000);
                runtime.call_tool("echo", json!({}), &context).await
            }));
        }
        for task in tasks {
            let result = task.await.expect("join").expect("call");
            assert_eq!(result["ok"], true);
        }
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_handshake_token_is_refused() {
        let scratch = Scratch::new("token");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}' ;;
    *'"tools/call"'*) echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "token", None, 2000);
        let error = runtime
            .call_tool("echo", json!({}), &context)
            .await
            .expect_err("token");
        assert!(error.to_string().contains("token"), "{error}");
        runtime.shutdown().await.expect("shutdown");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_answers_the_handshake_and_a_tool_call() {
        let scratch = Scratch::new("call");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}' ;;
    *) : ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "correlation", None, 2000);
        let result = runtime
            .call_tool("echo", serde_json::json!({"x": 1}), &context)
            .await
            .expect("call");
        assert_eq!(result["ok"], true);
        assert!(
            runtime.memory_used().unwrap_or(0) > 0,
            "the live child reports its resident memory"
        );
        runtime.shutdown().await.expect("shutdown");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_call_forwards_progress_notifications() {
        let scratch = Scratch::new("progress");
        std::fs::write(
            scratch.0.join("tools/echo.json"),
            r#"{"description":"Echo","input_schema":{"type":"object"}}"#,
        )
        .expect("tool");
        std::fs::write(
            scratch.0.join("server.sh"),
            r#"while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0","token":"%s"}}\n' "$BSH_IPC_TOKEN" ;;
    *'"tools/call"'*)
      echo '{"jsonrpc":"2.0","method":"$/progress","params":{"message":"half"}}'
      echo '{"jsonrpc":"2.0","id":2,"result":{"ok":true}}'
      ;;
    *) : ;;
  esac
done
"#,
        )
        .expect("server");
        let runtime = ProcessRuntime::new(&manifest(), &scratch.0).expect("runtime");
        let context = CallContext::new(manifest().id().clone(), "progress", None, 2000);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = runtime
            .call_tool_progress("echo", serde_json::json!({"x": 1}), &context, Some(sender))
            .await
            .expect("call");
        assert_eq!(result.to_value()["ok"], true);
        let progress = receiver.recv().await.expect("one progress");
        assert_eq!(progress["message"], "half");
        runtime.shutdown().await.expect("shutdown");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_missing_entry_is_an_error_not_a_panic() {
        let scratch = Scratch::new("missing");
        std::fs::write(scratch.0.join("tools/echo.json"), "{}").expect("tool");
        let mut manifest = manifest();
        manifest.runtime.entry = Some("missing-binary".to_string());
        manifest.runtime.args.clear();
        let runtime = ProcessRuntime::new(&manifest, &scratch.0).expect("runtime");
        let context = CallContext::new(manifest.id().clone(), "correlation", None, 200);
        let error = runtime
            .call_tool("echo", serde_json::json!({}), &context)
            .await
            .expect_err("missing entry");
        assert!(matches!(error, ExtensionError::Io { .. }), "{error}");
    }
}
