//! BSH adapter for the host-neutral extension manager.

use std::path::PathBuf;
use std::sync::Arc;

use bos_ext_core::capability::Capability;
use bos_ext_core::manager::ExtensionManager;
use bos_ext_core::{
    CapabilityRegistry, CapabilitySpec, ExtError, HostSpec, Risk, Scope, ScopePath,
    ServiceContractRegistry,
};

/// Host facts BSH passes to the framework manager.
pub(crate) struct BosHostSpec {
    root: PathBuf,
    workspace: Option<PathBuf>,
    capabilities: CapabilityRegistry,
    contracts: ServiceContractRegistry,
}

impl BosHostSpec {
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

fn risk(risk: bos_ext_core::capability::Risk) -> Risk {
    use bos_ext_core::capability::Risk as CapabilityRisk;
    match risk {
        CapabilityRisk::Low => Risk::Low,
        CapabilityRisk::Medium => Risk::Medium,
        CapabilityRisk::High => Risk::High,
        CapabilityRisk::Critical => Risk::Critical,
    }
}

impl HostSpec for BosHostSpec {
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
    ExtensionManager::open(Arc::new(BosHostSpec::new(root, workspace)))
}

/// BSH runtime resolver: wraps the standard runtimes in the generic adapter.
pub(crate) struct BosRuntimeResolver {
    workspace: Option<PathBuf>,
}

impl BosRuntimeResolver {
    /// Build a resolver for one workspace.
    pub(crate) fn new(workspace: Option<PathBuf>) -> Self {
        Self { workspace }
    }
}

impl bos_ext_core::RuntimeResolver for BosRuntimeResolver {
    fn create(
        &self,
        manifest: &bos_ext_core::manifest::Manifest,
        root: &std::path::Path,
        _data_root: &std::path::Path,
    ) -> Result<Arc<dyn bos_ext_core::Runtime>, ExtError> {
        let runtime = match manifest.runtime.kind {
            bos_ext_core::types::RuntimeKind::Declarative => {
                let inner = bos_ext_runtime::declarative::DeclarativeRuntime::new(root, manifest)
                    .map_err(runtime_error)?;
                Arc::new(inner) as Arc<dyn bos_ext_runtime::Runtime>
            }
            other => {
                return Err(ExtError {
                    code: bos_ext_core::ErrorCode::Runtime,
                    message: format!("runtime {other} is not wired through the manager yet"),
                    details: serde_json::Value::Null,
                })
            }
        };
        Ok(Arc::new(bos_ext_runtime::adapter::LegacyAdapter::new(
            manifest.id().clone(),
            root.to_path_buf(),
            self.workspace.clone(),
            runtime,
        )))
    }
}

fn runtime_error(error: impl std::fmt::Display) -> ExtError {
    ExtError {
        code: bos_ext_core::ErrorCode::Runtime,
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
impl bos_ext_core::ContributionLease for NoopLease {
    async fn revoke(&self) -> Result<(), ExtError> {
        Ok(())
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl bos_ext_core::ContributionSink for CollectSink {
    async fn apply(
        &self,
        _contribution: bos_ext_core::Contribution,
    ) -> Result<Box<dyn bos_ext_core::ContributionLease>, ExtError> {
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
            Arc::new(BosRuntimeResolver::new(None)),
            Arc::new(CollectSink(Arc::clone(&applied))),
        );
        let active = manager.apply(&plan).await.expect("apply");
        assert_eq!(active.len(), 1);
        assert_eq!(applied.load(std::sync::atomic::Ordering::SeqCst), 1);
        active.unload().await.expect("unload");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapter_registers_the_bsh_capability_vocabulary() {
        let spec = BosHostSpec::new(PathBuf::from("/tmp/bsh-ext"), None);
        assert_eq!(
            spec.capabilities()
                .get("agent.tools.register")
                .map(|spec| spec.id.as_str()),
            Some("agent.tools.register")
        );
        assert!(spec.scope_path().levels().is_empty());
    }
}
