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
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::{load_tool_descriptors, CallContext, Runtime, ToolDescriptor};
use crate::extension::events::{EventBus, EventFilter, ExtensionEvent, Subscription};
use crate::extension::manifest::{is_allowlisted_command, validate_relative_path, Manifest};
use crate::extension::types::{ExtensionError, ExtensionId, RuntimeKind, HOST_VERSION};

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
}

impl ProcessSession {
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout_ms: u64,
        extension: &ExtensionId,
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
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    // A notification or an earlier response; skip it rather
                    // than treating it as this call's answer.
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
        let mut command = Command::new(&program);
        configure_process_limits(&mut command, self.memory_bytes, self.cpu_seconds);
        command
            .args(&self.args)
            .current_dir(&self.root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
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
        };
        let initialized = session
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "extensionId": self.extension.as_str(),
                    "host": { "name": "bsh", "version": HOST_VERSION },
                    "config": self.config,
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
    }
}

#[async_trait]
impl Runtime for ProcessRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Process
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
            *guard = Some(self.start().await?);
        }
        let session = guard.as_mut().expect("session was just started");
        let result = session
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments,
                    "correlationId": context.correlation_id,
                }),
                context.timeout_ms.max(1).min(self.max_execution_ms),
                &self.extension,
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
    *'"initialize"'*) echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}' ;;
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
    *'"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"1.0"}}' ;;
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
