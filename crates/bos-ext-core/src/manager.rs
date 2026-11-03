//! Host-neutral extension manager.
//!
//! Phase 3 starts with the canonical store boundary. Installation, plan/apply
//! and reload are layered on top of this manager in later commits.

use std::path::PathBuf;
use std::sync::Arc;

use crate::package::ExtensionStore;
use crate::{ErrorCode, ExtError, HostSpec};

/// One installed extension version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledExtension {
    /// Extension id.
    pub id: String,
    /// Installed version.
    pub version: String,
}

/// The manager owns one canonical store rooted at HostSpec::root.
pub struct ExtensionManager {
    host: Arc<dyn HostSpec>,
    store: ExtensionStore,
}

impl ExtensionManager {
    /// Open the canonical store for a host.
    pub fn open(host: Arc<dyn HostSpec>) -> Result<Self, ExtError> {
        let store = ExtensionStore::open(host.root()).map_err(|error| ExtError {
            code: ErrorCode::Package,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        Ok(Self { host, store })
    }

    /// The host this manager serves.
    pub fn host(&self) -> &Arc<dyn HostSpec> {
        &self.host
    }

    /// The canonical store root.
    pub fn root(&self) -> PathBuf {
        self.store.root().to_path_buf()
    }

    /// Installed id/version pairs, sorted by id and version.
    pub fn installed(&self) -> Result<Vec<InstalledExtension>, ExtError> {
        let registry = self.store.read_registry().map_err(|error| ExtError {
            code: ErrorCode::Package,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        let mut installed = Vec::new();
        for (id, versions) in registry.extensions {
            for version in versions.keys() {
                installed.push(InstalledExtension {
                    id: id.clone(),
                    version: version.clone(),
                });
            }
        }
        installed.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapabilityRegistry, ScopePath, ServiceContractRegistry};

    struct DemoHost {
        root: PathBuf,
        capabilities: CapabilityRegistry,
        contracts: ServiceContractRegistry,
    }

    impl HostSpec for DemoHost {
        fn root(&self) -> PathBuf {
            self.root.clone()
        }

        fn scope_path(&self) -> ScopePath {
            ScopePath::default()
        }

        fn capabilities(&self) -> &CapabilityRegistry {
            &self.capabilities
        }

        fn contracts(&self) -> &ServiceContractRegistry {
            &self.contracts
        }
    }

    #[test]
    fn the_manager_opens_the_canonical_store() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let manager = ExtensionManager::open(host).expect("manager");
        assert_eq!(manager.root(), root.path());
        assert!(manager.installed().expect("installed").is_empty());
    }
}
