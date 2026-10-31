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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::declarative::{DeclarativeContributions, TextContribution};
use super::{CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::extension::manifest::{is_allowlisted_command, validate_relative_path, Manifest};
use crate::extension::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, HOST_VERSION,
};

/// The protocol version this adapter sends.
const MCP_PROTOCOL: &str = "2024-11-05";

/// The handshake deadline.
const HANDSHAKE_MS: u64 = 10_000;

/// The largest one-line response.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;

/// The largest number of each contribution kind read from one server.
const MAX_ITEMS: usize = 256;

/// One live MCP server.
struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Vec<u8>>,
    next_id: u64,
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
            extension: "mcp".to_string(),
            message: error.to_string(),
        })?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .map_err(|error| ExtensionError::Io {
                path: PathBuf::from("<mcp stdin>"),
                source: error,
            })?;
        self.stdin.flush().map_err(|error| ExtensionError::Io {
            path: PathBuf::from("<mcp stdin>"),
            source: error,
        })?;
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
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                if let Some(sender) = progress {
                    if value.get("method").and_then(Value::as_str) == Some("notifications/progress")
                    {
                        if let Some(params) = value.get("params") {
                            let _ = sender.send(params.clone());
                        }
                    }
                }
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(ExtensionError::Runtime {
                    extension: "mcp".to_string(),
                    message: error.to_string(),
                });
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
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
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|error| ExtensionError::Io {
                path: PathBuf::from("<mcp stdin>"),
                source: error,
            })
    }

    fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The MCP runtime for one server package.
pub(crate) struct McpRuntime {
    extension: ExtensionId,
    root: PathBuf,
    entry: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    config: String,
    max_execution_ms: u64,
    max_output_bytes: u64,
    catalog: Mutex<Option<Catalog>>,
    session: Mutex<Option<Session>>,
}

struct Catalog {
    tools: Vec<ToolDescriptor>,
    contexts: Vec<TextContribution>,
    prompts: Vec<TextContribution>,
}

impl McpRuntime {
    /// Prepare an MCP runtime without starting the server.
    pub(crate) fn new(
        manifest: &Manifest,
        root: &Path,
        config: String,
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
        Ok(Self {
            extension: manifest.id().clone(),
            root: root.to_path_buf(),
            entry,
            args: manifest.runtime.args.clone(),
            env: manifest.runtime.env.clone(),
            config,
            max_execution_ms: manifest.resources.execution_ms(),
            max_output_bytes: manifest.resources.output_bytes(),
            catalog: Mutex::new(None),
            session: Mutex::new(None),
        })
    }

    /// Start the server, handshake, and list its contributions.
    pub(crate) fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        let catalog = self.catalog()?;
        Ok(catalog
            .as_ref()
            .expect("catalog was just built")
            .tools
            .clone())
    }

    /// Context and prompt contributions discovered from resources/prompts.
    pub(crate) fn contributions(&self) -> Result<DeclarativeContributions, ExtensionError> {
        let catalog = self.catalog()?;
        let catalog = catalog.as_ref().expect("catalog was just built");
        Ok(DeclarativeContributions {
            contexts: catalog.contexts.clone(),
            prompts: catalog.prompts.clone(),
            ..DeclarativeContributions::default()
        })
    }

    fn catalog(&self) -> Result<std::sync::MutexGuard<'_, Option<Catalog>>, ExtensionError> {
        let mut catalog = self.catalog.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP catalog lock is poisoned".to_string(),
        })?;
        if catalog.is_none() {
            *catalog = Some(self.discover()?);
        }
        Ok(catalog)
    }

    fn discover(&self) -> Result<Catalog, ExtensionError> {
        let mut session = self.start()?;
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

    fn start(&self) -> Result<Session, ExtensionError> {
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
        let mut command = Command::new(&program);
        command
            .args(&self.args)
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
        let mut child = command.spawn().map_err(|source| ExtensionError::Io {
            path: program,
            source,
        })?;
        let stdin = child.stdin.take().ok_or_else(|| ExtensionError::Protocol {
            extension: self.extension.to_string(),
            message: "MCP stdin was not piped".to_string(),
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "MCP stdout was not piped".to_string(),
            })?;
        if let Some(stderr) = child.stderr.take() {
            let extension = self.extension.to_string();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    log::debug!(target: "bsh::extension", "{extension} mcp stderr: {line}");
                }
            });
        }
        let (sender, lines) = sync_channel(64);
        let extension = self.extension.to_string();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut buffer = Vec::new();
            let mut reader = reader;
            loop {
                buffer.clear();
                match reader.read_until(b'\n', &mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {
                        if buffer.len() > MAX_LINE_BYTES {
                            log::warn!(target: "bsh::extension", "{extension} mcp line exceeded the cap");
                            break;
                        }
                        if sender.send(buffer.clone()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut session = Session {
            child,
            stdin,
            lines,
            next_id: 1,
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
        if protocol.is_empty() {
            return Err(ExtensionError::Protocol {
                extension: self.extension.to_string(),
                message: "MCP initialize returned no protocolVersion".to_string(),
            });
        }
        session.notify("notifications/initialized", Value::Null)?;
        Ok(session)
    }

    fn read_catalog(&self, session: &mut Session) -> Result<Catalog, ExtensionError> {
        let timeout = self.max_execution_ms;
        let mut tools = Vec::new();
        let mut cursor = Value::Null;
        loop {
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
                for tool in list.iter().take(MAX_ITEMS) {
                    let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
                    let id = match ContributionId::parse(sanitize_id(name)) {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    tools.push(ToolDescriptor {
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
                    });
                }
            }
            cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
            if cursor.is_null() || tools.len() >= MAX_ITEMS {
                break;
            }
        }

        let mut contexts = Vec::new();
        if let Ok(resources) = session.request("resources/list", json!({}), timeout) {
            if let Some(list) = resources.get("resources").and_then(Value::as_array) {
                for resource in list.iter().take(MAX_ITEMS) {
                    let uri = resource
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let name = resource.get("name").and_then(Value::as_str).unwrap_or(uri);
                    let Some(id) = ContributionId::parse(sanitize_id(name)).ok() else {
                        continue;
                    };
                    let text = session
                        .request("resources/read", json!({ "uri": uri }), timeout)
                        .ok()
                        .and_then(|result| {
                            result
                                .get("contents")
                                .and_then(Value::as_array)
                                .and_then(|contents| contents.first())
                                .and_then(|content| content.get("text"))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        });
                    if let Some(text) = text {
                        contexts.push(TextContribution {
                            id,
                            description: resource
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or(uri)
                                .to_string(),
                            text: text.chars().take(64 * 1024).collect(),
                            scope: Vec::new(),
                        });
                    }
                }
            }
        }

        let mut prompts = Vec::new();
        if let Ok(result) = session.request("prompts/list", json!({}), timeout) {
            if let Some(list) = result.get("prompts").and_then(Value::as_array) {
                for prompt in list.iter().take(MAX_ITEMS) {
                    let name = prompt
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let Some(id) = ContributionId::parse(sanitize_id(name)).ok() else {
                        continue;
                    };
                    let text = session
                        .request("prompts/get", json!({ "name": name }), timeout)
                        .ok()
                        .and_then(|result| {
                            let messages = result.get("messages")?.as_array()?;
                            let text: Vec<String> = messages
                                .iter()
                                .filter_map(|message| {
                                    message
                                        .get("content")?
                                        .get("text")?
                                        .as_str()
                                        .map(str::to_string)
                                })
                                .collect();
                            Some(text.join("\n\n"))
                        });
                    if let Some(text) = text {
                        prompts.push(TextContribution {
                            id,
                            description: prompt
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or(name)
                                .to_string(),
                            text: text.chars().take(64 * 1024).collect(),
                            scope: Vec::new(),
                        });
                    }
                }
            }
        }

        Ok(Catalog {
            tools,
            contexts,
            prompts,
        })
    }
    fn call_tool_with_progress(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<Value, ExtensionError> {
        let mut guard = self.session.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP session lock is poisoned".to_string(),
        })?;
        if guard.is_none() {
            *guard = Some(self.start_with_retry()?);
        }
        let session = guard.as_mut().expect("session");
        let result = session.request_with_progress(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            self.max_execution_ms,
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
            Err(error) => {
                *guard = None;
                Err(error)
            }
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

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.descriptors().map(|_| ())
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        if let Ok(mut slot) = self.session.lock() {
            if let Some(mut session) = slot.take() {
                session.close();
            }
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.deactivate().await
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.descriptors()
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        _context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        let mut guard = self.session.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.root.clone(),
            message: "MCP session lock is poisoned".to_string(),
        })?;
        if guard.is_none() {
            *guard = Some(self.start_with_retry()?);
        }
        let session = guard.as_mut().expect("session");
        let result = session.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            self.max_execution_ms,
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
                if let Some(content) = value.get("content").and_then(Value::as_array) {
                    if content.len() == 1 {
                        if let Some(text) = content[0].get("text").and_then(Value::as_str) {
                            return Ok(Value::String(text.to_string()));
                        }
                    }
                }
                Ok(value)
            }
            Err(error) => {
                *guard = None;
                Err(error)
            }
        }
    }

    async fn call_tool_result(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool(name, arguments, context)
            .await
            .map(ToolResult::from_value)
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        _context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.call_tool_with_progress(name, arguments, progress)
            .map(ToolResult::from_value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::Manifest;

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
        let manifest = Manifest::load(&root).expect("manifest");
        let runtime = McpRuntime::new(&manifest, &root, "{}".to_string()).expect("runtime");
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
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = runtime
            .call_tool_progress("echo", serde_json::json!({"x": 1}), &context, Some(sender))
            .await
            .expect("call");
        assert_eq!(result.to_value(), serde_json::json!("{\"x\": 1}"));
        let progress = receiver.recv().await.expect("one progress notification");
        assert_eq!(progress["progress"], 1);
        runtime.shutdown().await.expect("shutdown");
    }
}
