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
fn load_validated_manifest(root: &std::path::Path) -> Result<crate::manifest::Manifest, ExtError> {
    let manifest = crate::manifest_v2::load_package_manifest(root).map_err(|error| ExtError {
        code: ErrorCode::Manifest,
        message: error.to_string(),
        details: serde_json::Value::Null,
    })?;
    let report = manifest.validate();
    if !report.is_valid() {
        return Err(ExtError {
            code: ErrorCode::Manifest,
            message: report
                .errors
                .iter()
                .map(|issue| format!("{}: {}", issue.field, issue.message))
                .collect::<Vec<_>>()
                .join("; "),
            details: serde_json::Value::Null,
        });
    }
    Ok(manifest)
}

/// Plans and owns Ferris extension activations for one host.
pub struct ExtensionManager {
    host: Arc<dyn HostSpec>,
    store: ExtensionStore,
    runtimes: Option<Arc<dyn RuntimeResolver>>,
    contributions: Option<Arc<dyn ContributionSink>>,
    /// Packages linked into the host binary, keyed by id.
    linked: std::collections::BTreeMap<String, LinkedPackage>,
}

/// One package compiled into the host rather than installed on disk.
///
/// It carries its manifest and its runtime, so it plans and activates by id
/// without a registry entry, an installed version or a store directory.
#[derive(Clone)]
struct LinkedPackage {
    /// The manifest the plan reads (identity, dependencies, services).
    manifest: crate::manifest::Manifest,
    /// The package root handed to the runtime resolver and at start.
    root: PathBuf,
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
            linked: std::collections::BTreeMap::new(),
        })
    }

    /// Register a package linked into the host binary.
    ///
    /// A linked package is not read from the store: the manifest and runtime
    /// are supplied here, so it plans and activates without an installed
    /// version on disk. Registering the same id twice replaces the earlier
    /// entry. The store's enabled flag still applies, so a reader can switch a
    /// linked package off like any other.
    pub fn register_linked(&mut self, manifest: crate::manifest::Manifest, root: PathBuf) {
        self.linked.insert(
            manifest.id().as_str().to_string(),
            LinkedPackage { manifest, root },
        );
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
        agent: &mut (dyn std::any::Any + Send),
        registry: &mut (dyn std::any::Any + Send),
        host: &mut dyn LeaseHost,
    ) -> Result<ActivationSet, ExtError> {
        let mut activations = Vec::new();
        for id in &plan.order {
            match self.build_activation(id, agent, registry).await {
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
        agent: &mut (dyn std::any::Any + Send),
        registry: &mut (dyn std::any::Any + Send),
        host: &mut dyn LeaseHost,
    ) -> Result<ActivationSet, ExtError> {
        let mut old: std::collections::BTreeMap<String, Activation> = current
            .activations
            .into_iter()
            .map(|activation| (activation.extension.clone(), activation))
            .collect();
        let mut activations = Vec::new();
        let mut leftovers = Vec::new();
        let mut primary: Option<ExtError> = None;
        for id in &plan.order {
            if let Some(existing) = old.remove(id) {
                match self.version_of(id) {
                    Ok(current_version) if existing.version == current_version => {
                        activations.push(existing);
                        continue;
                    }
                    Ok(_) => {
                        if let Err(error) = existing.shutdown(host).await {
                            primary = Some(error);
                            break;
                        }
                    }
                    Err(error) => {
                        leftovers.push(existing);
                        primary = Some(error);
                        break;
                    }
                }
            }
            match self.build_activation(id, agent, registry).await {
                Ok(activation) => activations.push(activation),
                Err(error) => {
                    primary = Some(error);
                    break;
                }
            }
        }
        if primary.is_none() {
            let mut removed: Vec<Activation> = std::mem::take(&mut old).into_values().collect();
            removed.append(&mut leftovers);
            let failures = shutdown_all_collect(removed, host).await;
            if !failures.is_empty() {
                primary = Some(cleanup_error(
                    ExtError {
                        code: ErrorCode::Lifecycle,
                        message: "reload could not shut down superseded activations".to_string(),
                        details: serde_json::Value::Null,
                    },
                    failures,
                ));
            }
        }
        if let Some(primary) = primary {
            let mut failures = shutdown_all_collect(activations, host).await;
            failures.extend(shutdown_all_collect(leftovers, host).await);
            failures.extend(shutdown_all_collect(old.into_values().collect(), host).await);
            return Err(cleanup_error(primary, failures));
        }
        Ok(ActivationSet { activations })
    }

    async fn build_activation(
        &self,
        id: &str,
        agent: &mut (dyn std::any::Any + Send),
        registry: &mut (dyn std::any::Any + Send),
    ) -> Result<Activation, ExtError> {
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
        let data_root = self.store.data_dir(&extension_id);
        let runtimes = self.runtimes.as_ref().ok_or_else(|| ExtError {
            code: ErrorCode::Runtime,
            message: "no runtime resolver is attached".to_string(),
            details: serde_json::Value::Null,
        })?;
        // A linked package supplies its manifest and root; an installed one
        // reads both back from the store. The runtime comes from the host's
        // resolver either way, which is where a builtin's adapter is registered.
        let (version, root, manifest) = if let Some(linked) = self.linked.get(id) {
            (
                linked.manifest.extension.version.to_string(),
                linked.root.clone(),
                linked.manifest.clone(),
            )
        } else {
            let version = self.version_of(id)?;
            let root = self.store.version_root(&extension_id, &version);
            let manifest = load_validated_manifest(&root)?;
            (version, root, manifest)
        };
        let runtime = runtimes.create(&manifest, &root, &data_root)?;
        let listed = match runtime
            .start(&RuntimeContext {
                extension: id.to_string(),
                root: root.clone(),
                data_root,
            })
            .await
        {
            Ok(listed) => listed,
            Err(primary) => {
                // start() may have created a child or background task before
                // failing; the runtime is not yet owned by an Activation, so
                // shut it down here and keep the original error primary.
                let cleanup = runtime.shutdown().await.err();
                return Err(match cleanup {
                    Some(error) => cleanup_error(primary, vec![(id.to_string(), error)]),
                    None => primary,
                });
            }
        };
        let lease = match contributions.apply(id, listed, agent, registry).await {
            Ok(lease) => lease,
            Err(error) => {
                // The runtime already started; a failed contribution must not
                // leave it running behind an error nobody can unload.
                let _ = runtime.shutdown().await;
                return Err(error);
            }
        };
        let leases: Vec<Box<dyn ContributionLease>> = vec![lease];
        Ok(Activation {
            extension: id.to_string(),
            version,
            runtime,
            leases,
        })
    }

    fn version_of(&self, id: &str) -> Result<String, ExtError> {
        if let Some(linked) = self.linked.get(id) {
            return Ok(linked.manifest.extension.version.to_string());
        }
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
        let mut versions: std::collections::BTreeMap<String, semver::Version> =
            std::collections::BTreeMap::new();
        for raw in registry.extensions.keys() {
            let id = ExtensionId::parse(raw.clone()).map_err(|error| ExtError {
                code: ErrorCode::Manifest,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
            if !self.store.state_of(&id).enabled {
                // A persisted disable must keep the package out of the plan:
                // planning it anyway started code the reader turned off.
                continue;
            }
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
            let manifest = load_validated_manifest(&root)?;
            let parsed = semver::Version::parse(&version).map_err(|error| ExtError {
                code: ErrorCode::Package,
                message: format!("{id} has an invalid installed version {version}: {error}"),
                details: serde_json::Value::Null,
            })?;
            versions.insert(id.as_str().to_string(), parsed);
            manifests.push(manifest);
        }
        // Linked packages have no registry entry and no version on disk, so
        // they contribute both here. A package the reader disabled stays out
        // of the plan like an installed one.
        for (id, linked) in &self.linked {
            let extension_id = ExtensionId::parse(id.clone()).map_err(|error| ExtError {
                code: ErrorCode::Manifest,
                message: error.to_string(),
                details: serde_json::Value::Null,
            })?;
            if !self.store.state_of(&extension_id).enabled {
                continue;
            }
            versions.insert(id.clone(), linked.manifest.extension.version.clone());
            manifests.push(linked.manifest.clone());
        }
        let included: std::collections::BTreeSet<String> = manifests
            .iter()
            .map(|manifest| manifest.id().as_str().to_string())
            .collect();
        for manifest in &manifests {
            for (dependency, requirement) in &manifest.dependencies {
                if !included.contains(dependency.as_str()) {
                    return Err(ExtError {
                        code: ErrorCode::Package,
                        message: format!(
                            "{} is enabled but depends on {}, which is disabled or not installed",
                            manifest.id(),
                            dependency
                        ),
                        details: serde_json::Value::Null,
                    });
                }
                if let Some(installed) = versions.get(dependency.as_str()) {
                    if !requirement.matches(installed) {
                        return Err(ExtError {
                            code: ErrorCode::Package,
                            message: format!(
                                "{} requires {} {requirement}, but {installed} is installed",
                                manifest.id(),
                                dependency
                            ),
                            details: serde_json::Value::Null,
                        });
                    }
                }
            }
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

/// Shut every activation down, collecting the failures instead of dropping them.
async fn shutdown_all_collect(
    activations: Vec<Activation>,
    host: &mut dyn LeaseHost,
) -> Vec<(String, ExtError)> {
    let mut failures = Vec::new();
    for activation in activations.into_iter().rev() {
        let id = activation.extension.clone();
        if let Err(error) = activation.shutdown(host).await {
            failures.push((id, error));
        }
    }
    failures
}

/// Preserve the primary error and report every cleanup failure beside it.
fn cleanup_error(primary: ExtError, failures: Vec<(String, ExtError)>) -> ExtError {
    if failures.is_empty() {
        return primary;
    }
    let detail = failures
        .iter()
        .map(|(id, error)| format!("{id}: {}", error.message))
        .collect::<Vec<_>>()
        .join("; ");
    ExtError {
        code: primary.code,
        message: format!("{}; cleanup failed for {detail}", primary.message),
        details: primary.details,
    }
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
        let mut first = None;
        for lease in self.leases.into_iter().rev() {
            if let Err(error) = lease.revoke(host).await {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Revoke the leases and shut the runtime down.
    pub async fn unload(self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        self.shutdown(host).await
    }

    async fn shutdown(self, host: &mut dyn LeaseHost) -> Result<(), ExtError> {
        let mut first = None;
        for lease in self.leases.into_iter().rev() {
            if let Err(error) = lease.revoke(host).await {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        if let Err(error) = self.runtime.shutdown().await {
            if first.is_none() {
                first = Some(error);
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
        let mut first = None;
        while let Some(activation) = self.activations.pop() {
            if let Err(error) = activation.shutdown(host).await {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
            _extension: &str,
            _contributions: Vec<crate::Contribution>,
            _agent: &mut (dyn std::any::Any + Send),
            _registry: &mut (dyn std::any::Any + Send),
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
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut host)
            .await
            .expect("apply");
        assert_eq!(active.len(), 1);
        active.unload(&mut host).await.expect("unload");
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A package linked into the host binary needs no store entry: the manager
    /// plans and activates it from the manifest and root it was registered with.
    /// This is the path a host's own built-in bundles take.
    #[tokio::test]
    async fn a_linked_package_plans_and_activates_without_install() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/legacy/hello");
        let manifest_path = example.join("extension.toml");
        let text = std::fs::read_to_string(&manifest_path).expect("the fixture manifest");
        let manifest = Manifest::parse(&text, &manifest_path).expect("parse");
        let id = manifest.id().as_str().to_string();

        let mut manager = ExtensionManager::open(host).expect("manager");
        manager.register_linked(manifest, example.clone());
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = manager.with_ports(
            Arc::new(MockResolver),
            Arc::new(MockSink(Arc::clone(&revoked))),
        );

        let plan = manager.plan().expect("plan");
        assert_eq!(
            plan.order,
            vec![id.clone()],
            "the linked package is planned without an installed version"
        );
        let mut host = TestHost;
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut host)
            .await
            .expect("apply");
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
        let active = manager
            .apply(&plan, &mut (), &mut (), &mut host)
            .await
            .expect("apply");
        let active = manager
            .reload(active, &plan, &mut (), &mut (), &mut host)
            .await
            .expect("reload");
        assert_eq!(active.len(), 1);
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 0);
        active.unload(&mut host).await.expect("unload");
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    struct CountingRuntime {
        shutdowns: Arc<std::sync::atomic::AtomicUsize>,
        fail_shutdown: bool,
    }

    #[async_trait::async_trait]
    impl crate::Runtime for CountingRuntime {
        fn kind(&self) -> &str {
            "counting"
        }

        async fn start(
            &self,
            _ctx: &crate::RuntimeContext,
        ) -> Result<Vec<crate::Contribution>, ExtError> {
            Ok(Vec::new())
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
            self.shutdowns
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_shutdown {
                Err(ExtError {
                    code: ErrorCode::Runtime,
                    message: "shutdown failed".to_string(),
                    details: serde_json::Value::Null,
                })
            } else {
                Ok(())
            }
        }
    }

    struct StartFailingRuntime {
        shutdowns: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl crate::Runtime for StartFailingRuntime {
        fn kind(&self) -> &str {
            "start-failing"
        }

        async fn start(
            &self,
            _ctx: &crate::RuntimeContext,
        ) -> Result<Vec<crate::Contribution>, ExtError> {
            Err(ExtError {
                code: ErrorCode::Runtime,
                message: "start failed".to_string(),
                details: serde_json::Value::Null,
            })
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
            self.shutdowns
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    struct StartFailResolver(Arc<std::sync::atomic::AtomicUsize>);

    impl crate::RuntimeResolver for StartFailResolver {
        fn create(
            &self,
            _manifest: &Manifest,
            _root: &Path,
            _data_root: &Path,
        ) -> Result<Arc<dyn crate::Runtime>, ExtError> {
            Ok(Arc::new(StartFailingRuntime {
                shutdowns: Arc::clone(&self.0),
            }))
        }
    }

    fn test_activation(
        extension: &str,
        version: &str,
        runtime: Arc<dyn crate::Runtime>,
        revoked: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Activation {
        Activation {
            extension: extension.to_string(),
            version: version.to_string(),
            runtime,
            leases: vec![Box::new(MockLease(revoked))],
        }
    }

    #[tokio::test]
    async fn a_failed_start_still_shuts_the_runtime_down() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let manager = ExtensionManager::open(host).expect("manager").with_ports(
            Arc::new(StartFailResolver(Arc::clone(&shutdowns))),
            Arc::new(MockSink(Arc::new(std::sync::atomic::AtomicUsize::new(0)))),
        );
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        manager.install(&example).expect("install");
        let error = manager
            .build_activation("com.example.hello", &mut (), &mut ())
            .await
            .err()
            .expect("start must fail");
        assert!(error.message.contains("start failed"), "{error:?}");
        assert_eq!(shutdowns.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_cleans_up_when_the_version_lookup_fails() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let manager = ExtensionManager::open(host).expect("manager");
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runtime: Arc<dyn crate::Runtime> = Arc::new(CountingRuntime {
            shutdowns: Arc::clone(&shutdowns),
            fail_shutdown: false,
        });
        let current = ActivationSet {
            activations: vec![test_activation(
                "com.example.missing",
                "1.0.0",
                runtime,
                Arc::clone(&revoked),
            )],
        };
        let plan = ManagerPlan {
            order: vec!["com.example.missing".to_string()],
        };
        let mut host = TestHost;
        let error = manager
            .reload(current, &plan, &mut (), &mut (), &mut host)
            .await
            .err()
            .expect("the version lookup fails");
        assert!(error.message.contains("version"), "{error:?}");
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(shutdowns.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reload_reports_a_removed_activation_shutdown_failure() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let manager = ExtensionManager::open(host).expect("manager");
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let runtime: Arc<dyn crate::Runtime> = Arc::new(CountingRuntime {
            shutdowns: Arc::clone(&shutdowns),
            fail_shutdown: true,
        });
        let current = ActivationSet {
            activations: vec![test_activation(
                "com.example.removed",
                "1.0.0",
                runtime,
                Arc::clone(&revoked),
            )],
        };
        let plan = ManagerPlan { order: Vec::new() };
        let mut host = TestHost;
        let error = manager
            .reload(current, &plan, &mut (), &mut (), &mut host)
            .await
            .err()
            .expect("the cleanup failure must be reported");
        assert!(error.message.contains("cleanup failed"), "{error:?}");
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

    #[test]
    fn a_disabled_package_is_not_planned() {
        let root = tempfile::tempdir().expect("tempdir");
        let host = Arc::new(DemoHost {
            root: root.path().to_path_buf(),
            capabilities: CapabilityRegistry::default(),
            contracts: ServiceContractRegistry::default(),
        });
        let manager = ExtensionManager::open(host).expect("manager");
        let example =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/hello");
        manager.install(&example).expect("install");
        manager
            .set_enabled("com.example.hello", false)
            .expect("disable");
        assert!(
            manager.plan().expect("plan").order.is_empty(),
            "a persisted disable must keep the package out of the activation plan"
        );
    }

    struct ShutdownRuntime(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait::async_trait]
    impl crate::Runtime for ShutdownRuntime {
        fn kind(&self) -> &str {
            "shutdown"
        }

        async fn start(
            &self,
            _ctx: &crate::RuntimeContext,
        ) -> Result<Vec<crate::Contribution>, ExtError> {
            Ok(Vec::new())
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
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingLease;

    #[async_trait::async_trait]
    impl crate::ContributionLease for FailingLease {
        async fn revoke(&self, _host: &mut dyn crate::LeaseHost) -> Result<(), ExtError> {
            Err(ExtError {
                code: ErrorCode::Contribution,
                message: "boom".to_string(),
                details: serde_json::Value::Null,
            })
        }
    }

    #[tokio::test]
    async fn a_failing_lease_still_shuts_the_runtime_down() {
        let shutdowns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let revoked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let activation = Activation {
            extension: "com.example.a".to_string(),
            version: "1.0.0".to_string(),
            runtime: Arc::new(ShutdownRuntime(Arc::clone(&shutdowns))),
            leases: vec![
                Box::new(MockLease(Arc::clone(&revoked))),
                Box::new(FailingLease),
            ],
        };
        let error = activation
            .shutdown(&mut TestHost)
            .await
            .expect_err("the lease failed");
        assert!(error.message.contains("boom"), "{}", error.message);
        assert_eq!(revoked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            shutdowns.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a failed lease must not skip runtime shutdown"
        );
    }
}
