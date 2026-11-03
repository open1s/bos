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
