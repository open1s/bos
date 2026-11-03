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
use super::lifecycle::{Health, LifecycleManager};
use bos_ext_core::capability::{
    CallScope, Capability, Decision, GrantScope, PolicyEngine, PolicyMode,
};
use bos_ext_core::composable::dispatch::InterceptorPolicy;
use bos_ext_core::composable::{
    Context, DisposalReport, Fiber, FiberState, Scope, ServiceAuthorizer, ServiceToken,
};
use bos_ext_core::events::{lifecycle_event, topics, EventBus, ExtensionEvent};
use bos_ext_core::package::{write_atomic, ExtensionStore, Package};
use bos_ext_core::resolver;
use bos_ext_core::types::{
    ContributionId, ExtensionError, ExtensionId, ExtensionSource, LifecycleState, RuntimeKind,
    StartupPolicy, TrustLevel, ValidationReport, EXTENSION_API_VERSION, HOST_VERSION,
};
use bos_ext_core::ui::{PanelLocation, PanelVisibility, ThemePalette, UiDocument};
use bos_ext_runtime::builtin::{BuiltinRuntime, ClosureExtension};
use bos_ext_runtime::declarative::{self, DeclarativeContributions};
use bos_ext_runtime::extism::ExtismRuntime;
use bos_ext_runtime::mcp::McpRuntime;
use bos_ext_runtime::process::ProcessRuntime;
use bos_ext_runtime::wasm::WasmRuntime;
use bos_ext_runtime::workflow::WorkflowRuntime;
use bos_ext_runtime::{empty_schema, CallContext, LazyRuntime, Runtime, ToolDescriptor};
use serde_json::Value;

/// The configuration that decides which store and which packages a host owns.
#[derive(Debug, Clone)]
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

impl PartialEq for HostConfig {
    /// Two configurations share one host when they name the same store, the
    /// same workspace and the same disabled set. The preset allowlist is
    /// deliberately excluded: it filters what one build applies, not which
    /// host owns the store. Including it made every status/theme read reopen
    /// the host (and tear down live runtimes) whenever a preset declared an
    /// allowlist.
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
            && self.workspace == other.workspace
            && self.disabled == other.disabled
    }
}

impl Eq for HostConfig {}

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
    ///
    /// The preset allowlist is applied when a build *applies* contributions
    /// (see `apply`), not when the host discovers packages: one host serves
    /// every preset, and a status row describes the store rather than the
    /// preset that happened to build last.
    pub(crate) fn allows(&self, id: &ExtensionId) -> bool {
        !self.disabled.contains(id.as_str())
    }

    /// Whether this preset admits the package.
    fn preset_allows(&self, id: &ExtensionId) -> bool {
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
pub(crate) fn extension_root(settings: &crate::settings::Settings) -> PathBuf {
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
    /// Whether this capability is required for activation.
    pub(crate) required: bool,
}

/// One node in the extension composition graph.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExtensionGraphNode {
    /// Extension id.
    pub(crate) id: String,
    /// Lifecycle state.
    pub(crate) state: String,
    /// Runtime kind.
    pub(crate) runtime: String,
    /// Whether configuration enables it.
    pub(crate) enabled: bool,
    /// Whether it is applied to a build.
    pub(crate) applied: bool,
    /// Declared package dependencies.
    pub(crate) dependencies: Vec<String>,
    /// Declared service providers, as name@version[scope].
    pub(crate) provides: Vec<String>,
    /// Declared service requirements, as name@range[scope].
    pub(crate) requires: Vec<String>,
    /// Declared effect surface, as kind:id (tool, command, panel, service, ...).
    pub(crate) effects: Vec<String>,
    /// Whether the live service-change driver has suspended this activation.
    pub(crate) live_suspended: bool,
    /// The last error, if any.
    pub(crate) error: Option<String>,
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
    /// Declared process executable, when the runtime is a process.
    pub(crate) program: Option<String>,
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
    /// Declared skill names.
    pub(crate) skills: Vec<String>,
    /// Declared language names.
    pub(crate) languages: Vec<String>,
    /// Declared integration names.
    pub(crate) integrations: Vec<String>,
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
    /// Policy the panel's actions are authorized against.
    pub(crate) policy: Arc<PolicyEngine>,
    /// Trust level of the owning package.
    pub(crate) trust: TrustLevel,
    /// The owning package's tool descriptors, so a panel action gets the same
    /// per-call capability check as an agent tool call.
    pub(crate) tools: Arc<Vec<ToolDescriptor>>,
    /// The workspace open for the build that created this surface, used to
    /// resolve a panel action's `path` argument before authorization.
    pub(crate) workspace: Option<PathBuf>,
    /// The package root, so an `asset/…` image or icon can be resolved to a
    /// file the renderer can actually load.
    pub(crate) root: PathBuf,
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
    /// Sum of the memory ceilings the loaded runtimes declared.
    pub(crate) memory_limit_bytes: u64,
    /// Sum of the current linear-memory usage the runtimes report.
    pub(crate) memory_used_bytes: u64,
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

/// Host-side authorizer for service acquisition. A provider's declared
/// capabilities are checked against the consumer's grants at acquisition time;
/// a service with no declared capabilities keeps the declaration-only rule.
struct ServicePolicyAuthorizer {
    policy: Arc<PolicyEngine>,
    consumer: String,
    trust: TrustLevel,
    capabilities: BTreeMap<String, Vec<Capability>>,
    workspace: Option<PathBuf>,
}

impl ServiceAuthorizer for ServicePolicyAuthorizer {
    fn authorize(&self, _consumer: &str, _scope: &Scope, name: &str) -> Result<(), String> {
        let Some(capabilities) = self.capabilities.get(name) else {
            return Ok(());
        };
        let call_scope = CallScope {
            workspace: self.workspace.clone(),
            path: self.workspace.clone(),
            ..CallScope::default()
        };
        for capability in capabilities {
            self.policy
                .authorize_call(&self.consumer, self.trust, *capability, &call_scope)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// Host-side interceptor authorizer. Registration is checked against the
/// consumer's agent.intercept grant and recorded in the audit log.
struct PolicyInterceptor {
    policy: Arc<PolicyEngine>,
    owner: String,
    trust: TrustLevel,
    workspace: Option<PathBuf>,
}

impl InterceptorPolicy for PolicyInterceptor {
    fn authorize(&self, owner: &str, _event: &str) -> Result<(), String> {
        if !self.owner.is_empty() && owner != self.owner {
            return Err("interceptor owner does not match the activation".to_string());
        }
        let scope = CallScope {
            workspace: self.workspace.clone(),
            path: self.workspace.clone(),
            ..CallScope::default()
        };
        // A host-wide policy has no activation-bound owner. In that case,
        // authorize and audit the principal supplied by the registration path.
        // An activation-bound policy continues to use its pinned owner after
        // the equality check above.
        let principal = if self.owner.is_empty() {
            owner
        } else {
            self.owner.as_str()
        };
        self.policy
            .authorize_call(principal, self.trust, Capability::AgentIntercept, &scope)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Activation handles held for one build.
///
/// Popping on drop disposes dependents before their providers because the
/// activation order is dependency-first. A caller that moves the set into a
/// turn keeps the same order through into_vec and the state map.
#[derive(Default)]
pub(crate) struct ActivationList(Vec<AppliedExtension>);

impl ActivationList {
    /// Take the underlying vector, preserving its order.
    pub(crate) fn into_vec(self) -> Vec<AppliedExtension> {
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: this is not dropped, so the vector is moved exactly once.
        unsafe { std::ptr::read(&this.0) }
    }
}

impl From<Vec<AppliedExtension>> for ActivationList {
    fn from(value: Vec<AppliedExtension>) -> Self {
        Self(value)
    }
}

impl std::ops::Deref for ActivationList {
    type Target = [AppliedExtension];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for ActivationList {
    fn drop(&mut self) {
        while self.0.pop().is_some() {}
    }
}

/// One extension's contributions installed into a single Agent build.
///
/// This is a build-scoped activation. The caller must invoke unload before
/// reusing the Agent/PluginRegistry for another build; dropping the build
/// naturally drops both registries and the Fiber's liveness guard.
pub(crate) struct AppliedExtension {
    pub(crate) info: crate::plugins::PluginInfo,
    fiber: Fiber,
    lease: contribution::ContributionLease,
    services: Arc<bos_ext_core::composable::ServiceRegistry>,
    extension_id: String,
    workspace: Option<PathBuf>,
    session_id: String,
    service_provides: Vec<bos_ext_core::manifest::ServiceProvideSpec>,
    service_requires: Vec<bos_ext_core::manifest::ServiceRequireSpec>,
}

impl AppliedExtension {
    /// Publish a typed value only when this extension declared the matching
    /// service name and visibility scope in its manifest.
    pub(crate) fn provide_service<T>(
        &mut self,
        scope: bos_ext_core::composable::Scope,
        name: &str,
        service: Arc<T>,
    ) -> Option<bos_ext_core::composable::ServiceToken>
    where
        T: std::any::Any + Send + Sync,
    {
        // AppliedExtension is owned by one Agent build. Its Fiber cannot own a
        // Host-, Workspace- or Session-lifetime registration: dropping this
        // build would revoke a service still used by other builds. Until those
        // scopes have dedicated lifecycle owners, fail closed and permit only
        // this extension's private scope.
        if !service_scope_is_activation_owned(&scope, &self.extension_id) {
            return None;
        }
        if !self.service_provides.iter().any(|declaration| {
            declaration.name == name
                && service_scope_matches(
                    &declaration.scope,
                    &scope,
                    &self.extension_id,
                    self.workspace.as_deref(),
                    &self.session_id,
                )
        }) {
            return None;
        }
        Some(self.services.provide_owned(
            scope,
            name.to_owned(),
            service,
            // The activation Fiber owns the registration and revokes it on
            // unload, preventing service values from outliving their provider.
            &mut self.fiber,
        ))
    }

    /// Acquire a typed service only when this extension declared a matching
    /// requirement. The returned lease must be held for the full call.
    pub(crate) fn acquire_service<T>(
        &self,
        scope: &bos_ext_core::composable::Scope,
        name: &str,
    ) -> Option<bos_ext_core::composable::ServiceLease<T>>
    where
        T: std::any::Any + Send + Sync,
    {
        self.service_requires
            .iter()
            .any(|requirement| {
                requirement.name == name
                    && service_scope_matches(
                        &requirement.scope,
                        scope,
                        &self.extension_id,
                        self.workspace.as_deref(),
                        &self.session_id,
                    )
            })
            .then(|| {
                self.fiber
                    .context()
                    .and_then(|context| context.acquire_at(scope, name))
            })
            .flatten()
    }

    /// Remove this activation's live tools/prompts/skills while keeping the
    /// commands and panels installed.
    pub(crate) fn suspend_live(&self, agent: &::agent::Agent) -> usize {
        self.lease.suspend_live(agent)
    }

    /// Restore the live overlay after a suspend.
    pub(crate) fn resume_live(&self, agent: &::agent::Agent) -> usize {
        self.lease.resume_live(agent)
    }

    /// Revoke this activation from the same Agent and PluginRegistry.
    pub(crate) fn unload(
        mut self,
        agent: &mut ::agent::Agent,
        registry: &mut ::agent::plugins::PluginRegistry,
    ) -> Result<usize, bos_ext_core::composable::FiberTransitionError> {
        self.fiber.unload()?;
        Ok(self.lease.revoke(agent, registry))
    }

    /// Revoke this activation, awaiting async effect cleanups.
    pub(crate) async fn unload_async(
        mut self,
        agent: &mut ::agent::Agent,
        registry: &mut ::agent::plugins::PluginRegistry,
    ) -> Result<DisposalReport, bos_ext_core::composable::FiberTransitionError> {
        let report = self.fiber.dispose_async().await?;
        let _ = self.lease.revoke(agent, registry);
        Ok(report)
    }

    /// The composition context attached to this activation.
    pub(crate) fn context(&self) -> Option<&Context> {
        self.fiber.context()
    }

    /// The service registry this activation resolves against.
    pub(crate) fn services(&self) -> Arc<bos_ext_core::composable::ServiceRegistry> {
        Arc::clone(&self.services)
    }

    /// A cloneable snapshot of the live lease for the reload driver.
    pub(crate) fn lease_snapshot(&self) -> contribution::ContributionLease {
        self.lease.clone()
    }

    /// Declared service requirements, for the reload driver.
    pub(crate) fn service_requirements(&self) -> &[bos_ext_core::manifest::ServiceRequireSpec] {
        &self.service_requires
    }

    /// Stable extension id for this activation.
    pub(crate) fn extension_id(&self) -> &str {
        &self.extension_id
    }

    /// Current live activation state.
    pub(crate) fn state(&self) -> FiberState {
        self.fiber.state()
    }

    /// Names of the async tools this activation registered on the build.
    ///
    /// The names are the qualified ones the tool registry holds, so a caller
    /// can look the live tool up and prove the Fiber-owned liveness gate
    /// without reaching into the lease.
    pub(crate) fn tool_names(&self) -> &[String] {
        self.lease.tool_names()
    }
}

#[cfg(test)]
mod service_scope_tests {
    use super::{service_scope_is_activation_owned, service_scope_matches};
    use bos_ext_core::composable::Scope;
    use bos_ext_core::manifest::ServiceScope;
    use std::path::Path;

    #[test]
    fn build_activation_can_own_only_its_private_extension_scope() {
        assert!(service_scope_is_activation_owned(
            &Scope::Extension("ext".into()),
            "ext",
        ));
        assert!(!service_scope_is_activation_owned(
            &Scope::Extension("other".into()),
            "ext",
        ));
        assert!(!service_scope_is_activation_owned(&Scope::Host, "ext"));
        assert!(!service_scope_is_activation_owned(
            &Scope::Workspace("/repo".into()),
            "ext",
        ));
        assert!(!service_scope_is_activation_owned(
            &Scope::Session("session-1".into()),
            "ext",
        ));
    }

    #[test]
    fn declared_service_scope_is_enforced_and_session_isolation_is_exact() {
        let session = "session-1";
        assert!(service_scope_matches(
            &ServiceScope::Host,
            &Scope::Host,
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(service_scope_matches(
            &ServiceScope::Workspace,
            &Scope::Workspace("/repo".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(service_scope_matches(
            &ServiceScope::Session,
            &Scope::Session(session.into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(service_scope_matches(
            &ServiceScope::Extension,
            &Scope::Extension("ext".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(!service_scope_matches(
            &ServiceScope::Session,
            &Scope::Session("session-2".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(!service_scope_matches(
            &ServiceScope::Workspace,
            &Scope::Workspace("/other".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(!service_scope_matches(
            &ServiceScope::Extension,
            &Scope::Extension("other".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        assert!(!service_scope_matches(
            &ServiceScope::Host,
            &Scope::Session("session-1".into()),
            "ext",
            Some(Path::new("/repo")),
            session,
        ));
        // A build with no workspace open cannot satisfy a workspace scope, even
        // when the requested id names a real path: the grant fails closed.
        assert!(!service_scope_matches(
            &ServiceScope::Workspace,
            &Scope::Workspace("/repo".into()),
            "ext",
            None,
            session,
        ));
    }
}

/// Whether a scope's lifetime is no longer than one AppliedExtension Fiber.
fn service_scope_is_activation_owned(
    requested: &bos_ext_core::composable::Scope,
    extension_id: &str,
) -> bool {
    matches!(
        requested,
        bos_ext_core::composable::Scope::Extension(id) if id == extension_id
    )
}

fn service_scope_matches(
    declared: &bos_ext_core::manifest::ServiceScope,
    requested: &bos_ext_core::composable::Scope,
    extension_id: &str,
    workspace: Option<&Path>,
    session_id: &str,
) -> bool {
    use bos_ext_core::composable::Scope;
    use bos_ext_core::manifest::ServiceScope;
    match (declared, requested) {
        (ServiceScope::Host, Scope::Host) => true,
        (ServiceScope::Workspace, Scope::Workspace(id)) => {
            workspace.is_some_and(|workspace| Path::new(id) == workspace)
        }
        (ServiceScope::Session, Scope::Session(id)) => id == session_id,
        (ServiceScope::Extension, Scope::Extension(id)) => id == extension_id,
        _ => false,
    }
}

/// One runtime attached to an observe hook point.
pub(crate) struct HookTarget {
    /// Extension id, for diagnostics.
    pub(crate) id: String,
    /// The runtime that receives the batch.
    pub(crate) runtime: Arc<dyn Runtime>,
}

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
    services: Arc<bos_ext_core::composable::ServiceRegistry>,
    terminal_token: Option<ServiceToken>,
    manager: LifecycleManager,
    loaded: BTreeMap<String, LoadedExtension>,
    builtins: Vec<BuiltinLoaded>,
    issues: Vec<String>,
    status_cache: Arc<Mutex<Vec<ExtensionStatus>>>,
    graph_cache: Arc<Mutex<Vec<ExtensionGraphNode>>>,
    last_stamp: Option<SystemTime>,
    pump_generation: Arc<AtomicU64>,
    startup_millis: u64,
    crash_history: Vec<String>,
    languages: Vec<LanguageEntry>,
    integrations: Vec<IntegrationEntry>,
}

/// One language service contributed by an applied extension.
#[derive(Debug, Clone)]
pub(crate) struct LanguageEntry {
    /// Package that owns the descriptor.
    pub(crate) owner: ExtensionId,
    /// Descriptor.
    pub(crate) contribution: bos_ext_runtime::declarative::LanguageContribution,
}

/// One external-system integration contributed by an applied extension.
#[derive(Debug, Clone)]
pub(crate) struct IntegrationEntry {
    /// Package that owns the descriptor.
    pub(crate) owner: ExtensionId,
    /// Descriptor.
    pub(crate) contribution: bos_ext_runtime::declarative::IntegrationContribution,
}

impl ExtensionHost {
    /// The host-scoped service registry shared by its extension activations.
    pub(crate) fn services(&self) -> Arc<bos_ext_core::composable::ServiceRegistry> {
        Arc::clone(&self.services)
    }

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
        crate::extension::dispatcher().set_interceptor_policy(Arc::new(PolicyInterceptor {
            policy: Arc::clone(&policy),
            owner: String::new(),
            trust: TrustLevel::Community,
            workspace: config.workspace.clone(),
        }));
        let events = EventBus::new();
        if let Ok(mut slot) = bus_slot().write() {
            *slot = Some(Arc::clone(&events));
        }
        super::hooks::install_http_tap();
        let status_cache = Arc::new(Mutex::new(Vec::new()));
        let graph_cache = Arc::new(Mutex::new(Vec::new()));
        let mut host = Self {
            config,
            store,
            policy,
            events,
            services: Arc::new(bos_ext_core::composable::ServiceRegistry::new()),
            terminal_token: None,
            manager: LifecycleManager::new(),
            loaded: BTreeMap::new(),
            builtins: Vec::new(),
            issues: Vec::new(),
            status_cache,
            graph_cache,
            last_stamp: None,
            pump_generation: Arc::new(AtomicU64::new(0)),
            startup_millis: 0,
            crash_history: Vec::new(),
            languages: Vec::new(),
            integrations: Vec::new(),
        };
        let terminal = Arc::new(super::terminal::TerminalHandle(Arc::new(
            super::terminal::HostTerminal,
        )));
        host.terminal_token = Some(host.services.provide(Scope::Host, "terminal", terminal));
        super::hooks::install_host_provider();
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

    /// Runtimes attached to one observe hook point.
    pub(crate) fn hook_targets(&self, point: &str) -> Vec<HookTarget> {
        self.loaded
            .values()
            .filter(|entry| entry.applied && entry.error.is_none())
            .filter(|entry| {
                entry
                    .package
                    .manifest
                    .contributes
                    .hooks
                    .iter()
                    .any(|declared| declared == point)
            })
            .filter_map(|entry| {
                entry.runtime.as_ref().map(|runtime| HookTarget {
                    id: entry.package.id().as_str().to_string(),
                    runtime: Arc::clone(runtime),
                })
            })
            .collect()
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
                    verification: bos_ext_core::signing::SignatureVerification::Missing,
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

        let graph_cache = Arc::clone(&self.graph_cache);
        let diagnostics = ClosureExtension::new(
            DIAGNOSTICS_MANIFEST,
            vec![
                ToolDescriptor {
                    id: ContributionId::parse("graph").expect("diagnostics tool id"),
                    description:
                        "Return the declared extension graph: state, runtime, applied flag, dependencies, services and effect surface."
                            .to_string(),
                    input_schema: empty_schema(),
                    capabilities: Vec::new(),
                },
                ToolDescriptor {
                    id: ContributionId::parse("why").expect("diagnostics tool id"),
                    description:
                        "Explain why one extension is or is not applied, including its declared effect surface."
                            .to_string(),
                    input_schema: serde_json::json!({
                        "type": "object",
                        "properties": { "id": { "type": "string" } },
                        "required": ["id"]
                    }),
                    capabilities: Vec::new(),
                },
            ],
            move |name, arguments| {
                let nodes = graph_cache
                    .lock()
                    .map(|nodes| nodes.clone())
                    .unwrap_or_default();
                match name {
                    "graph" => Ok(serde_json::json!({
                        "extensions": nodes,
                        "fibers": super::live::snapshot(),
                    })),
                    "why" => {
                        let id = arguments
                            .get("id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| ExtensionError::Runtime {
                                extension: "com.bos.bsh.diagnostics".to_string(),
                                message: "why needs an id".to_string(),
                            })?;
                        let node = nodes.into_iter().find(|node| node.id == id).ok_or_else(|| {
                            ExtensionError::Runtime {
                                extension: "com.bos.bsh.diagnostics".to_string(),
                                message: format!("extension {id} is not installed"),
                            }
                        })?;
                        serde_json::to_value(node).map_err(|error| ExtensionError::Runtime {
                            extension: "com.bos.bsh.diagnostics".to_string(),
                            message: error.to_string(),
                        })
                    }
                    _ => Err(ExtensionError::UnsupportedRuntime {
                        kind: format!("diagnostics tool {name}"),
                    }),
                }
            },
        );
        match BuiltinRuntime::new(Arc::new(diagnostics)) {
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
                    verification: bos_ext_core::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                });
            }
            Err(error) => log::warn!("built-in diagnostics was not registered: {error}"),
        }

        let annotations =
            ClosureExtension::new(ANNOTATIONS_MANIFEST, Vec::new(), |_name, _arguments| {
                Ok(serde_json::json!({}))
            });
        match BuiltinRuntime::new(Arc::new(annotations)) {
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
                    verification: bos_ext_core::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                });
            }
            Err(error) => log::warn!("built-in annotations was not registered: {error}"),
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
            // The linked-in packages take the same discovery path as an
            // installed one, so activation is a legal transition rather than
            // the old unconditional rewrite to Active.
            manager
                .transition(id.as_str(), LifecycleState::Validating)
                .ok();
            manager
                .transition(id.as_str(), LifecycleState::Resolved)
                .ok();
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
                        let wrapped = WorkflowRuntime::new(
                            runtime,
                            &contributions.workflows,
                            &tools,
                            Arc::clone(&self.policy),
                            package.trust,
                        )?;
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
        // Preserve the dependency planner's topological order. A BTreeSet here
        // would silently turn Provider -> Consumer ordering into lexical order.
        let mut ordered: Vec<String> = Vec::new();
        match resolver::resolve(&available, &roots) {
            Ok(resolution) => {
                // Package SemVer resolution chooses the concrete installed set;
                // service-contract planning then adds provider -> consumer edges
                // and validates interface versions before any package activates.
                let manifests: Vec<_> = resolution
                    .packages
                    .iter()
                    .map(|package| package.manifest.clone())
                    .collect();
                match bos_ext_core::composable::plan_service_dependencies(&manifests) {
                    Ok(plan) => {
                        ordered = plan.order;
                    }
                    Err(error) => {
                        let message = format!("service dependency planning failed: {error}");
                        issues.push(message.clone());
                        for root in &roots {
                            if let Some(entry) = loaded.get_mut(root.as_str()) {
                                entry.error = Some(message.clone());
                                manager.fail(root.as_str(), message.clone());
                            }
                        }
                    }
                }
                if !ordered.is_empty() {
                    if let Err(error) = write_atomic(
                        &self.store.lock_path(),
                        resolution.lock.to_toml()?.as_bytes(),
                    ) {
                        issues.push(format!("lockfile was not written: {error}"));
                    }
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
        for id in ordered {
            let Some(entry) = loaded.get_mut(&id) else {
                continue;
            };
            let mut scope = self.host_scope();
            if matches!(
                entry.package.manifest.runtime.kind,
                RuntimeKind::Process | RuntimeKind::Mcp
            ) {
                // A process/MCP grant is scoped to the declared executable and
                // its arguments. The required-capability check must carry the
                // same pair, or a program:python3:--version grant can never
                // cover the load-time check even though the per-spawn check
                // would pass.
                scope.program = entry.package.manifest.runtime.entry.clone();
                scope.args = Some(entry.package.manifest.runtime.args.clone());
            }
            let started = std::time::Instant::now();
            if let Err(error) = self.policy.authorize_required(
                &id,
                entry.package.trust,
                &entry.package.manifest.capabilities.required,
                &scope,
            ) {
                entry.error = Some(error.to_string());
                manager.fail(&id, error.to_string());
                continue;
            }
            entry.applied = true;
            manager.transition(&id, LifecycleState::Loading).ok();
            manager.transition(&id, LifecycleState::Starting).ok();
            manager
                .activated(&id, started.elapsed().as_millis() as u64)
                .ok();
        }

        // Packages that were applied before this scan and are not applied now
        // are unloaded; §9 promises subscribers an extension.unloaded event and
        // nothing used to publish one.
        let previous_applied: Vec<ExtensionId> = self
            .loaded
            .values()
            .filter(|entry| entry.applied)
            .map(|entry| entry.package.id().clone())
            .collect();
        self.manager = manager;
        self.loaded = loaded;
        self.issues = issues;
        for id in previous_applied {
            let still_applied = self
                .loaded
                .get(id.as_str())
                .is_some_and(|entry| entry.applied);
            if !still_applied {
                if let Ok(event) =
                    lifecycle_event(&id, topics::EXTENSION_UNLOADED, "unloaded", None)
                {
                    let _ = self.events.publish(&event);
                }
            }
        }
        let mut languages = Vec::new();
        let mut integrations = Vec::new();
        for entry in self.loaded.values().filter(|entry| entry.applied) {
            for contribution in &entry.contributions.languages {
                languages.push(LanguageEntry {
                    owner: entry.package.id().clone(),
                    contribution: contribution.clone(),
                });
            }
            for contribution in &entry.contributions.integrations {
                integrations.push(IntegrationEntry {
                    owner: entry.package.id().clone(),
                    contribution: contribution.clone(),
                });
            }
        }
        self.languages = languages;
        self.integrations = integrations;
        for entry in self.loaded.values() {
            if entry.applied {
                let _ = self.store.record_started(entry.package.id());
                if let Ok(event) =
                    lifecycle_event(entry.package.id(), topics::EXTENSION_LOADED, "active", None)
                {
                    let _ = self.events.publish(&event);
                }
            } else if let Some(error) = &entry.error {
                self.store.record_crash(entry.package.id().as_str(), error);
                let _ = self.store.record_failure(entry.package.id(), error);
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
        let annotations_enabled = statuses
            .iter()
            .any(|status| status.id == "com.bos.bsh.annotations" && status.applied);
        if annotations_enabled {
            super::notes::set_store(Arc::new(super::notes::FileNotesStore));
        } else {
            super::notes::set_store(Arc::new(super::notes::DisabledNotesStore));
        }
        if let Ok(mut cache) = self.status_cache.lock() {
            *cache = statuses;
        }
        let graph = self.graph();
        if let Ok(mut cache) = self.graph_cache.lock() {
            *cache = graph;
        }
        self.last_stamp = self.stamp();
        self.start_event_pump();
        self.warm_startup_runtimes();
        Ok(())
    }

    /// Start the packages that declared `startup = "on-startup"`.
    ///
    /// Loading a package registers its descriptors; nothing has started it yet.
    /// This is what gives the policy its meaning. The warm runs on a detached
    /// thread with its own current-thread reactor, so a slow handshake never
    /// blocks a build, and a failure is logged instead of failing the host. An
    /// on-demand package is untouched: only an explicit call activates it.
    fn warm_startup_runtimes(&self) {
        let mut runtimes: Vec<(String, Arc<dyn Runtime>)> = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.package.manifest.runtime.startup != StartupPolicy::OnStartup
            {
                continue;
            }
            if let Some(runtime) = &entry.runtime {
                runtimes.push((entry.package.id().as_str().to_string(), Arc::clone(runtime)));
            }
        }
        if runtimes.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            let Ok(reactor) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                log::warn!(target: "bsh::extension", "no reactor for on-startup activation");
                return;
            };
            reactor.block_on(async {
                for (id, runtime) in runtimes {
                    if let Err(error) = runtime.activate().await {
                        log::warn!(
                            target: "bsh::extension",
                            "{id} on-startup activation failed: {error}"
                        );
                    }
                }
            });
        });
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
            bos_ext_core::config::migrate(&package.manifest, &package.root, &data_root, &from, &to)?
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
        let config = bos_ext_core::config::load(&package.manifest, &package.root, &data_root)?;
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
                let runtime = McpRuntime::new(
                    &package.manifest,
                    &package.root,
                    config.to_json(),
                    Arc::clone(&self.policy),
                    package.trust,
                    Arc::clone(&self.events),
                )?;
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
                // The component serves commands and panels through its own
                // exports; prompts, contexts, languages and themes still come
                // from the declared files. Loading them here is what keeps a
                // declared contribution from silently disappearing.
                let contributions =
                    declarative::load_contributions(&package.root, &package.manifest)?;
                Ok((
                    Arc::new(LazyRuntime::new(Arc::new(runtime))),
                    tools,
                    contributions,
                ))
            }
            RuntimeKind::Cordis => {
                let runtime = bos_ext_runtime::cordis::CordisRuntime::new(
                    &package.manifest,
                    &package.root,
                    &data_root,
                    Arc::new(super::terminal::HostTerminal),
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
        preset_allowed: Option<&BTreeSet<String>>,
    ) -> Vec<crate::plugins::PluginInfo> {
        let mut infos = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            // The preset is a property of this build, not of the cached host:
            // the host is shared across presets, so the filter arrives here.
            if preset_allowed.is_some_and(|allowed| !allowed.contains(entry.package.id().as_str()))
            {
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
                || !contributions.contexts.is_empty()
                // A package may contribute only skills; skipping it here
                // silently dropped every skill it declared.
                || !entry.package.manifest.contributes.skills.is_empty()
                || !entry.package.manifest.services.provides.is_empty()
                || !entry.package.manifest.services.requires.is_empty();
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
                workspace: self.config.workspace.clone(),
                skills_dir: runtime.skills_dir().or_else(|| {
                    (!entry.package.manifest.contributes.skills.is_empty())
                        .then(|| entry.package.root.join("skills"))
                }),
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

    /// Apply enabled extensions with explicit Fiber ownership.
    ///
    /// Unlike the compatibility apply() path, this returns a lease for each
    /// activation. The caller retains these handles for the lifetime of the
    /// Agent build and calls AppliedExtension::unload before reusing its
    /// registries. The Fiber revokes tool liveness first; the lease then
    /// physically removes tools, commands and panels from their registries.
    pub(crate) fn apply_with_fibers(
        &self,
        agent: &mut ::agent::Agent,
        registry: &mut ::agent::plugins::PluginRegistry,
        preset_allowed: Option<&BTreeSet<String>>,
        session_id: &str,
    ) -> Vec<AppliedExtension> {
        let mut applied = Vec::new();
        let service_capabilities = self.service_capability_map();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            if preset_allowed.is_some_and(|allowed| !allowed.contains(entry.package.id().as_str()))
            {
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
                || !contributions.contexts.is_empty()
                || !entry.package.manifest.contributes.skills.is_empty()
                || !entry.package.manifest.services.provides.is_empty()
                || !entry.package.manifest.services.requires.is_empty();
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
                workspace: self.config.workspace.clone(),
                skills_dir: runtime.skills_dir().or_else(|| {
                    (!entry.package.manifest.contributes.skills.is_empty())
                        .then(|| entry.package.root.join("skills"))
                }),
                workflows: contributions.workflows.clone(),
                themes: contributions.themes.clone(),
                timeout_ms: entry.package.manifest.resources.execution_ms(),
            };
            let (info, lease) = contribution::install_with_lease(
                agent,
                registry,
                &contributed,
                Arc::clone(&self.policy),
                Arc::clone(&self.events),
                self.config.workspace.clone(),
            );
            let extension_id = entry.package.id().as_str().to_owned();
            let authorizer = Arc::new(ServicePolicyAuthorizer {
                policy: Arc::clone(&self.policy),
                consumer: extension_id.clone(),
                trust: entry.package.trust,
                capabilities: service_capabilities.clone(),
                workspace: self.config.workspace.clone(),
            });
            let context = Context::root_authorized(
                Arc::clone(&self.services),
                Scope::Extension(extension_id.clone()),
                extension_id,
                Some(authorizer),
            );
            let mut fiber = Fiber::with_context(entry.package.id().as_str(), context);
            if let Err(error) = fiber.transition(FiberState::Loading) {
                log::error!(
                    "extension {}: Fiber could not start: {error}",
                    entry.package.id()
                );
                let _ = lease.revoke(agent, registry);
                continue;
            }
            lease.attach_to_fiber(&mut fiber);
            if let Err(error) = fiber.transition(FiberState::Active) {
                log::error!(
                    "extension {}: Fiber could not activate: {error}",
                    entry.package.id()
                );
                let _ = fiber.unload();
                let _ = lease.revoke(agent, registry);
                continue;
            }
            applied.push(AppliedExtension {
                info,
                fiber,
                lease,
                services: Arc::clone(&self.services),
                extension_id: entry.package.id().as_str().to_owned(),
                workspace: self.config.workspace.clone(),
                session_id: session_id.to_owned(),
                service_provides: entry.package.manifest.services.provides.clone(),
                service_requires: entry.package.manifest.services.requires.clone(),
            });
        }
        applied
    }

    /// Every known extension as a status row.
    pub(crate) fn statuses(&self) -> Vec<ExtensionStatus> {
        self.loaded
            .values()
            .map(|entry| self.status_of(entry))
            .collect()
    }

    /// The composition graph as declared by the loaded packages.
    pub(crate) fn graph(&self) -> Vec<ExtensionGraphNode> {
        self.loaded
            .values()
            .map(|entry| {
                let manifest = &entry.package.manifest;
                let provides = manifest
                    .services
                    .provides
                    .iter()
                    .map(|service| {
                        format!("{}@{}[{:?}]", service.name, service.version, service.scope)
                    })
                    .collect();
                let requires = manifest
                    .services
                    .requires
                    .iter()
                    .map(|service| {
                        format!(
                            "{}@{}{}[{:?}]",
                            service.name,
                            service.version,
                            if service.optional { "?" } else { "" },
                            service.scope
                        )
                    })
                    .collect();
                let mut effects = Vec::new();
                for (kind, ids) in [
                    ("tool", &manifest.contributes.tools),
                    ("command", &manifest.contributes.commands),
                    ("panel", &manifest.contributes.panels),
                    ("prompt", &manifest.contributes.prompts),
                    ("context", &manifest.contributes.contexts),
                    ("skill", &manifest.contributes.skills),
                    ("workflow", &manifest.contributes.workflows),
                    ("language", &manifest.contributes.languages),
                    ("integration", &manifest.contributes.integrations),
                    ("model", &manifest.contributes.model_providers),
                    ("agent", &manifest.contributes.agent_providers),
                    ("theme", &manifest.contributes.themes),
                    ("preset", &manifest.contributes.presets),
                ] {
                    for id in ids {
                        effects.push(format!("{kind}:{id}"));
                    }
                }
                for topic in &manifest.contributes.events {
                    effects.push(format!("event:{topic}"));
                }
                for topic in &manifest.contributes.publishes {
                    effects.push(format!("publish:{topic}"));
                }
                for hook in &manifest.contributes.hooks {
                    effects.push(format!("hook:{hook}"));
                }
                for service in &manifest.services.provides {
                    effects.push(format!("service:{}", service.name));
                }
                ExtensionGraphNode {
                    id: entry.package.id().as_str().to_owned(),
                    state: self
                        .manager
                        .get(entry.package.id().as_str())
                        .map(|record| record.state.as_str().to_owned())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    runtime: manifest.runtime.kind.as_str().to_owned(),
                    enabled: entry.configured,
                    applied: entry.applied,
                    dependencies: manifest
                        .dependencies
                        .keys()
                        .map(|id| id.as_str().to_owned())
                        .collect(),
                    provides,
                    requires,
                    effects,
                    live_suspended: super::live::is_suspended(entry.package.id().as_str()),
                    error: entry.error.clone(),
                }
            })
            .collect()
    }

    /// Capabilities a consumer must hold for each declared service name.
    fn service_capability_map(&self) -> BTreeMap<String, Vec<Capability>> {
        let mut capabilities = BTreeMap::new();
        for entry in self.loaded.values() {
            for provide in &entry.package.manifest.services.provides {
                capabilities
                    .entry(provide.name.clone())
                    .or_insert_with(Vec::new)
                    .extend(provide.capabilities.iter().copied());
            }
        }
        capabilities
            .entry("terminal".to_string())
            .or_default()
            .push(Capability::ProcessExecute);
        capabilities
    }

    /// Explain why one extension is or is not applied.
    pub(crate) fn why(&self, id: &str) -> Option<String> {
        let entry = self.loaded.get(id)?;
        if entry.applied {
            if super::live::is_suspended(entry.package.id().as_str()) {
                return Some(
                    "applied; live overlay suspended because a required service is missing"
                        .to_string(),
                );
            }
            return Some("applied".to_string());
        }
        if let Some(error) = &entry.error {
            return Some(format!("not applied: {error}"));
        }
        if !entry.configured {
            return Some("not applied: disabled by configuration".to_string());
        }
        Some(format!(
            "not applied: startup policy is {:?}",
            entry.package.manifest.runtime.startup
        ))
    }

    /// Language services from every applied extension.
    pub(crate) fn languages(&self) -> &[LanguageEntry] {
        &self.languages
    }

    /// External-system integrations from every applied extension.
    pub(crate) fn integrations(&self) -> &[IntegrationEntry] {
        &self.integrations
    }

    /// Run one language service's declared diagnostics tool over a document.
    pub(crate) async fn language_check(
        &self,
        owner: &str,
        language: &str,
        path: &Path,
    ) -> Result<Value, ExtensionError> {
        let entry = self
            .languages
            .iter()
            .find(|entry| {
                entry.owner.as_str() == owner
                    && (entry.contribution.id.as_str() == language
                        || entry.contribution.name == language)
            })
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "language.check".to_string(),
                reason: format!("{language} is not a declared language"),
            })?;
        let text = std::fs::read_to_string(path).map_err(|source| ExtensionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if text.len() > 1024 * 1024 {
            return Err(ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "language.check".to_string(),
                reason: "document is larger than 1 MiB".to_string(),
            });
        }
        let loaded = self
            .loaded
            .get(owner)
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "language.check".to_string(),
                reason: "extension is not loaded".to_string(),
            })?;
        let runtime = loaded
            .runtime
            .clone()
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "language.check".to_string(),
                reason: "extension has no runtime".to_string(),
            })?;
        let context = CallContext::new(
            entry.owner.clone(),
            "language-check",
            self.config.workspace.clone(),
            30_000,
        );
        let arguments = serde_json::json!({
            "path": path.display().to_string(),
            "text": text,
            "language": entry.contribution.id.as_str(),
        });
        let tool = entry.contribution.tool.as_str();
        let descriptor = loaded
            .tools
            .iter()
            .find(|descriptor| descriptor.id.as_str() == tool)
            .ok_or_else(|| ExtensionError::NotFound {
                id: format!("{owner}::{tool}"),
            })?;
        let scope =
            bos_ext_core::capability::call_scope(&arguments, self.config.workspace.as_deref());
        bos_ext_core::capability::authorize_each(
            &self.policy,
            owner,
            loaded.package.trust,
            &descriptor.capabilities,
            &scope,
        )?;
        runtime.call_tool(tool, arguments, &context).await
    }

    /// Dispatch one integration's declared tool or command.
    pub(crate) async fn run_integration(
        &self,
        owner: &str,
        integration: &str,
        arguments: Value,
    ) -> Result<Value, ExtensionError> {
        let entry = self
            .integrations
            .iter()
            .find(|entry| {
                entry.owner.as_str() == owner
                    && (entry.contribution.id.as_str() == integration
                        || entry.contribution.name == integration)
            })
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "integration.run".to_string(),
                reason: format!("{integration} is not a declared integration"),
            })?;
        let loaded = self
            .loaded
            .get(owner)
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "integration.run".to_string(),
                reason: "extension is not loaded".to_string(),
            })?;
        let runtime = loaded
            .runtime
            .clone()
            .ok_or_else(|| ExtensionError::Denied {
                extension: owner.to_string(),
                capability: "integration.run".to_string(),
                reason: "extension has no runtime".to_string(),
            })?;
        let action = entry.contribution.action.as_str();
        if let Some(descriptor) = loaded
            .tools
            .iter()
            .find(|descriptor| descriptor.id.as_str() == action)
        {
            let scope =
                bos_ext_core::capability::call_scope(&arguments, self.config.workspace.as_deref());
            bos_ext_core::capability::authorize_each(
                &self.policy,
                owner,
                loaded.package.trust,
                &descriptor.capabilities,
                &scope,
            )?;
            let context = CallContext::new(
                entry.owner.clone(),
                "integration-run",
                self.config.workspace.clone(),
                60_000,
            );
            runtime.call_tool(action, arguments, &context).await
        } else {
            runtime.command(action, arguments).await
        }
    }

    /// Stop every loaded runtime and end the delivery thread.
    pub(crate) async fn shutdown(&mut self) {
        self.pump_generation.fetch_add(1, Ordering::SeqCst);
        let runtimes: Vec<Arc<dyn Runtime>> = self
            .loaded
            .values()
            .filter_map(|entry| entry.runtime.clone())
            .collect();
        self.loaded.clear();
        // Stop them together: sequential shutdown paid every process's
        // one-second kill deadline in turn, so a machine with four process
        // packages hung the closing window for four seconds.
        let stops = runtimes.into_iter().map(|runtime| async move {
            let _ = runtime.shutdown().await;
        });
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            futures::future::join_all(stops),
        )
        .await;
        if let Ok(mut slot) = bus_slot().write() {
            *slot = None;
        }
    }
    /// Clear a persisted failure and reload the package set.
    /// Apply a Cordis/profile patch file to installed packages and reload them.
    ///
    /// Rows name a package with package:<id> (or their own id). A row config
    /// is written to that package's extension config before the host rebuilds,
    /// so lowering a patch row is an actual mount, not only a plan.
    pub(crate) fn mount_profile(&mut self, path: &Path) -> Result<Vec<String>, ExtensionError> {
        let entries = bos_ext_core::composable::plan::load_cordis_patch(path, "profile").map_err(
            |error| ExtensionError::Runtime {
                extension: "profile".to_string(),
                message: error.to_string(),
            },
        )?;
        let candidates: Vec<_> = entries
            .iter()
            .filter(|entry| entry.enabled && !entry.group)
            .filter_map(|entry| {
                let wanted = entry
                    .source
                    .split_once('#')
                    .map(|(_, name)| name)
                    .unwrap_or(entry.id.as_str());
                let loaded = self.loaded.get(entry.id.as_str()).or_else(|| {
                    self.loaded.values().find(|loaded| {
                        loaded.package.manifest.extension.name == wanted
                            || loaded.package.id().as_str() == wanted
                            || loaded.package.id().as_str().ends_with(entry.id.as_str())
                    })
                })?;
                Some((
                    loaded.package.id().as_str().to_string(),
                    loaded.package.manifest.clone(),
                    loaded.package.root.clone(),
                    entry.config.clone(),
                ))
            })
            .collect();
        let mut mounted = Vec::new();
        for (id, manifest, root, config) in candidates {
            if let Some(config) = config {
                let id = bos_ext_core::types::ExtensionId::parse(id.clone())?;
                bos_ext_core::config::save(&manifest, &root, &self.store.data_dir(&id), config)?;
            }
            self.retry(&id)?;
            mounted.push(id);
        }
        Ok(mounted)
    }

    pub(crate) fn retry(&mut self, id: &str) -> Result<(), ExtensionError> {
        let id = bos_ext_core::types::ExtensionId::parse(id.to_string())?;
        self.store.clear_failure(&id)?;
        self.rebuild()
    }

    /// Grant one process executable and reload the package set.
    pub(crate) fn approve_process(
        &mut self,
        id: &str,
        program: &str,
    ) -> Result<(), ExtensionError> {
        self.policy.grant(
            id,
            bos_ext_core::capability::Capability::ProcessExecute,
            bos_ext_core::capability::GrantScope::Program(program.to_string()),
            Some("approved from the extensions pane".to_string()),
        )?;
        self.rebuild()
    }

    /// Grant every required capability a currently denied package asks for.
    ///
    /// This is the explicit UI decision behind the Installed row's Grant
    /// button. The button is only shown when a required capability is denied;
    /// each grant is written with the narrowest default scope available here
    /// (workspace for path capabilities, declared program for process, any for
    /// the rest) and is audited with the pane as its note.
    pub(crate) fn grant_required(&mut self, id: &str) -> Result<usize, ExtensionError> {
        let status = self
            .statuses()
            .into_iter()
            .find(|status| status.id == id)
            .ok_or_else(|| ExtensionError::NotFound { id: id.to_string() })?;
        let program = status.program.clone();
        let mut granted = 0usize;
        for capability in status
            .capabilities
            .iter()
            .filter(|item| item.required && !item.allowed)
        {
            let parsed: Capability = capability.capability.parse()?;
            let scope = match parsed {
                Capability::WorkspaceRead | Capability::WorkspaceWrite => GrantScope::Workspace,
                Capability::ProcessExecute => {
                    GrantScope::Program(program.clone().ok_or_else(|| ExtensionError::Denied {
                        extension: id.to_string(),
                        capability: capability.capability.clone(),
                        reason: "process extension has no declared executable".to_string(),
                    })?)
                }
                // A secret or a host cannot be named from this pane, and
                // granting "any" would be wider than the standard's
                // secret:NAME / domain: scope. Leave the denial; its reason
                // already shows the exact grant command and scope to use.
                Capability::SecretsRead | Capability::NetworkHttp => continue,
                _ => GrantScope::Any,
            };
            self.policy.grant(
                id,
                parsed,
                scope,
                Some("granted from the extensions pane".to_string()),
            )?;
            granted += 1;
        }
        if granted > 0 {
            self.rebuild()?;
        }
        Ok(granted)
    }

    /// Every panel an applied extension offers, ready for the host UI.
    pub(crate) fn panel_surfaces(&self) -> Vec<PanelSurface> {
        let mut surfaces = Vec::new();
        for entry in self.loaded.values() {
            if !entry.applied
                || entry.error.is_some()
                || super::live::is_suspended(entry.package.id().as_str())
            {
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
                    policy: Arc::clone(&self.policy),
                    trust: entry.package.trust,
                    tools: Arc::new(entry.tools.clone()),
                    workspace: self.config.workspace.clone(),
                    root: entry.package.root.clone(),
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
                    policy: Arc::clone(&self.policy),
                    trust: entry.package.trust,
                    tools: Arc::new(entry.tools.clone()),
                    workspace: self.config.workspace.clone(),
                    root: entry.package.root.clone(),
                });
            }
        }
        surfaces
    }

    /// Every theme a package contributes, keyed by a host-owned id.
    pub(crate) fn theme_surfaces(&self) -> BTreeMap<String, bos_ext_core::ui::ThemePalette> {
        let mut themes = BTreeMap::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            for theme in &entry.contributions.themes {
                let Ok(text) = std::fs::read_to_string(&theme.path) else {
                    continue;
                };
                if let Ok(palette) = serde_json::from_str::<bos_ext_core::ui::ThemePalette>(&text) {
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
            if !entry.applied
                || entry.error.is_some()
                || super::live::is_suspended(entry.package.id().as_str())
            {
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
        let calls = CALLS.lock().map(|calls| *calls).unwrap_or_default();
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
            memory_limit_bytes: self
                .loaded
                .values()
                .filter_map(|entry| entry.runtime.as_ref())
                .filter_map(|runtime| runtime.memory_limit())
                .sum(),
            memory_used_bytes: self
                .loaded
                .values()
                .filter_map(|entry| entry.runtime.as_ref())
                .filter_map(|runtime| runtime.memory_used())
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
        let mut scope = self.host_scope();
        if entry.package.manifest.runtime.kind == RuntimeKind::Process {
            scope.program = entry.package.manifest.runtime.entry.clone();
        }
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
                required: entry
                    .package
                    .manifest
                    .capabilities
                    .required
                    .contains(capability),
            });
        }
        ExtensionStatus {
            id: id.as_str().to_string(),
            name: entry.package.manifest.extension.name.clone(),
            version: entry.package.manifest.extension.version.to_string(),
            runtime: entry.package.manifest.runtime.kind.as_str().to_string(),
            program: (entry.package.manifest.runtime.kind == RuntimeKind::Process)
                .then(|| entry.package.manifest.runtime.entry.clone())
                .flatten(),
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
            skills: entry
                .package
                .manifest
                .contributes
                .skills
                .iter()
                .map(|skill| skill.as_str().to_string())
                .collect(),
            languages: entry
                .contributions
                .languages
                .iter()
                .map(|language| language.name.clone())
                .collect(),
            integrations: entry
                .contributions
                .integrations
                .iter()
                .map(|integration| integration.name.clone())
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
                        bos_ext_core::signing::SignatureVerification::Verified { publisher } => {
                            issues.push(DiagnosticIssue::new(
                                "info",
                                Some(id),
                                format!("package signature verified for publisher {publisher}"),
                            ));
                        }
                        bos_ext_core::signing::SignatureVerification::Untrusted {
                            publisher,
                            reason,
                        } => issues.push(DiagnosticIssue::new(
                            "warning",
                            Some(id),
                            format!("signature by {publisher} is untrusted: {reason}"),
                        )),
                        bos_ext_core::signing::SignatureVerification::Invalid { reason } => {
                            issues.push(DiagnosticIssue::new(
                                "error",
                                Some(id),
                                format!("package signature is invalid: {reason}"),
                            ));
                        }
                        bos_ext_core::signing::SignatureVerification::Missing => {
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

/// The live event bus, published outside the host lock.
///
/// `publish` is called from UI and agent threads while a build may hold the host
/// lock during `apply`; contending on that lock stalled the publisher behind
/// skills-dir reads and context globs.
static BUS: OnceLock<std::sync::RwLock<Option<Arc<EventBus>>>> = OnceLock::new();

fn bus_slot() -> &'static std::sync::RwLock<Option<Arc<EventBus>>> {
    BUS.get_or_init(|| std::sync::RwLock::new(None))
}

/// Tool-call counters, also outside the host lock: `record_tool_call` runs on
/// every tool completion and must not wait for a build.
static CALLS: Mutex<CallMetrics> = Mutex::new(CallMetrics {
    tool_calls: 0,
    tool_failures: 0,
    total_call_ms: 0,
    max_call_ms: 0,
    output_bytes: 0,
});

/// Install every enabled extension into one agent build.
///
/// This is the only function the rest of BSH needs. It never panics and never
/// fails a build: a broken extension is logged and reported through the status
/// and doctor surfaces, not by making the harness unusable.
pub(crate) fn install_with_fibers(
    agent: &mut ::agent::Agent,
    registry: &mut ::agent::plugins::PluginRegistry,
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    preset_allowed: Option<&[String]>,
    session_id: &str,
) -> Vec<AppliedExtension> {
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
    let allowed: Option<BTreeSet<String>> =
        preset_allowed.map(|ids| ids.iter().cloned().collect::<BTreeSet<_>>());
    guard
        .as_ref()
        .map(|host| host.apply_with_fibers(agent, registry, allowed.as_ref(), session_id))
        .unwrap_or_default()
}

/// Whether any applied package declares an observe hook point.
pub(crate) fn hook_attached() -> bool {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return false;
    };
    guard.as_ref().is_some_and(|host| {
        host.loaded
            .values()
            .any(|entry| entry.applied && !entry.package.manifest.contributes.hooks.is_empty())
    })
}

/// Runtimes attached to one hook point on the process-global host.
pub(crate) fn hook_targets(point: &str) -> Vec<HookTarget> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return Vec::new();
    };
    guard
        .as_ref()
        .map(|host| host.hook_targets(point))
        .unwrap_or_default()
}

/// Publish a host event on the process-global bus.
///
/// The host never invents a subscriber: a package that did not subscribe
/// receives nothing, and a full queue is counted rather than blocking the
/// publisher.
pub(crate) fn publish(topic: &str, payload: serde_json::Value) {
    let bus = bus_slot().read().ok().and_then(|slot| slot.clone());
    let Some(bus) = bus else {
        return;
    };
    if let Ok(event) = ExtensionEvent::new("bsh", topic, payload) {
        if let Err(error) = bus.publish(&event) {
            log::debug!(target: "bsh::extension", "event publish failed: {error}");
        }
    }
}

/// Stop the process-global host and every runtime it owns.
///
/// Called when the window closes: the extension host is a process-global cache,
/// so dropping the shell is not enough to release process children or WASM
/// stores. The delivery thread also stops because the generation moved.
pub(crate) fn shutdown() {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Some(mut host) = slot.lock().ok().and_then(|mut guard| guard.take()) else {
        return;
    };
    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        runtime.block_on(host.shutdown());
    }
}

/// Record one extension tool call on the process-global host.
pub(crate) fn record_tool_call(millis: u64, bytes: u64, ok: bool) {
    if let Ok(mut calls) = CALLS.lock() {
        calls.tool_calls = calls.tool_calls.saturating_add(1);
        if !ok {
            calls.tool_failures = calls.tool_failures.saturating_add(1);
        }
        calls.total_call_ms = calls.total_call_ms.saturating_add(millis);
        calls.max_call_ms = calls.max_call_ms.max(millis);
        calls.output_bytes = calls.output_bytes.saturating_add(bytes);
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
) -> BTreeMap<String, bos_ext_core::ui::ThemePalette> {
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

/// The status rows the current configuration offers, for the settings pane.
pub(crate) fn statuses(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
) -> Vec<ExtensionStatus> {
    with_host(settings, workspace, |host| host.statuses()).unwrap_or_default()
}

/// Approve one process executable from the settings pane.
pub(crate) fn approve_process(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    id: &str,
    program: &str,
) -> Result<(), ExtensionError> {
    with_host(settings, workspace, |host| {
        host.approve_process(id, program)
    })
    .unwrap_or_else(|| {
        Err(ExtensionError::NotFound {
            id: "extension host".to_string(),
        })
    })
}

/// Grant every required capability a package currently lacks.
pub(crate) fn grant_required(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    id: &str,
) -> Result<usize, ExtensionError> {
    with_host(settings, workspace, |host| host.grant_required(id)).unwrap_or_else(|| {
        Err(ExtensionError::NotFound {
            id: "extension host".to_string(),
        })
    })
}

/// Run a closure against the cached host, opening or refreshing it first.
fn with_host<R>(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    f: impl FnOnce(&mut ExtensionHost) -> R,
) -> Option<R> {
    let config = HostConfig::from_settings(settings, workspace.map(Path::to_path_buf), None);
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().ok()?;
    let reopen = guard.as_ref().is_none_or(|host| host.config != config);
    if reopen {
        match ExtensionHost::open(config) {
            Ok(host) => *guard = Some(host),
            Err(error) => {
                log::warn!("extension host could not open: {error}");
                return None;
            }
        }
    } else if let Some(host) = guard.as_mut() {
        if let Err(error) = host.ensure_fresh() {
            log::warn!("extension host could not refresh: {error}");
        }
    }
    guard.as_mut().map(f)
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

/// The manifest of the first-party workspace-annotation provider.
const ANNOTATIONS_MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.bos.bsh.annotations"
name = "Workspace Annotations"
version = "1.0.0"
description = "Owns the workspace note store used by the file annotations pane"
authors = ["BrickOS Team"]
license = "MIT"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "annotations"
startup = "on-startup"
"#;

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

/// The manifest of the built-in extension diagnostics package.
const DIAGNOSTICS_MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.bos.bsh.diagnostics"
name = "Extension Diagnostics"
version = "1.0.0"
description = "Graph and why diagnostics for the extension composition"
authors = ["BrickOS Team"]
license = "MIT"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "diagnostics"
startup = "on-demand"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["graph", "why"]
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use bos_ext_core::capability::GrantScope;
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

    fn write_language_package(root: &Path) {
        for directory in ["tools", "languages", "integrations"] {
            std::fs::create_dir_all(root.join(directory)).expect(directory);
        }
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.language"
name = "Language"
version = "1.0.0"
description = "A language and integration fixture"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["check"]
languages = ["rust"]
integrations = ["github"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("tools/check.json"),
            r#"{"description":"Check","implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
        std::fs::write(
            root.join("languages/rust.json"),
            r#"{"name":"Rust","extensions":["rs"],"tool":"check"}"#,
        )
        .expect("language");
        std::fs::write(
            root.join("integrations/github.json"),
            r#"{"name":"GitHub","kind":"github","action":"check"}"#,
        )
        .expect("integration");
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

    #[tokio::test]
    async fn language_and_integration_descriptors_dispatch() {
        let root = scratch("language-integration");
        let source = root.join("source");
        write_language_package(&source);
        let store = ExtensionStore::open(&root).expect("store");
        let package = store.install(&source).expect("install");
        let policy =
            PolicyEngine::open(store.grants_path(), store.audit_path(), PolicyMode::Enforce)
                .expect("policy");
        for capability in &package.manifest.capabilities.required {
            policy
                .grant(package.id().as_str(), *capability, GrantScope::Any, None)
                .expect("grant");
        }
        drop(policy);
        let host = ExtensionHost::open(config(&root)).expect("host");
        let document = root.join("main.rs");
        std::fs::write(&document, "fn main() {}").expect("document");
        let checked = host
            .language_check("com.example.language", "rust", &document)
            .await
            .expect("language dispatch");
        assert_eq!(checked["language"], "rust");
        assert_eq!(checked["text"], "fn main() {}");
        let integrated = host
            .run_integration(
                "com.example.language",
                "github",
                serde_json::json!({"q": 1}),
            )
            .await
            .expect("integration dispatch");
        assert_eq!(integrated["q"], 1);
    }

    #[tokio::test]
    async fn the_hook_demo_receives_a_live_host_event() {
        let root = scratch("hook-demo-e2e");
        std::fs::create_dir_all(root.join("workspace")).expect("workspace");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hook-demo");
        let store = ExtensionStore::open(&root).expect("store");
        let package = store.install(&source).expect("install");
        let policy =
            PolicyEngine::open(store.grants_path(), store.audit_path(), PolicyMode::Enforce)
                .expect("policy");
        for capability in &package.manifest.capabilities.required {
            policy
                .grant(package.id().as_str(), *capability, GrantScope::Any, None)
                .expect("grant");
        }
        drop(policy);
        let mut host = ExtensionHost::open(config(&root)).expect("host");
        let document = root.join("main.rs");
        std::fs::write(&document, "fn main() {}").expect("document");
        host.language_check("com.example.hook-demo", "hookdemo", &document)
            .await
            .expect("language check starts the process");
        for _ in 0..40 {
            if host.events().subscriber_count() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            host.events().subscriber_count() > 0,
            "the process subscribed before the event"
        );
        let event = ExtensionEvent::new(
            "bsh",
            topics::SESSION_CREATED,
            serde_json::json!({"session": "e2e"}),
        )
        .expect("event");
        let mut log = serde_json::Value::Null;
        for _ in 0..40 {
            // The builtin inspector subscribes before the process runtime does;
            // republishing until the process log names the event proves the
            // process's own subscription rather than any subscriber at all.
            host.events().publish(&event).expect("publish");
            log = host
                .run_integration("com.example.hook-demo", "github", serde_json::json!({}))
                .await
                .expect("integration dispatch");
            if log["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["topic"] == "session.created")
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            log["events"].as_array().is_some_and(|events| events
                .iter()
                .any(|event| event["topic"] == "session.created")),
            "session.created was not forwarded: {log}"
        );
        assert!(
            host.events().subscriber_count() > 0,
            "the runtime subscribed before shutdown"
        );
        host.shutdown().await;
        assert!(host.loaded.is_empty(), "shutdown releases every runtime");
        assert_eq!(
            host.events().subscriber_count(),
            0,
            "dropping the runtimes releases their subscriptions"
        );
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
        assert!(
            host.metrics().memory_limit_bytes > 0,
            "the loaded runtimes report their memory ceilings"
        );
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
            metrics.applied, 4,
            "the builtin inspector, diagnostics, annotations and good package still apply"
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
    fn disabling_the_builtin_inspector_removes_its_tool() {
        let root = scratch("builtin-disabled");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let id = ExtensionId::parse("com.bos.bsh.extensions").expect("id");
        host.store().set_enabled(&id, false).expect("disable");
        drop(host);

        let host = ExtensionHost::open(config(&root)).expect("reopen");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.bos.bsh.extensions")
            .expect("inspector");
        assert!(!status.configured);
        // The descriptor is still discovered, but it is not applied to an agent,
        // so the tool is not reachable.
        assert!(!status.applied);
        assert_ne!(status.state, LifecycleState::Active);
        assert!(host
            .why("com.bos.bsh.extensions")
            .is_some_and(|why| why.contains("disabled")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn disabling_the_builtin_diagnostics_removes_its_tools() {
        let root = scratch("diagnostics-disabled");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let id = ExtensionId::parse("com.bos.bsh.diagnostics").expect("id");
        host.store().set_enabled(&id, false).expect("disable");
        drop(host);

        let host = ExtensionHost::open(config(&root)).expect("reopen");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.bos.bsh.diagnostics")
            .expect("diagnostics");
        assert!(!status.configured);
        assert!(!status.applied);
        assert_ne!(status.state, LifecycleState::Active);
        assert!(host
            .why("com.bos.bsh.diagnostics")
            .is_some_and(|why| why.contains("disabled")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_host_terminal_service_runs_a_command() {
        let root = scratch("terminal");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let lease = host
            .services()
            .acquire::<crate::extension::terminal::TerminalHandle>(&Scope::Host, "terminal")
            .expect("terminal service");
        let output = lease.run("printf hello", None).expect("command");
        assert_eq!(output.stdout, "hello");
        assert_eq!(output.code, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn service_acquisition_checks_declared_capabilities() {
        let root = scratch("policy-authorizer");
        let policy = Arc::new(
            PolicyEngine::open(
                root.join("grants.toml"),
                root.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let mut capabilities = BTreeMap::new();
        capabilities.insert("search".to_string(), vec![Capability::SecretsRead]);
        let authorizer = ServicePolicyAuthorizer {
            policy: Arc::clone(&policy),
            consumer: "com.example.consumer".to_string(),
            trust: TrustLevel::Community,
            capabilities,
            workspace: Some(root.clone()),
        };

        assert!(authorizer
            .authorize("com.example.consumer", &Scope::Host, "search")
            .is_err());
        policy
            .grant(
                "com.example.consumer",
                Capability::SecretsRead,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        assert!(authorizer
            .authorize("com.example.consumer", &Scope::Host, "search")
            .is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_builtin_diagnostics_is_registered_and_applied() {
        let root = scratch("diagnostics");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.bos.bsh.diagnostics")
            .expect("diagnostics");
        assert!(status.applied);
        assert!(status.tools.iter().any(|tool| tool.ends_with("__graph")));
        assert!(status.tools.iter().any(|tool| tool.ends_with("__why")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn interceptor_registration_is_audited_and_capability_gated() {
        let root = scratch("interceptor-audit");
        let policy = Arc::new(
            PolicyEngine::open(
                root.join("grants.toml"),
                root.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let interceptor = PolicyInterceptor {
            policy: Arc::clone(&policy),
            owner: "com.example.interceptor".to_string(),
            trust: TrustLevel::Community,
            workspace: Some(root.clone()),
        };
        assert!(interceptor
            .authorize("com.example.interceptor", "tool.before")
            .is_err());
        assert!(
            interceptor
                .authorize("com.example.other", "tool.before")
                .is_err(),
            "activation-bound policy must reject a different owner"
        );
        let audit = std::fs::read_to_string(root.join("audit.jsonl")).unwrap_or_default();
        assert!(audit.contains("agent.intercept"), "audit: {audit}");

        policy
            .grant(
                "com.example.interceptor",
                Capability::AgentIntercept,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        assert!(interceptor
            .authorize("com.example.interceptor", "tool.before")
            .is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn global_interceptor_policy_uses_the_requesting_owner() {
        let root = scratch("global-interceptor-owner");
        let policy = Arc::new(
            PolicyEngine::open(
                root.join("grants.toml"),
                root.join("audit.jsonl"),
                PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let interceptor = PolicyInterceptor {
            policy: Arc::clone(&policy),
            owner: String::new(),
            trust: TrustLevel::Community,
            workspace: Some(root.clone()),
        };

        assert!(interceptor
            .authorize("com.example.requester", "tool.before")
            .is_err());
        policy
            .grant(
                "com.example.requester",
                Capability::AgentIntercept,
                GrantScope::Any,
                None,
            )
            .expect("grant");
        assert!(interceptor
            .authorize("com.example.requester", "tool.before")
            .is_ok());

        let audit = std::fs::read_to_string(root.join("audit.jsonl")).unwrap_or_default();
        assert!(
            audit.contains("com.example.requester"),
            "audit must attribute the request to its actual owner: {audit}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn graph_and_why_report_the_live_inventory() {
        let root = scratch("graph");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let graph = host.graph();
        let node = graph
            .iter()
            .find(|node| node.id == "com.bos.bsh.extensions")
            .expect("inspector node");
        assert!(node.applied);
        assert_eq!(node.runtime, "builtin");
        assert_eq!(
            host.why("com.bos.bsh.extensions").as_deref(),
            Some("applied")
        );
        assert!(host.why("com.example.missing").is_none());
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
    fn required_service_contract_is_enforced_before_activation() {
        let root = scratch("missing-service-provider");
        let source = root.join("source");
        write_package(&source);
        use std::io::Write as _;
        let mut manifest = std::fs::OpenOptions::new()
            .append(true)
            .open(source.join("extension.toml"))
            .expect("open manifest");
        manifest
            .write_all(
                br#"
[services]
requires = [{ name = "com.example.search", contract = "com.example.search-api", version = "^1.0", scope = "host" }]
"#,
            )
            .expect("append service requirement");
        let store = ExtensionStore::open(root.join("extensions")).expect("store");
        store.install(&source).expect("install");
        let host = ExtensionHost::open(config(&root.join("extensions"))).expect("host");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.hosted")
            .expect("hosted extension remains visible");
        assert!(
            !status.applied,
            "a missing required service must block activation"
        );
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|error| error.contains("service dependency planning")),
            "{status:?}"
        );
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
        // The mismatch is refused at load, not only reported by a command the
        // reader has to remember to run.
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.hosted")
            .expect("the tampered package is still listed");
        assert!(!status.applied, "{status:?}");
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|error| error.contains("hash")),
            "{status:?}"
        );
        let report = host.doctor();
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.message.contains("hash")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_preset_allowlist_filters_application_without_reopening_the_host() {
        let root = scratch("preset-builtin");
        let id = ExtensionId::parse("com.bos.bsh.extensions").expect("id");
        let mut narrowed = config(&root);
        narrowed.preset_allowed = Some(BTreeSet::new());
        // The store keeps the package enabled; the preset decides whether this
        // build applies it.
        assert!(narrowed.allows(&id));
        assert!(!narrowed.preset_allows(&id));
        // The cache key ignores the preset: otherwise every status read with a
        // preset allowlist reopened the host and tore down live runtimes.
        let mut plain = config(&root);
        plain.preset_allowed = None;
        assert_eq!(narrowed, plain);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_guarded_package(root: &Path) {
        for directory in ["tools", "languages"] {
            std::fs::create_dir_all(root.join(directory)).expect(directory);
        }
        std::fs::write(
            root.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.guarded"
name = "Guarded"
version = "1.0.0"
description = "A language tool that needs a secret"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[contributes]
tools = ["check"]
languages = ["rust"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            root.join("tools/check.json"),
            r#"{"description":"Check","capabilities":["secrets.read"],"implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
        std::fs::write(
            root.join("languages/rust.json"),
            r#"{"name":"Rust","extensions":["rs"],"tool":"check"}"#,
        )
        .expect("language");
    }

    #[tokio::test]
    async fn a_language_check_is_authorized_like_a_tool_call() {
        let root = scratch("language-denied");
        let source = root.join("source");
        write_guarded_package(&source);
        let store = ExtensionStore::open(&root).expect("store");
        store.install(&source).expect("install");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let document = root.join("main.rs");
        std::fs::write(&document, "fn main() {}").expect("document");
        let error = host
            .language_check("com.example.guarded", "rust", &document)
            .await
            .expect_err("secrets.read is not granted");
        assert!(
            matches!(error, ExtensionError::Denied { .. }),
            "the non-agent dispatch path must authorize: {error}"
        );
    }

    #[test]
    fn grant_required_never_widens_a_secret_or_host_scope() {
        let root = scratch("grant-narrow");
        let source = root.join("source");
        std::fs::create_dir_all(source.join("tools")).expect("tools");
        std::fs::write(
            source.join("extension.toml"),
            r#"
schema_version = "1.0"

[extension]
id = "com.example.secretive"
name = "Secretive"
version = "1.0.0"
description = "Needs a secret and a host"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "declarative"

[capabilities]
required = ["secrets.read", "network.http"]

[contributes]
tools = ["echo"]
"#,
        )
        .expect("manifest");
        std::fs::write(
            source.join("tools/echo.json"),
            r#"{"description":"Echo","implementation":{"kind":"echo_arguments"}}"#,
        )
        .expect("tool");
        let store = ExtensionStore::open(&root).expect("store");
        store.install(&source).expect("install");
        let mut host = ExtensionHost::open(config(&root)).expect("host");
        let granted = host
            .grant_required("com.example.secretive")
            .expect("grant attempt");
        assert_eq!(
            granted, 0,
            "a scope-free grant would be wider than secret:NAME / domain:host"
        );
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.secretive")
            .expect("status");
        assert!(!status.applied, "{status:?}");
    }
}
