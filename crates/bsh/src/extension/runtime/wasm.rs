//! The Wasmtime Component runtime.
//!
//! A component is compiled, validated and instantiated behind the same
//! Runtime contract as a process. The host side of the WIT world is implemented
//! here: every function authorizes against the package's capabilities, bounds
//! its input and output, and reports failures as normalized extension errors.
//!
//! Limits are enforced by the store: linear memory by StoreLimits, CPU by fuel,
//! wall clock by epoch interruption, and output bytes by the host wrapper.
//! Cancellation sets the epoch deadline to the current tick so a running guest
//! traps at its next epoch check.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::Value;
use wasmtime::component::{bindgen, Component, HasSelf, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use super::{CallContext, Runtime, ToolDescriptor, ToolResult};
use crate::extension::capability::{CallScope, Capability, PolicyEngine};
use crate::extension::events::{EventBus, EventFilter, ExtensionEvent, TopicPattern};
use crate::extension::manifest::{validate_relative_path, Manifest, ResourcesSpec};
use crate::extension::package::write_atomic;
use crate::extension::storage::ExtensionStorage;
use crate::extension::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, TrustLevel, HOST_VERSION,
};

bindgen!({
    path: "wit",
    world: "bsh-extension",
});

/// The largest file or network body a host function will return.
const MAX_HOST_BODY: usize = 8 * 1024 * 1024;

/// One epoch tick is ten milliseconds.
const EPOCH_TICK_MS: u64 = 10;

/// The process-wide engine. Compiling it is expensive, so every extension
/// shares one; per-extension limits live on its Store.
fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("the wasmtime engine configuration is valid");
        let ticker = engine.clone();
        std::thread::Builder::new()
            .name("bsh-wasm-epoch".to_string())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(EPOCH_TICK_MS));
                ticker.increment_epoch();
            })
            .expect("the epoch thread starts");
        engine
    })
}

/// State passed to every host function.
struct HostState {
    extension: ExtensionId,
    trust: TrustLevel,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    storage: ExtensionStorage,
    workspace: Option<PathBuf>,
    publishes: Vec<String>,
    config: String,
    limits: StoreLimits,
    subscriptions: BTreeMap<u64, crate::extension::events::Subscription>,
    next_subscription: u64,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
}

/// Map a guest-reported host error back onto the platform error.
fn host_failure(
    extension: &ExtensionId,
    error: self::bsh::extension::types::HostError,
) -> ExtensionError {
    use self::bsh::extension::types::HostError;
    match error {
        HostError::Denied(reason) => ExtensionError::Denied {
            extension: extension.to_string(),
            capability: "host".to_string(),
            reason,
        },
        HostError::NotFound(id) => ExtensionError::NotFound { id },
        HostError::Limited(message) => ExtensionError::Limit {
            extension: extension.to_string(),
            message,
        },
        HostError::Timeout(message) => ExtensionError::Timeout {
            extension: extension.to_string(),
            operation: message,
            millis: 0,
        },
        HostError::Runtime(message) => ExtensionError::Runtime {
            extension: extension.to_string(),
            message,
        },
    }
}

impl HostState {
    fn host_error(error: ExtensionError) -> self::bsh::extension::types::HostError {
        use self::bsh::extension::types::HostError;
        match error {
            ExtensionError::Denied { reason, .. }
            | ExtensionError::MissingCapability { reason, .. } => HostError::Denied(reason),
            ExtensionError::NotFound { id } => HostError::NotFound(id),
            ExtensionError::Limit { message, .. } => HostError::Limited(message),
            ExtensionError::Timeout { .. } => HostError::Timeout("host operation timed out".into()),
            other => HostError::Runtime(other.to_string()),
        }
    }

    fn authorize(
        &self,
        capability: Capability,
        scope: CallScope,
    ) -> Result<(), self::bsh::extension::types::HostError> {
        self.policy
            .authorize_call(self.extension.as_str(), self.trust, capability, &scope)
            .map(|_| ())
            .map_err(Self::host_error)
    }

    fn workspace_path(&self, raw: &str) -> Result<PathBuf, self::bsh::extension::types::HostError> {
        let Some(workspace) = &self.workspace else {
            return Err(self::bsh::extension::types::HostError::Denied(
                "no workspace is open".to_string(),
            ));
        };
        let relative =
            validate_relative_path(raw).map_err(self::bsh::extension::types::HostError::Denied)?;
        let path = workspace.join(relative);
        let canonical_workspace = workspace
            .canonicalize()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        let parent = path.parent().unwrap_or(workspace);
        std::fs::create_dir_all(parent)
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        if !canonical_parent.starts_with(&canonical_workspace) {
            return Err(self::bsh::extension::types::HostError::Denied(
                "path escapes the workspace".to_string(),
            ));
        }
        Ok(path)
    }

    fn publish_allowed(&self, topic: &str) -> bool {
        if topic.starts_with("tool.") {
            return true;
        }
        self.publishes.iter().any(|pattern| {
            TopicPattern::parse(pattern, true)
                .map(|pattern| pattern.matches(topic))
                .unwrap_or(false)
        })
    }

    fn remaining(&self) -> bool {
        !self.cancelled.load(Ordering::Relaxed) && Instant::now() < self.deadline
    }
}

impl self::bsh::extension::types::Host for HostState {}

impl self::bsh::extension::host::Host for HostState {
    fn log(&mut self, level: self::bsh::extension::types::LogLevel, message: String) {
        let message: String = message.chars().take(4096).collect();
        match level {
            self::bsh::extension::types::LogLevel::Trace => {
                log::trace!(target: "self::bsh::extension", "{}: {message}", self.extension)
            }
            self::bsh::extension::types::LogLevel::Debug => {
                log::debug!(target: "self::bsh::extension", "{}: {message}", self.extension)
            }
            self::bsh::extension::types::LogLevel::Info => {
                log::info!(target: "self::bsh::extension", "{}: {message}", self.extension)
            }
            self::bsh::extension::types::LogLevel::Warn => {
                log::warn!(target: "self::bsh::extension", "{}: {message}", self.extension)
            }
            self::bsh::extension::types::LogLevel::Error => {
                log::error!(target: "self::bsh::extension", "{}: {message}", self.extension)
            }
        }
    }

    fn emit(
        &mut self,
        event: self::bsh::extension::types::Event,
    ) -> Result<(), self::bsh::extension::types::HostError> {
        if !self.publish_allowed(&event.event_type) {
            return Err(self::bsh::extension::types::HostError::Denied(format!(
                "topic {} is not declared in contributes.publishes",
                event.event_type
            )));
        }
        let payload: Value = serde_json::from_str(&event.payload).unwrap_or(Value::Null);
        let mut published = ExtensionEvent::new(self.extension.as_str(), event.event_type, payload)
            .map_err(Self::host_error)?;
        published.correlation_id = event.correlation_id;
        self.events.publish(&published).map_err(Self::host_error)?;
        Ok(())
    }

    fn subscribe(&mut self, topic: String) -> Result<u64, self::bsh::extension::types::HostError> {
        let filter = EventFilter::from_topics(&[topic])
            .map_err(self::bsh::extension::types::HostError::Denied)?;
        let id = self.next_subscription;
        self.next_subscription = self.next_subscription.saturating_add(1);
        let subscription = self.events.subscribe(filter, 64);
        self.subscriptions.insert(id, subscription);
        Ok(id)
    }

    fn unsubscribe(&mut self, subscription: u64) {
        self.subscriptions.remove(&subscription);
    }

    fn read_file(
        &mut self,
        path: String,
    ) -> Result<Vec<u8>, self::bsh::extension::types::HostError> {
        let path = self.workspace_path(&path)?;
        self.authorize(
            Capability::WorkspaceRead,
            CallScope {
                workspace: self.workspace.clone(),
                path: Some(path.clone()),
                ..CallScope::default()
            },
        )?;
        let metadata = std::fs::metadata(&path)
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        if metadata.len() > MAX_HOST_BODY as u64 {
            return Err(self::bsh::extension::types::HostError::Limited(format!(
                "{} is larger than {MAX_HOST_BODY} bytes",
                path.display()
            )));
        }
        std::fs::read(&path)
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))
    }

    fn write_file(
        &mut self,
        path: String,
        bytes: Vec<u8>,
    ) -> Result<(), self::bsh::extension::types::HostError> {
        if bytes.len() > MAX_HOST_BODY {
            return Err(self::bsh::extension::types::HostError::Limited(format!(
                "write is {} bytes; the ceiling is {MAX_HOST_BODY}",
                bytes.len()
            )));
        }
        let path = self.workspace_path(&path)?;
        self.authorize(
            Capability::WorkspaceWrite,
            CallScope {
                workspace: self.workspace.clone(),
                path: Some(path.clone()),
                ..CallScope::default()
            },
        )?;
        write_atomic(&path, &bytes).map_err(Self::host_error)
    }

    fn fetch(
        &mut self,
        url: String,
        method: String,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, self::bsh::extension::types::HostError> {
        let parsed = reqwest::Url::parse(&url)
            .map_err(|error| self::bsh::extension::types::HostError::Denied(error.to_string()))?;
        if parsed.scheme() != "https" && parsed.scheme() != "http" {
            return Err(self::bsh::extension::types::HostError::Denied(
                "only http and https are allowed".to_string(),
            ));
        }
        self.authorize(
            Capability::NetworkHttp,
            CallScope {
                domain: parsed.host_str().map(str::to_string),
                method: Some(method.to_ascii_uppercase()),
                ..CallScope::default()
            },
        )?;
        if body.as_ref().is_some_and(|body| body.len() > MAX_HOST_BODY) {
            return Err(self::bsh::extension::types::HostError::Limited(
                "request body is too large".to_string(),
            ));
        }
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|error| self::bsh::extension::types::HostError::Denied(error.to_string()))?;
        if !matches!(
            method,
            reqwest::Method::GET
                | reqwest::Method::HEAD
                | reqwest::Method::POST
                | reqwest::Method::PUT
                | reqwest::Method::DELETE
                | reqwest::Method::PATCH
        ) {
            return Err(self::bsh::extension::types::HostError::Denied(format!(
                "method {method} is not allowed"
            )));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(15_000))
            .build()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        let mut request = client.request(method, parsed);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request
            .send()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        let bytes = response
            .bytes()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        if bytes.len() > MAX_HOST_BODY {
            return Err(self::bsh::extension::types::HostError::Limited(
                "response is too large".to_string(),
            ));
        }
        Ok(bytes.to_vec())
    }

    fn storage_get(
        &mut self,
        key: String,
    ) -> Result<Option<Vec<u8>>, self::bsh::extension::types::HostError> {
        self.authorize(Capability::ExtensionStorage, CallScope::default())?;
        self.storage.read(&key).map_err(Self::host_error)
    }

    fn storage_set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), self::bsh::extension::types::HostError> {
        self.authorize(Capability::ExtensionStorage, CallScope::default())?;
        self.storage.write(&key, &value).map_err(Self::host_error)
    }

    fn storage_list(
        &mut self,
        prefix: String,
    ) -> Result<Vec<String>, self::bsh::extension::types::HostError> {
        self.authorize(Capability::ExtensionStorage, CallScope::default())?;
        self.storage
            .list()
            .map(|keys| {
                keys.into_iter()
                    .filter(|key| key.starts_with(&prefix))
                    .collect()
            })
            .map_err(Self::host_error)
    }

    fn read_secret(
        &mut self,
        name: String,
    ) -> Result<String, self::bsh::extension::types::HostError> {
        self.authorize(
            Capability::SecretsRead,
            CallScope {
                secret: Some(name.clone()),
                ..CallScope::default()
            },
        )?;
        let variable = format!("BSH_EXT_SECRET_{}", name.to_ascii_uppercase());
        std::env::var(&variable).map_err(|_| {
            self::bsh::extension::types::HostError::NotFound(format!(
                "secret {name} is not available"
            ))
        })
    }

    fn configuration(&mut self) -> String {
        self.config.clone()
    }

    fn now_millis(&mut self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or_default()
    }

    fn progress(&mut self, value: u32, total: u32) {
        if let Some(sender) = &self.progress {
            let _ = sender.send(serde_json::json!({
                "progress": value,
                "total": total,
            }));
        }
    }
}

struct Instance {
    store: Store<HostState>,
    bindings: BshExtension,
}

/// The Wasmtime runtime for one component.
pub(crate) struct WasmRuntime {
    extension: ExtensionId,
    component: PathBuf,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    storage: ExtensionStorage,
    workspace: Option<PathBuf>,
    publishes: Vec<String>,
    config: String,
    resources: ResourcesSpec,
    trust: TrustLevel,
    cancelled: Arc<AtomicBool>,
    instance: Mutex<Option<Instance>>,
}

impl WasmRuntime {
    /// Prepare a component runtime. The component is compiled on activation.
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
                message: "the wasm-component runtime needs runtime.entry".to_string(),
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
                    "wasm component file is missing",
                ),
            });
        }
        let storage = ExtensionStorage::open(data_root, manifest.id())?;
        Ok(Self {
            extension: manifest.id().clone(),
            component,
            policy,
            events,
            storage,
            workspace,
            publishes: manifest.contributes.publishes.clone(),
            config,
            resources: manifest.resources.clone(),
            trust,
            cancelled: Arc::new(AtomicBool::new(false)),
            instance: Mutex::new(None),
        })
    }

    fn ensure_instance(&self) -> Result<(), ExtensionError> {
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        if guard.is_some() {
            return Ok(());
        }
        let engine = engine();
        let component = Component::from_file(engine, &self.component).map_err(|error| {
            ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            }
        })?;
        let limits = StoreLimitsBuilder::new()
            .memory_size((self.resources.memory_mb() * 1024 * 1024) as usize)
            .build();
        let state = HostState {
            extension: self.extension.clone(),
            trust: self.trust,
            policy: Arc::clone(&self.policy),
            events: Arc::clone(&self.events),
            storage: self.storage.clone(),
            workspace: self.workspace.clone(),
            publishes: self.publishes.clone(),
            config: self.config.clone(),
            limits,
            subscriptions: BTreeMap::new(),
            next_subscription: 1,
            deadline: Instant::now() + Duration::from_millis(self.resources.execution_ms()),
            cancelled: Arc::clone(&self.cancelled),
            progress: None,
        };
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.resources.execution_ms().saturating_mul(10_000))
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        store.epoch_deadline_trap();
        let initial_ticks = self.resources.execution_ms().max(1).div_ceil(EPOCH_TICK_MS);
        store.set_epoch_deadline(initial_ticks);
        let mut linker = Linker::new(engine);
        BshExtension::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state).map_err(
            |error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            },
        )?;
        let bindings =
            BshExtension::instantiate(&mut store, &component, &linker).map_err(|error| {
                ExtensionError::Runtime {
                    extension: self.extension.to_string(),
                    message: error.to_string(),
                }
            })?;
        bindings
            .bsh_extension_extension()
            .call_initialize(&mut store, HOST_VERSION, &self.config)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?
            .map_err(|error| host_failure(&self.extension, error))?;
        *guard = Some(Instance { store, bindings });
        Ok(())
    }

    fn prepare_call(&self, instance: &mut Instance, timeout_ms: u64) -> Result<(), ExtensionError> {
        let ticks = timeout_ms.max(1).div_ceil(EPOCH_TICK_MS);
        instance.store.set_epoch_deadline(ticks);
        instance
            .store
            .set_fuel(timeout_ms.saturating_mul(10_000))
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        Ok(())
    }

    fn deliver_events(&self, instance: &mut Instance) -> Result<(), ExtensionError> {
        let events: Vec<ExtensionEvent> = {
            let state = instance.store.data_mut();
            let mut events = Vec::new();
            for subscription in state.subscriptions.values() {
                while let Some(event) = subscription.try_recv() {
                    events.push(event);
                }
            }
            events
        };
        for event in events {
            instance
                .bindings
                .bsh_extension_extension()
                .call_on_event(&mut instance.store, &to_wit_event(&event))
                .map_err(|error| ExtensionError::Runtime {
                    extension: self.extension.to_string(),
                    message: error.to_string(),
                })?;
        }
        Ok(())
    }
}

fn to_wit_event(event: &ExtensionEvent) -> self::bsh::extension::types::Event {
    self::bsh::extension::types::Event {
        id: event.id.clone(),
        source: event.source.clone(),
        event_type: event.event_type.clone(),
        schema_version: event.schema_version.clone(),
        timestamp: event.timestamp,
        correlation_id: event.correlation_id.clone(),
        payload: event.payload.to_string(),
    }
}

impl WasmRuntime {
    /// List the component's tools synchronously, instantiating it if needed.
    ///
    /// The agent build is synchronous and needs descriptors before it can price
    /// a request, so a wasm package is compiled at this point rather than on
    /// its first call. That is the one runtime where "lazy" costs a compile.
    pub(crate) fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let instance = guard.as_mut().expect("instance");
        self.prepare_call(instance, self.resources.execution_ms())?;
        let tools = instance
            .bindings
            .bsh_extension_extension()
            .call_tools(&mut instance.store)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        let mut descriptors = Vec::new();
        for tool in tools {
            let id = ContributionId::parse(tool.id.clone()).map_err(|error| {
                ExtensionError::Manifest {
                    path: self.component.clone(),
                    message: error.to_string(),
                }
            })?;
            let input_schema = serde_json::from_str(&tool.input_schema)
                .unwrap_or_else(|_| serde_json::json!({"type":"object","properties":{}}));
            descriptors.push(ToolDescriptor {
                id,
                description: tool.description,
                input_schema,
                capabilities: Vec::new(),
            });
        }
        Ok(descriptors)
    }
}

#[async_trait]
impl Runtime for WasmRuntime {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::WasmComponent
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let instance = guard.as_mut().expect("instance was just created");
        self.prepare_call(instance, self.resources.execution_ms())?;
        instance
            .bindings
            .bsh_extension_extension()
            .call_activate(&mut instance.store)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?
            .map_err(|error| host_failure(&self.extension, error))?;
        Ok(())
    }

    async fn deactivate(&self) -> Result<(), ExtensionError> {
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        if let Some(instance) = guard.as_mut() {
            self.prepare_call(instance, self.resources.execution_ms())?;
            instance
                .bindings
                .bsh_extension_extension()
                .call_deactivate(&mut instance.store)
                .map_err(|error| ExtensionError::Runtime {
                    extension: self.extension.to_string(),
                    message: error.to_string(),
                })?
                .map_err(|error| host_failure(&self.extension, error))?;
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExtensionError> {
        self.cancelled.store(true, Ordering::Relaxed);
        self.deactivate().await?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        if let Some(instance) = guard.as_mut() {
            let _ = instance
                .bindings
                .bsh_extension_extension()
                .call_shutdown(&mut instance.store);
        }
        *guard = None;
        Ok(())
    }

    async fn cancel(&self) -> Result<(), ExtensionError> {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.instance.lock() {
            if let Some(instance) = guard.as_mut() {
                instance.store.set_epoch_deadline(0);
                let _ = instance.store.set_fuel(0);
            }
        }
        Ok(())
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
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: "runtime was cancelled".to_string(),
            });
        }
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let instance = guard.as_mut().expect("instance");
        self.prepare_call(instance, context.timeout_ms)?;
        let arguments = serde_json::to_string(&arguments).unwrap_or_default();
        let result = instance
            .bindings
            .bsh_extension_extension()
            .call_call_tool(
                &mut instance.store,
                name,
                &arguments,
                &context.correlation_id,
            )
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        let result = result.map_err(|error| host_failure(&self.extension, error))?;
        self.deliver_events(instance)?;
        if result.is_error {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: result.content,
            });
        }
        serde_json::from_str(&result.content).map_err(|error| ExtensionError::Runtime {
            extension: self.extension.to_string(),
            message: format!("tool result is not JSON: {error}"),
        })
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.ensure_instance()?;
        if let Ok(mut guard) = self.instance.lock() {
            if let Some(instance) = guard.as_mut() {
                instance.store.data_mut().progress = progress;
            }
        }
        let result = self.call_tool(name, arguments, context).await;
        if let Ok(mut guard) = self.instance.lock() {
            if let Some(instance) = guard.as_mut() {
                instance.store.data_mut().progress = None;
            }
        }
        result.map(ToolResult::from_value)
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let instance = guard.as_mut().expect("instance");
        self.prepare_call(instance, self.resources.execution_ms())?;
        let text = instance
            .bindings
            .bsh_extension_extension()
            .call_command(&mut instance.store, id, &arguments.to_string())
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        let text = text.map_err(|error| host_failure(&self.extension, error))?;
        self.deliver_events(instance)?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let instance = guard.as_mut().expect("instance");
        let text = instance
            .bindings
            .bsh_extension_extension()
            .call_panel(&mut instance.store, id)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        let text = text.map_err(|error| host_failure(&self.extension, error))?;
        self.deliver_events(instance)?;
        serde_json::from_str(&text).map_err(|error| ExtensionError::Runtime {
            extension: self.extension.to_string(),
            message: format!("panel document is not JSON: {error}"),
        })
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        let Some(instance) = guard.as_mut() else {
            return Ok(());
        };
        self.deliver_events(instance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::PolicyMode;
    use crate::extension::types::TrustLevel as TestTrust;

    fn scratch(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("bsh-wasm-runtime-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    fn example_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/wasm-echo")
            .canonicalize()
            .expect("the wasm example exists")
    }

    #[tokio::test]
    async fn a_component_runs_tools_commands_and_panels() {
        let root = example_root();
        let manifest = Manifest::load(&root).expect("manifest");
        let scratch = scratch("echo");
        let policy = Arc::new(
            PolicyEngine::open(
                scratch.join("grants.toml"),
                scratch.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let runtime = WasmRuntime::new(
            &manifest,
            &root,
            &scratch.join("data"),
            policy,
            EventBus::new(),
            None,
            "{}".to_string(),
            TestTrust::Builtin,
        )
        .expect("runtime");
        let tools = runtime.descriptors().expect("descriptors");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id.as_str(), "echo");
        let context = CallContext::new(
            ExtensionId::parse("com.example.wasm-echo").expect("id"),
            "test-correlation",
            None,
            5000,
        );
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let echoed = runtime
            .call_tool_progress("echo", serde_json::json!({"x": 1}), &context, Some(sender))
            .await
            .expect("call")
            .to_value();
        assert_eq!(echoed["x"], 1);
        let progress = receiver.recv().await.expect("progress");
        assert_eq!(progress["progress"], 1);
        let command = runtime
            .command("test", serde_json::json!({}))
            .await
            .expect("command");
        assert_eq!(command, serde_json::json!("command"));
        let panel = runtime.panel("status").await.expect("panel");
        assert_eq!(panel["schema_version"], "1.0");
        runtime.shutdown().await.expect("shutdown");
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
