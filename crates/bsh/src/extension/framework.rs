//! BSH adapter for the host-neutral extension manager.

use std::path::PathBuf;
use std::sync::Arc;

use ferris_core::capability::Capability;
use ferris_core::manager::ExtensionManager;
use ferris_core::{
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

fn risk(risk: ferris_core::capability::Risk) -> Risk {
    use ferris_core::capability::Risk as CapabilityRisk;
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
    policy: Option<Arc<ferris_core::capability::PolicyEngine>>,
    events: Option<Arc<ferris_core::events::EventBus>>,
    trust: ferris_core::types::TrustLevel,
    builtins: std::collections::BTreeMap<String, Arc<dyn ferris_runtime::Runtime>>,
    legacy: std::collections::BTreeMap<String, Arc<dyn ferris_runtime::Runtime>>,
}

impl FerrisRuntimeResolver {
    /// Build a resolver for one workspace.
    pub(crate) fn new(workspace: Option<PathBuf>) -> Self {
        Self {
            workspace,
            policy: None,
            events: None,
            trust: ferris_core::types::TrustLevel::Community,
            builtins: std::collections::BTreeMap::new(),
            legacy: std::collections::BTreeMap::new(),
        }
    }

    /// Register one already-built host runtime for manager activation.
    #[must_use]
    pub(crate) fn with_legacy(
        mut self,
        id: &str,
        runtime: Arc<dyn ferris_runtime::Runtime>,
    ) -> Self {
        self.legacy.insert(id.to_string(), runtime);
        self
    }

    /// Register one linked-in builtin runtime.
    #[must_use]
    pub(crate) fn with_builtin(
        mut self,
        id: &str,
        runtime: Arc<dyn ferris_runtime::Runtime>,
    ) -> Self {
        self.builtins.insert(id.to_string(), runtime);
        self
    }

    /// Attach the policy and event bus used by sandboxed runtimes.
    #[must_use]
    pub(crate) fn with_ports(
        mut self,
        policy: Arc<ferris_core::capability::PolicyEngine>,
        events: Arc<ferris_core::events::EventBus>,
        trust: ferris_core::types::TrustLevel,
    ) -> Self {
        self.policy = Some(policy);
        self.events = Some(events);
        self.trust = trust;
        self
    }
}

impl ferris_core::RuntimeResolver for FerrisRuntimeResolver {
    fn create(
        &self,
        manifest: &ferris_core::manifest::Manifest,
        root: &std::path::Path,
        data_root: &std::path::Path,
    ) -> Result<Arc<dyn ferris_core::Runtime>, ExtError> {
        if let Some(runtime) = self.legacy.get(manifest.id().as_str()).cloned() {
            return Ok(Arc::new(ferris_runtime::adapter::LegacyAdapter::new(
                manifest.id().clone(),
                root.to_path_buf(),
                self.workspace.clone(),
                runtime,
            )));
        }
        if manifest.runtime.kind == ferris_core::types::RuntimeKind::Builtin {
            let runtime = self
                .builtins
                .get(manifest.id().as_str())
                .cloned()
                .ok_or_else(|| ExtError {
                    code: ferris_core::ErrorCode::Runtime,
                    message: format!("builtin {} is not registered", manifest.id()),
                    details: serde_json::Value::Null,
                })?;
            return Ok(Arc::new(ferris_runtime::adapter::LegacyAdapter::new(
                manifest.id().clone(),
                root.to_path_buf(),
                self.workspace.clone(),
                runtime,
            )));
        }
        let runtime = match manifest.runtime.kind {
            ferris_core::types::RuntimeKind::Declarative => {
                let inner = ferris_runtime::declarative::DeclarativeRuntime::new(root, manifest)
                    .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            ferris_core::types::RuntimeKind::Process => {
                let inner = ferris_runtime::process::ProcessRuntime::new(manifest, root)
                    .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            ferris_core::types::RuntimeKind::Cordis => {
                let inner = ferris_runtime::cordis::CordisRuntime::new(
                    manifest,
                    root,
                    data_root,
                    Arc::new(super::terminal::HostTerminal),
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            ferris_core::types::RuntimeKind::Extism => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_runtime::extism::ExtismRuntime::new(
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
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            ferris_core::types::RuntimeKind::WasmComponent => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_runtime::wasm::WasmRuntime::new(
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
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            ferris_core::types::RuntimeKind::Mcp => {
                let policy = self.policy.clone().ok_or_else(|| missing_port("policy"))?;
                let events = self.events.clone().ok_or_else(|| missing_port("events"))?;
                let inner = ferris_runtime::mcp::McpRuntime::new(
                    manifest,
                    root,
                    "{}".to_string(),
                    policy,
                    self.trust,
                    events,
                )
                .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn ferris_runtime::Runtime>
            }
            other => {
                return Err(ExtError {
                    code: ferris_core::ErrorCode::Runtime,
                    message: format!("runtime {other} is not wired through the manager yet"),
                    details: serde_json::Value::Null,
                })
            }
        };
        Ok(Arc::new(ferris_runtime::adapter::LegacyAdapter::new(
            manifest.id().clone(),
            root.to_path_buf(),
            self.workspace.clone(),
            runtime,
        )))
    }
}

fn missing_port(name: &str) -> ExtError {
    ExtError {
        code: ferris_core::ErrorCode::Runtime,
        message: format!("runtime resolver has no {name} port"),
        details: serde_json::Value::Null,
    }
}

/// Sink used when the host applies its own agent contributions separately.
pub(crate) struct HostNoopSink;

/// Lease for a contributed runtime the host already owns.
pub(crate) struct HostNoopLease;

#[async_trait::async_trait]
impl ferris_core::ContributionLease for HostNoopLease {
    async fn revoke(&self) -> Result<(), ExtError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ferris_core::ContributionSink for HostNoopSink {
    async fn apply(
        &self,
        _contribution: ferris_core::Contribution,
    ) -> Result<Box<dyn ferris_core::ContributionLease>, ExtError> {
        Ok(Box::new(HostNoopLease))
    }
}

fn runtime_error(error: impl std::fmt::Display) -> ExtError {
    ExtError {
        code: ferris_core::ErrorCode::Runtime,
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
impl ferris_core::ContributionLease for NoopLease {
    async fn revoke(&self) -> Result<(), ExtError> {
        Ok(())
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl ferris_core::ContributionSink for CollectSink {
    async fn apply(
        &self,
        _contribution: ferris_core::Contribution,
    ) -> Result<Box<dyn ferris_core::ContributionLease>, ExtError> {
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
        let active = manager.apply(&plan).await.expect("apply");
        assert_eq!(active.len(), 1);
        assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 1);
        active.unload().await.expect("unload");
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
        let active = manager.apply(&plan).await.expect("apply");
        assert_eq!(active.len(), 1);
        assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 1);
        active.unload().await.expect("unload");
    }

    #[tokio::test]
    async fn the_manager_applies_a_cordis_package() {
        if ferris_runtime::bridge::find_interpreter().is_none() {
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
        let active = manager.apply(&plan).await.expect("apply");
        assert_eq!(active.len(), 1);
        assert!(applied.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        active.unload().await.expect("unload");
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
            ferris_core::capability::PolicyEngine::open(
                temp.path().join("grants.toml"),
                temp.path().join("audit.log"),
                ferris_core::capability::PolicyMode::Enforce,
            )
            .expect("policy"),
        );
        let resolver = FerrisRuntimeResolver::new(None).with_ports(
            policy,
            ferris_core::events::EventBus::new(),
            ferris_core::types::TrustLevel::Community,
        );
        let applied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(resolver),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager.apply(&plan).await.expect("apply");
        assert_eq!(active.len(), 1);
        assert!(applied.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        active.unload().await.expect("unload");
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
