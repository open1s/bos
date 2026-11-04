//! The Wasmtime Component runtime.
//!
//! A component is compiled, validated and instantiated behind the same
//! Runtime contract as a process. The host side of the WIT world is implemented
//! here: every function authorizes against the package's capabilities, bounds
//! its input and output, and reports failures as normalized extension errors.
//!
//! Limits are enforced by the store: linear memory by StoreLimits, CPU by fuel,
//! wall clock by epoch interruption, and output bytes by the host wrapper.
//! Cancellation advances the engine epoch; each Store's deadline callback
//! decides whether that runtime should interrupt or continue.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::Value;
use wasmtime::component::{bindgen, Component, HasSelf, Linker};
use wasmtime::{
    Config, Engine, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline,
};

use crate::contract::{CallContext, Runtime, ToolDescriptor, ToolResult};
use ferris_core::capability::{CallScope, Capability, PolicyEngine};
use ferris_core::events::{EventBus, EventFilter, ExtensionEvent, TopicPattern};
use ferris_core::manifest::{validate_relative_path, Manifest, ResourcesSpec};
use ferris_core::package::write_atomic;
use ferris_core::storage::ExtensionStorage;
use ferris_core::types::{
    ContributionId, ExtensionError, ExtensionId, RuntimeKind, TrustLevel, HOST_VERSION,
};

bindgen!({
    path: "wit",
    world: "bsh-extension",
});

/// The largest file or network body a host function will return.
const MAX_HOST_BODY: usize = 8 * 1024 * 1024;

/// The largest number of event subscriptions one component may hold.
const MAX_SUBSCRIPTIONS: usize = 16;

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

/// Store limits plus the current linear-memory high-water mark.
struct TrackingLimiter {
    limits: StoreLimits,
    used: Arc<AtomicU64>,
}

impl ResourceLimiter for TrackingLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let allowed = self.limits.memory_growing(current, desired, maximum)?;
        let size = if allowed { desired } else { current };
        self.used.store(size as u64, Ordering::Relaxed);
        Ok(allowed)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.limits.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }

    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.limits.table_grow_failed(error)
    }

    fn instances(&self) -> usize {
        self.limits.instances()
    }

    fn tables(&self) -> usize {
        self.limits.tables()
    }

    fn memories(&self) -> usize {
        self.limits.memories()
    }
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
    subscribes: Vec<String>,
    config: String,
    limits: TrackingLimiter,
    /// subscription id -> (topic, queue), so a repeat is idempotent.
    subscriptions: BTreeMap<u64, (String, ferris_core::events::Subscription)>,
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

    /// Resolve a guest path and prove it stays inside the workspace.
    ///
    /// Nothing is created here: containment is checked against the deepest
    /// existing ancestor, so a denied write does not first create directories
    /// and a symlinked parent is caught before it is followed.
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
        let mut ancestor = path.parent().unwrap_or(workspace).to_path_buf();
        while !ancestor.exists() {
            match ancestor.parent() {
                Some(parent) if parent.starts_with(workspace) => ancestor = parent.to_path_buf(),
                _ => break,
            }
        }
        let canonical_ancestor = ancestor
            .canonicalize()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        if !canonical_ancestor.starts_with(&canonical_workspace) {
            return Err(self::bsh::extension::types::HostError::Denied(
                "path escapes the workspace".to_string(),
            ));
        }
        // The ancestor walk resolves parents; an existing leaf still has to be
        // checked itself, because fs::read/write follow a final symlink.
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "the final path component is a symlink".to_string(),
                ));
            }
            let canonical = path.canonicalize().map_err(|error| {
                self::bsh::extension::types::HostError::Runtime(error.to_string())
            })?;
            if !canonical.starts_with(&canonical_workspace) {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "path escapes the workspace".to_string(),
                ));
            }
        }
        Ok(path)
    }

    /// Refuse a host call once the runtime has been cancelled.
    fn check_alive(&self) -> Result<(), self::bsh::extension::types::HostError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(self::bsh::extension::types::HostError::Denied(
                "the call was cancelled".to_string(),
            ));
        }
        Ok(())
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
        self.check_alive()?;
        self.authorize(Capability::EventsPublish, CallScope::default())?;
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
        self.check_alive()?;
        // A subscription is a declared contribution, not an open tap: the
        // topic must appear in contributes.events and pass the same
        // events.subscribe decision the manifest requested.
        if !self.subscribes.iter().any(|pattern| {
            TopicPattern::parse(pattern, true)
                .map(|pattern| pattern.matches(&topic))
                .unwrap_or(false)
        }) {
            return Err(self::bsh::extension::types::HostError::Denied(format!(
                "topic {topic} is not declared in contributes.events"
            )));
        }
        self.authorize(Capability::EventsSubscribe, CallScope::default())?;
        if let Some((id, _)) = self
            .subscriptions
            .iter()
            .find(|(_, (existing, _))| existing == &topic)
        {
            return Ok(*id);
        }
        if self.subscriptions.len() >= MAX_SUBSCRIPTIONS {
            return Err(self::bsh::extension::types::HostError::Limited(format!(
                "the extension already holds {MAX_SUBSCRIPTIONS} event subscriptions"
            )));
        }
        let filter = EventFilter::from_topics(std::slice::from_ref(&topic))
            .map_err(self::bsh::extension::types::HostError::Denied)?;
        let id = self.next_subscription;
        self.next_subscription = self.next_subscription.saturating_add(1);
        let subscription = self
            .events
            .subscribe(filter, 64)
            .map_err(Self::host_error)?;
        self.subscriptions.insert(id, (topic, subscription));
        Ok(id)
    }

    fn unsubscribe(&mut self, subscription: u64) {
        self.subscriptions.remove(&subscription);
    }

    fn read_file(
        &mut self,
        path: String,
    ) -> Result<Vec<u8>, self::bsh::extension::types::HostError> {
        self.check_alive()?;
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
        self.check_alive()?;
        let path = self.workspace_path(&path)?;
        self.authorize(
            Capability::WorkspaceWrite,
            CallScope {
                workspace: self.workspace.clone(),
                path: Some(path.clone()),
                ..CallScope::default()
            },
        )?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                self::bsh::extension::types::HostError::Runtime(error.to_string())
            })?;
        }
        write_atomic(&path, &bytes).map_err(Self::host_error)
    }

    fn fetch(
        &mut self,
        url: String,
        method: String,
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, self::bsh::extension::types::HostError> {
        self.check_alive()?;
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
        // A blocking host fetch cannot be interrupted by the guest's cancel
        // flag, so it must not outlive the call deadline: derive the client
        // timeout from the remaining per-call budget.
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(self::bsh::extension::types::HostError::Timeout(
                "the call deadline elapsed before the fetch".to_string(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(remaining)
            // Every redirect is followed by hand so each hop is authorized
            // against its own host. reqwest's default policy would follow up
            // to ten hops on the strength of the first authorization.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| self::bsh::extension::types::HostError::Runtime(error.to_string()))?;
        let requested_bytes = body.as_ref().map(|body| body.len() as u64).unwrap_or(0);
        let method_text = method.as_str().to_string();
        let started = Instant::now();
        let mut current = parsed;
        let mut current_method = method;
        let mut current_body = body;
        let mut redirects = 0usize;
        let response = loop {
            let mut request = client.request(current_method.clone(), current.clone());
            if let Some(body) = &current_body {
                request = request.body(body.clone());
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    let message = error.to_string();
                    crate::hooks::observe(
                        "http.exchange",
                        crate::hooks::http_exchange(
                            self.extension.as_str(),
                            &method_text,
                            current.as_str(),
                            None,
                            requested_bytes,
                            0,
                            None,
                            0,
                            started.elapsed().as_millis() as u64,
                            Some(&message),
                        ),
                    );
                    return Err(self::bsh::extension::types::HostError::Runtime(message));
                }
            };
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "the call was cancelled".to_string(),
                ));
            }
            if !response.status().is_redirection() {
                break response;
            }
            redirects += 1;
            if redirects > 5 {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "too many redirects".to_string(),
                ));
            }
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
            else {
                // A redirect without a location is returned to the guest
                // rather than guessed at.
                break response;
            };
            let next = current.join(&location).map_err(|error| {
                self::bsh::extension::types::HostError::Denied(error.to_string())
            })?;
            if !matches!(next.scheme(), "http" | "https") {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "redirect target is not http or https".to_string(),
                ));
            }
            if current.scheme() == "https" && next.scheme() == "http" {
                return Err(self::bsh::extension::types::HostError::Denied(
                    "redirect downgrades https to http".to_string(),
                ));
            }
            let next_method = if matches!(response.status().as_u16(), 307 | 308) {
                current_method.clone()
            } else {
                reqwest::Method::GET
            };
            self.authorize(
                Capability::NetworkHttp,
                CallScope {
                    domain: next.host_str().map(str::to_string),
                    method: Some(next_method.as_str().to_ascii_uppercase()),
                    ..CallScope::default()
                },
            )?;
            if next_method == reqwest::Method::GET {
                current_body = None;
            }
            current_method = next_method;
            current = next;
        };
        let ttfb_ms = started.elapsed().as_millis() as u64;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        // Read through a hard take: response.bytes() buffers the whole body
        // before the ceiling is checked, so an unbounded response could be
        // allocated in full before it was refused.
        use std::io::Read as _;
        let mut bytes = Vec::new();
        let mut limited = response.take((MAX_HOST_BODY as u64) + 1);
        if let Err(error) = limited.read_to_end(&mut bytes) {
            let message = error.to_string();
            crate::hooks::observe(
                "http.exchange",
                crate::hooks::http_exchange(
                    self.extension.as_str(),
                    &method_text,
                    &url,
                    Some(status),
                    requested_bytes,
                    0,
                    content_type.as_deref(),
                    ttfb_ms,
                    started.elapsed().as_millis() as u64,
                    Some(&message),
                ),
            );
            return Err(self::bsh::extension::types::HostError::Runtime(message));
        }
        crate::hooks::observe(
            "http.exchange",
            crate::hooks::http_exchange(
                self.extension.as_str(),
                &method_text,
                &url,
                Some(status),
                requested_bytes,
                bytes.len() as u64,
                content_type.as_deref(),
                ttfb_ms,
                started.elapsed().as_millis() as u64,
                None,
            ),
        );
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
        self.check_alive()?;
        self.authorize(Capability::ExtensionStorage, CallScope::default())?;
        self.storage.read(&key).map_err(Self::host_error)
    }

    fn storage_set(
        &mut self,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), self::bsh::extension::types::HostError> {
        self.check_alive()?;
        self.authorize(Capability::ExtensionStorage, CallScope::default())?;
        self.storage.write(&key, &value).map_err(Self::host_error)
    }

    fn storage_list(
        &mut self,
        prefix: String,
    ) -> Result<Vec<String>, self::bsh::extension::types::HostError> {
        self.check_alive()?;
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
        self.check_alive()?;
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
        if self.cancelled.load(Ordering::Relaxed) {
            return;
        }
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
pub struct WasmRuntime {
    extension: ExtensionId,
    component: PathBuf,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    storage: ExtensionStorage,
    workspace: Option<PathBuf>,
    publishes: Vec<String>,
    subscribes: Vec<String>,
    config: String,
    resources: ResourcesSpec,
    trust: TrustLevel,
    cancelled: Arc<AtomicBool>,
    /// Set by shutdown only. Cancellation is a per-call state that the next
    /// call clears, but a shut-down runtime must never be revived by one.
    stopped: AtomicBool,
    instance: Mutex<Option<Instance>>,
    fuel_budget: AtomicU64,
    fuel_used: AtomicU64,
    memory_used: Arc<AtomicU64>,
}

impl WasmRuntime {
    /// Prepare a component runtime. The component is compiled on activation.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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
            subscribes: manifest.contributes.events.clone(),
            config,
            resources: manifest.resources.clone(),
            trust,
            cancelled: Arc::new(AtomicBool::new(false)),
            stopped: AtomicBool::new(false),
            fuel_budget: AtomicU64::new(0),
            fuel_used: AtomicU64::new(0),
            memory_used: Arc::new(AtomicU64::new(0)),
            instance: Mutex::new(None),
        })
    }

    fn ensure_running(&self) -> Result<(), ExtensionError> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: "runtime is shut down".to_string(),
            });
        }
        Ok(())
    }

    fn ensure_instance(&self) -> Result<(), ExtensionError> {
        self.ensure_running()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        // Shutdown can race with a caller waiting for the instance mutex.
        self.ensure_running()?;
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
            .table_elements(self.resources.table_elements() as usize)
            .build();
        let state = HostState {
            extension: self.extension.clone(),
            trust: self.trust,
            policy: Arc::clone(&self.policy),
            events: Arc::clone(&self.events),
            storage: self.storage.clone(),
            workspace: self.workspace.clone(),
            publishes: self.publishes.clone(),
            subscribes: self.subscribes.clone(),
            config: self.config.clone(),
            limits: TrackingLimiter {
                limits,
                used: Arc::clone(&self.memory_used),
            },
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
        // Check cancellation and the wall-clock budget in a Store-local callback.
        // The callback is evaluated on the guest thread, so cancellation never
        // needs to acquire the Store mutex held by a running component.
        store.epoch_deadline_callback(|state| {
            if state.data().cancelled.load(Ordering::SeqCst)
                || Instant::now() >= state.data().deadline
            {
                Ok(UpdateDeadline::Interrupt)
            } else {
                Ok(UpdateDeadline::Continue(1))
            }
        });
        store.set_epoch_deadline(1);
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
        // Every serialized guest entry point starts a fresh cancellation scope.
        self.cancelled.store(false, Ordering::SeqCst);
        instance.store.data_mut().deadline =
            Instant::now() + Duration::from_millis(timeout_ms.max(1));
        instance.store.set_epoch_deadline(1);
        let fuel = timeout_ms.saturating_mul(10_000);
        self.fuel_budget.store(fuel, Ordering::Relaxed);
        self.fuel_used.store(0, Ordering::Relaxed);
        instance
            .store
            .set_fuel(fuel)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        Ok(())
    }

    fn record_fuel(&self, store: &Store<HostState>) {
        let budget = self.fuel_budget.load(Ordering::Relaxed);
        let remaining = store.get_fuel().unwrap_or(0);
        self.fuel_used
            .store(budget.saturating_sub(remaining), Ordering::Relaxed);
    }

    fn deliver_events(&self, instance: &mut Instance) -> Result<(), ExtensionError> {
        // Deliver one queued event at a time. Draining the whole queue first
        // lost every remaining event when one callback failed; now the failed
        // event is dropped and the rest stay queued for the next pump.
        loop {
            let event = {
                let state = instance.store.data_mut();
                let mut next = None;
                for (_, subscription) in state.subscriptions.values() {
                    if let Some(event) = subscription.try_recv() {
                        next = Some(event);
                        break;
                    }
                }
                next
            };
            let Some(event) = event else {
                break;
            };
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
    /// Enforce the manifest output ceiling on a guest string reply.
    fn bounded_output(&self, text: &str) -> Result<(), ExtensionError> {
        let bytes = text.len() as u64;
        if bytes > self.resources.output_bytes() {
            return Err(ExtensionError::Limit {
                extension: self.extension.to_string(),
                message: format!(
                    "component reply is {bytes} bytes; the ceiling is {}",
                    self.resources.output_bytes()
                ),
            });
        }
        Ok(())
    }

    /// Invoke one guest tool under the instance lock.
    ///
    /// The progress sender is installed and cleared inside the same lock
    /// scope as the guest call; assigning it in a separate lock scope (the old
    /// call_tool_progress) let a second caller overwrite the sender before the
    /// first call reached the guest.
    fn run_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<Value, ExtensionError> {
        if self.stopped.load(Ordering::Relaxed) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: "runtime is shut down".to_string(),
            });
        }
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        // A queued call must not revive a runtime shut down while it waited.
        if self.stopped.load(Ordering::SeqCst) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: "runtime is shut down".to_string(),
            });
        }
        let instance = guard.as_mut().expect("instance");
        self.prepare_call(
            instance,
            context.timeout_ms.min(self.resources.execution_ms()),
        )?;
        instance.store.data_mut().progress = progress;
        let arguments = serde_json::to_string(&arguments).unwrap_or_default();
        let result = instance.bindings.bsh_extension_extension().call_call_tool(
            &mut instance.store,
            name,
            &arguments,
            &context.correlation_id,
        );
        // Clear even when the guest trapped, so the next call cannot inherit
        // a sender that belongs to this invocation.
        instance.store.data_mut().progress = None;
        self.record_fuel(&instance.store);
        let result = result.map_err(|error| ExtensionError::Runtime {
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
        if result.content.len() as u64 > self.resources.output_bytes() {
            return Err(ExtensionError::Limit {
                extension: self.extension.to_string(),
                message: format!(
                    "component result is {} bytes; the ceiling is {}",
                    result.content.len(),
                    self.resources.output_bytes()
                ),
            });
        }
        serde_json::from_str(&result.content).map_err(|error| ExtensionError::Runtime {
            extension: self.extension.to_string(),
            message: format!("tool result is not JSON: {error}"),
        })
    }

    /// List the component's tools synchronously, instantiating it if needed.
    ///
    /// The agent build is synchronous and needs descriptors before it can price
    /// a request, so a wasm package is compiled at this point rather than on
    /// its first call. That is the one runtime where "lazy" costs a compile.
    pub fn descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        self.ensure_running()?;
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

    fn fuel_used(&self) -> Option<u64> {
        Some(self.fuel_used.load(Ordering::Relaxed))
    }

    fn memory_limit(&self) -> Option<u64> {
        Some(self.resources.memory_mb().saturating_mul(1024 * 1024))
    }

    fn memory_used(&self) -> Option<u64> {
        Some(self.memory_used.load(Ordering::Relaxed))
    }

    async fn activate(&self) -> Result<(), ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        self.ensure_running()?;
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
        self.stopped.store(true, Ordering::SeqCst);
        self.cancelled.store(true, Ordering::SeqCst);
        // Interrupt a currently running guest before waiting for its Store lock.
        engine().increment_epoch();
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
        // Epochs are engine-global, but the callback above checks this Store's
        // own cancellation flag. Other Stores that observe the same tick keep
        // running by extending their own deadlines.
        engine().increment_epoch();
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
        self.run_tool(name, arguments, context, None)
    }

    async fn call_tool_progress(
        &self,
        name: &str,
        arguments: Value,
        context: &CallContext,
        progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>,
    ) -> Result<ToolResult, ExtensionError> {
        self.run_tool(name, arguments, context, progress)
            .map(ToolResult::from_value)
    }

    async fn command(&self, id: &str, arguments: Value) -> Result<Value, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        self.ensure_running()?;
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
        self.record_fuel(&instance.store);
        let text = text.map_err(|error| host_failure(&self.extension, error))?;
        self.bounded_output(&text)?;
        self.deliver_events(instance)?;
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    async fn panel(&self, id: &str) -> Result<Value, ExtensionError> {
        self.ensure_instance()?;
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        self.ensure_running()?;
        let instance = guard.as_mut().expect("instance");
        self.prepare_call(instance, self.resources.execution_ms())?;
        let text = instance
            .bindings
            .bsh_extension_extension()
            .call_panel(&mut instance.store, id)
            .map_err(|error| ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: error.to_string(),
            })?;
        self.record_fuel(&instance.store);
        let text = text.map_err(|error| host_failure(&self.extension, error))?;
        self.bounded_output(&text)?;
        self.deliver_events(instance)?;
        serde_json::from_str(&text).map_err(|error| ExtensionError::Runtime {
            extension: self.extension.to_string(),
            message: format!("panel document is not JSON: {error}"),
        })
    }

    async fn pump(&self) -> Result<(), ExtensionError> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(ExtensionError::Runtime {
                extension: self.extension.to_string(),
                message: "runtime is shut down".to_string(),
            });
        }
        let mut guard = self.instance.lock().map_err(|_| ExtensionError::Corrupt {
            path: self.component.clone(),
            message: "wasm instance lock is poisoned".to_string(),
        })?;
        self.ensure_running()?;
        let Some(instance) = guard.as_mut() else {
            return Ok(());
        };
        // A pump is a call too: its callback must get a fresh deadline and
        // fuel, not the previous call's exhausted budget or cancelled flag.
        self.prepare_call(instance, self.resources.execution_ms())?;
        self.deliver_events(instance)?;
        self.record_fuel(&instance.store);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::bsh::extension::host::Host as _;
    use super::*;
    use ferris_core::capability::{GrantScope, PolicyMode};
    use ferris_core::types::TrustLevel as TestTrust;

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
        let manifest = ferris_core::manifest_v2::load_package_manifest(&root).expect("manifest");
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
        assert!(runtime.fuel_used().unwrap_or(0) > 0);
        assert!(runtime.memory_used().unwrap_or(0) > 0);
        runtime.shutdown().await.expect("shutdown");
        assert!(runtime.tools().await.is_err(), "shutdown must be terminal");
        assert!(runtime.panel("status").await.is_err());
        assert!(runtime
            .command("test", serde_json::json!({}))
            .await
            .is_err());
        assert!(runtime.pump().await.is_err());
        assert!(
            runtime.instance.lock().expect("instance lock").is_none(),
            "post-shutdown entry points must not recreate the component"
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    fn host_state(workspace: &Path, grants: &[(Capability, GrantScope)]) -> HostState {
        let id = ExtensionId::parse("com.example.wasm-host").expect("id");
        let policy = PolicyEngine::open(
            workspace.join("grants.toml"),
            workspace.join("audit.jsonl"),
            PolicyMode::Enforce,
        )
        .expect("policy");
        for (capability, scope) in grants {
            policy
                .grant(id.as_str(), *capability, scope.clone(), None)
                .expect("grant");
        }
        HostState {
            extension: id.clone(),
            trust: TestTrust::Local,
            policy: Arc::new(policy),
            events: EventBus::new(),
            storage: ExtensionStorage::open(&workspace.join("data"), &id).expect("storage"),
            workspace: Some(workspace.to_path_buf()),
            publishes: Vec::new(),
            subscribes: Vec::new(),
            config: "{}".to_string(),
            limits: TrackingLimiter {
                limits: StoreLimitsBuilder::new().build(),
                used: Arc::new(AtomicU64::new(0)),
            },
            subscriptions: BTreeMap::new(),
            next_subscription: 1,
            deadline: Instant::now() + Duration::from_secs(5),
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_final_symlink_cannot_escape_the_workspace() {
        let workspace = scratch("symlink");
        let outside = workspace.with_extension("outside");
        std::fs::write(&outside, "secret").expect("outside file");
        std::os::unix::fs::symlink(&outside, workspace.join("link")).expect("symlink");
        let state = host_state(&workspace, &[(Capability::WorkspaceRead, GrantScope::Any)]);
        let error = state
            .workspace_path("link")
            .expect_err("a final symlink is refused");
        match error {
            self::bsh::extension::types::HostError::Denied(reason) => {
                assert!(reason.contains("symlink"), "{reason}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn a_redirect_to_an_unauthorized_host_is_refused() {
        let workspace = scratch("redirect");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = "HTTP/1.1 302 Found\r\nLocation: http://example.invalid/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
            }
        });
        let mut state = host_state(
            &workspace,
            &[(
                Capability::NetworkHttp,
                GrantScope::Domain("127.0.0.1".to_string()),
            )],
        );
        let error = state
            .fetch(format!("http://{address}/start"), "GET".to_string(), None)
            .expect_err("the redirect target is not granted");
        match error {
            self::bsh::extension::types::HostError::Denied(reason) => {
                assert!(reason.contains("grant"), "{reason}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn a_stalled_fetch_is_bounded_by_the_call_deadline() {
        let workspace = scratch("stalled-fetch");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            use std::io::Read;
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        let mut state = host_state(
            &workspace,
            &[(
                Capability::NetworkHttp,
                GrantScope::Domain("127.0.0.1".to_string()),
            )],
        );
        state.deadline = Instant::now() + Duration::from_millis(250);
        let started = Instant::now();
        state
            .fetch(format!("http://{address}/stall"), "GET".to_string(), None)
            .expect_err("a stalled fetch must fail at the call deadline");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the fetch outlived the call deadline: {:?}",
            started.elapsed()
        );
    }
}
