//! Host-neutral extension manager.
//!
//! Phase 3 starts with the canonical store boundary. Installation, plan/apply
//! and reload are layered on top of this manager in later commits.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::package::ExtensionStore;
use crate::types::ExtensionId;
use crate::{
    ContributionLease, ContributionSink, ErrorCode, ExtError, HostSpec, LeaseHost, Runtime,
    RuntimeContext, RuntimeResolver,
};

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
    runtimes: Option<Arc<dyn RuntimeResolver>>,
    contributions: Option<Arc<dyn ContributionSink>>,
}

impl ExtensionManager {
    /// Open the canonical store for a host.
    pub fn open(host: Arc<dyn HostSpec>) -> Result<Self, ExtError> {
        let store = ExtensionStore::open(host.root()).map_err(|error| ExtError {
            code: ErrorCode::Package,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        Ok(Self {
            host,
            store,
            runtimes: None,
            contributions: None,
        })
    }

    /// Attach the runtime resolver and contribution sink used by apply.
    #[must_use]
    pub fn with_ports(
        mut self,
        runtimes: Arc<dyn RuntimeResolver>,
        contributions: Arc<dyn ContributionSink>,
    ) -> Self {
        self.runtimes = Some(runtimes);
        self.contributions = Some(contributions);
        self
    }

    /// The host this manager serves.
    pub fn host(&self) -> &Arc<dyn HostSpec> {
        &self.host
    }

    /// The canonical store root.
    pub fn root(&self) -> PathBuf {
        self.store.root().to_path_buf()
    }

    /// The canonical package root for one installed version.
    pub fn package_root(&self, id: &str, version: &str) -> Result<PathBuf, ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        Ok(self.store.version_root(&id, version))
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

    /// Install one package tree with its source label.
    pub fn install_package(
        &self,
        source: &Path,
        origin: crate::types::ExtensionSource,
    ) -> Result<crate::package::Package, ExtError> {
        self.store
            .install_with(source, origin)
            .map_err(package_error)
    }

    /// Remove one installed extension, optionally purging its data.
    pub fn uninstall(&self, id: &str, purge_data: bool) -> Result<(), ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store.uninstall(&id, purge_data).map_err(package_error)
    }

    /// Enable or disable one installed extension.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store
            .set_enabled(&id, enabled)
            .map(|_| ())
            .map_err(package_error)
    }

    /// Every installed version of one extension, ascending.
    pub fn versions(&self, id: &str) -> Result<Vec<String>, ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store.versions(&id).map_err(package_error)
    }

    /// The active version of one extension, when one is installed.
    pub fn active_version(&self, id: &str) -> Result<Option<String>, ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        Ok(self.store.active_version(&id))
    }

    /// Activate one installed version.
    pub fn activate(&self, id: &str, version: &str) -> Result<(), ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store
            .set_active(&id, version)
            .map(|_| ())
            .map_err(package_error)
    }

    /// Activate the version before the current one.
    pub fn rollback(&self, id: &str) -> Result<String, ExtError> {
        let versions = self.versions(id)?;
        let active = self.active_version(id)?;
        let target = match active
            .as_deref()
            .and_then(|active| versions.iter().position(|version| version == active))
        {
            Some(0) => None,
            Some(position) => Some(versions[position - 1].clone()),
            None => versions.last().cloned(),
        }
        .ok_or_else(|| ExtError {
            code: ErrorCode::Package,
            message: "no earlier version to roll back to".to_string(),
            details: serde_json::Value::Null,
        })?;
        self.activate(id, &target)?;
        Ok(target)
    }

    /// Clear a persisted failure for one extension.
    pub fn clear_failure(&self, id: &str) -> Result<(), ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store.clear_failure(&id).map_err(package_error)
    }

    /// Move one extension's code to quarantine.
    pub fn quarantine(&self, id: &str, reason: &str) -> Result<PathBuf, ExtError> {
        let id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store.quarantine(&id, reason).map_err(package_error)
    }

    /// Install one local package tree and return its identity.
    pub fn install(&self, source: &Path) -> Result<InstalledExtension, ExtError> {
        let package = self.store.install(source).map_err(package_error)?;
        Ok(InstalledExtension {
            id: package.id().as_str().to_string(),
            version: package.version(),
        })
    }

    /// Apply one plan: start each runtime and apply its contributions.
    pub async fn apply(
        &self,
        plan: &ManagerPlan,
        host: &mut dyn LeaseHost,
    ) -> Result<ActivationSet, ExtError> {
        let mut activations = Vec::new();
        for id in &plan.order {
            match self.build_activation(id).await {
                Ok(activation) => activations.push(activation),
                Err(error) => {
                    shutdown_all(activations, host).await;
                    return Err(error);
                }
            }
        }
        Ok(ActivationSet { activations })
    }

    /// Reconcile an existing activation set with a new plan.
    pub async fn reload(
        &self,
        current: ActivationSet,
        plan: &ManagerPlan,
        host: &mut dyn LeaseHost,
    ) -> Result<ActivationSet, ExtError> {
        let mut old: std::collections::BTreeMap<String, Activation> = current
            .activations
            .into_iter()
            .map(|activation| (activation.extension.clone(), activation))
            .collect();
        let mut activations = Vec::new();
        for id in &plan.order {
            if let Some(existing) = old.remove(id) {
                let current_version = self.version_of(id)?;
                if existing.version == current_version {
                    activations.push(existing);
                    continue;
                }
                existing.shutdown(host).await?;
            }
            match self.build_activation(id).await {
                Ok(activation) => activations.push(activation),
                Err(error) => {
                    shutdown_all(activations, host).await;
                    shutdown_all(old.into_values().collect(), host).await;
                    return Err(error);
                }
            }
        }
        for (_, activation) in old {
            activation.shutdown(host).await?;
        }
        Ok(ActivationSet { activations })
    }

    async fn build_activation(&self, id: &str) -> Result<Activation, ExtError> {
        let runtimes = self.runtimes.as_ref().ok_or_else(|| ExtError {
            code: ErrorCode::Runtime,
            message: "no runtime resolver is attached".to_string(),
            details: serde_json::Value::Null,
        })?;
        let contributions = self.contributions.as_ref().ok_or_else(|| ExtError {
            code: ErrorCode::Contribution,
            message: "no contribution sink is attached".to_string(),
            details: serde_json::Value::Null,
        })?;
        let extension_id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        let version = self.version_of(id)?;
        let root = self.store.version_root(&extension_id, &version);
        let manifest =
            crate::manifest_v2::load_package_manifest(&root).map_err(|error| ExtError {
                code: ErrorCode::Manifest,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
        let data_root = self.store.data_dir(&extension_id);
        let runtime = runtimes.create(&manifest, &root, &data_root)?;
        let listed = runtime
            .start(&RuntimeContext {
                extension: id.to_string(),
                root: root.clone(),
                data_root,
            })
            .await?;
        let mut leases: Vec<Box<dyn ContributionLease>> = Vec::new();
        for contribution in listed {
            leases.push(contributions.apply(contribution).await?);
        }
        Ok(Activation {
            extension: id.to_string(),
            version,
            runtime,
            leases,
        })
    }

    fn version_of(&self, id: &str) -> Result<String, ExtError> {
        let extension_id = ExtensionId::parse(id.to_string()).map_err(|error| ExtError {
            code: ErrorCode::Manifest,
            message: error.to_string(),
            details: serde_json::Value::Null,
        })?;
        self.store
            .active_version(&extension_id)
            .or_else(|| {
                self.store
                    .versions(&extension_id)
                    .ok()
                    .and_then(|versions| versions.last().cloned())
            })
            .ok_or_else(|| ExtError {
                code: ErrorCode::Package,
                message: format!("{id} has no installed version"),
                details: serde_json::Value::Null,
            })
    }

    /// Resolve installed packages into an activation order.
    #[cfg(feature = "tokio")]
    pub fn plan(&self) -> Result<ManagerPlan, ExtError> {
        let registry = self.store.read_registry().map_err(package_error)?;
        let mut manifests = Vec::new();
        for raw in registry.extensions.keys() {
            let id = ExtensionId::parse(raw.clone()).map_err(|error| ExtError {
                code: ErrorCode::Manifest,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
            let version = self
                .store
                .active_version(&id)
                .or_else(|| {
                    self.store
                        .versions(&id)
                        .ok()
                        .and_then(|versions| versions.last().cloned())
                })
                .ok_or_else(|| ExtError {
                    code: ErrorCode::Package,
                    message: format!("{id} has no installed version"),
                    details: serde_json::Value::Null,
                })?;
            let root = self.store.version_root(&id, &version);
            let manifest =
                crate::manifest_v2::load_package_manifest(&root).map_err(|error| ExtError {
                    code: ErrorCode::Manifest,
                    message: error.to_string(),
                    details: serde_json::Value::Null,
                })?;
            manifests.push(manifest);
        }
        let plan =
            crate::composable::plan_service_dependencies(&manifests).map_err(|error| ExtError {
                code: ErrorCode::Service,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
        Ok(ManagerPlan { order: plan.order })
    }
}

/// One started extension.
pub struct Activation {
    /// Extension id.
    pub extension: String,
    /// Installed version this activation was built from.
    pub version: String,
    /// Runtime holding the contributions.
    pub runtime: Arc<dyn Runtime>,
    leases: Vec<Box<dyn ContributionLease>>,
}

async fn shutdown_all(activations: Vec<Activation>, host: &mut dyn LeaseHost) {
    for activation in activations.into_iter().rev() {
        let _ = activation.shutdown(host).await;
    }
}

impl Activation {
    /// Revoke this activation outside the owning set.
    /// Revoke this activation's leases without shutting down the runtime.
    ///
    /// Used when the host owns a shared runtime across several agent builds.
    pub async fn unload_leases(self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        for lease in self.leases.into_iter().rev() {
            lease.revoke(host).await?;
        }
        Ok(())
    }

    /// Revoke the leases and shut the runtime down.
    pub async fn unload(self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        self.shutdown(host).await
    }

    async fn shutdown(self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        for lease in self.leases.into_iter().rev() {
            lease.revoke(host).await?;
        }
        self.runtime.shutdown().await
    }
}

/// The set of started extensions for one applied plan.
pub struct ActivationSet {
    activations: Vec<Activation>,
}

impl ActivationSet {
    /// Attach one host lease to an extension already started by this set.
    pub fn attach(
        &mut self,
        extension: &str,
        lease: Box<dyn ContributionLease>,
    ) -> Result<(), ExtError> {
        let activation = self
            .activations
            .iter_mut()
            .find(|activation| activation.extension == extension)
            .ok_or_else(|| ExtError {
                code: ErrorCode::Contribution,
                message: format!("{extension} is not in this activation set"),
                details: serde_json::Value::Null,
            })?;
        activation.leases.push(lease);
        Ok(())
    }

    /// Remove one activation so its owner can unload it independently.
    pub fn take(&mut self, extension: &str) -> Option<Activation> {
        let index = self
            .activations
            .iter()
            .position(|activation| activation.extension == extension)?;
        Some(self.activations.remove(index))
    }

    /// Number of started extensions.
    pub fn len(&self) -> usize {
        self.activations.len()
    }

    /// Whether no extension was started.
    pub fn is_empty(&self) -> bool {
        self.activations.is_empty()
    }

    /// Shut down in reverse activation order, revoking every contribution.
    pub async fn unload(mut self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        while let Some(activation) = self.activations.pop() {
            activation.shutdown(host).await?;
        }
        Ok(())
    }
}

/// One resolved activation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerPlan {
    /// Extension ids in dependency-first order.
    pub order: Vec<String>,
}

fn package_error(error: crate::types::ExtensionError) -> ExtError {
    ExtError {
        code: ErrorCode::Package,
        message: error.to_string(),
        details: serde_json::Value::Null,
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
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

    struct MockRuntime;
    struct MockLease(Arc<std::sync::atomic::AtomicUsize>);
    struct MockSink(Arc<std::sync::atomic::AtomicUsize>);
    struct MockResolver;

    #[async_trait::async_trait]
    impl crate::Runtime for MockRuntime {
        fn kind(&self) -> &str {
            "mock"
        }

        async fn start(
            &self,
            _ctx: &crate::RuntimeContext,
        ) -> Result<Vec<crate::Contribution>, ExtError> {
            Ok(vec![crate::Contribution {
                kind: "demo.echo".to_string(),
                id: "echo".to_string(),
                payload: serde_json::Value::Null,
            }])
        }

        async fn invoke(
            &self,
            _contribution: &crate::ContributionRef,
            _input: serde_json::Value,
            _ctx: &crate::InvokeContext,
        ) -> Result<serde_json::Value, ExtError> {
            Ok(serde_json::Value::Null)
        }

        async fn shutdown(&self) -> Result<(), ExtError> {
            Ok(())
        }
    }

    struct TestHost;

    impl crate::LeaseHost for TestHost {
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
    impl crate::ContributionLease for MockLease {
        async fn revoke(&self, _host: &mut dyn crate::LeaseHost) -> Result<(), ExtError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl crate::ContributionSink for MockSink {
        async fn apply(
            &self,
            _contribution: crate::Contribution,
        ) -> Result<Box<dyn crate::ContributionLease>, ExtError> {
            Ok(Box::new(MockLease(Arc::clone(&self.0))))
        }
    }

    impl crate::RuntimeResolver for MockResolver {
        fn create(
            &self,
            _manifest: &Manifest,
            _root: &Path,
            _data_root: &Path,
        ) -> Result<Arc<dyn crate::Runtime>, ExtError> {
            Ok(Arc::new(MockRuntime))
        }
    }

    #[tokio::test]
    async fn apply_starts_and_unloads_with_leases() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = ExtensionManager::open(host).expect("manager").with_ports(
            Arc::new(MockResolver),
            Arc::new(MockSink(Arc::clone(&revoked))),
        );
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        manager.install(&example).expect("install");
        let plan = manager.plan().expect("plan");
        let mut host = TestHost;
        let active = manager.apply(&plan, &mut host).await.expect("apply");
        assert_eq!(active.len(), 1);
        active.unload(&mut host).await.expect("unload");
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_keeps_unchanged_activations() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = ExtensionManager::open(host).expect("manager").with_ports(
            Arc::new(MockResolver),
            Arc::new(MockSink(Arc::clone(&revoked))),
        );
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        manager.install(&example).expect("install");
        let plan = manager.plan().expect("plan");
        let mut host = TestHost;
        let active = manager.apply(&plan, &mut host).await.expect("apply");
        let active = manager
            .reload(active, &plan, &mut host)
            .await
            .expect("reload");
        assert_eq!(active.len(), 1);
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 0);
        active.unload(&mut host).await.expect("unload");
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn install_then_plan_orders_the_package() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let manager = ExtensionManager::open(host).expect("manager");
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        let installed = manager.install(&example).expect("install");
        assert_eq!(installed.id, "com.example.hello");
        let plan = manager.plan().expect("plan");
        assert!(plan.order.iter().any(|id| id == "com.example.hello"));
    }
}
