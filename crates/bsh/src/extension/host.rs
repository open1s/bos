//! The extension host: discovery, resolution, authorization, lifecycle and the
//! one seam the rest of BSH uses.
//!
//! The host is opened once per configuration and cached in a process global.
//! A build calls install(); the host refreshes only when the store or the state
//! files changed, so a render pass never re-reads every manifest. All extension
//! execution is behind LazyRuntime: the build stays synchronous and a process
//! or wasm runtime starts its work on the first call that needs it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;

use serde::Serialize;

use super::agent as contribution;
use super::capability::{CallScope, Decision, PolicyEngine, PolicyMode};
use super::events::{lifecycle_event, topics, EventBus, ExtensionEvent};
use super::lifecycle::{Health, LifecycleManager};
use super::package::{write_atomic, ExtensionStore, Package};
use super::resolver;
use super::runtime::builtin::{BuiltinRuntime, ClosureExtension};
use super::runtime::declarative::{self, DeclarativeContributions};
use super::runtime::extism::ExtismRuntime;
use super::runtime::mcp::McpRuntime;
use super::runtime::process::ProcessRuntime;
use super::runtime::wasm::WasmRuntime;
use super::runtime::workflow::WorkflowRuntime;
use super::runtime::{empty_schema, LazyRuntime, Runtime, ToolDescriptor};
use super::types::{
    ContributionId, ExtensionError, ExtensionId, ExtensionSource, LifecycleState, RuntimeKind,
    StartupPolicy, TrustLevel, ValidationReport, EXTENSION_API_VERSION, HOST_VERSION,
};
use super::ui::{PanelLocation, PanelVisibility, ThemePalette, UiDocument};

/// The configuration that decides which store and which packages a host owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostConfig {
    /// The managed extension root.
    pub(crate) root: PathBuf,
    /// The workspace open for this build, used for scoped grants.
    pub(crate) workspace: Option<PathBuf>,
    /// Extension ids the workspace disabled.
    pub(crate) disabled: BTreeSet<String>,
    /// The preset's explicit allowlist, if it has one.
    pub(crate) preset_allowed: Option<BTreeSet<String>>,
}

impl HostConfig {
    /// Build the configuration for one agent build.
    pub(crate) fn from_settings(
        settings: &crate::settings::Settings,
        workspace: Option<PathBuf>,
        preset_allowed: Option<&[String]>,
    ) -> Self {
        let root = extension_root(settings);
        Self {
            root,
            workspace,
            disabled: settings
                .workspace
                .disabled
                .extensions
                .iter()
                .cloned()
                .collect(),
            preset_allowed: preset_allowed.map(|ids| ids.iter().cloned().collect()),
        }
    }

    /// Whether a package is enabled by configuration alone.
    pub(crate) fn allows(&self, id: &ExtensionId) -> bool {
        if self.disabled.contains(id.as_str()) {
            return false;
        }
        self.preset_allowed
            .as_ref()
            .is_none_or(|allowed| allowed.contains(id.as_str()))
    }
}

/// The managed extension root for one settings tree.
///
/// A test build must never read or write the reader's real extension store:
/// beside the settings, the legacy market and a future package manager also use
/// that path. A test's save_root is its scratch store, so the platform root
/// follows it. A test that has no save_root gets one process-wide scratch root
/// instead of $HOME.
fn extension_root(settings: &crate::settings::Settings) -> PathBuf {
    if let Some(save_root) = &settings.save_root {
        return save_root.join("extensions");
    }
    if let Some(raw) = std::env::var_os("BSH_EXTENSION_ROOT") {
        return PathBuf::from(shellexpand::tilde(&raw.to_string_lossy()).into_owned());
    }
    #[cfg(test)]
    {
        static TEST_ROOT: OnceLock<PathBuf> = OnceLock::new();
        TEST_ROOT
            .get_or_init(|| {
                std::env::temp_dir().join(format!("bsh-ext-test-{}", std::process::id()))
            })
            .clone()
    }
    #[cfg(not(test))]
    {
        crate::settings::dir().join("extensions")
    }
}

/// One capability's decision for a status row.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CapabilityStatus {
    /// Capability id.
    pub(crate) capability: String,
    /// Whether policy allows it.
    pub(crate) allowed: bool,
    /// The scope it was allowed under.
    pub(crate) scope: Option<String>,
    /// Why it was denied.
    pub(crate) reason: Option<String>,
    /// The risk bucket.
    pub(crate) risk: String,
}

/// One extension as a status row.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExtensionStatus {
    /// Extension id.
    pub(crate) id: String,
    /// Display name.
    pub(crate) name: String,
    /// Version.
    pub(crate) version: String,
    /// Runtime kind.
    pub(crate) runtime: String,
    /// Trust level.
    pub(crate) trust: String,
    /// Source label.
    pub(crate) source: String,
    /// Package root.
    pub(crate) root: String,
    /// Package hash.
    pub(crate) hash: String,
    /// Lifecycle state.
    pub(crate) state: LifecycleState,
    /// Health.
    pub(crate) health: Health,
    /// Whether configuration enabled it.
    pub(crate) configured: bool,
    /// Whether it is loaded and its contributions are applied.
    pub(crate) applied: bool,
    /// Qualified tool names.
    pub(crate) tools: Vec<String>,
    /// Qualified command names.
    pub(crate) commands: Vec<String>,
    /// Qualified panel names.
    pub(crate) panels: Vec<String>,
    /// Capability decisions.
    pub(crate) capabilities: Vec<CapabilityStatus>,
    /// The last failure.
    pub(crate) error: Option<String>,
    /// Activation duration.
    pub(crate) startup_ms: u64,
    /// Unix seconds of installation.
    pub(crate) installed_at: u64,
    /// Declared dependency ids.
    pub(crate) dependencies: Vec<String>,
    /// Signature verdict: unsigned, verified, untrusted or invalid.
    pub(crate) signature: String,
    /// Publisher name from the signature, when one was present.
    pub(crate) publisher: Option<String>,
}

/// One command the composer palette can insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandSurface {
    /// Qualified command name.
    pub(crate) name: String,
    /// One-line description.
    pub(crate) description: String,
    /// Prompt body inserted when chosen.
    pub(crate) template: String,
}

/// One panel the host can mount in the UI.
#[derive(Clone)]
pub(crate) struct PanelSurface {
    /// Qualified panel id for the host surface.
    pub(crate) panel_id: String,
    /// Contribution id the runtime answers for.
    pub(crate) contribution_id: String,
    /// Panel title.
    pub(crate) title: String,
    /// Optional package-relative icon.
    pub(crate) icon: Option<String>,
    /// Where the host should mount it.
    pub(crate) location: PanelLocation,
    /// Whether it starts visible.
    pub(crate) visibility: PanelVisibility,
    /// The runtime that serves it.
    pub(crate) runtime: Arc<dyn Runtime>,
    /// The owning extension, for validation and action ownership.
    pub(crate) owner: ExtensionId,
    /// The first theme the extension declares, if any.
    pub(crate) theme: Option<ThemePalette>,
    /// A declarative view that is ready without a call, when one exists.
    pub(crate) initial: Option<UiDocument>,
}

/// Aggregate tool-call counters.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct CallMetrics {
    /// Calls started.
    pub(crate) tool_calls: u64,
    /// Calls that returned an error.
    pub(crate) tool_failures: u64,
    /// Summed wall-clock milliseconds.
    pub(crate) total_call_ms: u64,
    /// Slowest call in milliseconds.
    pub(crate) max_call_ms: u64,
    /// Summed serialized result bytes.
    pub(crate) output_bytes: u64,
}

/// The host's observability snapshot.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HostMetrics {
    /// Registered extensions.
    pub(crate) extensions: usize,
    /// Applied extensions.
    pub(crate) applied: usize,
    /// Tool-call counters.
    pub(crate) calls: CallMetrics,
    /// Events accepted for publication.
    pub(crate) events_published: u64,
    /// Event deliveries accepted.
    pub(crate) events_delivered: u64,
    /// Event deliveries dropped on a full queue.
    pub(crate) events_dropped: u64,
    /// Live event subscribers.
    pub(crate) subscribers: usize,
    /// Extensions that failed to load.
    pub(crate) failed: usize,
    /// Runtimes currently carrying work (applied extensions).
    pub(crate) active_tasks: usize,
    /// Host open-to-ready duration in milliseconds.
    pub(crate) startup_millis: u64,
    /// Fuel consumed by the latest WASM call across runtimes.
    pub(crate) fuel_used: u64,
    /// Current load failures.
    pub(crate) failures: Vec<String>,
    /// The last recorded load failures, retained across host opens.
    pub(crate) crash_history: Vec<String>,
}

/// One diagnostic line.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticIssue {
    /// error, warning or info.
    pub(crate) severity: String,
    /// The extension the issue belongs to, when one does.
    pub(crate) extension: Option<String>,
    /// What is wrong.
    pub(crate) message: String,
}

impl DiagnosticIssue {
    fn new(
        severity: impl Into<String>,
        extension: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: severity.into(),
            extension: extension.map(str::to_string),
            message: message.into(),
        }
    }
}

/// The doctor's full report.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiagnosticsReport {
    /// Host version.
    pub(crate) host_version: String,
    /// Extension API generation.
    pub(crate) extension_api: String,
    /// The managed root.
    pub(crate) root: String,
    /// Every known extension.
    pub(crate) extensions: Vec<ExtensionStatus>,
    /// Problems and warnings.
    pub(crate) issues: Vec<DiagnosticIssue>,
}

struct BuiltinLoaded {
    package: Package,
    runtime: Arc<dyn Runtime>,
    tools: Vec<ToolDescriptor>,
}

/// A runtime plus the descriptors and file contributions loaded for it.
type BuiltRuntime = (
    Arc<dyn Runtime>,
    Vec<ToolDescriptor>,
    DeclarativeContributions,
);

struct LoadedExtension {
    package: Package,
    runtime: Option<Arc<dyn Runtime>>,
    tools: Vec<ToolDescriptor>,
    contributions: DeclarativeContributions,
    configured: bool,
    applied: bool,
    error: Option<String>,
}

/// The extension host.
pub(crate) struct ExtensionHost {
    config: HostConfig,
    store: ExtensionStore,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    manager: LifecycleManager,
    loaded: BTreeMap<String, LoadedExtension>,
    builtins: Vec<BuiltinLoaded>,
    issues: Vec<String>,
    status_cache: Arc<Mutex<Vec<ExtensionStatus>>>,
    last_stamp: Option<SystemTime>,
    pump_generation: Arc<AtomicU64>,
    calls: Mutex<CallMetrics>,
    startup_millis: u64,
    crash_history: Vec<String>,
}

impl ExtensionHost {
    /// Open a host: create the store, load policy, register built-ins and
    /// perform the first scan.
    pub(crate) fn open(config: HostConfig) -> Result<Self, ExtensionError> {
        let started = std::time::Instant::now();
        let store = ExtensionStore::open(&config.root)?;
        let policy = Arc::new(PolicyEngine::open(
            store.grants_path(),
            store.audit_path(),
            PolicyMode::Enforce,
        )?);
        let events = EventBus::new();
        let status_cache = Arc::new(Mutex::new(Vec::new()));
        let mut host = Self {
            config,
            store,
            policy,
            events,
            manager: LifecycleManager::new(),
            loaded: BTreeMap::new(),
            builtins: Vec::new(),
            issues: Vec::new(),
            status_cache,
            last_stamp: None,
            pump_generation: Arc::new(AtomicU64::new(0)),
            calls: Mutex::new(CallMetrics::default()),
            startup_millis: 0,
            crash_history: Vec::new(),
        };
        host.register_builtins();
        host.rebuild()?;
        host.crash_history = host.store.crash_history(16);
        host.startup_millis = started.elapsed().as_millis() as u64;
        Ok(host)
    }

    /// The managed root.
    pub(crate) fn root(&self) -> &Path {
        self.store.root()
    }

    /// The policy engine, for the CLI's grant commands.
    pub(crate) fn policy(&self) -> &Arc<PolicyEngine> {
        &self.policy
    }

    /// The event bus.
    pub(crate) fn events(&self) -> &Arc<EventBus> {
        &self.events
    }

    /// The store, for the CLI.
    pub(crate) fn store(&self) -> &ExtensionStore {
        &self.store
    }

    /// The lifecycle manager, for the CLI.
    pub(crate) fn manager(&self) -> &LifecycleManager {
        &self.manager
    }

    fn register_builtins(&mut self) {
        let cache = Arc::clone(&self.status_cache);
        let extension = ClosureExtension::new(
            INSPECTOR_MANIFEST,
            vec![ToolDescriptor {
                id: ContributionId::parse("list").expect("built-in tool id"),
                description:
                    "List the BSH extensions installed on this machine, with their lifecycle state, capabilities and tool names."
                        .to_string(),
                input_schema: empty_schema(),
                capabilities: Vec::new(),
            }],
            move |_name, _arguments| {
                let extensions = cache
                    .lock()
                    .map(|statuses| serde_json::to_value(&*statuses).unwrap_or_default())
                    .unwrap_or(serde_json::Value::Null);
                Ok(serde_json::json!({
                    "host": HOST_VERSION,
                    "extensionApi": EXTENSION_API_VERSION,
                    "extensions": extensions,
                }))
            },
        );
        match BuiltinRuntime::new(Arc::new(extension)) {
            Ok(runtime) => {
                let package = Package {
                    root: PathBuf::from("<builtin>"),
                    manifest: runtime.manifest().clone(),
                    report: ValidationReport::default(),
                    source: ExtensionSource::Builtin,
                    trust: TrustLevel::Builtin,
                    hash: String::new(),
                    installed_at: 0,
                    signature: None,
                    verification: crate::extension::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                });
            }
            Err(error) => log::warn!("built-in extension inspector was not registered: {error}"),
        }
    }

    fn rebuild(&mut self) -> Result<(), ExtensionError> {
        let mut manager = LifecycleManager::new();
        let mut loaded: BTreeMap<String, LoadedExtension> = BTreeMap::new();
        let mut issues: Vec<String> = Vec::new();
        for builtin in &self.builtins {
            let id = builtin.package.id().clone();
            let configured = self.store.state_of(&id).enabled && self.config.allows(&id);
            manager.register(id.as_str(), configured);
            loaded.insert(
                id.as_str().to_string(),
                LoadedExtension {
                    package: builtin.package.clone(),
                    runtime: Some(Arc::clone(&builtin.runtime)),
                    tools: builtin.tools.clone(),
                    contributions: DeclarativeContributions::default(),
                    configured,
                    applied: false,
                    error: None,
                },
            );
        }
        let (packages, failures) = self.store.scan_installed()?;
        for failure in failures {
            issues.push(format!("{}: {}", failure.path.display(), failure.error));
        }
        for package in select_active_versions(packages, &self.store) {
            let id = package.id().clone();
            if loaded.contains_key(id.as_str()) {
                issues.push(format!(
                    "{}: a second package with this id is installed at {}",
                    id,
                    package.root.display()
                ));
                continue;
            }
            let configured = self.store.state_of(&id).enabled && self.config.allows(&id);
            manager.register(id.as_str(), configured);
            let mut entry = LoadedExtension {
                package: package.clone(),
                runtime: None,
                tools: Vec::new(),
                contributions: DeclarativeContributions::default(),
                configured,
                applied: false,
                error: None,
            };
            if !package.report.is_valid() {
                let message = package.error_message();
                entry.error = Some(message.clone());
                manager.fail(id.as_str(), message);
                loaded.insert(id.as_str().to_string(), entry);
                continue;
            }
            if let Err(error) = self.migrate_package(&package) {
                let message = error.to_string();
                entry.error = Some(message.clone());
                manager.fail(id.as_str(), message);
                loaded.insert(id.as_str().to_string(), entry);
                continue;
            }
            match self.build_runtime(&package) {
                Ok((runtime, tools, contributions)) => {
                    let runtime: Arc<dyn Runtime> = runtime;
                    let (runtime, tools) = if contributions.workflows.is_empty() {
                        (runtime, tools)
                    } else {
                        let wrapped =
                            WorkflowRuntime::new(runtime, &contributions.workflows, &tools)?;
                        let tools = wrapped.descriptors();
                        (
                            Arc::new(LazyRuntime::new(Arc::new(wrapped))) as Arc<dyn Runtime>,
                            tools,
                        )
                    };
                    entry.runtime = Some(runtime);
                    entry.tools = tools;
                    entry.contributions = contributions;
                    manager
                        .transition(id.as_str(), LifecycleState::Validating)
                        .ok();
                    manager
                        .transition(id.as_str(), LifecycleState::Resolved)
                        .ok();
                }
                Err(error) => {
                    let message = error.to_string();
                    entry.error = Some(message.clone());
                    manager.fail(id.as_str(), message);
                }
            }
            loaded.insert(id.as_str().to_string(), entry);
        }

        // Resolve the enabled roots. A dependency error fails the roots it
        // touches rather than loading a partial graph: a tool from a package
        // whose dependency is missing may call into code that was never
        // installed.
        let available: Vec<Package> = loaded
            .values()
            // A disabled package is not a candidate, even as a dependency: the
            // reader switched it off, and resolving it anyway would run code a
            // dependent named but the reader did not enable.
            .filter(|entry| entry.configured && entry.error.is_none() && entry.runtime.is_some())
            .map(|entry| entry.package.clone())
            .collect();
        let roots: Vec<ExtensionId> = loaded
            .values()
            .filter(|entry| entry.configured && startup_allows(&entry.package, &self.config))
            .filter(|entry| entry.error.is_none() && entry.runtime.is_some())
            .map(|entry| entry.package.id().clone())
            .collect();
        let mut ordered: BTreeSet<String> = BTreeSet::new();
        match resolver::resolve(&available, &roots) {
            Ok(resolution) => {
                for package in &resolution.packages {
                    ordered.insert(package.id().as_str().to_string());
                }
                if let Err(error) = write_atomic(
                    &self.store.lock_path(),
                    resolution.lock.to_toml()?.as_bytes(),
                ) {
                    issues.push(format!("lockfile was not written: {error}"));
                }
            }
            Err(error) => {
                issues.push(format!("dependency resolution failed: {error}"));
                for root in &roots {
                    if let Some(entry) = loaded.get_mut(root.as_str()) {
                        entry.error = Some(error.to_string());
                        manager.fail(root.as_str(), error.to_string());
                    }
                }
            }
        }
        let scope = self.host_scope();
        for (id, entry) in loaded.iter_mut() {
            if !ordered.contains(id) {
                continue;
            }
            if let Err(error) = self.policy.authorize_required(
                id,
                entry.package.trust,
                &entry.package.manifest.capabilities.required,
                &scope,
            ) {
                entry.error = Some(error.to_string());
                manager.fail(id, error.to_string());
                continue;
            }
            entry.applied = true;
            manager.transition(id, LifecycleState::Loading).ok();
            manager.transition(id, LifecycleState::Starting).ok();
            manager.activated(id, 0).ok();
        }

        self.manager = manager;
        self.loaded = loaded;
        self.issues = issues;
        for entry in self.loaded.values() {
            if entry.applied {
                if let Ok(event) =
                    lifecycle_event(entry.package.id(), topics::EXTENSION_LOADED, "active", None)
                {
                    let _ = self.events.publish(&event);
                }
            } else if let Some(error) = &entry.error {
                self.store.record_crash(entry.package.id().as_str(), error);
                if let Ok(event) = lifecycle_event(
                    entry.package.id(),
                    topics::EXTENSION_FAILED,
                    "failed",
                    Some(error),
                ) {
                    let _ = self.events.publish(&event);
                }
            }
        }
        let statuses = self.statuses();
        if let Ok(mut cache) = self.status_cache.lock() {
            *cache = statuses;
        }
        self.last_stamp = self.stamp();
        self.start_event_pump();
        Ok(())
    }

    /// Bring an upgraded package's data document to its current schema.
    fn migrate_package(&self, package: &Package) -> Result<(), ExtensionError> {
        let id = package.id();
        let state = self.store.state_of(id);
        let Some(from) = state.data_version.clone() else {
            self.store.set_data_version(id, &package.version())?;
            return Ok(());
        };
        let to = package.version();
        if from == to {
            return Ok(());
        }
        let data_root = self.store.data_dir(id);
        if let Some(version) =
            super::config::migrate(&package.manifest, &package.root, &data_root, &from, &to)?
        {
            self.store.set_data_version(id, &version)?;
        }
        Ok(())
    }

    /// Deliver subscribed events on a small background runtime.
    ///
    /// The thread holds Weak handles, so dropping the host ends it. A new
    /// rebuild increments the generation and the previous thread exits, which
    /// keeps one delivery loop per host rather than one per scan.
    fn start_event_pump(&self) {
        let runtimes: Vec<Weak<dyn Runtime>> = self
            .loaded
            .values()
            .filter_map(|entry| entry.runtime.as_ref())
            .map(Arc::downgrade)
            .collect();
        if runtimes.is_empty() {
            return;
        }
        let generation = self.pump_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let watched = Arc::clone(&self.pump_generation);
        std::thread::Builder::new()
            .name("bsh-extension-events".to_string())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                loop {
                    if watched.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let live: Vec<Arc<dyn Runtime>> =
                        runtimes.iter().filter_map(Weak::upgrade).collect();
                    if live.is_empty() {
                        return;
                    }
                    runtime.block_on(async {
                        for runtime in &live {
                            let _ = runtime.pump().await;
                        }
                    });
                    drop(live);
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            })
            .ok();
    }

    fn build_runtime(&self, package: &Package) -> Result<BuiltRuntime, ExtensionError> {
        let data_root = self.store.data_dir(package.id());
        let config = super::config::load(&package.manifest, &package.root, &data_root)?;
        match package.manifest.runtime.kind {
            RuntimeKind::Declarative => {
                let runtime =
                    declarative::DeclarativeRuntime::new(&package.root, &package.manifest)?;
                let tools = runtime.descriptors();
                let contributions = runtime.contributions().clone();
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    contributions,
                ))
            }
            RuntimeKind::Process => {
                let runtime = ProcessRuntime::new(&package.manifest, &package.root)?
                    .with_config(config.to_json())
                    .with_policy(Arc::clone(&self.policy), package.trust)
                    .with_workspace(self.config.workspace.clone())
                    .with_data_root(Some(data_root.clone()))
                    .with_events(
                        Arc::clone(&self.events),
                        &package.manifest.contributes.events,
                    );
                let tools = runtime.descriptors();
                let contributions =
                    declarative::load_contributions(&package.root, &package.manifest)?;
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    contributions,
                ))
            }
            RuntimeKind::Extism => {
                let runtime = ExtismRuntime::new(
                    &package.manifest,
                    &package.root,
                    &data_root,
                    Arc::clone(&self.policy),
                    Arc::clone(&self.events),
                    self.config.workspace.clone(),
                    config.to_json(),
                    package.trust,
                )?;
                let tools = runtime.descriptors()?;
                let contributions =
                    declarative::load_contributions(&package.root, &package.manifest)?;
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    contributions,
                ))
            }
            RuntimeKind::Mcp => {
                let runtime = McpRuntime::new(&package.manifest, &package.root, config.to_json())?;
                let tools = runtime.descriptors()?;
                let contributions = runtime.contributions()?;
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    contributions,
                ))
            }
            RuntimeKind::WasmComponent => {
                let runtime = WasmRuntime::new(
                    &package.manifest,
                    &package.root,
                    &data_root,
                    Arc::clone(&self.policy),
                    Arc::clone(&self.events),
                    self.config.workspace.clone(),
                    config.to_json(),
                    package.trust,
                )?;
                let tools = runtime.descriptors()?;
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    DeclarativeContributions::default(),
                ))
            }
            RuntimeKind::Builtin => Err(ExtensionError::Manifest {
                path: package.root.join("extension.toml"),
                message: "the builtin runtime is linked in; an installed package cannot claim it"
                    .to_string(),
            }),
        }
    }

    fn host_scope(&self) -> CallScope {
        CallScope {
            workspace: self.config.workspace.clone(),
            path: self.config.workspace.clone(),
            ..CallScope::default()
        }
    }

    fn stamp(&self) -> Option<SystemTime> {
        let mut newest = None;
        for path in [
            self.config.root.clone(),
            self.store.installed_root(),
            self.store.state_path(),
            self.store.registry_path(),
            self.store.grants_path(),
        ] {
            if let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
                newest = Some(match newest {
                    Some(current) if current >= modified => current,
                    _ => modified,
                });
            }
        }
        newest
    }

    /// Refresh if a store or state file changed since the last scan.
    pub(crate) fn ensure_fresh(&mut self) -> Result<(), ExtensionError> {
        if self.stamp() != self.last_stamp {
            self.rebuild()?;
        }
        Ok(())
    }

    /// Apply every enabled extension to one agent build.
    pub(crate) fn apply(
        &self,
        agent: &mut ::agent::Agent,
        registry: &mut ::agent::plugins::PluginRegistry,
    ) -> Vec<crate::plugins::PluginInfo> {
        let mut infos = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            let Some(runtime) = &entry.runtime else {
                continue;
            };
            let contributions = &entry.contributions;
            let has_agent_contribution = !entry.tools.is_empty()
                || !contributions.commands.is_empty()
                || !contributions.panels.is_empty()
                || !contributions.prompts.is_empty()
                || !contributions.contexts.is_empty();
            if !has_agent_contribution {
                continue;
            }
            let contributed = contribution::Contributed {
                extension: entry.package.id().clone(),
                name: entry.package.manifest.extension.name.clone(),
                runtime: Arc::clone(runtime),
                trust: entry.package.trust,
                tools: entry.tools.clone(),
                commands: contributions.commands.clone(),
                panels: contributions.panels.clone(),
                prompts: contributions.prompts.clone(),
                contexts: contributions.contexts.clone(),
                workflows: contributions.workflows.clone(),
                themes: contributions.themes.clone(),
                timeout_ms: entry.package.manifest.resources.execution_ms(),
            };
            infos.push(contribution::install(
                agent,
                registry,
                &contributed,
                Arc::clone(&self.policy),
                Arc::clone(&self.events),
                self.config.workspace.clone(),
            ));
        }
        infos
    }

    /// Every known extension as a status row.
    pub(crate) fn statuses(&self) -> Vec<ExtensionStatus> {
        self.loaded
            .values()
            .map(|entry| self.status_of(entry))
            .collect()
    }

    /// Every panel an applied extension offers, ready for the host UI.
    pub(crate) fn panel_surfaces(&self) -> Vec<PanelSurface> {
        let mut surfaces = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            let Some(runtime) = &entry.runtime else {
                continue;
            };
            let theme = load_theme(entry);
            let mut declared = std::collections::BTreeSet::new();
            for panel in &entry.contributions.panels {
                declared.insert(panel.id.as_str().to_string());
                surfaces.push(PanelSurface {
                    panel_id: entry.package.id().qualify(&panel.id),
                    contribution_id: panel.id.as_str().to_string(),
                    title: panel.title.clone(),
                    icon: panel.icon.clone(),
                    location: panel.location,
                    visibility: panel.visibility,
                    runtime: Arc::clone(runtime),
                    owner: entry.package.id().clone(),
                    theme: theme.clone(),
                    initial: Some(panel.view.clone()),
                });
            }
            for id in &entry.package.manifest.contributes.panels {
                if declared.contains(id.as_str()) {
                    continue;
                }
                surfaces.push(PanelSurface {
                    panel_id: entry.package.id().qualify(id),
                    contribution_id: id.as_str().to_string(),
                    title: id.local_name().to_string(),
                    icon: None,
                    location: PanelLocation::Right,
                    visibility: PanelVisibility::Visible,
                    runtime: Arc::clone(runtime),
                    owner: entry.package.id().clone(),
                    theme: theme.clone(),
                    initial: None,
                });
            }
        }
        surfaces
    }

    /// Every theme a package contributes, keyed by a host-owned id.
    pub(crate) fn theme_surfaces(&self) -> BTreeMap<String, crate::extension::ui::ThemePalette> {
        let mut themes = BTreeMap::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            for theme in &entry.contributions.themes {
                let Ok(text) = std::fs::read_to_string(&theme.path) else {
                    continue;
                };
                if let Ok(palette) =
                    serde_json::from_str::<crate::extension::ui::ThemePalette>(&text)
                {
                    themes.insert(
                        format!("ext.{}.{}", entry.package.id().as_str(), theme.id.as_str()),
                        palette,
                    );
                }
            }
        }
        themes
    }

    /// Every command contribution an applied package offers, ready for the
    /// composer palette.
    pub(crate) fn command_surfaces(&self) -> Vec<CommandSurface> {
        let mut commands = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            for command in &entry.contributions.commands {
                commands.push(CommandSurface {
                    name: entry.package.id().qualify(&command.id),
                    description: command.description.clone(),
                    template: command.template.clone(),
                });
            }
        }
        commands.sort_by(|left, right| left.name.cmp(&right.name));
        commands
    }

    /// Model providers a package contributes, keyed by a host-owned id.
    pub(crate) fn model_provider_surfaces(
        &self,
    ) -> BTreeMap<String, crate::settings::global::Provider> {
        let mut providers = BTreeMap::new();
        for entry in self.loaded.values() {
            if !entry.applied {
                continue;
            }
            for contribution in &entry.contributions.model_providers {
                let Ok(text) = std::fs::read_to_string(&contribution.path) else {
                    continue;
                };
                if let Ok(provider) =
                    serde_json::from_str::<crate::settings::global::Provider>(&text)
                {
                    providers.insert(
                        format!(
                            "ext.{}.{}",
                            entry.package.id().as_str(),
                            contribution.id.as_str()
                        ),
                        provider,
                    );
                }
            }
        }
        providers
    }

    /// Agent presets a package contributes, keyed by a host-owned id.
    pub(crate) fn preset_surfaces(&self) -> BTreeMap<String, crate::settings::global::Preset> {
        let mut presets = BTreeMap::new();
        for entry in self.loaded.values() {
            if !entry.applied {
                continue;
            }
            for (kind, contributions) in [
                ("", &entry.contributions.presets),
                ("agent.", &entry.contributions.agent_providers),
            ] {
                for contribution in contributions {
                    let Ok(text) = std::fs::read_to_string(&contribution.path) else {
                        continue;
                    };
                    if let Ok(preset) =
                        serde_json::from_str::<crate::settings::global::Preset>(&text)
                    {
                        presets.insert(
                            format!(
                                "ext.{}.{}{}",
                                entry.package.id().as_str(),
                                kind,
                                contribution.id.as_str()
                            ),
                            preset,
                        );
                    }
                }
            }
        }
        presets
    }

    /// Every model and agent provider descriptor an applied package declares.
    pub(crate) fn provider_descriptors(&self) -> Vec<serde_json::Value> {
        let mut providers = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied {
                continue;
            }
            for (kind, contributions) in [
                ("model", &entry.contributions.model_providers),
                ("agent", &entry.contributions.agent_providers),
                ("preset", &entry.contributions.presets),
            ] {
                for contribution in contributions {
                    let descriptor = std::fs::read_to_string(&contribution.path)
                        .ok()
                        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    providers.push(serde_json::json!({
                        "extension": entry.package.id().as_str(),
                        "kind": kind,
                        "id": contribution.id.as_str(),
                        "path": contribution.path.display().to_string(),
                        "descriptor": descriptor,
                    }));
                }
            }
        }
        providers
    }

    /// Aggregate counters for the status view and diagnostics.
    pub(crate) fn metrics(&self) -> HostMetrics {
        let bus = self.events.metrics();
        let calls = self.calls.lock().map(|calls| *calls).unwrap_or_default();
        HostMetrics {
            extensions: self.loaded.len(),
            applied: self.loaded.values().filter(|entry| entry.applied).count(),
            calls,
            events_published: bus.published,
            events_delivered: bus.delivered,
            events_dropped: bus.dropped,
            subscribers: self.events.subscriber_count(),
            failed: self
                .loaded
                .values()
                .filter(|entry| entry.error.is_some())
                .count(),
            active_tasks: self.loaded.values().filter(|entry| entry.applied).count(),
            startup_millis: self.startup_millis,
            fuel_used: self
                .loaded
                .values()
                .filter_map(|entry| entry.runtime.as_ref())
                .filter_map(|runtime| runtime.fuel_used())
                .sum(),
            failures: self
                .loaded
                .values()
                .filter_map(|entry| {
                    entry
                        .error
                        .as_ref()
                        .map(|error| format!("{}: {error}", entry.package.id().as_str()))
                })
                .collect(),
            crash_history: self.crash_history.clone(),
        }
    }

    fn record_call(&self, millis: u64, bytes: u64, ok: bool) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.tool_calls = calls.tool_calls.saturating_add(1);
            if !ok {
                calls.tool_failures = calls.tool_failures.saturating_add(1);
            }
            calls.total_call_ms = calls.total_call_ms.saturating_add(millis);
            calls.max_call_ms = calls.max_call_ms.max(millis);
            calls.output_bytes = calls.output_bytes.saturating_add(bytes);
        }
    }

    fn status_of(&self, entry: &LoadedExtension) -> ExtensionStatus {
        let id = entry.package.id();
        let record = self.manager.get(id.as_str());
        let state = record
            .map(|record| record.state)
            .unwrap_or(LifecycleState::Discovered);
        let health = record
            .map(|record| record.health())
            .unwrap_or(Health::Unknown);
        let error = entry
            .error
            .clone()
            .or_else(|| record.and_then(|record| record.last_error.clone()));
        let scope = self.host_scope();
        let mut capabilities = Vec::new();
        let mut seen = BTreeSet::new();
        for capability in entry
            .package
            .manifest
            .capabilities
            .required
            .iter()
            .chain(entry.package.manifest.capabilities.optional.iter())
        {
            if !seen.insert(*capability) {
                continue;
            }
            let decision =
                self.policy
                    .preview(id.as_str(), entry.package.trust, *capability, &scope);
            let (allowed, granted_scope, reason) = match &decision {
                Decision::Allow { scope } => (true, Some(scope.to_string()), None),
                Decision::Deny { reason } => (false, None, Some(reason.clone())),
            };
            capabilities.push(CapabilityStatus {
                capability: capability.as_str().to_string(),
                allowed,
                scope: granted_scope,
                reason,
                risk: capability.risk().as_str().to_string(),
            });
        }
        ExtensionStatus {
            id: id.as_str().to_string(),
            name: entry.package.manifest.extension.name.clone(),
            version: entry.package.manifest.extension.version.to_string(),
            runtime: entry.package.manifest.runtime.kind.as_str().to_string(),
            trust: entry.package.trust.as_str().to_string(),
            source: entry.package.source.label(),
            root: entry.package.root.display().to_string(),
            hash: entry.package.hash.clone(),
            state,
            health,
            configured: entry.configured,
            applied: entry.applied && error.is_none(),
            tools: entry
                .tools
                .iter()
                .map(|tool| id.qualify(&tool.id))
                .collect(),
            commands: entry
                .contributions
                .commands
                .iter()
                .map(|command| id.qualify(&command.id))
                .collect(),
            panels: entry
                .contributions
                .panels
                .iter()
                .map(|panel| id.qualify(&panel.id))
                .collect(),
            capabilities,
            error,
            startup_ms: record.map(|record| record.startup_ms).unwrap_or(0),
            installed_at: entry.package.installed_at,
            dependencies: entry
                .package
                .manifest
                .dependencies
                .keys()
                .map(|id| id.as_str().to_string())
                .collect(),
            signature: entry.package.verification.label().to_string(),
            publisher: entry
                .package
                .signature
                .as_ref()
                .map(|signature| signature.publisher.clone()),
        }
    }

    /// The doctor's report.
    pub(crate) fn doctor(&self) -> DiagnosticsReport {
        let statuses = self.statuses();
        let mut issues: Vec<DiagnosticIssue> = self
            .issues
            .iter()
            .map(|message| DiagnosticIssue::new("error", None, message.clone()))
            .collect();
        let registry = self.store.read_registry().ok();
        let mut tool_names: BTreeMap<String, String> = BTreeMap::new();
        for entry in self.loaded.values() {
            let id = entry.package.id().as_str();
            if entry.package.source != ExtensionSource::Builtin {
                if let Some(record) = registry.as_ref().and_then(|registry| {
                    registry.record(id, &entry.package.manifest.extension.version.to_string())
                }) {
                    if !record.hash.is_empty()
                        && !entry.package.hash.is_empty()
                        && record.hash != entry.package.hash
                    {
                        issues.push(DiagnosticIssue::new(
                            "error",
                            Some(id),
                            "installed files no longer match the hash recorded at install time",
                        ));
                    }
                    match &entry.package.verification {
                        crate::extension::signing::SignatureVerification::Verified {
                            publisher,
                        } => {
                            issues.push(DiagnosticIssue::new(
                                "info",
                                Some(id),
                                format!("package signature verified for publisher {publisher}"),
                            ));
                        }
                        crate::extension::signing::SignatureVerification::Untrusted {
                            publisher,
                            reason,
                        } => issues.push(DiagnosticIssue::new(
                            "warning",
                            Some(id),
                            format!("signature by {publisher} is untrusted: {reason}"),
                        )),
                        crate::extension::signing::SignatureVerification::Invalid { reason } => {
                            issues.push(DiagnosticIssue::new(
                                "error",
                                Some(id),
                                format!("package signature is invalid: {reason}"),
                            ));
                        }
                        crate::extension::signing::SignatureVerification::Missing => {
                            if entry.package.signature.is_some() {
                                issues.push(DiagnosticIssue::new(
                                    "warning",
                                    Some(id),
                                    "package ships signature metadata but has not been verified",
                                ));
                            }
                        }
                    }
                }
            }
            for warning in &entry.package.report.warnings {
                issues.push(DiagnosticIssue::new(
                    "warning",
                    Some(id),
                    warning.to_string(),
                ));
            }
            if entry.applied && entry.error.is_none() {
                for tool in &entry.tools {
                    let qualified = entry.package.id().qualify(&tool.id);
                    if let Some(previous) = tool_names.insert(qualified.clone(), id.to_string()) {
                        issues.push(DiagnosticIssue::new(
                            "error",
                            Some(id),
                            format!("tool name {qualified} is also claimed by {previous}"),
                        ));
                    }
                }
            }
        }
        if self.loaded.is_empty() {
            issues.push(DiagnosticIssue::new(
                "warning",
                None,
                "no extensions are installed or registered",
            ));
        }
        DiagnosticsReport {
            host_version: HOST_VERSION.to_string(),
            extension_api: EXTENSION_API_VERSION.to_string(),
            root: self.store.root().display().to_string(),
            extensions: statuses,
            issues,
        }
    }
}

/// The first declared theme for an extension, if it parses.
fn load_theme(entry: &LoadedExtension) -> Option<ThemePalette> {
    let theme = entry.contributions.themes.first()?;
    let text = std::fs::read_to_string(&theme.path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Pick one version per id: the pinned active version, else the highest.
///
/// Every version stays on disk for rollback; only the selected one is loaded,
/// so two versions cannot register the same tool namespace.
fn select_active_versions(packages: Vec<Package>, store: &ExtensionStore) -> Vec<Package> {
    let mut by_id: BTreeMap<String, Vec<Package>> = BTreeMap::new();
    for package in packages {
        by_id
            .entry(package.id().as_str().to_string())
            .or_default()
            .push(package);
    }
    let mut selected = Vec::new();
    for (id, mut versions) in by_id {
        let active = ExtensionId::parse(id.clone())
            .ok()
            .and_then(|id| store.active_version(&id));
        if let Some(active) = active {
            if let Some(position) = versions
                .iter()
                .position(|package| package.version() == active)
            {
                selected.push(versions.swap_remove(position));
                continue;
            }
        }
        versions.sort_by(|left, right| {
            left.manifest
                .extension
                .version
                .cmp(&right.manifest.extension.version)
        });
        if let Some(package) = versions.pop() {
            selected.push(package);
        }
    }
    selected
}

fn startup_allows(package: &Package, config: &HostConfig) -> bool {
    match package.manifest.runtime.startup {
        StartupPolicy::OnStartup | StartupPolicy::OnDemand => true,
        StartupPolicy::OnWorkspaceOpen => config.workspace.is_some(),
        StartupPolicy::Manual => false,
    }
}

/// The process-wide host. One per configuration; a changed configuration opens
/// a fresh host and drops the previous runtimes.
static HOST: OnceLock<Mutex<Option<ExtensionHost>>> = OnceLock::new();

/// Install every enabled extension into one agent build.
///
/// This is the only function the rest of BSH needs. It never panics and never
/// fails a build: a broken extension is logged and reported through the status
/// and doctor surfaces, not by making the harness unusable.
pub(crate) fn install(
    agent: &mut ::agent::Agent,
    registry: &mut ::agent::plugins::PluginRegistry,
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    preset_allowed: Option<&[String]>,
) -> Vec<crate::plugins::PluginInfo> {
    let config =
        HostConfig::from_settings(settings, workspace.map(Path::to_path_buf), preset_allowed);
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(_) => {
            log::warn!("extension host lock is poisoned; extensions are not loaded");
            return Vec::new();
        }
    };
    let reopen = guard.as_ref().is_none_or(|host| host.config != config);
    if reopen {
        match ExtensionHost::open(config) {
            Ok(host) => *guard = Some(host),
            Err(error) => {
                log::warn!("extension host could not open: {error}");
                return Vec::new();
            }
        }
    } else if let Some(host) = guard.as_mut() {
        if let Err(error) = host.ensure_fresh() {
            log::warn!("extension host could not refresh: {error}");
        }
    }
    guard
        .as_ref()
        .map(|host| host.apply(agent, registry))
        .unwrap_or_default()
}

/// Publish a host event on the process-global bus.
///
/// The host never invents a subscriber: a package that did not subscribe
/// receives nothing, and a full queue is counted rather than blocking the
/// publisher.
pub(crate) fn publish(topic: &str, payload: serde_json::Value) {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return;
    };
    let Some(host) = guard.as_ref() else {
        return;
    };
    if let Ok(event) = ExtensionEvent::new("bsh", topic, payload) {
        if let Err(error) = host.events.publish(&event) {
            log::debug!(target: "bsh::extension", "event publish failed: {error}");
        }
    }
}

/// Record one extension tool call on the process-global host.
pub(crate) fn record_tool_call(millis: u64, bytes: u64, ok: bool) {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return;
    };
    if let Some(host) = guard.as_ref() {
        host.record_call(millis, bytes, ok);
    }
}

/// Model providers from the already-open host.
pub(crate) fn cached_model_providers() -> BTreeMap<String, crate::settings::global::Provider> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return BTreeMap::new();
    };
    guard
        .as_ref()
        .map(ExtensionHost::model_provider_surfaces)
        .unwrap_or_default()
}

/// Agent presets from the already-open host.
pub(crate) fn cached_presets() -> BTreeMap<String, crate::settings::global::Preset> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return BTreeMap::new();
    };
    guard
        .as_ref()
        .map(ExtensionHost::preset_surfaces)
        .unwrap_or_default()
}

/// Commands from the already-open host, without opening one.
pub(crate) fn cached_commands() -> Vec<CommandSurface> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return Vec::new();
    };
    guard
        .as_ref()
        .map(ExtensionHost::command_surfaces)
        .unwrap_or_default()
}

/// The panels the current configuration offers, for the host UI.
pub(crate) fn panels(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    preset_allowed: Option<&[String]>,
) -> Vec<PanelSurface> {
    let config =
        HostConfig::from_settings(settings, workspace.map(Path::to_path_buf), preset_allowed);
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(_) => return Vec::new(),
    };
    let reopen = guard.as_ref().is_none_or(|host| host.config != config);
    if reopen {
        match ExtensionHost::open(config) {
            Ok(host) => *guard = Some(host),
            Err(error) => {
                log::warn!("extension host could not open: {error}");
                return Vec::new();
            }
        }
    } else if let Some(host) = guard.as_mut() {
        if let Err(error) = host.ensure_fresh() {
            log::warn!("extension host could not refresh: {error}");
        }
    }
    guard
        .as_ref()
        .map(ExtensionHost::panel_surfaces)
        .unwrap_or_default()
}

/// The extension themes the current configuration offers.
pub(crate) fn themes(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
) -> BTreeMap<String, crate::extension::ui::ThemePalette> {
    let config = HostConfig::from_settings(settings, workspace.map(Path::to_path_buf), None);
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(_) => return BTreeMap::new(),
    };
    let reopen = guard.as_ref().is_none_or(|host| host.config != config);
    if reopen {
        match ExtensionHost::open(config) {
            Ok(host) => *guard = Some(host),
            Err(error) => {
                log::warn!("extension host could not open: {error}");
                return BTreeMap::new();
            }
        }
    } else if let Some(host) = guard.as_mut() {
        if let Err(error) = host.ensure_fresh() {
            log::warn!("extension host could not refresh: {error}");
        }
    }
    guard
        .as_ref()
        .map(ExtensionHost::theme_surfaces)
        .unwrap_or_default()
}

/// Open a host for the CLI or a test, without touching the process global.
pub(crate) fn open(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
) -> Result<ExtensionHost, ExtensionError> {
    ExtensionHost::open(HostConfig::from_settings(
        settings,
        workspace.map(Path::to_path_buf),
        None,
    ))
}

/// A build that only needs the report (the CLI doctor path).
pub(crate) fn doctor_report(config: HostConfig) -> Result<DiagnosticsReport, ExtensionError> {
    Ok(ExtensionHost::open(config)?.doctor())
}

/// The manifest of the built-in extension inspector.
const INSPECTOR_MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.bos.bsh.extensions"
name = "Extension Inspector"
version = "1.0.0"
description = "Reports the extensions installed on this machine and their host state"
authors = ["BrickOS Team"]
license = "MIT"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "inspector"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["list"]
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::GrantScope;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn scratch(label: &str) -> PathBuf {
        let index = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "bsh-ext-host-{label}-{}-{index}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        path
    }

    fn config(root: &Path) -> HostConfig {
        HostConfig {
            root: root.to_path_buf(),
            workspace: Some(root.join("workspace")),
            disabled: BTreeSet::new(),
            preset_allowed: None,
        }
    }

    fn write_package(root: &Path) {
        std::fs::create_dir_all(root.join("tools")).expect("tools");
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.hosted"
name = "Hosted"
version = "1.0.0"
description = "A hosted declarative package"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["echo"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("tools/echo.json"),
            r#"{"description":"Echo","implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
    }

    fn write_dependent(root: &Path, id: &str, dependency: &str) {
        std::fs::create_dir_all(root.join("tools")).expect("tools");
        let manifest = format!(
            r#"
schema_version = "1.0"

[extension]
id = "{id}"
name = "Dependent"
version = "1.0.0"
description = "A package that depends on another"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["echo"]

[dependencies]
"{dependency}" = "^1.0"
"#
        );
        std::fs::write(root.join("extension.toml"), manifest).expect("manifest");
        std::fs::write(
            root.join("tools/echo.json"),
            r#"{"description":"Echo","implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
    }

    #[cfg(unix)]
    #[test]
    fn every_runtime_loads_through_one_host() {
        let root = scratch("all-runtimes");
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        let store = ExtensionStore::open(&root).expect("store");
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions");
        let mut packages = Vec::new();
        for name in [
            "hello",
            "process-echo",
            "wasm-echo",
            "mcp-echo",
            "extism-echo",
        ] {
            let package = store.install(&examples.join(name)).expect(name);
            packages.push(package);
        }
        let policy =
            PolicyEngine::open(store.grants_path(), store.audit_path(), PolicyMode::Enforce)
                .expect("policy");
        for package in &packages {
            for capability in &package.manifest.capabilities.required {
                policy
                    .grant(package.id().as_str(), *capability, GrantScope::Any, None)
                    .expect("grant");
            }
        }
        drop(policy);
        let host = ExtensionHost::open(config(&root)).expect("host");
        for id in [
            "com.example.hello",
            "com.example.process-echo",
            "com.example.wasm-echo",
            "com.example.mcp-echo",
            "com.example.extism-echo",
        ] {
            let status = host
                .statuses()
                .into_iter()
                .find(|status| status.id == id)
                .unwrap_or_else(|| panic!("{id} is missing"));
            assert!(status.applied, "{id}: {:?}", status.error);
            assert!(status.error.is_none(), "{id}: {:?}", status.error);
        }
        let tools: Vec<String> = host
            .statuses()
            .into_iter()
            .flat_map(|status| status.tools)
            .collect();
        for qualified in [
            "hello__echo",
            "process-echo__echo",
            "wasm-echo__echo",
            "mcp-echo__echo",
            "extism-echo__echo",
        ] {
            assert!(
                tools.iter().any(|tool| tool == qualified),
                "{qualified} in {tools:?}"
            );
        }
        let providers = host.provider_descriptors();
        for kind in ["model", "agent", "preset"] {
            assert!(
                providers.iter().any(
                    |provider| provider["extension"] == "com.example.extism-echo"
                        && provider["kind"] == kind
                ),
                "{kind} is missing from {providers:?}"
            );
        }
        let panels = host.panel_surfaces();
        assert!(panels
            .iter()
            .any(|panel| panel.owner.as_str() == "com.example.extism-echo"));
        assert!(host
            .model_provider_surfaces()
            .contains_key("ext.com.example.extism-echo.example"));
        let presets = host.preset_surfaces();
        assert!(presets.contains_key("ext.com.example.extism-echo.reader"));
        assert!(presets.contains_key("ext.com.example.extism-echo.agent.helper"));
        assert!(host
            .theme_surfaces()
            .contains_key("ext.com.example.extism-echo.midnight"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_package_does_not_stop_a_good_one() {
        let root = scratch("isolation");
        let good = root.join("good");
        write_package(&good);
        let bad = root.join("bad");
        write_package(&bad);
        let bad_manifest = std::fs::read_to_string(bad.join("extension.toml"))
            .expect("bad manifest")
            .replace("com.example.hosted", "com.example.broken");
        std::fs::write(bad.join("extension.toml"), bad_manifest).expect("bad manifest");
        let store = ExtensionStore::open(&root).expect("store");
        store.install(&good).expect("good install");
        store.install(&bad).expect("bad install");
        std::fs::write(
            store
                .installed_root()
                .join("com.example.broken/1.0.0/extension.toml"),
            "not toml",
        )
        .expect("corrupt manifest");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let metrics = host.metrics();
        assert_eq!(
            metrics.applied, 2,
            "the builtin inspector and good package still apply"
        );
        let statuses = host.statuses();
        assert!(
            statuses
                .iter()
                .any(|status| status.id == "com.example.hosted"),
            "the good package is still listed"
        );
        assert!(
            !statuses
                .iter()
                .any(|status| status.id == "com.example.broken"),
            "the broken package is isolated from the good one"
        );
    }
    #[test]
    fn the_builtin_inspector_is_registered_and_applied() {
        let root = scratch("builtin");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.bos.bsh.extensions")
            .expect("inspector");
        assert!(status.applied);
        assert_eq!(status.tools, vec!["extensions__list"]);
        assert_eq!(status.state, LifecycleState::Active);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_installed_package_is_discovered_and_applied() {
        let root = scratch("installed");
        let source = root.join("source");
        write_package(&source);
        let store = ExtensionStore::open(root.join("extensions")).expect("store");
        store.install(&source).expect("install");
        let host = ExtensionHost::open(config(&root.join("extensions"))).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.hosted")
            .expect("hosted");
        assert!(status.applied, "{status:?}");
        assert_eq!(status.tools, vec!["hosted__echo"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_disabled_package_is_listed_but_not_applied() {
        let root = scratch("disabled");
        let source = root.join("source");
        write_package(&source);
        let store = ExtensionStore::open(root.join("extensions")).expect("store");
        store.install(&source).expect("install");
        let id = ExtensionId::parse("com.example.hosted").expect("id");
        store.set_enabled(&id, false).expect("disable");
        let host = ExtensionHost::open(config(&root.join("extensions"))).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.hosted")
            .expect("hosted");
        assert!(!status.applied);
        assert!(!status.configured);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_disabled_dependency_fails_the_dependent() {
        let root = scratch("disabled-dependency");
        let base = root.join("source-base");
        let dependent = root.join("source-dependent");
        write_package(&base);
        write_dependent(&dependent, "com.example.dependent", "com.example.hosted");
        let store = ExtensionStore::open(root.join("extensions")).expect("store");
        store.install(&base).expect("base");
        store.install(&dependent).expect("dependent");
        let base_id = ExtensionId::parse("com.example.hosted").expect("id");
        store.set_enabled(&base_id, false).expect("disable");
        let host = ExtensionHost::open(config(&root.join("extensions"))).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.dependent")
            .expect("dependent");
        assert!(!status.applied);
        assert!(status.error.is_some(), "{status:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn doctor_reports_a_hash_tampered_package() {
        let root = scratch("tamper");
        let source = root.join("source");
        write_package(&source);
        let store = ExtensionStore::open(root.join("extensions")).expect("store");
        store.install(&source).expect("install");
        let installed = store
            .installed_root()
            .join("com.example.hosted")
            .join("1.0.0");
        std::fs::write(installed.join("extra.txt"), "changed").expect("tamper");
        let host = ExtensionHost::open(config(&root.join("extensions"))).expect("host");
        let report = host.doctor();
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.message.contains("hash")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_preset_allowlist_also_gates_a_builtin() {
        let root = scratch("preset-builtin");
        let mut narrowed = config(&root);
        narrowed.preset_allowed = Some(BTreeSet::new());
        let host = ExtensionHost::open(narrowed).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.bos.bsh.extensions")
            .expect("inspector");
        assert!(!status.configured);
        assert!(!status.applied);
        let _ = std::fs::remove_dir_all(&root);
    }
}
