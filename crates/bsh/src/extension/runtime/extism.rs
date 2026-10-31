//! The Extism runtime.
//!
//! Extism plugins are core Wasm modules using the Extism PDK. BSH keeps its
//! own contribution contract: a plugin exports bsh_tools, bsh_call_tool,
//! bsh_command, bsh_panel, bsh_event, and optional lifecycle functions. Host
//! access goes through one bsh_host function carrying a JSON request; every
//! operation is authorized by the same policy engine as the WASM and process
//! runtimes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use extism::{Manifest as ExtismManifest, Plugin, PluginBuilder, UserData, ValType, Wasm};
use serde_json::{json, Value};

use super::declarative::DeclarativeContributions;
use super::{load_tool_descriptors, CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::extension::capability::{CallScope, Capability, PolicyEngine};
use crate::extension::events::{EventBus, EventFilter, ExtensionEvent, Subscription, TopicPattern};
use crate::extension::manifest::{validate_relative_path, Manifest};
use crate::extension::storage::ExtensionStorage;
use crate::extension::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, TrustLevel,
};

/// The largest request or result body the bridge accepts.
const MAX_BRIDGE_BODY: usize = 8 * 1024 * 1024;

/// The Extism host bridge shared by one plugin.
struct Bridge {
    extension: ExtensionId,
    trust: TrustLevel,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    storage: ExtensionStorage,
    workspace: Option<PathBuf>,
    publishes: Vec<String>,
    subscribes: Vec<String>,
    config: String,
    subscriptions: Mutex<Vec<Subscription>>,
    events_out: Mutex<Vec<ExtensionEvent>>,
}

impl Bridge {
    fn authorize(&self, capability: Capability, scope: CallScope) -> Result<(), ExtensionError> {
        self.policy
            .authorize_call(self.extension.as_str(), self.trust, capability, &scope)
            .map(|_| ())
    }

    fn path(&self, raw: &str) -> Result<PathBuf, String> {
        let Some(workspace) = &self.workspace else {
            return Err("no workspace is open".to_string());
        };
        let relative = validate_relative_path(raw).map_err(|error| error.to_string())?;
        let path = workspace.join(relative);
        let canonical_workspace = workspace
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let parent = path.parent().unwrap_or(workspace);
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
        if !canonical_parent.starts_with(&canonical_workspace) {
            return Err("path escapes the workspace".to_string());
        }
        Ok(path)
    }

    fn dispatch(&self, raw: &str) -> Value {
        let request: Value = match serde_json::from_str(raw) {
            Ok(request) => request,
            Err(error) => return json!({"ok": false, "error": error.to_string()}),
        };
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match self.run_op(op, &request) {
            Ok(value) => json!({"ok": true, "value": value}),
            Err(error) => json!({"ok": false, "error": error}),
        }
    }

    fn run_op(&self, op: &str, request: &Value) -> Result<Value, String> {
        match op {
            "log" => {
                let message = request
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                log::info!(target: "bsh::extension", "{}: {message}", self.extension);
                Ok(Value::Null)
            }
            "config" => Ok(Value::String(self.config.clone())),
            "now_millis" => Ok(json!(SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as u64)
                .unwrap_or_default())),
            "read_file" => {
                let raw = request
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "path is required".to_string())?;
                let path = self.path(raw)?;
                self.authorize(
                    Capability::WorkspaceRead,
                    CallScope {
                        path: Some(path.clone()),
                        ..CallScope::default()
                    },
                )
                .map_err(|error| error.to_string())?;
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                if bytes.len() > MAX_BRIDGE_BODY {
                    return Err("file is too large".to_string());
                }
                Ok(match String::from_utf8(bytes.clone()) {
                    Ok(text) => json!({"text": text}),
                    Err(_) => json!({"base64": STANDARD.encode(bytes)}),
                })
            }
            "write_file" => {
                let raw = request
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "path is required".to_string())?;
                let data = request
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "data is required".to_string())?;
                let path = self.path(raw)?;
                self.authorize(
                    Capability::WorkspaceWrite,
                    CallScope {
                        path: Some(path.clone()),
                        ..CallScope::default()
                    },
                )
                .map_err(|error| error.to_string())?;
                if data.len() > MAX_BRIDGE_BODY {
                    return Err("data is too large".to_string());
                }
                std::fs::write(&path, data).map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            "fetch" => {
                let raw = request
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "url is required".to_string())?;
                let parsed = reqwest::Url::parse(raw).map_err(|error| error.to_string())?;
                if parsed.scheme() != "https" && parsed.scheme() != "http" {
                    return Err("only http and https are allowed".to_string());
                }
                let method = request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET")
                    .to_ascii_uppercase();
                self.authorize(
                    Capability::NetworkHttp,
                    CallScope {
                        domain: parsed.host_str().map(str::to_string),
                        method: Some(method.clone()),
                        ..CallScope::default()
                    },
                )
                .map_err(|error| error.to_string())?;
                let method = reqwest::Method::from_bytes(method.as_bytes())
                    .map_err(|error| error.to_string())?;
                let client = reqwest::blocking::Client::builder()
                    .timeout(Duration::from_millis(15_000))
                    .build()
                    .map_err(|error| error.to_string())?;
                let mut outgoing = client.request(method, parsed);
                if let Some(body) = request.get("body").and_then(Value::as_str) {
                    outgoing = outgoing.body(body.to_string());
                }
                let response = outgoing.send().map_err(|error| error.to_string())?;
                let bytes = response.bytes().map_err(|error| error.to_string())?;
                if bytes.len() > MAX_BRIDGE_BODY {
                    return Err("response is too large".to_string());
                }
                Ok(match String::from_utf8(bytes.to_vec()) {
                    Ok(text) => json!({"text": text}),
                    Err(error) => json!({"base64": STANDARD.encode(error.into_bytes())}),
                })
            }
            "storage_get" => {
                self.authorize(Capability::ExtensionStorage, CallScope::default())
                    .map_err(|error| error.to_string())?;
                let key = request
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "key is required".to_string())?;
                self.storage
                    .read(key)
                    .map_err(|error| error.to_string())
                    .map(|value| match value {
                        Some(bytes) => match String::from_utf8(bytes.clone()) {
                            Ok(text) => json!({"text": text}),
                            Err(_) => json!({"base64": STANDARD.encode(bytes)}),
                        },
                        None => Value::Null,
                    })
            }
            "storage_set" => {
                self.authorize(Capability::ExtensionStorage, CallScope::default())
                    .map_err(|error| error.to_string())?;
                let key = request
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "key is required".to_string())?;
                let data = request
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "data is required".to_string())?;
                self.storage
                    .write(key, data.as_bytes())
                    .map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            "storage_list" => {
                self.authorize(Capability::ExtensionStorage, CallScope::default())
                    .map_err(|error| error.to_string())?;
                self.storage
                    .list()
                    .map(|keys| json!(keys))
                    .map_err(|error| error.to_string())
            }
            "read_secret" => {
                let name = request
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "name is required".to_string())?;
                self.authorize(
                    Capability::SecretsRead,
                    CallScope {
                        secret: Some(name.to_string()),
                        ..CallScope::default()
                    },
                )
                .map_err(|error| error.to_string())?;
                let variable = format!("BSH_EXT_SECRET_{}", name.to_ascii_uppercase());
                std::env::var(&variable)
                    .map(Value::String)
                    .map_err(|_| format!("secret {name} is not available"))
            }
            "emit" => {
                let topic = request
                    .get("topic")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "topic is required".to_string())?;
                if !topic.starts_with("tool.")
                    && !self.publishes.iter().any(|pattern| {
                        TopicPattern::parse(pattern, true)
                            .map(|pattern| pattern.matches(topic))
                            .unwrap_or(false)
                    })
                {
                    return Err(format!(
                        "topic {topic} is not declared in contributes.publishes"
                    ));
                }
                let payload = request.get("payload").cloned().unwrap_or(Value::Null);
                let event = ExtensionEvent::new(self.extension.as_str(), topic, payload)
                    .map_err(|error| error.to_string())?;
                self.events
                    .publish(&event)
                    .map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            "subscribe" => {
                let topic = request
                    .get("topic")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "topic is required".to_string())?;
                if !self.subscribes.iter().any(|pattern| {
                    TopicPattern::parse(pattern, true)
                        .map(|pattern| pattern.matches(topic))
                        .unwrap_or(false)
                }) {
                    return Err(format!(
                        "topic {topic} is not declared in contributes.events"
                    ));
                }
                let filter = EventFilter::from_topics(&[topic.to_string()])
                    .map_err(|error| error.to_string())?;
                let subscription = self.events.subscribe(filter, 64);
                let id = subscription.id();
                self.subscriptions
                    .lock()
                    .map_err(|_| "subscription lock is poisoned".to_string())?
                    .push(subscription);
                Ok(json!(id))
            }
            other => Err(format!("unknown host operation {other:?}")),
        }
    }

    fn take_events(&self) -> Vec<ExtensionEvent> {
        let mut drained = Vec::new();
        if let Ok(subscriptions) = self.subscriptions.lock() {
            for subscription in subscriptions.iter() {
                while let Some(event) = subscription.try_recv() {
                    drained.push(event);
                }
            }
        }
        if let Ok(mut events) = self.events_out.lock() {
            drained.extend(std::mem::take(&mut *events));
        }
        drained
    }
}

/// The Extism runtime for one plugin.
pub(crate) struct ExtismRuntime {
    extension: ExtensionId,
    component: PathBuf,
    config: String,
    bridge: Arc<Bridge>,
    plugin: Mutex<Option<Plugin>>,
    tools: Vec<ToolDescriptor>,
    execution_ms: u64,
    memory_mb: u64,
    fuel_used: AtomicU64,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    deadline: Mutex<Option<Instant>>,
}

impl ExtismRuntime {
    /// Prepare an Extism runtime without instantiating the plugin.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        manifest: &Manifest,
        root: &Path,
        data_root: &Path,
        policy: Arc<PolicyEngine>,
        events: Arc<EventBus>,
        workspace: Option<PathBuf>,
        config: String,
        trust: TrustLevel,
    ) -> Result<Self, ExtensionError> {
        let entry = manifest
            .runtime
            .entry
            .clone()
            .ok_or_else(|| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: "the Extism runtime needs runtime.entry".to_string(),
            })?;
        let relative =
            validate_relative_path(&entry).map_err(|reason| ExtensionError::Manifest {
                path: root.join("extension.toml"),
                message: reason,
            })?;
        let component = root.join(relative);
        if !component.is_file() {
            return Err(ExtensionError::Io {
                path: component,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Extism plugin file is missing",
                ),
            });
        }
        let storage = ExtensionStorage::open(data_root, manifest.id()).map_err(|error| {
            ExtensionError::Runtime {
                extension: manifest.id().to_string(),
                message: error.to_string(),
            }
        })?;
        let bridge = Arc::new(Bridge {
            extension: manifest.id().clone(),
            trust,
            policy,
            events,
            storage,
            workspace,
            publishes: manifest.contributes.publishes.clone(),
            subscribes: manifest.contributes.events.clone(),
            config: config.clone(),
            subscriptions: Mutex::new(Vec::new()),
            events_out: Mutex::new(Vec::new()),
        });
        let tools = load_tool_descriptors(root, manifest)?;
        Ok(Self {
            extension: manifest.id().clone(),
            component,
            config,
            bridge,
            plugin: Mutex::new(None),
            tools,
            execution_ms: manifest.resources.execution_ms(),
            memory_mb: manifest.resources.memory_mb(),
            fuel_used: AtomicU64::new(0),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            deadline: Mutex::new(None),
        })
    }

    fn ensure_plugin(&self) -> Result<(), ExtensionError> {
        let mut guard = self.plugin.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "Extism plugin lock is poisoned".to_string(),
        })?;
        if guard.is_some() {
            return Ok(());
        }
        let bytes = std::fs::read(&self.component).map_err(|source| ExtensionError::Io {
            path: self.component.clone(),
            source,
        })?;
        let manifest = ExtismManifest::new([Wasm::data(bytes)])
            .with_memory_max(self.memory_mb.saturating_mul(16).min(u32::MAX as u64) as u32);
        let bridge = Arc::clone(&self.bridge);
        let plugin = PluginBuilder::new(manifest)
            .with_wasi(false)
            .with_fuel_limit(self.execution_ms.saturating_mul(10_000).max(10_000))
            .with_function(
                "bsh_host",
                [ValType::I64],
                [ValType::I64],
                UserData::new(()),
                move |plugin, inputs, outputs, _| {
                    let handle = plugin.memory_from_val(&inputs[0]);
                    let request = match handle {
                        Some(handle) => plugin.memory_get::<String>(handle)?,
                        None => "{}".to_string(),
                    };
                    let response = bridge.dispatch(&request);
                    let out = plugin.memory_new(response.to_string())?;
                    outputs[0] = plugin.memory_to_val(out);
                    Ok(())
                },
            )
            .build()
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        *guard = Some(plugin);
        Ok(())
    }

    fn call<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        input: &T,
    ) -> Result<Option<R>, ExtensionError> {
        self.ensure_plugin()?;
        let mut guard = self.plugin.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "Extism plugin lock is poisoned".to_string(),
        })?;
        let plugin = guard.as_mut().expect("plugin was just built");
        let text = serde_json::to_string(input).map_err(|error| ExtensionError::Json {
            path: self.component.clone(),
            message: error.to_string(),
        })?;
        let output = plugin.call::<&str, String>(name, text.as_str());
        if let Some(fuel) = plugin.fuel_consumed() {
            self.fuel_used.store(fuel, Ordering::Relaxed);
        }
        match output {
            Ok(output) => {
                serde_json::from_str(&output)
                    .map(Some)
                    .map_err(|error| ExtensionError::Json {
                        path: self.component.clone(),
                        message: error.to_string(),
                    })
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("unknown function") || message.contains("not found") {
                    Ok(None)
                } else {
                    Err(ExtensionError::Runtime {
                        extension: self.extension.to_string(),
                        message,
                    })
                }
            }
        }
    }

    /// The tools the plugin exports, falling back to its descriptor files.
    pub(crate) fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        let Some(items) = self.call::<Value, Vec<Value>>("bsh_tools", &json!({}))? else {
            return Ok(self.tools.clone());
        };
        let mut tools = Vec::new();
        for item in items {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            let id = ContributionId::parse(id).map_err(|error| ExtensionError::Manifest {
                path: self.component.clone(),
                message: error.to_string(),
            })?;
            let capabilities = item
                .get("capabilities")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| ExtensionError::Json {
                    path: self.component.clone(),
                    message: error.to_string(),
                })?
                .unwrap_or_default();
            tools.push(ToolDescriptor {
                id,
                description: item
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input_schema: item
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                capabilities,
            });
        }
        Ok(tools)
    }

    /// Contributions the plugin exports for the other host surfaces.
    pub(crate) fn contributions(&self) -> DeclarativeContributions {
        DeclarativeContributions::default()
    }

    fn remaining(&self) -> bool {
        !self.cancelled.load(Ordering::Relaxed)
            && self
                .deadline
                .lock()
                .ok()
                .and_then(|deadline| *deadline)
                .is_none_or(|deadline| Instant::now() < deadline)
    }
}

#[async_trait]
impl Runtime for ExtismRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::Extism
    }

    fn memory_limit(&self) -> Option<u64> {
        Some(self.memory_mb.saturating_mul(1024 * 1024))
    }

    fn fuel_used(&self) -> Option<u64> {
        Some(self.fuel_used.load(Ordering::Relaxed))
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.ensure_plugin()
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        if let Ok(mut plugin) = self.plugin.lock() {
            *plugin = None;
        }
        Ok(())
    }

    async fn cancel(&self) -> Result<(), ExtensionError> {
        self.cancelled.store(true, Ordering::SeqCst);
        self.deactivate().await
    }

    async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.descriptors()
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
    ) -> Result<Value, ExtensionError> {
        let input = json!({
            "name": name,
            "arguments": arguments,
            "correlationId": context.correlation_id,
            "config": self.config,
        });
        self.call::<_, Value>("bsh_call_tool", &input)?
            .ok_or_else(|| ExtensionError::NotFound {
                id: format!("{}::{name}", self.extension),
            })
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

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.call::<_, Value>("bsh_command", &json!({"id": id, "arguments": arguments}))?
            .ok_or_else(|| ExtensionError::NotFound {
                id: format!("{}::{id}", self.extension),
            })
    }

    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.call::<_, Value>("bsh_panel", &json!({"id": id}))?
            .ok_or_else(|| ExtensionError::NotFound {
                id: format!("{}::{id}", self.extension),
            })
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        self.ensure_plugin()?;
        let events = self.bridge.take_events();
        if events.is_empty() {
            return Ok(());
        }
        let mut guard = self.plugin.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "Extism plugin lock is poisoned".to_string(),
        })?;
        let plugin = guard.as_mut().expect("plugin was just built");
        for event in events {
            let text = serde_json::to_string(&event).unwrap_or_default();
            let _ = plugin.call::<&str, String>("bsh_event", text.as_str());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/extism-echo")
            .canonicalize()
            .expect("the Extism example exists")
    }

    #[tokio::test]
    async fn an_extism_plugin_runs_tools_commands_and_panels() {
        let root = example_root();
        let manifest = Manifest::load(&root).expect("manifest");
        let scratch =
            std::env::temp_dir().join(format!("bsh-extism-runtime-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(scratch.join("data")).expect("data");
        let policy = Arc::new(
            PolicyEngine::open(
                scratch.join("grants.toml"),
                scratch.join("audit.jsonl"),
                crate::extension::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let runtime = ExtismRuntime::new(
            &manifest,
            &root,
            &scratch.join("data"),
            policy,
            EventBus::new(),
            None,
            "{}".to_string(),
            TrustLevel::Builtin,
        )
        .expect("runtime");
        let tools = runtime.descriptors().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id.as_str(), "echo");
        let context = CallContext::new(
            ExtensionId::parse("com.example.extism-echo").expect("id"),
            "test",
            None,
            5000,
        );
        let result = runtime
            .call_tool_result("echo", json!({"text": "hello"}), &context)
            .await
            .expect("call");
        assert_eq!(result.content.len(), 1);
        assert!(
            result.structured.is_some(),
            "the guest's bsh_host clock round-trip is preserved"
        );
        let command = runtime.command("greet", json!({})).await.expect("command");
        assert_eq!(command["id"], "greet");
        let panel = runtime.panel("status").await.expect("panel");
        assert_eq!(panel["schema_version"], "1.0");
        assert!(runtime.fuel_used().unwrap_or(0) > 0);
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&scratch);
    }
    #[test]
    fn bridge_rejects_unknown_operations() {
        let root = std::env::temp_dir().join(format!("bsh-ext-extism-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).expect("data");
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        let id = ExtensionId::parse("com.example.extism").expect("id");
        let storage = ExtensionStorage::open(&root.join("data"), &id).expect("storage");
        let bridge = Bridge {
            extension: ExtensionId::parse("com.example.extism").expect("id"),
            trust: TrustLevel::Local,
            policy: Arc::new(
                PolicyEngine::open(
                    root.join("grants.toml"),
                    root.join("audit.jsonl"),
                    crate::extension::capability::PolicyMode::Enforce,
                )
                .expect("policy"),
            ),
            events: EventBus::new(),
            storage,
            workspace: Some(root.join("workspace")),
            publishes: Vec::new(),
            subscribes: Vec::new(),
            config: "{}".to_string(),
            subscriptions: Mutex::new(Vec::new()),
            events_out: Mutex::new(Vec::new()),
        };
        let response = bridge.dispatch(r#"{"op":"nope"}"#);
        assert_eq!(response["ok"], false);
        let denied = bridge.dispatch(r#"{"op":"read_file","path":"a.txt"}"#);
        assert_eq!(denied["ok"], false);
        assert!(!denied["error"].as_str().unwrap_or_default().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
