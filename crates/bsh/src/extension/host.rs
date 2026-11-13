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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::SystemTime;

use serde::Serialize;

use super::agent as contribution;
use ferris_sdk::capability::{
    CallScope, Capability, Decision, GrantScope, PolicyEngine, PolicyMode,
};
use ferris_sdk::composable::dispatch::InterceptorPolicy;
use ferris_sdk::composable::{
    Context, DisposalReport, Fiber, FiberState, Scope, ServiceAuthorizer, ServiceToken,
};
use ferris_sdk::events::{lifecycle_event, topics, EventBus, ExtensionEvent};
use ferris_sdk::host_runtime::builtin::{BuiltinRuntime, ClosureExtension};
use ferris_sdk::host_runtime::declarative::DeclarativeContributions;
use ferris_sdk::host_runtime::{empty_schema, CallContext, LazyRuntime, Runtime, ToolDescriptor};
use ferris_sdk::lifecycle::{Health, LifecycleManager};
use ferris_sdk::package::{write_atomic, ExtensionStore, Package};
use ferris_sdk::resolver;
use ferris_sdk::types::{
    ContributionId, ExtensionError, ExtensionId, ExtensionSource, LifecycleState, RuntimeKind,
    StartupPolicy, TrustLevel, ValidationReport, EXTENSION_API_VERSION, HOST_VERSION,
};
use ferris_sdk::ui::{PanelLocation, PanelVisibility, ThemePalette, UiDocument};
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
        // One resolver for GUI, CLI and host (issue #307): absolute wins,
        // tilde expands once, relative anchors to home, native paths kept.
        return crate::settings::resolve_root_override(Some(raw), ".bos/bsh/extensions");
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
    /// Every installed version of this id, ascending.
    ///
    /// The market row needs this to tell an installed-but-inactive version from
    /// one that is not on disk: the first is a switch, the second an install.
    pub(crate) versions: Vec<String>,
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
    /// The schema 1.1 surface, when the panel shipped one.
    pub(crate) surface: Option<ferris_sdk::surface::SurfaceDocument>,
    /// The panel's own dark palette, when it names one.
    pub(crate) theme_dark: Option<ThemePalette>,
    /// How often the host should re-read the live card, when the panel says.
    pub(crate) refresh_ms: Option<u64>,
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
    /// Live surface instances the host owns, one per mounted panel.
    pub(crate) surface_instances: usize,
    /// Panels whose declared surface document was refused.
    pub(crate) surface_refused: usize,
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
    activation: Option<ferris_sdk::manager::Activation>,
    services: Arc<ferris_sdk::composable::ServiceRegistry>,
    extension_id: String,
    workspace: Option<PathBuf>,
    session_id: String,
    service_provides: Vec<ferris_sdk::core_manifest::ServiceProvideSpec>,
    service_requires: Vec<ferris_sdk::core_manifest::ServiceRequireSpec>,
    /// Revocation tokens for shared-scope (Host/Workspace/Session) services
    /// this activation published into the host registry (issue #302). The
    /// registration lives in the shared registry, but unload revokes only
    /// this provider's tokens, so a service never outlives its provider.
    service_tokens: Vec<ferris_sdk::composable::ServiceToken>,
}

/// Host context handed to manager-owned leases at revoke time.
pub(crate) struct BshLeaseHost<'a> {
    pub(crate) agent: &'a mut ::agent::Agent,
    pub(crate) registry: &'a mut ::agent::plugins::PluginRegistry,
}

impl ferris_sdk::LeaseHost for BshLeaseHost<'_> {
    fn remove_tools(&mut self, _owner: &str, tools: &[String]) -> usize {
        self.agent.remove_live_async_tools(tools)
    }

    fn remove_prompts(&mut self, _owner: &str, sections: &[String]) -> usize {
        sections
            .iter()
            .filter(|section| self.agent.remove_live_prompt_section(section))
            .count()
    }

    fn remove_skills(&mut self, _owner: &str, paths: &[std::path::PathBuf]) -> usize {
        self.agent.remove_live_skills_by_paths(paths)
    }

    fn remove_registry_owner(&mut self, owner: &str) -> usize {
        self.registry.remove_owner(owner)
    }
}

/// Combine the Fiber and lease cleanup results without losing either.
fn combine_unload<T>(
    fiber: Result<T, ferris_sdk::composable::FiberTransitionError>,
    lease: Result<(), String>,
) -> Result<T, UnloadError> {
    match (fiber, lease) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(fiber), Ok(())) => Err(UnloadError::Fiber(fiber)),
        (Ok(_), Err(lease)) => Err(UnloadError::Lease(lease)),
        (Err(fiber), Err(lease)) => Err(UnloadError::Both { fiber, lease }),
    }
}

/// The two cleanup failures an activation unload can have, kept separate.
#[derive(Debug)]
pub(crate) enum UnloadError {
    Fiber(ferris_sdk::composable::FiberTransitionError),
    Lease(String),
    Both {
        fiber: ferris_sdk::composable::FiberTransitionError,
        lease: String,
    },
}

impl std::fmt::Display for UnloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fiber(error) => write!(f, "fiber disposal failed: {error}"),
            Self::Lease(error) => write!(f, "lease cleanup failed: {error}"),
            Self::Both { fiber, lease } => {
                write!(
                    f,
                    "fiber disposal failed: {fiber}; lease cleanup failed: {lease}"
                )
            }
        }
    }
}

impl std::error::Error for UnloadError {}

impl AppliedExtension {
    /// Publish a typed value only when this extension declared the matching
    /// service name and visibility scope in its manifest.
    pub(crate) fn provide_service<T>(
        &mut self,
        scope: ferris_sdk::composable::Scope,
        name: &str,
        contract: &str,
        version: &semver::Version,
        service: Arc<T>,
    ) -> Option<ferris_sdk::composable::ServiceToken>
    where
        T: std::any::Any + Send + Sync,
    {
        // The manifest must declare this exact name/contract/version/scope.
        if !self.service_provides.iter().any(|declaration| {
            declaration.name == name
                && declaration.contract == contract
                && declaration.version == *version
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
        if service_scope_is_activation_owned(&scope, &self.extension_id) {
            // The private Extension scope is owned by the activation Fiber,
            // which revokes it on unload.
            return Some(self.services.provide_owned_contract(
                scope,
                name.to_owned(),
                contract,
                version.clone(),
                service,
                &mut self.fiber,
            ));
        }
        // Host/Workspace/Session scopes live in the SHARED host registry, not
        // this build's Fiber, so other builds can resolve them (issue #302).
        // The activation keeps the revocation token and revokes it on unload,
        // so the service still cannot outlive its provider.
        let token = self.services.provide_contract(
            scope,
            name.to_owned(),
            contract,
            version.clone(),
            service,
        );
        self.service_tokens.push(token.clone());
        Some(token)
    }

    /// Publish a first-party runtime's provided service (issue #302).
    ///
    /// The manifest must declare the exact name/contract/version/scope, exactly
    /// as for provide_service. The publication closure preserves the value's
    /// concrete type so a consumer acquires it back typed. The activation keeps
    /// the revocation token and revokes it on unload.
    pub(crate) fn publish_provided(
        &mut self,
        service: &ferris_sdk::host_runtime::contract::ProvidedService,
    ) -> Option<ferris_sdk::composable::ServiceToken> {
        if !self.service_provides.iter().any(|declaration| {
            declaration.name == service.name
                && declaration.contract == service.contract
                && declaration.version == service.version
                && service_scope_matches(
                    &declaration.scope,
                    &service.scope,
                    &self.extension_id,
                    self.workspace.as_deref(),
                    &self.session_id,
                )
        }) {
            return None;
        }
        let token = service.publish(&self.services);
        self.service_tokens.push(token.clone());
        Some(token)
    }

    /// Acquire a typed service only when this extension declared a matching
    /// requirement. The returned lease must be held for the full call.
    pub(crate) fn acquire_service<T>(
        &self,
        scope: &ferris_sdk::composable::Scope,
        name: &str,
        contract: &str,
        requirement: &semver::VersionReq,
    ) -> Option<ferris_sdk::composable::ServiceLease<T>>
    where
        T: std::any::Any + Send + Sync,
    {
        self.service_requires
            .iter()
            .any(|declared| {
                declared.name == name
                    && declared.contract == contract
                    && service_scope_matches(
                        &declared.scope,
                        scope,
                        &self.extension_id,
                        self.workspace.as_deref(),
                        &self.session_id,
                    )
            })
            .then(|| {
                self.fiber.context().and_then(|context| {
                    context.acquire_contract_at(scope, name, contract, requirement)
                })
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
    ) -> Result<usize, UnloadError> {
        // Revoke this provider's shared-scope services before the Fiber goes,
        // so no service outlives its provider (issue #302).
        for token in self.service_tokens.drain(..) {
            self.services.revoke(&token);
        }
        let fiber = self.fiber.unload();
        let lease = if let Some(activation) = self.activation.take() {
            let mut host = BshLeaseHost { agent, registry };
            super::block_on_tokio_scoped(activation.unload_leases(&mut host))
                .map_err(|error| error.to_string())
        } else {
            let (_, drained) = self.lease.revoke(agent, registry);
            if drained {
                Ok(())
            } else {
                Err("lease removed but calls did not drain".to_string())
            }
        };
        combine_unload(fiber, lease)
    }

    /// Revoke this activation, awaiting async effect cleanups.
    pub(crate) async fn unload_async(
        mut self,
        agent: &mut ::agent::Agent,
        registry: &mut ::agent::plugins::PluginRegistry,
    ) -> Result<DisposalReport, UnloadError> {
        for token in self.service_tokens.drain(..) {
            self.services.revoke(&token);
        }
        let fiber = self.fiber.dispose_async().await;
        let lease = if let Some(activation) = self.activation.take() {
            let mut host = BshLeaseHost { agent, registry };
            super::block_on_tokio_scoped(activation.unload_leases(&mut host))
                .map_err(|error| error.to_string())
        } else {
            let (_, drained) = self.lease.revoke(agent, registry);
            if drained {
                Ok(())
            } else {
                Err("lease removed but calls did not drain".to_string())
            }
        };
        combine_unload(fiber, lease)
    }

    /// The composition context attached to this activation.
    pub(crate) fn context(&self) -> Option<&Context> {
        self.fiber.context()
    }

    /// The service registry this activation resolves against.
    pub(crate) fn services(&self) -> Arc<ferris_sdk::composable::ServiceRegistry> {
        Arc::clone(&self.services)
    }

    /// A cloneable snapshot of the live lease for the reload driver.
    pub(crate) fn lease_snapshot(&self) -> contribution::ContributionLease {
        self.lease.clone()
    }

    /// Declared service requirements, for the reload driver.
    pub(crate) fn service_requirements(&self) -> &[ferris_sdk::core_manifest::ServiceRequireSpec] {
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
    use ferris_sdk::composable::Scope;
    use ferris_sdk::core_manifest::ServiceScope;
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
    requested: &ferris_sdk::composable::Scope,
    extension_id: &str,
) -> bool {
    matches!(
        requested,
        ferris_sdk::composable::Scope::Extension(id) if id == extension_id
    )
}

fn service_scope_matches(
    declared: &ferris_sdk::core_manifest::ServiceScope,
    requested: &ferris_sdk::composable::Scope,
    extension_id: &str,
    workspace: Option<&Path>,
    session_id: &str,
) -> bool {
    use ferris_sdk::composable::Scope;
    use ferris_sdk::core_manifest::ServiceScope;
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

/// The directory one package's skills actually come from.
///
/// Two forms coexist. A runtime can materialize skills when the extension
/// activates - a Cordis package calls `ctx.skills.register(..)` and the runtime
/// writes each registration to `<data>/skills/<name>/SKILL.md` - while a
/// declarative package ships `skills/<id>/SKILL.md` files its manifest declares.
///
/// The runtime's directory is authoritative when it HOLDS a skill: it is the
/// set the extension itself registered. An absent or empty directory must not
/// hide the declared files, so it falls back rather than winning by being
/// present. Loading both would register the same name twice, and the skill
/// catalogue and the `/name` menu would then draw a duplicate row.
fn contributed_skills_dir(entry: &LoadedExtension) -> Option<PathBuf> {
    if let Some(dir) = entry
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.skills_dir())
    {
        if holds_skills(&dir) {
            return Some(dir);
        }
    }
    (!entry.package.manifest.contributes.skills.is_empty())
        .then(|| entry.package.root.join("skills"))
}

/// The skill names a directory holds, read from its `<name>/SKILL.md`
/// children.
///
/// This is the shape `agent::skills::SkillLoader` reads, so the host can choose
/// between a runtime's directory and the package's declared one, and describe
/// the set on a status row, without registering a skill to find out.
fn skill_names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    (path.is_dir() && path.join("SKILL.md").is_file())
                        .then(|| entry.file_name().to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `dir` holds at least one skill.
fn holds_skills(dir: &Path) -> bool {
    !skill_names_in(dir).is_empty()
}

/// One host-owned on-startup activation batch.
///
/// The worker is not detached: rebuild and shutdown cancel it through the
/// cancel flag and wait (bounded) for done, so a superseded activation cannot
/// keep starting runtimes after the host moved on. Cancellation also races the
/// in-flight activate() future; a call that completes after the cancel is
/// rolled back with shutdown() instead of being treated as live.
struct WarmStart {
    cancel: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    done: std::sync::mpsc::Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WarmStart {
    /// Cancel the batch and wait up to two seconds for the worker to observe it.
    ///
    /// A runtime that ignores cancellation is left detached rather than
    /// blocking a rebuild forever; it still cannot activate another runtime
    /// because the worker checks the token before every step.
    /// Returns false when the worker could not be joined within the grace
    /// period; the caller records that cleanup is still pending.
    fn stop(mut self) -> bool {
        self.cancel.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
        if self
            .done
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
        {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            true
        } else {
            false
        }
    }
}

/// Start one on-startup batch and return its ownership token.
fn start_warm(runtimes: Vec<(String, Arc<dyn Runtime>)>) -> WarmStart {
    let cancel = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(tokio::sync::Notify::new());
    let (done_tx, done) = std::sync::mpsc::channel();
    let worker_cancel = Arc::clone(&cancel);
    let worker_wake = Arc::clone(&wake);
    let thread = std::thread::spawn(move || {
        let Ok(reactor) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            log::warn!(target: "bsh::extension", "no reactor for on-startup activation");
            let _ = done_tx.send(());
            return;
        };
        reactor.block_on(async move {
            for (id, runtime) in runtimes {
                // Enable the waiter before checking the flag. notify_waiters()
                // does not retain a permit, so a stop between the check and the
                // first poll of notified() would otherwise be lost and leave
                // this worker blocked in activate().
                let notified = worker_wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if worker_cancel.load(Ordering::SeqCst) {
                    return;
                }
                // Racing the activation future is what makes shutdown prompt:
                // an activation blocked on a handshake is dropped here, then
                // rolled back, rather than keeping the worker alive.
                let activated = tokio::select! {
                    biased;
                    _ = &mut notified => None,
                    result = runtime.activate() => Some(result),
                };
                match activated {
                    None => {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            runtime.shutdown(),
                        )
                        .await;
                        return;
                    }
                    Some(Ok(())) => {}
                    Some(Err(error)) => log::warn!(
                        target: "bsh::extension",
                        "{id} on-startup activation failed: {error}"
                    ),
                }
                // A cancel that landed while activate() was finishing must not
                // leave a live runtime behind: shut it back down.
                if worker_cancel.load(Ordering::SeqCst) {
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(2), runtime.shutdown())
                            .await;
                    return;
                }
            }
        });
        let _ = done_tx.send(());
    });
    WarmStart {
        cancel,
        wake,
        done,
        thread: Some(thread),
    }
}

/// What happened to the runtimes a rebuild or shutdown replaced.
#[derive(Debug, Default)]
pub(crate) struct ShutdownReport {
    completed: Vec<String>,
    failed: Vec<(String, String)>,
    timed_out: Vec<String>,
}

impl ShutdownReport {
    fn is_clean(&self) -> bool {
        self.failed.is_empty() && self.timed_out.is_empty()
    }

    fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (id, error) in &self.failed {
            parts.push(format!("{id} failed: {error}"));
        }
        for id in &self.timed_out {
            parts.push(format!("{id} timed out"));
        }
        parts.join("; ")
    }
}

/// One host-owned event-pump thread, so rebuild/shutdown can stop and join it.
struct PumpHandle {
    generation: Arc<AtomicU64>,
    done: std::sync::mpsc::Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PumpHandle {
    /// Stop the worker after its current batch and join within two seconds.
    /// Returns false when a stuck Runtime::pump kept it alive; the caller
    /// records cleanup-pending and detaches it.
    fn stop(mut self) -> bool {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if self
            .done
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
        {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            true
        } else {
            false
        }
    }
}

/// The extension host.
pub(crate) struct ExtensionHost {
    config: HostConfig,
    store: ExtensionStore,
    policy: Arc<PolicyEngine>,
    events: Arc<EventBus>,
    services: Arc<ferris_sdk::composable::ServiceRegistry>,
    terminal_token: Option<ServiceToken>,
    manager: LifecycleManager,
    loaded: BTreeMap<String, LoadedExtension>,
    builtins: Vec<BuiltinLoaded>,
    issues: Vec<String>,
    status_cache: Arc<Mutex<Vec<ExtensionStatus>>>,
    graph_cache: Arc<Mutex<Vec<ExtensionGraphNode>>>,
    last_stamp: Option<SystemTime>,
    pump_generation: Arc<AtomicU64>,
    /// The active event-pump worker, owned so rebuild/shutdown can join it.
    pump: Option<PumpHandle>,
    /// The active on-startup batch, owned so rebuild/shutdown can stop it.
    warm: Option<WarmStart>,
    /// Runtimes whose shutdown timed out; retried by the next rebuild.
    pending_shutdown: Vec<(String, Arc<dyn Runtime>)>,
    startup_millis: u64,
    crash_history: Vec<String>,
    languages: Vec<LanguageEntry>,
    integrations: Vec<IntegrationEntry>,
    /// The surface instances behind the mounted panels.
    surface_host: super::surface::SurfaceHost,
}

/// One language service contributed by an applied extension.
#[derive(Debug, Clone)]
pub(crate) struct LanguageEntry {
    /// Package that owns the descriptor.
    pub(crate) owner: ExtensionId,
    /// Descriptor.
    pub(crate) contribution: ferris_sdk::host_runtime::declarative::LanguageContribution,
}

/// One external-system integration contributed by an applied extension.
#[derive(Debug, Clone)]
pub(crate) struct IntegrationEntry {
    /// Package that owns the descriptor.
    pub(crate) owner: ExtensionId,
    /// Descriptor.
    pub(crate) contribution: ferris_sdk::host_runtime::declarative::IntegrationContribution,
}

impl ExtensionHost {
    fn build_framework_resolver(&self) -> super::framework::FerrisRuntimeResolver {
        let mut resolver =
            super::framework::FerrisRuntimeResolver::new(self.config.workspace.clone()).with_ports(
                Arc::clone(&self.policy),
                Arc::clone(&self.events),
                ferris_sdk::types::TrustLevel::Community,
            );
        for builtin in &self.builtins {
            resolver =
                resolver.with_builtin(builtin.package.id().as_str(), Arc::clone(&builtin.runtime));
        }
        for entry in self.loaded.values().filter(|entry| entry.applied) {
            if let Some(runtime) = &entry.runtime {
                resolver = resolver.with_legacy(entry.package.id().as_str(), Arc::clone(runtime));
            }
        }
        resolver
    }

    fn build_framework_activations(&self) -> Option<ferris_sdk::manager::ActivationSet> {
        let resolver = self.build_framework_resolver();
        let manager = self
            .framework_manager()
            .ok()?
            .with_ports(Arc::new(resolver), Arc::new(super::framework::HostNoopSink));
        let plan = manager.plan().ok()?;
        super::block_on_tokio(async move {
            manager
                .apply(&plan, &mut (), &mut (), &mut super::framework::HostNoopHost)
                .await
        })
        .ok()
    }

    /// Build the host-neutral runtime resolver with this host's policy/events.
    pub(crate) fn framework_resolver(
        &self,
    ) -> std::sync::Arc<super::framework::FerrisRuntimeResolver> {
        let mut resolver =
            super::framework::FerrisRuntimeResolver::new(self.config.workspace.clone()).with_ports(
                std::sync::Arc::clone(&self.policy),
                std::sync::Arc::clone(&self.events),
                ferris_sdk::types::TrustLevel::Community,
            );
        for builtin in &self.builtins {
            resolver = resolver.with_builtin(
                builtin.package.id().as_str(),
                std::sync::Arc::clone(&builtin.runtime),
            );
        }
        std::sync::Arc::new(resolver)
    }

    /// Open the host-neutral manager against this host's root.
    ///
    /// Built-ins are linked into the binary, so the manager's store has no
    /// registry entry and no installed version to read back for them: register
    /// each one with its manifest, runtime and root so it can be planned and
    /// activated like an installed package.
    pub(crate) fn framework_manager(
        &self,
    ) -> Result<ferris_sdk::manager::ExtensionManager, ferris_sdk::ExtError> {
        let mut manager =
            super::framework::manager_for(self.config.root.clone(), self.config.workspace.clone())?;
        for builtin in &self.builtins {
            manager.register_linked(
                builtin.package.manifest.clone(),
                builtin.package.root.clone(),
            );
        }
        Ok(manager)
    }

    /// The host-scoped service registry shared by its extension activations.
    pub(crate) fn services(&self) -> Arc<ferris_sdk::composable::ServiceRegistry> {
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
            services: Arc::new(ferris_sdk::composable::ServiceRegistry::new()),
            terminal_token: None,
            manager: LifecycleManager::new(),
            loaded: BTreeMap::new(),
            builtins: Vec::new(),
            issues: Vec::new(),
            status_cache,
            graph_cache,
            last_stamp: None,
            pump_generation: Arc::new(AtomicU64::new(0)),
            pump: None,
            warm: None,
            pending_shutdown: Vec::new(),
            startup_millis: 0,
            crash_history: Vec::new(),
            languages: Vec::new(),
            integrations: Vec::new(),
            surface_host: super::surface::SurfaceHost::new(),
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
                    verification: ferris_sdk::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                let id = package.id().to_string();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(id, Arc::new(runtime))),
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
                    verification: ferris_sdk::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                let id = package.id().to_string();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(id, Arc::new(runtime))),
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
                    verification: ferris_sdk::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                let id = package.id().to_string();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(id, Arc::new(runtime))),
                    tools,
                });
            }
            Err(error) => log::warn!("built-in annotations was not registered: {error}"),
        }
        #[cfg(test)]
        self.register_service_test_builtin();
    }

    /// Test-only first-party extension that publishes a typed service, so the
    /// production activation path's service publication is exercised end to end
    /// (issue #302).
    #[cfg(test)]
    fn register_service_test_builtin(&mut self) {
        match BuiltinRuntime::new(Arc::new(ServiceTestProvider)) {
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
                    verification: ferris_sdk::signing::SignatureVerification::Missing,
                    source_path: None,
                };
                let tools = runtime.descriptors();
                let id = package.id().to_string();
                self.builtins.push(BuiltinLoaded {
                    package,
                    runtime: Arc::new(LazyRuntime::new(id, Arc::new(runtime))),
                    tools,
                });
            }
            Err(error) => log::warn!("service test builtin not registered: {error}"),
        }
    }

    /// Runtimes a replacement package set no longer uses, deduplicated.
    fn superseded_runtimes(
        previous: impl Iterator<Item = (String, Arc<dyn Runtime>)>,
        current: &[Arc<dyn Runtime>],
    ) -> Vec<(String, Arc<dyn Runtime>)> {
        let live: std::collections::HashSet<*const ()> = current
            .iter()
            .map(|runtime| Arc::as_ptr(runtime) as *const ())
            .collect();
        let mut seen: std::collections::HashSet<*const ()> = std::collections::HashSet::new();
        let mut superseded = Vec::new();
        for (id, runtime) in previous {
            let pointer = Arc::as_ptr(&runtime) as *const ();
            if !live.contains(&pointer) && seen.insert(pointer) {
                superseded.push((id, runtime));
            }
        }
        superseded
    }

    /// Stop runtimes together, bounded so one hung child cannot block a rebuild.
    fn stop_runtimes(runtimes: Vec<(String, Arc<dyn Runtime>)>) -> ShutdownReport {
        super::block_on_tokio(Self::stop_runtimes_async(runtimes))
    }

    /// The async half of [Self::stop_runtimes]; shutdown() awaits it directly.
    async fn stop_runtimes_async(runtimes: Vec<(String, Arc<dyn Runtime>)>) -> ShutdownReport {
        let mut report = ShutdownReport::default();
        if runtimes.is_empty() {
            return report;
        }
        let ids: Vec<String> = runtimes.iter().map(|(id, _)| id.clone()).collect();
        let (done_tx, mut done_rx) =
            tokio::sync::mpsc::unbounded_channel::<(String, Result<(), String>)>();
        let stops = runtimes.into_iter().map(|(id, runtime)| {
            let done = done_tx.clone();
            async move {
                let result = runtime.shutdown().await.map_err(|error| error.to_string());
                let _ = done.send((id, result));
            }
        });
        let timed_out = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            futures::future::join_all(stops),
        )
        .await
        .is_err();
        drop(done_tx);
        while let Ok((id, result)) = done_rx.try_recv() {
            match result {
                Ok(()) => report.completed.push(id),
                Err(error) => report.failed.push((id, error)),
            }
        }
        if timed_out {
            report.timed_out = ids
                .into_iter()
                .filter(|id| {
                    !report.completed.contains(id)
                        && !report.failed.iter().any(|(failed, _)| failed == id)
                })
                .collect();
        }
        report
    }

    fn rebuild(&mut self) -> Result<(), ExtensionError> {
        // A previous batch must not activate a runtime that this rebuild is
        // about to replace, disable or remove.
        self.stop_warm_startup();
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
        let mut packages = select_active_versions(packages, &self.store);
        // The neutral manager owns dependency-first ordering; fall back to id
        // order when it cannot produce a plan (a broken manifest is reported
        // separately below).
        let order: std::collections::BTreeMap<String, usize> = self
            .framework_manager()
            .and_then(|manager| manager.plan())
            .map(|plan| {
                plan.order
                    .into_iter()
                    .enumerate()
                    .map(|(index, id)| (id, index))
                    .collect()
            })
            .unwrap_or_default();
        packages.sort_by_key(|package| {
            order
                .get(package.id().as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        for package in packages {
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
                match ferris_sdk::composable::plan_service_dependencies(&manifests) {
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
        let previous = std::mem::take(&mut self.loaded);
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
        // A rebuild replaces the package set, so the old entries are dropped.
        // Dropping an Arc does not stop a process, guest or thread it owns;
        // every runtime the new set no longer uses must be shut down or a
        // disable/upgrade leaks a live child until the process exits.
        let current: Vec<Arc<dyn Runtime>> = self
            .loaded
            .values()
            .filter_map(|entry| entry.runtime.clone())
            .collect();
        let mut superseded = Self::superseded_runtimes(
            previous.values().filter_map(|entry| {
                entry
                    .runtime
                    .clone()
                    .map(|runtime| (entry.package.id().to_string(), runtime))
            }),
            &current,
        );
        // Retry anything a previous rebuild timed out on before starting the
        // new generation's work.
        superseded.append(&mut self.pending_shutdown);
        let retain: BTreeMap<String, Arc<dyn Runtime>> = superseded.iter().cloned().collect();
        let report = Self::stop_runtimes(superseded);
        for id in &report.timed_out {
            if let Some(runtime) = retain.get(id) {
                self.pending_shutdown
                    .push((id.clone(), Arc::clone(runtime)));
            }
        }
        if !report.is_clean() {
            let summary = report.summary();
            log::warn!(
                target: "bsh::extension",
                "rebuild left superseded runtimes pending: {summary}"
            );
            self.issues
                .push(format!("runtime cleanup pending: {summary}"));
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
    fn warm_startup_runtimes(&mut self) {
        self.stop_warm_startup();
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
        self.warm = Some(start_warm(runtimes));
    }

    /// Cancel and join the on-startup batch, bounded.
    fn stop_warm_startup(&mut self) {
        if let Some(warm) = self.warm.take() {
            if !warm.stop() {
                log::warn!(
                    target: "bsh::extension",
                    "on-startup activation did not stop within 2s; it can no longer activate runtimes"
                );
                self.issues
                    .push("on-startup activation cleanup pending".to_string());
            }
        }
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
            ferris_sdk::config::migrate(&package.manifest, &package.root, &data_root, &from, &to)?
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
    fn start_event_pump(&mut self) {
        self.stop_event_pump();
        let runtimes: Vec<(String, Weak<dyn Runtime>)> = self
            .loaded
            .values()
            .filter_map(|entry| {
                entry
                    .runtime
                    .as_ref()
                    .map(|runtime| (entry.package.id().to_string(), Arc::downgrade(runtime)))
            })
            .collect();
        if runtimes.is_empty() {
            return;
        }
        // Build the reactor on the host thread so failure is observable here
        // instead of silently disabling event delivery inside the worker.
        let Ok(reactor) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            log::warn!(target: "bsh::extension", "no reactor for the extension event pump");
            self.issues
                .push("event pump reactor initialization failed".to_string());
            return;
        };
        let generation = self.pump_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let watched = Arc::clone(&self.pump_generation);
        let (done_tx, done) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("bsh-extension-events".to_string())
            .spawn(move || {
                let mut failures: std::collections::HashMap<String, u32> =
                    std::collections::HashMap::new();
                loop {
                    if watched.load(Ordering::SeqCst) != generation {
                        break;
                    }
                    let live: Vec<(String, Arc<dyn Runtime>)> = runtimes
                        .iter()
                        .filter_map(|(id, runtime)| {
                            runtime.upgrade().map(|runtime| (id.clone(), runtime))
                        })
                        .collect();
                    if live.is_empty() {
                        break;
                    }
                    let mut failed = false;
                    reactor.block_on(async {
                        for (id, runtime) in &live {
                            match runtime.pump().await {
                                Ok(()) => {
                                    failures.remove(id);
                                }
                                Err(error) => {
                                    failed = true;
                                    let count = failures.entry(id.clone()).or_insert(0);
                                    *count += 1;
                                    // Rate-limit a permanently failing pump:
                                    // first failure, then every tenth.
                                    if *count == 1 || count.is_multiple_of(10) {
                                        log::warn!(
                                            target: "bsh::extension",
                                            "{id} event pump failed {count} times: {error}"
                                        );
                                    }
                                }
                            }
                        }
                    });
                    drop(live);
                    let backoff = if failed {
                        let streak = failures.values().copied().max().unwrap_or(1);
                        150_u64 << streak.min(4)
                    } else {
                        150
                    };
                    std::thread::sleep(std::time::Duration::from_millis(backoff.min(2_000)));
                }
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(thread) => {
                self.pump = Some(PumpHandle {
                    generation: Arc::clone(&self.pump_generation),
                    done,
                    thread: Some(thread),
                });
            }
            Err(error) => {
                log::warn!(
                    target: "bsh::extension",
                    "could not start the extension event pump: {error}"
                );
                self.issues
                    .push(format!("event pump thread failed to start: {error}"));
            }
        }
    }

    /// Stop the event pump, bounded; a stuck pump is reported as pending.
    fn stop_event_pump(&mut self) {
        if let Some(pump) = self.pump.take() {
            if !pump.stop() {
                log::warn!(
                    target: "bsh::extension",
                    "event pump did not stop within 2s; a Runtime::pump is still running"
                );
                self.issues
                    .push("extension event pump cleanup pending".to_string());
            }
        }
    }

    fn build_runtime(&self, package: &Package) -> Result<BuiltRuntime, ExtensionError> {
        let data_root = self.store.data_dir(package.id());
        let config = ferris_sdk::config::load(&package.manifest, &package.root, &data_root)?;
        let built = ferris_sdk::host_runtime::factory::build(
            &package.manifest,
            &package.root,
            ferris_sdk::host_runtime::factory::RuntimePorts {
                policy: Arc::clone(&self.policy),
                events: Arc::clone(&self.events),
                workspace: self.config.workspace.clone(),
                data_root,
                config: config.to_json(),
                trust: package.trust,
                terminal: Some(Arc::new(super::terminal::HostTerminal)),
            },
        )?;
        Ok((built.runtime, built.tools, built.contributions))
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

    /// Discover an on-demand MCP package's tools and declarative
    /// contributions at agent-build time, the point at which the host
    /// activates it. Host construction skips this discovery for such packages.
    fn on_demand_projection(
        &self,
        runtime: &Arc<dyn Runtime>,
    ) -> (Vec<ToolDescriptor>, DeclarativeContributions) {
        let for_tools = Arc::clone(runtime);
        let tools = super::block_on_tokio_scoped(async move { for_tools.tools().await })
            .unwrap_or_default();
        let contributions = runtime.declarative();
        (tools, contributions)
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
            let on_demand = entry.package.manifest.runtime.kind == RuntimeKind::Mcp
                && entry.package.manifest.runtime.startup == StartupPolicy::OnDemand;
            let (tools, contributions) = if on_demand {
                self.on_demand_projection(runtime)
            } else {
                (entry.tools.clone(), entry.contributions.clone())
            };
            // A package may contribute only skills; skipping it here silently
            // dropped every skill it declared. The directory is resolved once so
            // the gate and the install cannot disagree about which form the
            // package uses: a runtime-registered skill has no manifest
            // declaration, only a materialized directory.
            let skills_dir = contributed_skills_dir(entry);
            let has_agent_contribution = !tools.is_empty()
                || !contributions.commands.is_empty()
                || !contributions.panels.is_empty()
                || !contributions.prompts.is_empty()
                || !contributions.contexts.is_empty()
                || skills_dir.is_some()
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
                tools: tools.clone(),
                commands: contributions.commands.clone(),
                panels: contributions.panels.clone(),
                prompts: contributions.prompts.clone(),
                contexts: contributions.contexts.clone(),
                workspace: self.config.workspace.clone(),
                skills_dir,
                workflows: contributions.workflows.clone(),
                themes: contributions.themes.clone(),
                timeout_ms: entry.package.manifest.resources.execution_ms(),
                max_output_bytes: entry.package.manifest.resources.output_bytes(),
                max_concurrent_calls: entry.package.manifest.resources.concurrent_calls() as usize,
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
        let manager = match self.framework_manager() {
            Ok(manager) => manager,
            Err(error) => {
                log::error!("extension manager unavailable; no package applied: {error}");
                return applied;
            }
        };
        let sink = Arc::new(super::framework::HostContributionSink::new(
            Arc::clone(&self.policy),
            Arc::clone(&self.events),
            self.config.workspace.clone(),
        ));
        let mut eligible = BTreeSet::new();
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
            let on_demand = entry.package.manifest.runtime.kind == RuntimeKind::Mcp
                && entry.package.manifest.runtime.startup == StartupPolicy::OnDemand;
            let (tools, contributions) = if on_demand {
                self.on_demand_projection(runtime)
            } else {
                (entry.tools.clone(), entry.contributions.clone())
            };
            let skills_dir = contributed_skills_dir(entry);
            let has_agent_contribution = !tools.is_empty()
                || !contributions.commands.is_empty()
                || !contributions.panels.is_empty()
                || !contributions.prompts.is_empty()
                || !contributions.contexts.is_empty()
                || skills_dir.is_some()
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
                tools: tools.clone(),
                commands: contributions.commands.clone(),
                panels: contributions.panels.clone(),
                prompts: contributions.prompts.clone(),
                contexts: contributions.contexts.clone(),
                workspace: self.config.workspace.clone(),
                skills_dir,
                workflows: contributions.workflows.clone(),
                themes: contributions.themes.clone(),
                timeout_ms: entry.package.manifest.resources.execution_ms(),
                max_output_bytes: entry.package.manifest.resources.output_bytes(),
                max_concurrent_calls: entry.package.manifest.resources.concurrent_calls() as usize,
            };
            sink.insert(entry.package.id().as_str(), contributed);
            eligible.insert(entry.package.id().as_str().to_owned());
        }
        let manager = manager.with_ports(
            Arc::new(self.build_framework_resolver()),
            Arc::clone(&sink) as Arc<dyn ferris_sdk::ContributionSink>,
        );
        let mut plan = match manager.plan() {
            Ok(plan) => plan,
            Err(error) => {
                log::error!("extension plan failed; no package applied: {error}");
                return applied;
            }
        };
        // Keep the planner's dependency-first order; only exclude packages
        // this build did not prepare (disabled, failed or outside the preset).
        plan.order.retain(|id| eligible.contains(id));
        if plan.order.is_empty() {
            return applied;
        }
        let mut set = match super::block_on_tokio_scoped(manager.apply(
            &plan,
            agent,
            registry,
            &mut super::framework::HostNoopHost,
        )) {
            Ok(set) => set,
            Err(error) => {
                log::error!("extension activation failed; no package applied: {error}");
                return applied;
            }
        };
        for id in plan.order {
            let Some(entry) = self.loaded.get(&id) else {
                continue;
            };
            let Some(activation) = set.take(&id) else {
                log::error!("extension {id}: the manager returned no activation");
                continue;
            };
            let (Some(info), Some(lease)) = (sink.take_info(&id), sink.lease(&id)) else {
                log::error!("extension {id}: contribution sink returned no info or lease");
                continue;
            };
            let extension_id = id.clone();
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
            let mut activation_ext = AppliedExtension {
                info,
                activation: Some(activation),
                fiber,
                lease,
                services: Arc::clone(&self.services),
                extension_id: entry.package.id().as_str().to_owned(),
                workspace: self.config.workspace.clone(),
                session_id: session_id.to_owned(),
                service_provides: entry.package.manifest.services.provides.clone(),
                service_requires: entry.package.manifest.services.requires.clone(),
                service_tokens: Vec::new(),
            };
            // Publish the runtime's typed services into the shared registry
            // (issue #302). Only a first-party runtime supplies any; the
            // manifest declaration is enforced per service, and each token is
            // revoked when this activation unloads.
            if let Some(runtime) = &entry.runtime {
                for service in runtime.provided_services() {
                    if activation_ext.publish_provided(&service).is_none() {
                        log::warn!(
                            "extension {id}: service {} not published (no matching manifest declaration)",
                            service.name
                        );
                    }
                }
            }
            applied.push(activation_ext);
        }
        applied
    }

    /// Every known extension as a status row.
    pub(crate) fn statuses(&self) -> Vec<ExtensionStatus> {
        let mut statuses: Vec<ExtensionStatus> = self
            .loaded
            .values()
            .map(|entry| self.status_of(entry))
            .collect();
        let known: std::collections::BTreeSet<String> =
            statuses.iter().map(|status| status.id.clone()).collect();
        if let Ok(manager) = self.framework_manager() {
            if let Ok(installed) = manager.installed() {
                // One row per id, not one per version: an extension with two
                // installed versions is still one row, and its active version
                // names the one the host would load.
                let mut by_id: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                for extension in installed {
                    by_id
                        .entry(extension.id)
                        .or_default()
                        .push(extension.version);
                }
                for (id, versions) in by_id {
                    if known.contains(&id) {
                        continue;
                    }
                    let active = manager
                        .active_version(&id)
                        .ok()
                        .flatten()
                        .filter(|version| versions.iter().any(|candidate| candidate == version))
                        .or_else(|| versions.last().cloned())
                        .unwrap_or_default();
                    let package_root = manager.package_root(&id, &active).ok();
                    let manifest = package_root
                        .as_deref()
                        .and_then(|root| ferris_sdk::manifest_v2::load_package_manifest(root).ok());
                    let error = package_root.as_deref().and_then(|root| {
                        ferris_sdk::manifest_v2::load_package_manifest(root)
                            .err()
                            .map(|error| error.to_string())
                    });
                    let root = package_root
                        .map(|root| root.display().to_string())
                        .unwrap_or_default();
                    statuses.push(ExtensionStatus {
                        id: id.clone(),
                        name: manifest
                            .as_ref()
                            .map(|manifest| manifest.extension.name.clone())
                            .unwrap_or_else(|| id.clone()),
                        version: active,
                        versions,
                        runtime: manifest
                            .as_ref()
                            .map(|manifest| manifest.runtime.kind.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        program: None,
                        trust: "unknown".to_string(),
                        source: "installed".to_string(),
                        root,
                        hash: String::new(),
                        state: LifecycleState::Installed,
                        health: Health::Unknown,
                        configured: true,
                        applied: false,
                        tools: Vec::new(),
                        commands: Vec::new(),
                        panels: Vec::new(),
                        skills: Vec::new(),
                        languages: Vec::new(),
                        integrations: Vec::new(),
                        capabilities: Vec::new(),
                        error,
                        startup_ms: 0,
                        installed_at: 0,
                        dependencies: Vec::new(),
                        signature: "unsigned".to_string(),
                        publisher: None,
                    });
                }
            }
        }
        statuses
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
                if let Ok(v2) = ferris_sdk::manifest_v2::ManifestV2::load(&entry.package.root) {
                    for contribution in &v2.contributions {
                        let kind = match contribution.kind.as_str() {
                            "event.subscribe" => "event",
                            "event.publish" => "publish",
                            "model-provider" => "model",
                            "agent-provider" => "agent",
                            other => other,
                        };
                        effects.push(format!("{kind}:{}", contribution.id));
                    }
                } else {
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
            ferris_sdk::capability::call_scope(&arguments, self.config.workspace.as_deref());
        ferris_sdk::capability::authorize_each(
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
                ferris_sdk::capability::call_scope(&arguments, self.config.workspace.as_deref());
            ferris_sdk::capability::authorize_each(
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
        // Stop the pump and activation before stopping the runtimes they drive.
        self.stop_event_pump();
        self.stop_warm_startup();
        let mut runtimes: Vec<(String, Arc<dyn Runtime>)> = self
            .loaded
            .values()
            .filter_map(|entry| {
                entry
                    .runtime
                    .clone()
                    .map(|runtime| (entry.package.id().to_string(), runtime))
            })
            .collect();
        self.loaded.clear();
        // A runtime whose shutdown timed out during an earlier rebuild is no
        // longer in `loaded`; include it in the final attempt once.
        let mut seen: std::collections::HashSet<*const ()> = runtimes
            .iter()
            .map(|(_, runtime)| Arc::as_ptr(runtime) as *const ())
            .collect();
        for (id, runtime) in std::mem::take(&mut self.pending_shutdown) {
            let pointer = Arc::as_ptr(&runtime) as *const ();
            if seen.insert(pointer) {
                runtimes.push((id, runtime));
            }
        }
        // Stop them together: sequential shutdown paid every process's
        // one-second kill deadline in turn, so a machine with four process
        // packages hung the closing window for four seconds.
        let report = Self::stop_runtimes_async(runtimes).await;
        if !report.is_clean() {
            log::warn!(
                target: "bsh::extension",
                "host shutdown left runtimes pending: {}",
                report.summary()
            );
        }
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
        let entries =
            ferris_sdk::composable::plan::load_cordis_patch(path, "profile").map_err(|error| {
                ExtensionError::Runtime {
                    extension: "profile".to_string(),
                    message: error.to_string(),
                }
            })?;
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
                let id = ferris_sdk::types::ExtensionId::parse(id.clone())?;
                ferris_sdk::config::save(&manifest, &root, &self.store.data_dir(&id), config)?;
            }
            self.retry(&id)?;
            mounted.push(id);
        }
        Ok(mounted)
    }

    pub(crate) fn retry(&mut self, id: &str) -> Result<(), ExtensionError> {
        let id = ferris_sdk::types::ExtensionId::parse(id.to_string())?;
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
            ferris_sdk::capability::Capability::ProcessExecute,
            ferris_sdk::capability::GrantScope::Program(program.to_string()),
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
    pub(crate) fn panel_surfaces(&mut self) -> Vec<PanelSurface> {
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
            let theme = load_theme(entry, None);
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
                    theme: load_theme(entry, panel.theme.as_deref()),
                    theme_dark: panel
                        .theme_dark
                        .as_deref()
                        .and_then(|id| load_theme(entry, Some(id))),
                    initial: Some(panel.view.clone()),
                    surface: panel.surface.clone(),
                    refresh_ms: panel.refresh_ms,
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
                    theme_dark: None,
                    initial: None,
                    surface: None,
                    refresh_ms: None,
                    policy: Arc::clone(&self.policy),
                    trust: entry.package.trust,
                    tools: Arc::new(entry.tools.clone()),
                    workspace: self.config.workspace.clone(),
                    root: entry.package.root.clone(),
                });
            }
        }
        // Agent-authored surfaces are panels too: the same renderer, the same
        // engine and the same disposal path, with an agent badge in the title.
        for view in super::agent_surface::views() {
            surfaces.push(PanelSurface {
                panel_id: format!("bos.agent.{}", view.id),
                contribution_id: view.id.clone(),
                title: format!("Agent \u{b7} {}", view.title),
                icon: None,
                location: view.location,
                visibility: PanelVisibility::Visible,
                runtime: Arc::clone(&view.runtime),
                owner: ExtensionId::parse("bos.agent").expect("a static extension id"),
                theme: None,
                theme_dark: None,
                initial: Some(view.document.to_ui_document()),
                surface: Some(view.document.clone()),
                refresh_ms: None,
                policy: Arc::clone(&self.policy),
                trust: TrustLevel::Local,
                tools: Arc::new(Vec::new()),
                workspace: self.config.workspace.clone(),
                root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            });
        }
        let admissions: Vec<super::surface::PanelAdmission<'_>> = surfaces
            .iter()
            .map(|panel| super::surface::PanelAdmission {
                panel_id: panel.panel_id.as_str(),
                location: panel.location,
                document: panel.initial.as_ref(),
                surface: panel.surface.as_ref(),
            })
            .collect();
        let report = self.surface_host.sync(&admissions);
        if !report.errors.is_empty() {
            log::warn!(
                "surface admission refused {} panel(s): {}",
                report.errors.len(),
                report.errors.join("; ")
            );
        }
        surfaces
    }

    /// One panel's surface in this generation, by qualified panel id.
    ///
    /// A panel view is created from a snapshot and outlives the generation that
    /// made it: a rebuild shuts the old runtime down and builds a new one. This
    /// is how the view asks for the replacement without reopening the host.
    pub(crate) fn live_panel(&mut self, panel_id: &str) -> Option<PanelSurface> {
        self.panel_surfaces()
            .into_iter()
            .find(|panel| panel.panel_id == panel_id)
    }

    /// The binding a panel control names, when the panel ships one.
    pub(crate) fn surface_binding(
        &self,
        panel_id: &str,
        key: &str,
    ) -> Option<ferris_sdk::surface::ActionBinding> {
        self.surface_host.binding(panel_id, key)
    }

    /// The document and revision the surface host holds for a panel.
    pub(crate) fn surface_view(&self, panel_id: &str) -> Option<(ferris_sdk::ui::UiDocument, u64)> {
        let (document, revision) = self.surface_host.view(panel_id)?;
        Some((document.to_ui_document(), revision.value()))
    }

    /// Apply a live panel card to its surface instance.
    pub(crate) fn surface_apply(
        &mut self,
        panel_id: &str,
        document: &ferris_sdk::ui::UiDocument,
    ) -> Result<(ferris_sdk::ui::UiDocument, u64), String> {
        let (document, revision) = self.surface_host.apply(panel_id, document)?;
        Ok((document.to_ui_document(), revision.value()))
    }

    /// Refuse an action the surface has moved past.
    pub(crate) fn surface_accept(&self, panel_id: &str, revision: u64) -> Result<(), String> {
        self.surface_host.accept(panel_id, revision)
    }

    /// Apply a frame that arrived from a transport.
    pub(crate) fn surface_frame(
        &mut self,
        frame: &ferris_sdk::surface::SurfaceFrame,
    ) -> Result<ferris_sdk::surface::Revision, String> {
        self.surface_host.apply_frame(frame)
    }

    /// The mounted surface rows, for the inspector.
    pub(crate) fn surface_rows(&self) -> Vec<super::surface::SurfaceRow> {
        self.surface_host.rows()
    }

    /// Dispose an instance a transport asked to close.
    pub(crate) fn surface_close_instance(
        &mut self,
        instance: ferris_sdk::surface::InstanceId,
    ) -> bool {
        self.surface_host.close_instance(instance)
    }

    /// Replace a panel's provider generation after a runtime rebind.
    pub(crate) fn surface_renew(&mut self, panel_id: &str) -> Result<(), String> {
        self.surface_host.renew(panel_id)
    }

    /// Every theme a package contributes, keyed by a host-owned id.
    pub(crate) fn theme_surfaces(&self) -> BTreeMap<String, ferris_sdk::ui::ThemePalette> {
        let mut themes = BTreeMap::new();
        for entry in self.loaded.values() {
            if !entry.applied || entry.error.is_some() {
                continue;
            }
            for theme in &entry.contributions.themes {
                let Ok(text) = std::fs::read_to_string(&theme.path) else {
                    continue;
                };
                if let Ok(palette) = serde_json::from_str::<ferris_sdk::ui::ThemePalette>(&text) {
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
            surface_instances: self.surface_host.len(),
            surface_refused: self.surface_host.refused_count(),
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
            versions: self.store.versions(id).unwrap_or_default(),
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
            // The declared file ids, plus the names a runtime materialized at
            // activation. The runtime form is invisible to the manifest, so a
            // row built from the declaration alone would tell the reader a
            // Cordis package has no skills while the model can load them.
            skills: {
                let mut names: BTreeSet<String> = entry
                    .package
                    .manifest
                    .contributes
                    .skills
                    .iter()
                    .map(|skill| skill.as_str().to_string())
                    .collect();
                if let Some(dir) = entry
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.skills_dir())
                {
                    names.extend(skill_names_in(&dir));
                }
                names.into_iter().collect()
            },
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
                        ferris_sdk::signing::SignatureVerification::Verified { publisher } => {
                            issues.push(DiagnosticIssue::new(
                                "info",
                                Some(id),
                                format!("package signature verified for publisher {publisher}"),
                            ));
                        }
                        ferris_sdk::signing::SignatureVerification::Untrusted {
                            publisher,
                            reason,
                        } => issues.push(DiagnosticIssue::new(
                            "warning",
                            Some(id),
                            format!("signature by {publisher} is untrusted: {reason}"),
                        )),
                        ferris_sdk::signing::SignatureVerification::Invalid { reason } => {
                            issues.push(DiagnosticIssue::new(
                                "error",
                                Some(id),
                                format!("package signature is invalid: {reason}"),
                            ));
                        }
                        ferris_sdk::signing::SignatureVerification::Missing => {
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

/// One declared theme for an extension, by id, or the first when none is named.
///
/// The panel file names its theme, so a package with a light and a dark theme
/// (or several looks) can ship all of them and pick per panel.
fn load_theme(entry: &LoadedExtension, id: Option<&str>) -> Option<ThemePalette> {
    let theme = match id {
        Some(id) => entry
            .contributions
            .themes
            .iter()
            .find(|theme| theme.id.as_str() == id)?,
        None => entry.contributions.themes.first()?,
    };
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
        .as_mut()
        .map(ExtensionHost::panel_surfaces)
        .unwrap_or_default()
}

/// One panel's live surface, from the already-open host.
///
/// A mounted view holds the surface it was built with; after a rebuild that
/// runtime is shut down and this returns the current generation's. It never
/// opens or refreshes the host, so a panel render never pays for a store scan.
pub(crate) fn live_panel(panel_id: &str) -> Option<PanelSurface> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().ok()?;
    guard.as_mut()?.live_panel(panel_id)
}

/// The document and revision the surface host holds for a panel.
pub(crate) fn surface_binding(
    panel_id: &str,
    key: &str,
) -> Option<ferris_sdk::surface::ActionBinding> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let guard = slot.lock().ok()?;
    guard.as_ref()?.surface_binding(panel_id, key)
}

pub(crate) fn surface_view(panel_id: &str) -> Option<(ferris_sdk::ui::UiDocument, u64)> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let guard = slot.lock().ok()?;
    guard.as_ref()?.surface_view(panel_id)
}

/// Apply a live panel card to its surface instance.
pub(crate) fn surface_apply(
    panel_id: &str,
    document: &ferris_sdk::ui::UiDocument,
) -> Result<(ferris_sdk::ui::UiDocument, u64), String> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| "the extension host lock is poisoned".to_string())?;
    guard
        .as_mut()
        .ok_or_else(|| "the extension host is not open".to_string())?
        .surface_apply(panel_id, document)
}

/// Replace a panel's provider generation after a runtime rebind.
pub(crate) fn surface_renew(panel_id: &str) -> Result<(), String> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| "the extension host lock is poisoned".to_string())?;
    guard
        .as_mut()
        .ok_or_else(|| "the extension host is not open".to_string())?
        .surface_renew(panel_id)
}

/// Refuse an action the surface has moved past.
pub(crate) fn surface_accept(panel_id: &str, revision: u64) -> Result<(), String> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let guard = slot
        .lock()
        .map_err(|_| "the extension host lock is poisoned".to_string())?;
    guard
        .as_ref()
        .ok_or_else(|| "the extension host is not open".to_string())?
        .surface_accept(panel_id, revision)
}

/// One row per mounted surface, for the extensions pane.
pub(crate) fn surface_rows() -> Vec<super::surface::SurfaceRow> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return Vec::new();
    };
    guard
        .as_ref()
        .map(|host| host.surface_rows())
        .unwrap_or_default()
}

/// Publish one surface message on the local bus.
///
/// The payload carries the bounded, versioned bytes; a subscriber decodes them
/// with the framework's wire limits rather than trusting the payload shape.
pub(crate) fn surface_publish(message: &ferris_sdk::surface::SurfaceMessage) -> Result<(), String> {
    let bytes = ferris_sdk::surface::encode(message).map_err(|error| error.to_string())?;
    publish(
        crate::extension::topics::SURFACE,
        serde_json::json!({ "bytes": bytes }),
    );
    Ok(())
}

/// Ingest one surface message from the local bus.
///
/// A frame is applied to the instance it names, and a close disposes it. An
/// action is refused here on purpose: it must pass the host's authorization
/// pipeline, not a transport shortcut.
pub(crate) fn surface_ingest(bytes: &[u8]) -> Result<(), String> {
    let message = ferris_sdk::surface::decode(bytes).map_err(|error| error.to_string())?;
    match message {
        ferris_sdk::surface::SurfaceMessage::Frame { frame } => {
            let slot = HOST.get_or_init(|| Mutex::new(None));
            let mut guard = slot
                .lock()
                .map_err(|_| "the extension host lock is poisoned".to_string())?;
            let host = guard
                .as_mut()
                .ok_or_else(|| "the extension host is not open".to_string())?;
            host.surface_frame(&frame).map(|_| ())
        }
        ferris_sdk::surface::SurfaceMessage::Close { instance, .. } => {
            let slot = HOST.get_or_init(|| Mutex::new(None));
            let mut guard = slot
                .lock()
                .map_err(|_| "the extension host lock is poisoned".to_string())?;
            let host = guard
                .as_mut()
                .ok_or_else(|| "the extension host is not open".to_string())?;
            if host.surface_close_instance(instance) {
                Ok(())
            } else {
                Err("the instance is not mounted".to_string())
            }
        }
        ferris_sdk::surface::SurfaceMessage::Action { .. } => {
            Err("a surface action must go through the host pipeline, not the transport".to_string())
        }
    }
}

/// The extension themes the current configuration offers.
pub(crate) fn themes(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
) -> BTreeMap<String, ferris_sdk::ui::ThemePalette> {
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

/// Uninstall one package from the managed store.
pub(crate) fn uninstall(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    id: &str,
    purge_data: bool,
) -> Result<(), String> {
    let manager =
        super::framework::manager_for(extension_root(settings), workspace.map(Path::to_path_buf))
            .map_err(|error| error.to_string())?;
    manager
        .uninstall(id, purge_data)
        .map_err(|error| error.to_string())
}

/// Pin one installed version as the one the host loads.
///
/// The store keeps every version for rollback; switching is a state write, not
/// a reinstall, so a version already on disk is selected rather than installed
/// again (the installer refuses an existing version).
pub(crate) fn activate(
    settings: &crate::settings::Settings,
    workspace: Option<&Path>,
    id: &str,
    version: &str,
) -> Result<(), String> {
    let manager =
        super::framework::manager_for(extension_root(settings), workspace.map(Path::to_path_buf))
            .map_err(|error| error.to_string())?;
    manager
        .activate(id, version)
        .map_err(|error| error.to_string())
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

/// One extension tool the federation node may expose.
#[derive(Debug, Clone)]
pub(crate) struct NodeToolInfo {
    /// The model-facing, globally unique tool name (extension-qualified).
    pub(crate) qualified: String,
    /// The extension's display name.
    pub(crate) extension: String,
    /// The description the model sees.
    pub(crate) description: String,
    /// The argument schema the runtime declared.
    pub(crate) schema: Value,
}

/// The extension tools the loaded host can serve.
///
/// The list is empty until the host is open. It is deliberately built from
/// configured entries, not applied ones: the federation node is not a
/// preset-bound agent build, and the node's own expose list is the allowlist
/// that decides what it serves.
pub(crate) fn node_tool_catalogue() -> Vec<NodeToolInfo> {
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(guard) = slot.lock() else {
        return Vec::new();
    };
    let Some(host) = guard.as_ref() else {
        return Vec::new();
    };
    let mut tools = Vec::new();
    for entry in host
        .loaded
        .values()
        .filter(|entry| entry.configured && entry.runtime.is_some())
    {
        for descriptor in &entry.tools {
            tools.push(NodeToolInfo {
                qualified: entry.package.id().qualify(&descriptor.id),
                extension: entry.package.manifest.extension.name.clone(),
                description: descriptor.description.clone(),
                schema: descriptor.input_schema.clone(),
            });
        }
    }
    tools.sort_by(|left, right| left.qualified.cmp(&right.qualified));
    tools
}

/// Built-in extension tools, which live in linked-in packages rather than the
/// installed store. Keep in step with register_builtins.
const DECLARED_BUILTIN_TOOLS: [(&str, &str, &str); 3] = [
    (
        "extensions__list",
        "Extension Inspector",
        "List the extensions installed on this machine",
    ),
    (
        "diagnostics__graph",
        "Extension Diagnostics",
        "Return the declared extension graph",
    ),
    (
        "diagnostics__why",
        "Extension Diagnostics",
        "Explain why one extension is or is not applied",
    ),
];

/// The extension tools the installed manifests declare, without starting a
/// runtime.
///
/// This is what the pane lists and what the node uses to decide whether an
/// exposed id is an extension tool at all. Descriptions and schemas come from
/// the manifest only, so the catalogue stays cheap; the host's own catalogue
/// supplies the full descriptor once the node actually serves the tool.
pub(crate) fn declared_tool_catalogue(settings: &crate::settings::Settings) -> Vec<NodeToolInfo> {
    let config = HostConfig::from_settings(settings, None, None);
    let mut tools: Vec<NodeToolInfo> = DECLARED_BUILTIN_TOOLS
        .iter()
        .map(|(qualified, extension, description)| NodeToolInfo {
            qualified: qualified.to_string(),
            extension: extension.to_string(),
            description: description.to_string(),
            schema: serde_json::json!({ "type": "object" }),
        })
        .collect();
    let Ok(store) = ExtensionStore::open(&config.root) else {
        tools.sort_by(|left, right| left.qualified.cmp(&right.qualified));
        return tools;
    };
    let Ok((packages, _)) = store.scan_installed() else {
        tools.sort_by(|left, right| left.qualified.cmp(&right.qualified));
        return tools;
    };
    let packages = select_active_versions(packages, &store);
    for package in packages {
        if !package.report.is_valid()
            || !store.state_of(package.id()).enabled
            || config.disabled.contains(package.id().as_str())
        {
            continue;
        }
        for tool in &package.manifest.contributes.tools {
            let qualified = package.id().qualify(tool);
            if tools.iter().any(|info| info.qualified == qualified) {
                continue;
            }
            tools.push(NodeToolInfo {
                qualified,
                extension: package.manifest.extension.name.clone(),
                description: "Extension tool".to_string(),
                schema: serde_json::json!({ "type": "object" }),
            });
        }
    }
    tools.sort_by(|left, right| left.qualified.cmp(&right.qualified));
    tools
}

/// Open the extension host for a federation node when it is not open yet.
///
/// The node starts before any agent build, so it cannot borrow a session's
/// host. Opening here makes the extension runtimes available to the node's
/// proxy tools; a later UI build may reopen the host with a preset allowlist,
/// and the node's proxy looks the tool up again on every call for that reason.
pub(crate) fn ensure_host(settings: &crate::settings::Settings, workspace: Option<&Path>) -> bool {
    let config = HostConfig::from_settings(settings, workspace.map(Path::to_path_buf), None);
    let slot = HOST.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        return false;
    };
    if guard.as_ref().is_some_and(|host| host.config == config) {
        return true;
    }
    match ExtensionHost::open(config) {
        Ok(host) => {
            *guard = Some(host);
            true
        }
        Err(error) => {
            log::warn!("extension host for the federation node did not open: {error}");
            false
        }
    }
}

/// Run one extension tool the host owns.
///
/// The host is looked up on every call: a UI build can reopen it with a
/// different preset allowlist, so a handle captured at node assembly would go
/// stale. The extension's own capability policy still authorizes the call; the
/// federation grant is the separate gate in front of this function.
pub(crate) async fn call_node_tool(
    qualified: &str,
    arguments: Value,
) -> Result<Value, ExtensionError> {
    let parts = {
        let slot = HOST.get_or_init(|| Mutex::new(None));
        let guard = slot.lock().map_err(|_| ExtensionError::Denied {
            extension: "federation".to_string(),
            capability: "extension.call".to_string(),
            reason: "the extension host is poisoned".to_string(),
        })?;
        let host = guard.as_ref().ok_or_else(|| ExtensionError::Denied {
            extension: "federation".to_string(),
            capability: "extension.call".to_string(),
            reason: "the extension host is not running".to_string(),
        })?;
        let mut found = None;
        for entry in host
            .loaded
            .values()
            .filter(|entry| entry.configured && entry.runtime.is_some())
        {
            let Some(runtime) = entry.runtime.as_ref() else {
                continue;
            };
            if let Some(descriptor) = entry
                .tools
                .iter()
                .find(|descriptor| entry.package.id().qualify(&descriptor.id) == qualified)
            {
                found = Some((
                    Arc::clone(runtime),
                    Arc::clone(&host.policy),
                    entry.package.id().clone(),
                    entry.package.trust,
                    descriptor.clone(),
                    host.config.workspace.clone(),
                    entry.package.manifest.resources.execution_ms(),
                ));
                break;
            }
        }
        found.ok_or_else(|| ExtensionError::NotFound {
            id: qualified.to_string(),
        })?
    };
    let (runtime, policy, owner, trust, descriptor, workspace, timeout) = parts;
    let scope = ferris_sdk::capability::call_scope(&arguments, workspace.as_deref());
    ferris_sdk::capability::authorize_each(
        &policy,
        owner.as_str(),
        trust,
        &descriptor.capabilities,
        &scope,
    )?;
    let context = CallContext::new(owner, "federation-node", workspace, timeout);
    runtime
        .call_tool(descriptor.id.as_str(), arguments, &context)
        .await
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

/// Test-only first-party extension that publishes a typed counter service at
/// Host scope, so the production activation path's service publication is
/// exercised end to end (issue #302).
#[cfg(test)]
struct ServiceTestProvider;

#[cfg(test)]
const SERVICE_TEST_MANIFEST: &str = r#"
schema_version = "1.0"

[extension]
id = "com.bos.bsh.service-test"
name = "Service Test"
version = "1.0.0"
description = "Publishes a typed counter service for the activation test"
authors = ["BrickOS Team"]
license = "MIT"

[compatibility]
bsh = ">=3.0.0, <4.0.0"
extension_api = "^1.0"

[runtime]
kind = "builtin"
entry = "service-test"
startup = "on-demand"

[[services.provides]]
name = "com.bos.bsh.counter"
contract = "com.bos.bsh.counter-api"
version = "1.0.0"
scope = "host"
"#;

#[cfg(test)]
impl ferris_sdk::host_runtime::builtin::BuiltinExtension for ServiceTestProvider {
    fn manifest_toml(&self) -> &'static str {
        SERVICE_TEST_MANIFEST
    }
    fn tools(&self) -> Vec<ferris_sdk::host_runtime::contract::ToolDescriptor> {
        Vec::new()
    }
    fn call_tool(
        &self,
        _name: &str,
        _arguments: &serde_json::Value,
        _context: &ferris_sdk::host_runtime::contract::CallContext,
    ) -> Result<serde_json::Value, ferris_sdk::types::ExtensionError> {
        Err(ferris_sdk::types::ExtensionError::UnsupportedRuntime {
            kind: "service-test".to_string(),
        })
    }
    fn provided_services(&self) -> Vec<ferris_sdk::host_runtime::contract::ProvidedService> {
        vec![ferris_sdk::host_runtime::contract::ProvidedService::new(
            "com.bos.bsh.counter",
            "com.bos.bsh.counter-api",
            semver::Version::new(1, 0, 0),
            ferris_sdk::composable::Scope::Host,
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(42)),
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_sdk::capability::GrantScope;
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

    /// The production activation path publishes a first-party extension's
    /// typed service into the shared host registry, a consumer acquires it back
    /// typed, and unloading the provider revokes it (issue #302).
    #[test]
    fn the_activation_publishes_a_first_party_service_end_to_end() {
        let root = scratch("service-pub");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let mut agent = ::agent::Agent::from_config(::agent::AgentConfig::default());
        let mut registry = ::agent::plugins::PluginRegistry::new();
        let applied = host.apply_with_fibers(&mut agent, &mut registry, None, "session-test");

        // The service-test builtin published its typed service on activation.
        assert!(
            host.services().has_name("com.bos.bsh.counter"),
            "the activation published the first-party service"
        );
        // A consumer acquires it back as the concrete type, version-checked.
        let requirement = semver::VersionReq::parse("^1").unwrap();
        let lease = host
            .services()
            .acquire_contract::<std::sync::atomic::AtomicU64>(
                &ferris_sdk::composable::Scope::Host,
                "com.bos.bsh.counter",
                "com.bos.bsh.counter-api",
                &requirement,
            )
            .expect("a consumer acquires the typed service");
        assert_eq!(lease.load(std::sync::atomic::Ordering::SeqCst), 42);
        drop(lease);

        // Unloading the provider activation revokes only its service.
        let provider = applied
            .into_iter()
            .find(|activation| activation.extension_id() == "com.bos.bsh.service-test")
            .expect("the service provider was applied");
        provider.unload(&mut agent, &mut registry).expect("unload");
        assert!(
            !host.services().has_name("com.bos.bsh.counter"),
            "unload revoked the provider's service"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_manifest(root: &Path, legacy: &str) {
        let manifest =
            ferris_sdk::core_manifest::Manifest::parse(legacy, &root.join("extension.toml"))
                .expect("legacy manifest");
        let v2 = ferris_sdk::manifest_v2::ManifestV2::from_legacy(&manifest);
        std::fs::write(
            root.join("extension.toml"),
            v2.to_toml().expect("2.0 manifest"),
        )
        .expect("manifest");
    }

    fn write_package(root: &Path) {
        std::fs::create_dir_all(root.join("tools")).expect("tools");
        write_manifest(
            root,
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
        );
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
        write_manifest(
            root,
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
        );
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
        write_manifest(root, &manifest);
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
        let mut host = ExtensionHost::open(config(&root)).expect("host");
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
        // mcp-echo is on-demand: its tools are discovered when an agent build
        // activates it, not when the host scans the store (covered by the
        // factory test an_on_demand_mcp_package_is_not_discovered_by_the_factory).
        for qualified in [
            "hello__echo",
            "process-echo__echo",
            "wasm-echo__echo",
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
            metrics.applied, 5,
            "the builtin inspector, diagnostics, annotations, service-test and good package still apply"
        );
        let statuses = host.statuses();
        assert!(
            statuses
                .iter()
                .any(|status| status.id == "com.example.hosted"),
            "the good package is still listed"
        );
        let broken = statuses
            .iter()
            .find(|status| status.id == "com.example.broken")
            .expect("the broken package is still listed as installed");
        assert!(!broken.applied, "the broken package is not applied");
        assert!(broken.error.is_some(), "the broken manifest is reported");
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

    #[test]
    fn statuses_include_manager_installed_packages() {
        let root = scratch("status-manager");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/hello");
        host.store.install(&source).expect("install");
        let status = host
            .statuses()
            .into_iter()
            .find(|status| status.id == "com.example.hello")
            .expect("installed status");
        assert_eq!(status.state, LifecycleState::Installed);
        assert!(!status.applied);
    }

    #[tokio::test]
    async fn the_manager_activates_an_applied_package() {
        let root = scratch("manager-activation");
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/hello");
        let mut host = ExtensionHost::open(config(&root)).expect("host");
        host.store.install(&source).expect("install");
        host.rebuild().expect("rebuild");
        let activations = host
            .build_framework_activations()
            .expect("manager activation succeeds");
        activations
            .unload(&mut crate::extension::framework::HostNoopHost)
            .await
            .expect("unload");
    }

    /// A rebuild retires the runtime a mounted panel holds, and the panel's id
    /// still resolves to the live one. This is the regression behind
    /// `Panel error: extension lazy runtime error: runtime is shut down`: the
    /// view keeps a `PanelSurface` across generations, so both halves have to
    /// hold — the old handle is dead, and the id finds its replacement.
    #[test]
    fn a_rebuild_retires_the_runtime_an_open_panel_holds() {
        let root = scratch("panel-generation");
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/hello");
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
        let held = host
            .panel_surfaces()
            .into_iter()
            .find(|panel| panel.owner.as_str() == "com.example.hello")
            .expect("the hello panel is mounted");
        assert!(!held.runtime.is_shutdown(), "the first generation is live");

        host.rebuild().expect("second rebuild");
        assert!(
            held.runtime.is_shutdown(),
            "the rebuild retires the runtime the open view still holds"
        );
        let fresh = host
            .live_panel(&held.panel_id)
            .expect("the panel is offered by the current generation");
        assert!(
            !Arc::ptr_eq(&held.runtime, &fresh.runtime),
            "the lookup answers with the new runtime, not the retired one"
        );
        assert!(!fresh.runtime.is_shutdown(), "the replacement serves calls");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_framework_manager_opens_the_host_root() {
        let root = scratch("framework-manager");
        let host = ExtensionHost::open(config(&root)).expect("host");
        let manager = host.framework_manager().expect("manager");
        assert_eq!(manager.root(), root);
        assert!(manager.installed().expect("installed").is_empty());
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
        write_manifest(
            root,
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
        );
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
        write_manifest(
            &source,
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
        );
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

    struct BlockingActivate {
        started: Arc<AtomicBool>,
        shut: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl Runtime for BlockingActivate {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn activate(&self) -> Result<(), ExtensionError> {
            self.started.store(true, Ordering::SeqCst);
            futures::future::pending::<()>().await;
            Ok(())
        }

        async fn shutdown(&self) -> Result<(), ExtensionError> {
            self.shut.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn a_superseded_warm_start_is_cancelled_and_rolled_back() {
        let started = Arc::new(AtomicBool::new(false));
        let shut = Arc::new(AtomicBool::new(false));
        let runtime: Arc<dyn Runtime> = Arc::new(BlockingActivate {
            started: Arc::clone(&started),
            shut: Arc::clone(&shut),
        });
        let warm = start_warm(vec![("com.example.block".to_string(), runtime)]);
        for _ in 0..200 {
            if started.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(started.load(Ordering::SeqCst), "activation never started");
        warm.stop();
        assert!(
            shut.load(Ordering::SeqCst),
            "a cancelled activation must be shut back down, not published"
        );
    }
    #[test]
    fn unload_errors_keep_fiber_and_lease_failures_separate() {
        let fiber = ferris_sdk::composable::FiberTransitionError {
            from: ferris_sdk::composable::FiberState::Disposed,
            to: ferris_sdk::composable::FiberState::Active,
        };
        let both = combine_unload::<()>(Err(fiber), Err("lease failed".to_string()))
            .expect_err("both failures must surface");
        assert!(matches!(both, UnloadError::Both { .. }), "{both:?}");
        assert!(both.to_string().contains("lease failed"), "{both}");
        let lease_only = combine_unload::<()>(Ok(()), Err("lease failed".to_string()))
            .expect_err("lease failure must surface");
        assert!(
            matches!(lease_only, UnloadError::Lease(_)),
            "{lease_only:?}"
        );
        let fiber_only =
            combine_unload::<()>(Err(fiber), Ok(())).expect_err("fiber failure must surface");
        assert!(
            matches!(fiber_only, UnloadError::Fiber(_)),
            "{fiber_only:?}"
        );
    }
    #[test]
    fn the_event_pump_handle_joins_and_reports_a_stuck_worker() {
        let generation = Arc::new(AtomicU64::new(0));
        let (tx, done) = std::sync::mpsc::channel();
        let quick = std::thread::spawn(move || {
            let _ = tx.send(());
        });
        let pump = PumpHandle {
            generation: Arc::clone(&generation),
            done,
            thread: Some(quick),
        };
        assert!(pump.stop(), "an idle worker is joined");

        let (tx, done) = std::sync::mpsc::channel();
        let stuck = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let _ = tx.send(());
        });
        let pump = PumpHandle {
            generation,
            done,
            thread: Some(stuck),
        };
        let started = std::time::Instant::now();
        assert!(
            !pump.stop(),
            "a pump still in Runtime::pump is reported as pending"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }
    struct FailingShutdown;

    #[async_trait::async_trait]
    impl Runtime for FailingShutdown {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn shutdown(&self) -> Result<(), ExtensionError> {
            Err(ExtensionError::Runtime {
                extension: "com.example.fail".to_string(),
                message: "shutdown failed".to_string(),
            })
        }
    }

    struct HangingShutdown;

    #[async_trait::async_trait]
    impl Runtime for HangingShutdown {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn shutdown(&self) -> Result<(), ExtensionError> {
            futures::future::pending::<()>().await;
            Ok(())
        }
    }

    #[test]
    fn a_shutdown_report_names_failures_and_timeouts() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let runtimes: Vec<(String, Arc<dyn Runtime>)> = vec![
            (
                "ok".to_string(),
                Arc::new(CountShutdown(Arc::clone(&stopped))) as Arc<dyn Runtime>,
            ),
            (
                "err".to_string(),
                Arc::new(FailingShutdown) as Arc<dyn Runtime>,
            ),
            (
                "hang".to_string(),
                Arc::new(HangingShutdown) as Arc<dyn Runtime>,
            ),
        ];
        let report = ExtensionHost::stop_runtimes(runtimes);
        assert_eq!(report.completed, vec!["ok".to_string()]);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("shutdown failed"));
        assert_eq!(report.timed_out, vec!["hang".to_string()]);
        assert!(!report.is_clean());
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }
    struct CountShutdown(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl Runtime for CountShutdown {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::Builtin
        }

        async fn tools(&self) -> Result<Vec<ToolDescriptor>, ExtensionError> {
            Ok(Vec::new())
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: Value,
            _context: &CallContext,
        ) -> Result<Value, ExtensionError> {
            Ok(Value::Null)
        }

        async fn shutdown(&self) -> Result<(), ExtensionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn a_rebuild_stops_exactly_the_runtimes_it_replaces() {
        let stopped = Arc::new(AtomicUsize::new(0));
        let kept: Arc<dyn Runtime> = Arc::new(CountShutdown(Arc::clone(&stopped)));
        let replaced: Arc<dyn Runtime> = Arc::new(CountShutdown(Arc::clone(&stopped)));
        let previous = vec![
            ("kept".to_string(), Arc::clone(&kept)),
            ("replaced".to_string(), Arc::clone(&replaced)),
            ("replaced".to_string(), Arc::clone(&replaced)),
        ];
        let current = vec![Arc::clone(&kept)];
        let superseded = ExtensionHost::superseded_runtimes(previous.into_iter(), &current);
        assert_eq!(superseded.len(), 1, "duplicates are stopped once");
        assert!(Arc::ptr_eq(&superseded[0].1, &replaced));
        let report = ExtensionHost::stop_runtimes(superseded);
        assert!(report.is_clean(), "{}", report.summary());
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        assert_eq!(
            stopped.load(Ordering::SeqCst),
            1,
            "the kept runtime is not stopped"
        );
    }

    /// A shared-scope service published into the host registry is resolvable
    /// by another consumer and revoked when its provider unloads (issue #302).
    /// This is the lifecycle-owner contract provide_service now uses for
    /// Host/Workspace/Session scopes: the registry owns the registration, the
    /// provider holds a revocation token.
    #[test]
    fn a_shared_scope_service_is_resolvable_and_revocable() {
        use ferris_sdk::composable::{Scope, ServiceRegistry};
        use std::sync::Arc;

        #[derive(Clone)]
        struct Hello;
        impl Hello {
            fn greet(&self) -> String {
                "hello".to_string()
            }
        }

        let registry = ServiceRegistry::new();
        let version = semver::Version::new(1, 0, 0);
        // Provider publishes at Host scope into the shared registry.
        let token = registry.provide_contract(
            Scope::Host,
            "greeter",
            "test.greeter",
            version.clone(),
            Arc::new(Hello),
        );
        // A different consumer resolves it from the same shared registry.
        let requirement = semver::VersionReq::parse("^1").unwrap();
        let lease = registry
            .acquire_contract::<Hello>(&Scope::Host, "greeter", "test.greeter", &requirement)
            .expect("a consumer resolves the shared service");
        assert_eq!(lease.greet(), "hello");
        drop(lease);
        // The provider unloads: revoking its token removes the service.
        assert!(registry.revoke(&token));
        assert!(registry
            .acquire_contract::<Hello>(&Scope::Host, "greeter", "test.greeter", &requirement)
            .is_none());
    }
}
