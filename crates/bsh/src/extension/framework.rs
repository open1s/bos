//! BSH adapter for the host-neutral extension manager.

use std::path::PathBuf;
use std::sync::Arc;

use ferris_sdk::capability::Capability;
use ferris_sdk::manager::ExtensionManager;
use ferris_sdk::{
    CapabilityRegistry, CapabilitySpec, ExtError, HostSpec, Risk, Scope, ScopePath,
    ServiceContractRegistry,
};

/// Host facts BSH passes to the framework manager.
pub(crate) struct FerrisHostSpec {
    root: PathBuf,
    workspace: Option<PathBuf>,
    capabilities: CapabilityRegistry,
    contracts: ServiceContractRegistry,
}

impl FerrisHostSpec {
    /// Build the adapter for one managed root and workspace.
    pub(crate) fn new(root: PathBuf, workspace: Option<PathBuf>) -> Self {
        let mut capabilities = CapabilityRegistry::default();
        for capability in Capability::ALL {
            capabilities.register(CapabilitySpec {
                id: capability.as_str().to_string(),
                description: capability.description().to_string(),
                risk: risk(capability.risk()),
            });
        }
        Self {
            root,
            workspace,
            capabilities,
            contracts: ServiceContractRegistry::default(),
        }
    }
}

fn risk(risk: ferris_sdk::capability::Risk) -> Risk {
    use ferris_sdk::capability::Risk as CapabilityRisk;
    match risk {
        CapabilityRisk::Low => Risk::Low,
        CapabilityRisk::Medium => Risk::Medium,
        CapabilityRisk::High => Risk::High,
        CapabilityRisk::Critical => Risk::Critical,
    }
}

impl HostSpec for FerrisHostSpec {
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    fn scope_path(&self) -> ScopePath {
        self.workspace
            .as_ref()
            .map(|workspace| {
                ScopePath::new(vec![Scope {
                    kind: "workspace".to_string(),
                    id: workspace.display().to_string(),
                }])
            })
            .unwrap_or_default()
    }

    fn capabilities(&self) -> &CapabilityRegistry {
        &self.capabilities
    }

    fn contracts(&self) -> &ServiceContractRegistry {
        &self.contracts
    }
}

/// Open the framework manager for one BSH host configuration.
pub(crate) fn manager_for(
    root: PathBuf,
    workspace: Option<PathBuf>,
) -> Result<ExtensionManager, ExtError> {
    ExtensionManager::open(Arc::new(FerrisHostSpec::new(root, workspace)))
}

/// BSH runtime resolver: wraps the standard runtimes in the generic adapter.
pub(crate) struct FerrisRuntimeResolver {
    workspace: Option<PathBuf>,
    policy: Option<Arc<ferris_sdk::capability::PolicyEngine>>,
    events: Option<Arc<ferris_sdk::events::EventBus>>,
    trust: ferris_sdk::types::TrustLevel,
    builtins: std::collections::BTreeMap<String, Arc<dyn ferris_sdk::host_runtime::Runtime>>,
    legacy: std::collections::BTreeMap<String, Arc<dyn ferris_sdk::host_runtime::Runtime>>,
}

impl FerrisRuntimeResolver {
    /// Build a resolver for one workspace.
    pub(crate) fn new(workspace: Option<PathBuf>) -> Self {
        Self {
            workspace,
            policy: None,
            events: None,
            trust: ferris_sdk::types::TrustLevel::Community,
            builtins: std::collections::BTreeMap::new(),
            legacy: std::collections::BTreeMap::new(),
        }
    }

    /// Register one already-built host runtime for manager activation.
    #[must_use]
    pub(crate) fn with_legacy(
        mut self,
        id: &str,
        runtime: Arc<dyn ferris_sdk::host_runtime::Runtime>,
    ) -> Self {
        self.legacy.insert(id.to_string(), runtime);
        self
    }

    /// Register one linked-in builtin runtime.
    #[must_use]
    pub(crate) fn with_builtin(
        mut self,
        id: &str,
        runtime: Arc<dyn ferris_sdk::host_runtime::Runtime>,
    ) -> Self {
        self.builtins.insert(id.to_string(), runtime);
        self
    }

    /// Attach the policy and event bus used by sandboxed runtimes.
    #[must_use]
    pub(crate) fn with_ports(
        mut self,
        policy: Arc<ferris_sdk::capability::PolicyEngine>,
        events: Arc<ferris_sdk::events::EventBus>,
        trust: ferris_sdk::types::TrustLevel,
    ) -> Self {
        self.policy = Some(policy);
        self.events = Some(events);
        self.trust = trust;
        self
    }
}

impl ferris_sdk::RuntimeResolver for FerrisRuntimeResolver {
    fn create(
        &self,
        manifest: &ferris_sdk::core_manifest::Manifest,
        root: &std::path::Path,
        data_root: &std::path::Path,
    ) -> Result<Arc<dyn ferris_sdk::Runtime>, ExtError> {
        if let Some(runtime) = self.legacy.get(manifest.id().as_str()).cloned() {
            return Ok(Arc::new(
                ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                    manifest.id().clone(),
                    root.to_path_buf(),
                    self.workspace.clone(),
                    runtime,
                ),
            ));
        }
        if manifest.runtime.kind == ferris_sdk::types::RuntimeKind::Builtin {
            let runtime = self
                .builtins
                .get(manifest.id().as_str())
                .cloned()
                .ok_or_else(|| ExtError {
                    code: ferris_sdk::ErrorCode::Runtime,
                    message: format!("builtin {} is not registered", manifest.id()),
                    details: serde_json::Value::Null,
                })?;
            return Ok(Arc::new(
                ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                    manifest.id().clone(),
                    root.to_path_buf(),
                    self.workspace.clone(),
                    runtime,
                ),
            ));
        }
        let runtime = match manifest.runtime.kind {
            ferris_sdk::types::RuntimeKind::Declarative => {
                let inner =
                    ferris_sdk::host_runtime::declarative::DeclarativeRuntime::new(root, manifest)
                        .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            ferris_sdk::types::RuntimeKind::Process => {
                let inner = ferris_sdk::host_runtime::process::ProcessRuntime::new(manifest, root)
                    .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            ferris_sdk::types::RuntimeKind::Cordis => {
                let inner = ferris_sdk::host_runtime::cordis::CordisRuntime::new(
                    manifest,
                    root,
                    data_root,
                    Arc::new(super::terminal::HostTerminal),
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            ferris_sdk::types::RuntimeKind::Extism => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_sdk::host_runtime::extism::ExtismRuntime::new(
                    manifest,
                    root,
                    data_root,
                    policy,
                    events,
                    self.workspace.clone(),
                    "{}".to_string(),
                    self.trust,
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            ferris_sdk::types::RuntimeKind::WasmComponent => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_sdk::host_runtime::wasm::WasmRuntime::new(
                    manifest,
                    root,
                    data_root,
                    policy,
                    events,
                    self.workspace.clone(),
                    "{}".to_string(),
                    self.trust,
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            ferris_sdk::types::RuntimeKind::Mcp => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_sdk::host_runtime::mcp::McpRuntime::new(
                    manifest,
                    root,
                    "{}".to_string(),
                    policy,
                    self.trust,
                    events,
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_sdk::host_runtime::Runtime>
            }
            other => {
                return Err(ExtError {
                    code: ferris_sdk::ErrorCode::Runtime,
                    message: format!("runtime {other} is not wired through the manager yet"),
                    details: serde_json::Value::Null,
                })
            }
        };
        Ok(Arc::new(
            ferris_sdk::host_runtime::adapter::LegacyAdapter::new(
                manifest.id().clone(),
                root.to_path_buf(),
                self.workspace.clone(),
                runtime,
            ),
        ))
    }
}

fn missing_port(name: &str) -> ExtError {
    ExtError {
        code: ferris_sdk::ErrorCode::Runtime,
        message: format!("runtime resolver has no {name} port"),
        details: serde_json::Value::Null,
    }
}

/// Host sink that installs agent contributions for the manager.
pub(crate) struct HostContributionSink {
    bundles: std::sync::Mutex<std::collections::HashMap<String, super::agent::Contributed>>,
    leases: std::sync::Mutex<std::collections::HashMap<String, super::agent::ContributionLease>>,
    infos: std::sync::Mutex<std::collections::HashMap<String, crate::plugins::PluginInfo>>,
    policy: Arc<ferris_sdk::capability::PolicyEngine>,
    events: Arc<ferris_sdk::events::EventBus>,
    workspace: Option<PathBuf>,
}

impl HostContributionSink {
    /// Build the sink for one agent build.
    pub(crate) fn new(
        policy: Arc<ferris_sdk::capability::PolicyEngine>,
        events: Arc<ferris_sdk::events::EventBus>,
        workspace: Option<PathBuf>,
    ) -> Self {
        Self {
            bundles: std::sync::Mutex::new(std::collections::HashMap::new()),
            leases: std::sync::Mutex::new(std::collections::HashMap::new()),
            infos: std::sync::Mutex::new(std::collections::HashMap::new()),
            policy,
            events,
            workspace,
        }
    }

    /// Register one extension's host bundle before manager apply.
    pub(crate) fn insert(&self, extension: &str, contributed: super::agent::Contributed) {
        if let Ok(mut bundles) = self.bundles.lock() {
            bundles.insert(extension.to_string(), contributed);
        }
    }

    /// Take the installed info for one extension.
    pub(crate) fn take_info(&self, extension: &str) -> Option<crate::plugins::PluginInfo> {
        self.infos
            .lock()
            .ok()
            .and_then(|mut infos| infos.remove(extension))
    }

    /// Clone the host lease for suspend/resume bookkeeping.
    pub(crate) fn lease(&self, extension: &str) -> Option<super::agent::ContributionLease> {
        self.leases
            .lock()
            .ok()
            .and_then(|leases| leases.get(extension).cloned())
    }
}

#[async_trait::async_trait]
impl ferris_sdk::ContributionSink for HostContributionSink {
    async fn apply(
        &self,
        extension: &str,
        _contributions: Vec<ferris_sdk::Contribution>,
        agent: &mut (dyn std::any::Any + Send),
        registry: &mut (dyn std::any::Any + Send),
    ) -> Result<Box<dyn ferris_sdk::ContributionLease>, ExtError> {
        let agent = agent
            .downcast_mut::<::agent::Agent>()
            .ok_or_else(|| ExtError {
                code: ferris_sdk::ErrorCode::Contribution,
                message: "host sink expected an agent".to_string(),
                details: serde_json::Value::Null,
            })?;
        let registry = registry
            .downcast_mut::<::agent::plugins::PluginRegistry>()
            .ok_or_else(|| ExtError {
                code: ferris_sdk::ErrorCode::Contribution,
                message: "host sink expected a plugin registry".to_string(),
                details: serde_json::Value::Null,
            })?;
        let bundle = self
            .bundles
            .lock()
            .ok()
            .and_then(|mut bundles| bundles.remove(extension))
            .ok_or_else(|| ExtError {
                code: ferris_sdk::ErrorCode::Contribution,
                message: format!("no host bundle for {extension}"),
                details: serde_json::Value::Null,
            })?;
        let (info, lease) = super::agent::install_with_lease(
            agent,
            registry,
            &bundle,
            Arc::clone(&self.policy),
            Arc::clone(&self.events),
            self.workspace.clone(),
        );
        if let Ok(mut infos) = self.infos.lock() {
            infos.insert(extension.to_string(), info);
        }
        if let Ok(mut leases) = self.leases.lock() {
            leases.insert(extension.to_string(), lease.clone());
        }
        Ok(Box::new(lease))
    }
}

/// Sink used when the host applies its own agent contributions separately.
pub(crate) struct HostNoopSink;

/// Lease for a contributed runtime the host already owns.
pub(crate) struct HostNoopLease;

pub(crate) struct HostNoopHost;

impl ferris_sdk::LeaseHost for HostNoopHost {
    fn remove_tools(&mut self, _owner: &str, _tools: &[String]) -> usize {
        0
    }
    fn remove_prompts(&mut self, _owner: &str, _sections: &[String]) -> usize {
        0
    }
    fn remove_skills(&mut self, _owner: &str, _paths: &[std::path::PathBuf]) -> usize {
        0
    }
    fn remove_registry_owner(&mut self, _owner: &str) -> usize {
        0
    }
}

#[async_trait::async_trait]
impl ferris_sdk::ContributionLease for HostNoopLease {
    async fn revoke(&self, _host: &mut dyn ferris_sdk::LeaseHost) -> Result<(), ExtError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ferris_sdk::ContributionSink for HostNoopSink {
    async fn apply(
        &self,
        _extension: &str,
        _contributions: Vec<ferris_sdk::Contribution>,
        _agent: &mut (dyn std::any::Any + Send),
        _registry: &mut (dyn std::any::Any + Send),
    ) -> Result<Box<dyn ferris_sdk::ContributionLease>, ExtError> {
        Ok(Box::new(HostNoopLease))
    }
}

fn runtime_error(error: impl std::fmt::Display) -> ExtError {
    ExtError {
        code: ferris_sdk::ErrorCode::Runtime,
        message: error.to_string(),
        details: serde_json::Value::Null,
    }
}

#[cfg(test)]
struct CollectSink(Arc<std::sync::atomic::AtomicUsize>);

#[cfg(test)]
struct NoopLease;

#[cfg(test)]
#[async_trait::async_trait]
impl ferris_sdk::ContributionLease for NoopLease {
    async fn revoke(&self, _host: &mut dyn ferris_sdk::LeaseHost) -> Result<(), ExtError> {
        Ok(())
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl ferris_sdk::ContributionSink for CollectSink {
    async fn apply(
        &self,
        _extension: &str,
        _contributions: Vec<ferris_sdk::Contribution>,
        _agent: &mut (dyn std::any::Any + Send),
        _registry: &mut (dyn std::any::Any + Send),
    ) -> Result<Box<dyn ferris_sdk::ContributionLease>, ExtError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Box::new(NoopLease))
    }
}

#[cfg(test)]
mod manager_tests {
    use super::*;

    #[tokio::test]
    async fn the_manager_applies_a_declarative_package() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let temp = tempfile::tempdir().expect("tempdir");
        let manager = manager_for(temp.path().to_path_buf(), None).expect("manager");
        manager.install(&source).expect("install");
        let plan = manager.plan().expect("plan");
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(FerrisRuntimeResolver::new(None)),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut HostNoopHost)
            .await
            .expect("apply");
        assert_eq!(active.len(), 1);
        assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 1);
        active.unload(&mut HostNoopHost).await.expect("unload");
    }

    #[tokio::test]
    async fn the_manager_applies_a_process_package() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/extensions/process-echo");
        let temp = tempfile::tempdir().expect("tempdir");
        let manager = manager_for(temp.path().to_path_buf(), None).expect("manager");
        manager.install(&source).expect("install");
        let plan = manager.plan().expect("plan");
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(FerrisRuntimeResolver::new(None)),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut HostNoopHost)
            .await
            .expect("apply");
        assert_eq!(active.len(), 1);
        assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 1);
        active.unload(&mut HostNoopHost).await.expect("unload");
    }

    #[tokio::test]
    async fn the_manager_applies_a_cordis_package() {
        if ferris_sdk::host_runtime::bridge::find_interpreter().is_none() {
            return;
        }
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/cordis-echo");
        let temp = tempfile::tempdir().expect("tempdir");
        let manager = manager_for(temp.path().to_path_buf(), None).expect("manager");
        manager.install(&source).expect("install");
        let plan = manager.plan().expect("plan");
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(FerrisRuntimeResolver::new(None)),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut HostNoopHost)
            .await
            .expect("apply");
        assert_eq!(active.len(), 1);
        assert!(applied.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        active.unload(&mut HostNoopHost).await.expect("unload");
    }

    #[tokio::test]
    async fn the_manager_applies_an_extism_package() {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/extism-echo");
        let temp = tempfile::tempdir().expect("tempdir");
        let manager = manager_for(temp.path().to_path_buf(), None).expect("manager");
        manager.install(&source).expect("install");
        let plan = manager.plan().expect("plan");
        let policy = Arc::new(
            ferris_sdk::capability::PolicyEngine::open(
                temp.path().join("grants.toml"),
                temp.path().join("audit.log"),
                ferris_sdk::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let resolver = FerrisRuntimeResolver::new(None).with_ports(
            policy,
            ferris_sdk::events::EventBus::new(),
            ferris_sdk::types::TrustLevel::Community,
        );
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(resolver),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut HostNoopHost)
            .await
            .expect("apply");
        assert_eq!(active.len(), 1);
        assert!(applied.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        active.unload(&mut HostNoopHost).await.expect("unload");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapter_registers_the_bsh_capability_vocabulary() {
        let spec = FerrisHostSpec::new(PathBuf::from("/tmp/bsh-ext"), None);
        assert_eq!(
            spec.capabilities()
                .get("agent.tools.register")
                .map(|spec| spec.id.as_str()),
            Some("agent.tools.register")
        );
        assert!(spec.scope_path().levels().is_empty());
    }
}
