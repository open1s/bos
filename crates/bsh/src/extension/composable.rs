//! Composable extension runtime primitives.
//!
//! This module is the Rust-native foundation for Extension 2.0. It is kept
//! separate from package discovery and execution: a service being discoverable
//! does not grant permission to call it. Callers must still pass through the
//! existing capability and policy checks at the host boundary.
//!
//! The primitives here provide:
//! - typed, scoped service registration with generation-safe removal;
//! - deterministic dependency ordering and cycle/missing-dependency errors;
//! - a small Fiber state machine;
//! - reverse-order, idempotent cleanup of effects owned by a Fiber.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::manifest::{Manifest, ServiceScope};

/// The lifetime boundary in which a service or Fiber is visible.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Scope {
    /// Shared by the host process.
    Host,
    /// Shared by one workspace.
    Workspace(String),
    /// Shared by one interactive session.
    Session(String),
    /// Owned by one extension.
    Extension(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ServiceKey {
    name: String,
    scope: Scope,
    type_id: TypeId,
}

/// A generation-specific ownership token for one provided service.
#[derive(Debug, Clone)]
pub(crate) struct ServiceToken {
    key: ServiceKey,
    generation: u64,
}

/// A service availability transition published by the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceChangeKind {
    Available,
    Replaced,
    Unavailable,
}

/// A service availability transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceChange {
    pub(crate) name: String,
    pub(crate) scope: Scope,
    pub(crate) generation: u64,
    pub(crate) kind: ServiceChangeKind,
}

/// A typed service registry. Replacing a service invalidates the old token, so
/// cleanup from an older provider cannot accidentally remove its replacement.
/// Subscribers receive bounded availability notifications and can re-query the
/// registry for the current provider.
pub(crate) struct ServiceRegistry {
    entries: RwLock<HashMap<ServiceKey, (u64, Arc<dyn Any + Send + Sync>)>>,
    next_generation: AtomicU64,
    changes: tokio::sync::broadcast::Sender<ServiceChange>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        let (changes, _) = tokio::sync::broadcast::channel(128);
        Self {
            entries: RwLock::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            changes,
        }
    }
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Subscribe to service availability transitions.
    pub(crate) fn subscribe_changes(
        &self,
    ) -> tokio::sync::broadcast::Receiver<ServiceChange> {
        self.changes.subscribe()
    }

    /// Provide or replace a typed service in a scope.
    pub(crate) fn provide<T>(
        &self,
        scope: Scope,
        name: impl Into<String>,
        service: Arc<T>,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        let key = ServiceKey {
            name: name.into(),
            scope,
            type_id: TypeId::of::<T>(),
        };
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self
            .entries
            .write()
            .expect("service registry lock poisoned")
            .insert(key.clone(), (generation, service));
        let _ = self.changes.send(ServiceChange {
            name: key.name.clone(),
            scope: key.scope.clone(),
            generation,
            kind: if previous.is_some() {
                ServiceChangeKind::Replaced
            } else {
                ServiceChangeKind::Available
            },
        });
        ServiceToken { key, generation }
    }

    /// Resolve a typed service in exactly the requested scope.
    pub(crate) fn get<T>(
        &self,
        scope: &Scope,
        name: &str,
    ) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        let key = ServiceKey {
            name: name.to_owned(),
            scope: scope.clone(),
            type_id: TypeId::of::<T>(),
        };
        let entries = self.entries.read().ok()?;
        let service = entries.get(&key)?.1.clone();
        Arc::downcast::<T>(service).ok()
    }

    /// Provide a service whose registration is revoked with its owning Fiber.
    ///
    /// The registry is weakly captured by the cleanup effect: a Fiber may
    /// outlive the host registry, but it must never keep the registry alive.
    /// Generation checking ensures an older Fiber cannot remove a replacement.
    pub(crate) fn provide_owned<T>(
        self: &Arc<Self>,
        scope: Scope,
        name: impl Into<String>,
        service: Arc<T>,
        fiber: &mut Fiber,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        let token = self.provide(scope, name, service);
        let registry = Arc::downgrade(self);
        let cleanup_token = token.clone();
        fiber.own(move || {
            if let Some(registry) = registry.upgrade() {
                registry.revoke(&cleanup_token);
            }
        });
        token
    }

    /// Remove a service only if the token still owns the current generation.
    /// Returns false for stale tokens and already-removed entries.
    pub(crate) fn revoke(&self, token: &ServiceToken) -> bool {
        let Ok(mut entries) = self.entries.write() else {
            return false;
        };
        if entries
            .get(&token.key)
            .is_some_and(|(generation, _)| *generation == token.generation)
        {
            entries.remove(&token.key);
            drop(entries);
            let _ = self.changes.send(ServiceChange {
                name: token.key.name.clone(),
                scope: token.key.scope.clone(),
                generation: token.generation,
                kind: ServiceChangeKind::Unavailable,
            });
            true
        } else {
            false
        }
    }

    /// Whether a service of this exact type/name/scope is currently available.
    pub(crate) fn contains<T>(&self, scope: &Scope, name: &str) -> bool
    where
        T: Any + Send + Sync,
    {
        self.get::<T>(scope, name).is_some()
    }
}

/// A validated dependency graph in dependency-first order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencyPlan {
    pub(crate) order: Vec<String>,
}

/// Why a dependency graph cannot be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DependencyError {
    Missing { extension: String, dependency: String },
    Cycle { members: Vec<String> },
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { extension, dependency } => {
                write!(f, "{extension} requires missing extension {dependency}")
            }
            Self::Cycle { members } => {
                write!(f, "extension dependency cycle: {}", members.join(" -> "))
            }
        }
    }
}

impl std::error::Error for DependencyError {}

/// A deterministic extension dependency graph.
#[derive(Debug, Clone, Default)]
pub(crate) struct DependencyGraph {
    dependencies: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyGraph {
    /// Add or replace one extension's dependency declaration.
    pub(crate) fn insert(
        &mut self,
        extension: impl Into<String>,
        dependencies: impl IntoIterator<Item = String>,
    ) {
        self.dependencies
            .insert(extension.into(), dependencies.into_iter().collect());
    }

    /// Produce a stable dependency-first activation order.
    pub(crate) fn plan(&self) -> Result<DependencyPlan, DependencyError> {
        let mut indegree = BTreeMap::<String, usize>::new();
        let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();

        for extension in self.dependencies.keys() {
            indegree.insert(extension.clone(), 0);
        }
        for (extension, required) in &self.dependencies {
            for dependency in required {
                if !self.dependencies.contains_key(dependency) {
                    return Err(DependencyError::Missing {
                        extension: extension.clone(),
                        dependency: dependency.clone(),
                    });
                }
                *indegree.get_mut(extension).expect("inserted above") += 1;
                dependents
                    .entry(dependency.clone())
                    .or_default()
                    .insert(extension.clone());
            }
        }

        let mut ready: BTreeSet<String> = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
            .collect();
        let mut order = Vec::with_capacity(indegree.len());

        while let Some(id) = ready.pop_first() {
            order.push(id.clone());
            if let Some(children) = dependents.get(&id) {
                for child in children {
                    let degree = indegree.get_mut(child).expect("known dependent");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }

        if order.len() != indegree.len() {
            let members = indegree
                .into_iter()
                .filter_map(|(id, degree)| (degree > 0).then_some(id))
                .collect();
            return Err(DependencyError::Cycle { members });
        }
        Ok(DependencyPlan { order })
    }
}

/// A provider selected for one declared service contract.
#[derive(Debug, Clone)]
struct ServiceProvider {
    extension: String,
    version: semver::Version,
}

/// Why service declarations cannot produce an activation graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceContractError {
    DuplicateProvider { service: String, first: String, second: String },
    MissingProvider { extension: String, service: String },
    VersionMismatch {
        extension: String,
        service: String,
        required: String,
        provided: String,
    },
    Dependency(DependencyError),
}

impl fmt::Display for ServiceContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider { service, first, second } => write!(
                f,
                "service {service} has multiple providers: {first} and {second}"
            ),
            Self::MissingProvider { extension, service } => {
                write!(f, "{extension} requires missing service {service}")
            }
            Self::VersionMismatch { extension, service, required, provided } => write!(
                f,
                "{extension} requires {service} {required}, but provider exposes {provided}"
            ),
            Self::Dependency(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ServiceContractError {}

/// Build a dependency-first activation plan from package and service contracts.
///
/// Service contracts are resolved before any extension code runs. A required
/// service contributes a provider -> consumer graph edge; optional missing
/// services do not. Explicit package dependencies remain authoritative too.
pub(crate) fn plan_service_dependencies(
    manifests: &[Manifest],
) -> Result<DependencyPlan, ServiceContractError> {
    // Extension-scoped services are private to their declaring extension.
    // Other scopes are shared within their respective host/workspace/session.
    type ServiceKey = (ServiceScope, String, String, String);
    let mut providers = BTreeMap::<ServiceKey, ServiceProvider>::new();

    for manifest in manifests {
        let extension = manifest.extension.id.as_str().to_owned();
        for service in &manifest.services.provides {
            let key = (
                service.scope,
                if service.scope == ServiceScope::Extension {
                    extension.clone()
                } else {
                    String::new()
                },
                service.name.clone(),
                service.contract.clone(),
            );
            let provider = ServiceProvider {
                extension: extension.clone(),
                version: service.version.clone(),
            };
            if let Some(previous) = providers.insert(key, provider.clone()) {
                return Err(ServiceContractError::DuplicateProvider {
                    service: format!(
                        "{}@{}",
                        service.name, service.contract
                    ),
                    first: previous.extension,
                    second: provider.extension,
                });
            }
        }
    }

    let mut graph = DependencyGraph::default();
    for manifest in manifests {
        let extension = manifest.extension.id.as_str().to_owned();
        let mut dependencies: BTreeSet<String> = manifest
            .dependencies
            .keys()
            .map(|id| id.as_str().to_owned())
            .collect();

        for requirement in &manifest.services.requires {
            let key = (
                requirement.scope,
                if requirement.scope == ServiceScope::Extension {
                    extension.clone()
                } else {
                    String::new()
                },
                requirement.name.clone(),
                requirement.contract.clone(),
            );
            let Some(provider) = providers.get(&key) else {
                if requirement.optional {
                    continue;
                }
                return Err(ServiceContractError::MissingProvider {
                    extension: extension.clone(),
                    service: format!("{}@{}", requirement.name, requirement.contract),
                });
            };
            if !requirement.version.matches(&provider.version) {
                if requirement.optional {
                    continue;
                }
                return Err(ServiceContractError::VersionMismatch {
                    extension: extension.clone(),
                    service: format!("{}@{}", requirement.name, requirement.contract),
                    required: requirement.version.to_string(),
                    provided: provider.version.to_string(),
                });
            }
            if provider.extension != extension {
                dependencies.insert(provider.extension.clone());
            }
        }
        graph.insert(extension, dependencies);
    }

    graph.plan().map_err(ServiceContractError::Dependency)
}

/// Runtime state of one composable extension Fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Unloading,
    Disposed,
}

/// Invalid Fiber state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FiberTransitionError {
    pub(crate) from: FiberState,
    pub(crate) to: FiberState,
}

impl fmt::Display for FiberTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid Fiber transition: {:?} -> {:?}", self.from, self.to)
    }
}

impl std::error::Error for FiberTransitionError {}

type Cleanup = Box<dyn FnOnce() + Send + 'static>;

/// A set of effects owned by one Fiber. Cleanup is idempotent, runs in reverse
/// registration order, and continues after an individual cleanup panics.
#[derive(Default)]
pub(crate) struct EffectSet {
    cleanups: Vec<Cleanup>,
    disposed: bool,
}

impl EffectSet {
    /// Create an empty effect set.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register an undo action. Actions added after disposal are run immediately.
    pub(crate) fn push(&mut self, cleanup: impl FnOnce() + Send + 'static) {
        if self.disposed {
            let _ = catch_unwind(AssertUnwindSafe(cleanup));
        } else {
            self.cleanups.push(Box::new(cleanup));
        }
    }

    /// Run all cleanup actions in reverse order. Returns the number attempted.
    pub(crate) fn rollback(&mut self) -> usize {
        if self.disposed {
            return 0;
        }
        self.disposed = true;
        let mut attempted = 0;
        while let Some(cleanup) = self.cleanups.pop() {
            attempted += 1;
            let _ = catch_unwind(AssertUnwindSafe(cleanup));
        }
        attempted
    }

    /// Number of pending cleanup actions.
    pub(crate) fn pending(&self) -> usize {
        self.cleanups.len()
    }
}

impl Drop for EffectSet {
    fn drop(&mut self) {
        self.rollback();
    }
}

/// A Cordis-inspired runtime Fiber with explicit effect ownership.
pub(crate) struct Fiber {
    id: String,
    state: FiberState,
    effects: EffectSet,
}

impl Fiber {
    /// Create a Fiber that has not started loading.
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: FiberState::Pending,
            effects: EffectSet::new(),
        }
    }

    /// Stable Fiber identifier.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Current state.
    pub(crate) fn state(&self) -> FiberState {
        self.state
    }

    /// Register a cleanup owned by this Fiber.
    pub(crate) fn own(&mut self, cleanup: impl FnOnce() + Send + 'static) {
        self.effects.push(cleanup);
    }

    /// Move to the next state if the transition is legal.
    pub(crate) fn transition(
        &mut self,
        to: FiberState,
    ) -> Result<(), FiberTransitionError> {
        let valid = matches!(
            (self.state, to),
            (FiberState::Pending, FiberState::Loading)
                | (FiberState::Loading, FiberState::Active)
                | (FiberState::Loading, FiberState::Failed)
                | (FiberState::Active, FiberState::Failed)
                | (FiberState::Active, FiberState::Unloading)
                | (FiberState::Failed, FiberState::Unloading)
                | (FiberState::Pending, FiberState::Unloading)
                | (FiberState::Loading, FiberState::Unloading)
                | (FiberState::Unloading, FiberState::Disposed)
        );
        if !valid {
            return Err(FiberTransitionError { from: self.state, to });
        }
        self.state = to;
        if to == FiberState::Disposed {
            self.effects.rollback();
        }
        Ok(())
    }

    /// Unload and revoke every effect owned by this Fiber.
    pub(crate) fn unload(&mut self) -> Result<usize, FiberTransitionError> {
        if self.state == FiberState::Disposed {
            return Ok(0);
        }
        if self.state != FiberState::Unloading {
            self.transition(FiberState::Unloading)?;
        }
        let attempted = self.effects.rollback();
        self.transition(FiberState::Disposed)?;
        Ok(attempted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn service_replacement_is_generation_safe() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Session("s1".into());
        let old = registry.provide(scope.clone(), "clock", Arc::new(1_u64));
        let new = registry.provide(scope.clone(), "clock", Arc::new(2_u64));

        assert_eq!(*registry.get::<u64>(&scope, "clock").unwrap(), 2);
        assert!(!registry.revoke(&old), "stale cleanup must not revoke replacement");
        assert_eq!(*registry.get::<u64>(&scope, "clock").unwrap(), 2);
        assert!(registry.revoke(&new));
        assert!(!registry.contains::<u64>(&scope, "clock"));
    }

    #[test]
    fn service_registry_publishes_availability_and_replacement() {
        let registry = ServiceRegistry::new();
        let mut changes = registry.subscribe_changes();
        let scope = Scope::Workspace("w1".into());

        let first = registry.provide(scope.clone(), "index", Arc::new(1_u64));
        let available = changes.try_recv().expect("availability event");
        assert_eq!(available.kind, ServiceChangeKind::Available);
        assert_eq!(available.generation, first.generation);

        let second = registry.provide(scope.clone(), "index", Arc::new(2_u64));
        let replaced = changes.try_recv().expect("replacement event");
        assert_eq!(replaced.kind, ServiceChangeKind::Replaced);
        assert_eq!(replaced.generation, second.generation);

        assert!(!registry.revoke(&first), "stale token cannot revoke the replacement");
        assert!(changes.try_recv().is_err(), "stale cleanup emits no unavailable event");
        assert!(registry.revoke(&second));
        let unavailable = changes.try_recv().expect("unavailable event");
        assert_eq!(unavailable.kind, ServiceChangeKind::Unavailable);
        assert_eq!(unavailable.generation, second.generation);
    }

    #[test]
    fn fiber_owned_service_cleanup_cannot_remove_a_replacement() {
        let registry = Arc::new(ServiceRegistry::new());
        let scope = Scope::Session("s1".into());
        let mut fiber = Fiber::new("extension-a");
        fiber.transition(FiberState::Loading).unwrap();
        fiber.transition(FiberState::Active).unwrap();

        registry.provide_owned(
            scope.clone(),
            "clock",
            Arc::new(1_u64),
            &mut fiber,
        );
        registry.provide(scope.clone(), "clock", Arc::new(2_u64));

        fiber.unload().unwrap();
        assert_eq!(
            *registry.get::<u64>(&scope, "clock").unwrap(),
            2,
            "stale Fiber cleanup must not revoke the replacement provider"
        );
    }

    #[test]
    fn service_scope_isolation_is_strict() {
        let registry = ServiceRegistry::new();
        registry.provide(Scope::Workspace("w1".into()), "index", Arc::new(7_u32));
        assert!(registry.get::<u32>(&Scope::Workspace("w1".into()), "index").is_some());
        assert!(registry.get::<u32>(&Scope::Workspace("w2".into()), "index").is_none());
        assert!(registry.get::<u32>(&Scope::Host, "index").is_none());
    }

    fn manifest_with_service_contract(
        id: &str,
        provides: &str,
        requires: &str,
    ) -> Manifest {
        let base = include_str!("manifest.rs");
        let start = base.find("fn sample() -> String").expect("sample helper");
        let source = &base[start..];
        let sample_start = source.find("r#\"").expect("raw manifest start") + 3;
        let sample_end = source[sample_start..]
            .find("\"#")
            .expect("raw manifest end")
            + sample_start;
        let mut text = source[sample_start..sample_end].to_owned();
        text = text.replace("com.example.git-assistant", id);
        if !provides.is_empty() {
            text.push_str("\n[services]\nprovides = ");
            text.push_str(provides);
            text.push('\n');
        }
        if !requires.is_empty() {
            if provides.is_empty() {
                text.push_str("\n[services]\n");
            }
            text.push_str("requires = ");
            text.push_str(requires);
            text.push('\n');
        }
        Manifest::parse(&text, std::path::Path::new("extension.toml")).unwrap()
    }

    #[test]
    fn service_contracts_add_dependency_edges_and_order_provider_first() {
        let provider = manifest_with_service_contract(
            "com.example.provider",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "1.2.0", scope = "workspace" }]"#,
            "",
        );
        let consumer = manifest_with_service_contract(
            "com.example.consumer",
            "",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "^1.0", scope = "workspace" }]"#,
        );
        let plan = plan_service_dependencies(&[consumer, provider]).unwrap();
        assert_eq!(
            plan.order,
            vec!["com.example.provider", "com.example.consumer"]
        );
    }

    #[test]
    fn service_contracts_reject_missing_and_incompatible_providers() {
        let consumer = manifest_with_service_contract(
            "com.example.consumer",
            "",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "^2.0", scope = "workspace" }]"#,
        );
        assert!(matches!(
            plan_service_dependencies(std::slice::from_ref(&consumer)),
            Err(ServiceContractError::MissingProvider { .. })
        ));

        let provider = manifest_with_service_contract(
            "com.example.provider",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "1.2.0", scope = "workspace" }]"#,
            "",
        );
        assert!(matches!(
            plan_service_dependencies(&[consumer, provider]),
            Err(ServiceContractError::VersionMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_service_providers_are_rejected_before_activation() {
        let first = manifest_with_service_contract(
            "com.example.provider-a",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "1.0.0", scope = "host" }]"#,
            "",
        );
        let second = manifest_with_service_contract(
            "com.example.provider-b",
            r#"[{ name = "com.example.search", contract = "com.example.search-api", version = "1.1.0", scope = "host" }]"#,
            "",
        );
        assert!(matches!(
            plan_service_dependencies(&[first, second]),
            Err(ServiceContractError::DuplicateProvider { .. })
        ));
    }

    #[test]
    fn service_contract_cycles_are_rejected() {
        let first = manifest_with_service_contract(
            "com.example.first",
            r#"[{ name = "com.example.first-api", contract = "com.example.first-contract", version = "1.0.0", scope = "host" }]"#,
            r#"[{ name = "com.example.second-api", contract = "com.example.second-contract", version = "^1.0", scope = "host" }]"#,
        );
        let second = manifest_with_service_contract(
            "com.example.second",
            r#"[{ name = "com.example.second-api", contract = "com.example.second-contract", version = "1.0.0", scope = "host" }]"#,
            r#"[{ name = "com.example.first-api", contract = "com.example.first-contract", version = "^1.0", scope = "host" }]"#,
        );
        assert!(matches!(
            plan_service_dependencies(&[first, second]),
            Err(ServiceContractError::Dependency(DependencyError::Cycle { .. }))
        ));
    }

    #[test]
    fn optional_service_requirement_does_not_block_activation() {
        let consumer = manifest_with_service_contract(
            "com.example.consumer",
            "",
            r#"[{ name = "com.example.telemetry", contract = "com.example.telemetry-api", version = "^1.0", scope = "host", optional = true }]"#,
        );
        let plan = plan_service_dependencies(&[consumer]).unwrap();
        assert_eq!(plan.order, vec!["com.example.consumer"]);
    }

    #[test]
    fn extension_scoped_services_are_private_to_each_extension() {
        let first = manifest_with_service_contract(
            "com.example.first",
            r#"[{ name = "com.example.cache", contract = "com.example.cache-api", version = "1.0.0", scope = "extension" }]"#,
            "",
        );
        let second = manifest_with_service_contract(
            "com.example.second",
            r#"[{ name = "com.example.cache", contract = "com.example.cache-api", version = "1.0.0", scope = "extension" }]"#,
            "",
        );
        let plan = plan_service_dependencies(&[first, second]).unwrap();
        assert_eq!(plan.order, vec!["com.example.first", "com.example.second"]);

        let external_consumer = manifest_with_service_contract(
            "com.example.consumer",
            "",
            r#"[{ name = "com.example.cache", contract = "com.example.cache-api", version = "^1.0", scope = "extension" }]"#,
        );
        let provider = manifest_with_service_contract(
            "com.example.provider",
            r#"[{ name = "com.example.cache", contract = "com.example.cache-api", version = "1.0.0", scope = "extension" }]"#,
            "",
        );
        assert!(matches!(
            plan_service_dependencies(&[provider, external_consumer]),
            Err(ServiceContractError::MissingProvider { .. })
        ));
    }

    #[test]
    fn optional_incompatible_service_does_not_block_activation() {
        let consumer = manifest_with_service_contract(
            "com.example.consumer",
            "",
            r#"[{ name = "com.example.metrics", contract = "com.example.metrics-api", version = "^2.0", scope = "host", optional = true }]"#,
        );
        let provider = manifest_with_service_contract(
            "com.example.provider",
            r#"[{ name = "com.example.metrics", contract = "com.example.metrics-api", version = "1.0.0", scope = "host" }]"#,
            "",
        );
        let plan = plan_service_dependencies(&[consumer, provider]).unwrap();
        assert_eq!(plan.order, vec!["com.example.consumer", "com.example.provider"]);
    }

    #[test]
    fn dependency_plan_is_stable_and_dependency_first() {
        let mut graph = DependencyGraph::default();
        graph.insert("app", ["db".into(), "cache".into()]);
        graph.insert("db", std::iter::empty());
        graph.insert("cache", std::iter::empty());

        let plan = graph.plan().unwrap();
        assert_eq!(plan.order, vec!["cache", "db", "app"]);
    }

    #[test]
    fn dependency_graph_reports_missing_nodes_and_cycles() {
        let mut missing = DependencyGraph::default();
        missing.insert("app", ["database".into()]);
        assert!(matches!(missing.plan(), Err(DependencyError::Missing { .. })));

        let mut cyclic = DependencyGraph::default();
        cyclic.insert("a", ["b".into()]);
        cyclic.insert("b", ["a".into()]);
        assert!(matches!(cyclic.plan(), Err(DependencyError::Cycle { .. })));
    }

    #[test]
    fn fiber_unload_rolls_back_effects_once_in_reverse_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fiber = Fiber::new("com.example.plugin");
        for value in [1, 2, 3] {
            let calls = calls.clone();
            fiber.own(move || calls.lock().unwrap().push(value));
        }

        fiber.transition(FiberState::Loading).unwrap();
        fiber.transition(FiberState::Active).unwrap();
        assert_eq!(fiber.unload().unwrap(), 3);
        assert_eq!(*calls.lock().unwrap(), vec![3, 2, 1]);
        assert_eq!(fiber.state(), FiberState::Disposed);
        assert_eq!(fiber.unload().unwrap(), 0);
    }

    #[test]
    fn effect_cleanup_continues_after_a_panicking_action() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut effects = EffectSet::new();
        let first = count.clone();
        effects.push(move || { first.fetch_add(1, Ordering::SeqCst); });
        effects.push(|| panic!("cleanup failure"));
        let last = count.clone();
        effects.push(move || { last.fetch_add(1, Ordering::SeqCst); });

        assert_eq!(effects.rollback(), 3);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(effects.rollback(), 0);
    }
}
