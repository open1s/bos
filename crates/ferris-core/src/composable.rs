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
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::manifest::{Manifest, ServiceScope};

mod context;
pub mod dispatch;
mod effects;
pub mod plan;
mod reactivity;
pub mod runtime;

pub use context::{Context, ServiceAuthorizer};
pub use effects::{BoxFuture, DisposalReport, EffectScope};
pub use reactivity::{Epoch, Inject};

/// The lifetime boundary in which a service or Fiber is visible.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
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
    realm: Option<String>,
    /// The manifest contract this service implements. Two providers may share
    /// a name and type while implementing different contracts; the key keeps
    /// them apart instead of letting one silently replace the other.
    contract: String,
}

/// A generation-specific ownership token for one provided service.
#[derive(Debug, Clone)]
pub struct ServiceToken {
    key: ServiceKey,
    generation: u64,
}

/// A service availability transition published by the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceChangeKind {
    Available,
    Replaced,
    Unavailable,
}

/// A service availability transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceChange {
    pub name: String,
    pub scope: Scope,
    pub realm: Option<String>,
    pub generation: u64,
    pub kind: ServiceChangeKind,
}

/// One immutable provider generation. Leases keep the value alive and
/// account for in-flight calls after the generation stops accepting new calls.
struct ServiceGeneration {
    value: Arc<dyn Any + Send + Sync>,
    in_flight: AtomicUsize,
    accepting: AtomicBool,
    drained: tokio::sync::Notify,
}

impl ServiceGeneration {
    fn new(value: Arc<dyn Any + Send + Sync>) -> Self {
        Self {
            value,
            in_flight: AtomicUsize::new(0),
            accepting: AtomicBool::new(true),
            drained: tokio::sync::Notify::new(),
        }
    }

    fn retire(&self) {
        self.accepting.store(false, Ordering::Release);
        if self.in_flight.load(Ordering::Acquire) == 0 {
            self.drained.notify_waiters();
        }
    }

    fn release(&self) {
        if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1
            && !self.accepting.load(Ordering::Acquire)
        {
            self.drained.notify_waiters();
        }
    }
}

/// A call-scoped reference to one immutable provider generation.
///
/// The lease must be held for the complete operation. Dropping it decrements
/// the generation's in-flight count and may complete a pending drain.
pub struct ServiceLease<T> {
    value: Arc<T>,
    generation: u64,
    owner: Arc<ServiceGeneration>,
}

impl<T> ServiceLease<T> {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl<T> Deref for ServiceLease<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> Drop for ServiceLease<T> {
    fn drop(&mut self) {
        self.owner.release();
    }
}

/// Handle to a replaced or revoked provider generation.
pub struct ServiceRetirement {
    generation: u64,
    owner: Arc<ServiceGeneration>,
}

impl ServiceRetirement {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn in_flight(&self) -> usize {
        self.owner.in_flight.load(Ordering::Acquire)
    }

    pub fn is_drained(&self) -> bool {
        self.in_flight() == 0
    }

    /// Wait until all calls using this generation release their leases.
    /// Returns false when the timeout expires.
    pub async fn wait_drained(&self, timeout: Duration) -> bool {
        let wait = async {
            loop {
                let notified = self.owner.drained.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.is_drained() {
                    break;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.is_ok()
    }
}

/// A typed service registry. Replacing a service invalidates the old token, so
/// cleanup from an older provider cannot accidentally remove its replacement.
/// Subscribers receive bounded availability notifications and can re-query the
/// registry for the current provider.
type GenerationKey = (Scope, Option<String>, String, String);

/// One live provider: its generation, its value, and the contract version.
type ServiceEntry = (u64, Arc<ServiceGeneration>, semver::Version);

pub struct ServiceRegistry {
    entries: RwLock<HashMap<ServiceKey, ServiceEntry>>,
    /// Generation index keyed by scope, realm and name. Each name keeps every
    /// concrete type currently registered under it, so revoking one type does
    /// not hide a different one that is still live.
    generations: RwLock<HashMap<GenerationKey, HashMap<TypeId, u64>>>,
    next_generation: AtomicU64,
    changes: tokio::sync::broadcast::Sender<ServiceChange>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        let (changes, _) = tokio::sync::broadcast::channel(128);
        Self {
            entries: RwLock::new(HashMap::new()),
            generations: RwLock::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            changes,
        }
    }
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to service availability transitions.
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<ServiceChange> {
        self.changes.subscribe()
    }

    /// Provide or replace a typed service in a scope.
    ///
    /// Existing callers retain the legacy token-only result. Use
    /// provide_with_retirement when the previous generation must be drained.
    pub fn provide<T>(&self, scope: Scope, name: impl Into<String>, service: Arc<T>) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        self.provide_with_retirement(scope, name, service).0
    }

    /// Provide in a named isolation realm. A realm is part of the service key,
    /// so the same scope/name in two realms are independent services.
    pub fn provide_in_realm<T>(
        &self,
        scope: Scope,
        realm: Option<&str>,
        name: impl Into<String>,
        service: Arc<T>,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        self.provide_with_retirement_in_realm(scope, realm, name, service)
            .0
    }

    /// Replace a provider and return a drain handle for its previous generation.
    pub fn provide_with_retirement<T>(
        &self,
        scope: Scope,
        name: impl Into<String>,
        service: Arc<T>,
    ) -> (ServiceToken, Option<ServiceRetirement>)
    where
        T: Any + Send + Sync,
    {
        self.provide_with_retirement_in_realm(scope, None, name, service)
    }

    /// Replace a provider in a named realm (legacy typed API, no contract).
    pub fn provide_with_retirement_in_realm<T>(
        &self,
        scope: Scope,
        realm: Option<&str>,
        name: impl Into<String>,
        service: Arc<T>,
    ) -> (ServiceToken, Option<ServiceRetirement>)
    where
        T: Any + Send + Sync,
    {
        self.provide_with_contract(
            scope,
            realm,
            name,
            "",
            semver::Version::new(0, 0, 0),
            service,
        )
    }

    /// Provide a service under its manifest contract and semantic version.
    ///
    /// The contract is part of the registry key, so two providers that share a
    /// name and Rust type but declare different contracts coexist instead of
    /// replacing one another. The version is stored with the entry and checked
    /// against a consumer's requirement at acquisition.
    pub fn provide_contract<T>(
        &self,
        scope: Scope,
        name: impl Into<String>,
        contract: &str,
        version: semver::Version,
        service: Arc<T>,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        self.provide_with_contract(scope, None, name, contract, version, service)
            .0
    }

    pub(crate) fn provide_with_contract<T>(
        &self,
        scope: Scope,
        realm: Option<&str>,
        name: impl Into<String>,
        contract: &str,
        version: semver::Version,
        service: Arc<T>,
    ) -> (ServiceToken, Option<ServiceRetirement>)
    where
        T: Any + Send + Sync,
    {
        let key = ServiceKey {
            name: name.into(),
            scope,
            type_id: TypeId::of::<T>(),
            realm: realm.map(str::to_owned),
            contract: contract.to_string(),
        };
        let mut entries = self
            .entries
            .write()
            .expect("service registry lock poisoned");
        // Allocate under the write lock so concurrent registrations cannot
        // publish generations out of order.
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let new_generation = Arc::new(ServiceGeneration::new(service));
        let previous = entries.insert(key.clone(), (generation, new_generation, version.clone()));
        if let Ok(mut generations) = self.generations.write() {
            generations
                .entry((
                    key.scope.clone(),
                    key.realm.clone(),
                    key.name.clone(),
                    key.contract.clone(),
                ))
                .or_default()
                .insert(key.type_id, generation);
        }
        // Retire while holding the write lock. No new lease can race in after
        // the replacement becomes visible.
        let retirement = previous.map(|(old_generation, owner, _)| {
            owner.retire();
            ServiceRetirement {
                generation: old_generation,
                owner,
            }
        });
        // Publish while holding the write lock so concurrent changes cannot
        // reorder their generation notifications.
        let _ = self.changes.send(ServiceChange {
            name: key.name.clone(),
            scope: key.scope.clone(),
            realm: key.realm.clone(),
            generation,
            kind: if retirement.is_some() {
                ServiceChangeKind::Replaced
            } else {
                ServiceChangeKind::Available
            },
        });
        drop(entries);
        (ServiceToken { key, generation }, retirement)
    }

    /// Resolve an untracked typed snapshot in exactly the requested scope.
    ///
    /// This compatibility lookup does not participate in in-flight draining;
    /// callers that perform work through the service must use acquire().
    pub fn get<T>(&self, scope: &Scope, name: &str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        self.get_in_realm(scope, None, name)
    }

    /// Resolve in one named isolation realm.
    pub fn get_in_realm<T>(&self, scope: &Scope, realm: Option<&str>, name: &str) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        let key = ServiceKey {
            name: name.to_owned(),
            scope: scope.clone(),
            type_id: TypeId::of::<T>(),
            realm: realm.map(str::to_owned),
            contract: String::new(),
        };
        let entries = self.entries.read().ok()?;
        let service = entries.get(&key)?.1.value.clone();
        Arc::downcast::<T>(service).ok()
    }

    /// Acquire a call-scoped lease on the current provider generation.
    ///
    /// New calls are rejected after replacement/revocation. Existing leases
    /// continue to use their original immutable generation until dropped.
    pub fn acquire<T>(&self, scope: &Scope, name: &str) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        self.acquire_in_realm(scope, None, name)
    }

    /// Acquire in one named isolation realm (legacy typed API, no contract).
    pub fn acquire_in_realm<T>(
        &self,
        scope: &Scope,
        realm: Option<&str>,
        name: &str,
    ) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        self.acquire_with_contract::<T>(scope, realm, name, "", &semver::VersionReq::STAR)
    }

    /// Acquire a provider of one named manifest contract and compatible version.
    ///
    /// A provider of the same name but a different contract is not a candidate,
    /// and an incompatible version is refused even when the Rust type matches.
    pub fn acquire_contract<T>(
        &self,
        scope: &Scope,
        name: &str,
        contract: &str,
        requirement: &semver::VersionReq,
    ) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        self.acquire_with_contract::<T>(scope, None, name, contract, requirement)
    }

    fn acquire_with_contract<T>(
        &self,
        scope: &Scope,
        realm: Option<&str>,
        name: &str,
        contract: &str,
        requirement: &semver::VersionReq,
    ) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        let key = ServiceKey {
            name: name.to_owned(),
            scope: scope.clone(),
            type_id: TypeId::of::<T>(),
            realm: realm.map(str::to_owned),
            contract: contract.to_string(),
        };
        let entries = self.entries.read().ok()?;
        let (generation, owner, version) = entries.get(&key)?;
        if !owner.accepting.load(Ordering::Acquire) {
            return None;
        }
        if !requirement.matches(version) {
            return None;
        }
        owner.in_flight.fetch_add(1, Ordering::AcqRel);
        let value = match Arc::downcast::<T>(Arc::clone(&owner.value)) {
            Ok(value) => value,
            Err(_) => {
                owner.release();
                return None;
            }
        };
        Some(ServiceLease {
            value,
            generation: *generation,
            owner: Arc::clone(owner),
        })
    }

    /// Provide a service whose registration is revoked with its owning Fiber.
    ///
    /// The registry is weakly captured by the cleanup effect: a Fiber may
    /// outlive the host registry, but it must never keep the registry alive.
    /// Generation checking ensures an older Fiber cannot remove a replacement.
    pub fn provide_owned<T>(
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

    /// Provide a service under its contract, revoked with its owning Fiber.
    pub fn provide_owned_contract<T>(
        self: &Arc<Self>,
        scope: Scope,
        name: impl Into<String>,
        contract: &str,
        version: semver::Version,
        service: Arc<T>,
        fiber: &mut Fiber,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        let token = self
            .provide_with_contract(scope, None, name, contract, version, service)
            .0;
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
    pub fn revoke(&self, token: &ServiceToken) -> bool {
        self.revoke_with_retirement(token).is_some()
    }

    /// Revoke the current generation and return a handle for draining its calls.
    pub fn revoke_with_retirement(&self, token: &ServiceToken) -> Option<ServiceRetirement> {
        let mut entries = self.entries.write().ok()?;
        if !entries
            .get(&token.key)
            .is_some_and(|(generation, _, _)| *generation == token.generation)
        {
            return None;
        }
        let (generation, owner, _) = entries.remove(&token.key)?;
        let name_key = (
            token.key.scope.clone(),
            token.key.realm.clone(),
            token.key.name.clone(),
            token.key.contract.clone(),
        );
        let mut reported_generation = generation;
        let mut kind = ServiceChangeKind::Unavailable;
        if let Ok(mut generations) = self.generations.write() {
            if let Some(state) = generations.get_mut(&name_key) {
                state.remove(&token.key.type_id);
                if state.is_empty() {
                    generations.remove(&name_key);
                } else {
                    // Another concrete type still provides this name: the name
                    // remains available and its generation is the newest live
                    // provider, not the one that was just revoked.
                    kind = ServiceChangeKind::Replaced;
                    reported_generation = state.values().copied().max().unwrap_or(generation);
                }
            }
        }
        owner.retire();
        let _ = self.changes.send(ServiceChange {
            name: token.key.name.clone(),
            scope: token.key.scope.clone(),
            realm: token.key.realm.clone(),
            generation: reported_generation,
            kind,
        });
        drop(entries);
        Some(ServiceRetirement { generation, owner })
    }

    /// Current generation for a scope and name, independent of the concrete
    /// type. A missing value means no provider is available.
    pub fn generation(&self, scope: &Scope, name: &str) -> Option<u64> {
        self.generation_in_realm(scope, None, name)
    }

    /// Whether any scope currently publishes `name`.
    pub fn has_name(&self, name: &str) -> bool {
        self.generations
            .read()
            .map(|generations| {
                generations
                    .keys()
                    .any(|(_, _, service, contract)| service == name && contract.is_empty())
            })
            .unwrap_or(false)
    }

    /// Current generation in one named isolation realm.
    pub fn generation_in_realm(
        &self,
        scope: &Scope,
        realm: Option<&str>,
        name: &str,
    ) -> Option<u64> {
        self.generations
            .read()
            .ok()?
            .get(&(
                scope.clone(),
                realm.map(str::to_owned),
                name.to_owned(),
                String::new(),
            ))
            .and_then(|state| state.values().copied().max())
    }

    /// Whether a service of this exact type/name/scope is currently available.
    pub fn contains<T>(&self, scope: &Scope, name: &str) -> bool
    where
        T: Any + Send + Sync,
    {
        self.get::<T>(scope, name).is_some()
    }

    /// Whether one named realm has a service of this type/name/scope.
    pub fn contains_in_realm<T>(&self, scope: &Scope, realm: Option<&str>, name: &str) -> bool
    where
        T: Any + Send + Sync,
    {
        self.get_in_realm::<T>(scope, realm, name).is_some()
    }
}

/// A validated dependency graph in dependency-first order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyPlan {
    pub order: Vec<String>,
}

/// Why a dependency graph cannot be activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    Missing {
        extension: String,
        dependency: String,
    },
    Cycle {
        members: Vec<String>,
    },
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing {
                extension,
                dependency,
            } => {
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
pub struct DependencyGraph {
    dependencies: BTreeMap<String, BTreeSet<String>>,
}

impl DependencyGraph {
    /// Add or replace one extension's dependency declaration.
    pub fn insert(
        &mut self,
        extension: impl Into<String>,
        dependencies: impl IntoIterator<Item = String>,
    ) {
        self.dependencies
            .insert(extension.into(), dependencies.into_iter().collect());
    }

    /// Produce a stable dependency-first activation order.
    pub fn plan(&self) -> Result<DependencyPlan, DependencyError> {
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
pub enum ServiceContractError {
    DuplicateProvider {
        service: String,
        first: String,
        second: String,
    },
    MissingProvider {
        extension: String,
        service: String,
    },
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
            Self::DuplicateProvider {
                service,
                first,
                second,
            } => write!(
                f,
                "service {service} has multiple providers: {first} and {second}"
            ),
            Self::MissingProvider { extension, service } => {
                write!(f, "{extension} requires missing service {service}")
            }
            Self::VersionMismatch {
                extension,
                service,
                required,
                provided,
            } => write!(
                f,
                "{extension} requires {service} {required}, but provider exposes {provided}"
            ),
            Self::Dependency(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for ServiceContractError {}

/// Build a dependency-first activation plan from package and service contracts.
///
/// Service contracts are resolved before any extension code runs. A required
/// service contributes a provider -> consumer graph edge; optional missing
/// services do not. Explicit package dependencies remain authoritative too.
pub fn plan_service_dependencies(
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
                    service: format!("{}@{}", service.name, service.contract),
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
pub enum FiberState {
    Pending,
    Loading,
    Active,
    Failed,
    Unloading,
    Disposed,
}

/// Invalid Fiber state transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiberTransitionError {
    pub from: FiberState,
    pub to: FiberState,
}

impl fmt::Display for FiberTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid Fiber transition: {:?} -> {:?}",
            self.from, self.to
        )
    }
}

impl std::error::Error for FiberTransitionError {}

/// Compatibility name for the effect scope owned by a Fiber.
pub type EffectSet = EffectScope;

/// A Cordis-inspired runtime Fiber with explicit effect ownership.
pub struct Fiber {
    id: String,
    state: FiberState,
    effects: EffectScope,
    context: Option<Context>,
    inject: Inject,
    epoch: Epoch,
    revision: u64,
}

impl Fiber {
    /// Create a Fiber that has not started loading.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state: FiberState::Pending,
            effects: EffectScope::new(),
            context: None,
            inject: Inject::default(),
            epoch: Epoch::default(),
            revision: 0,
        }
    }

    /// Create a Fiber whose effects are owned by a context.
    pub fn with_context(id: impl Into<String>, context: Context) -> Self {
        Self {
            id: id.into(),
            state: FiberState::Pending,
            effects: context.effects().clone(),
            context: Some(context),
            inject: Inject::default(),
            epoch: Epoch::default(),
            revision: 0,
        }
    }

    /// Stable Fiber identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Current state.
    pub fn state(&self) -> FiberState {
        self.state
    }

    /// Number of effects still waiting to be disposed.
    pub fn pending_effects(&self) -> usize {
        self.effects.pending()
    }

    /// The context this Fiber activates, when one was attached.
    pub fn context(&self) -> Option<&Context> {
        self.context.as_ref()
    }

    /// Attach a context and share its effect scope. Effects registered before
    /// this call are disposed rather than silently leaked.
    pub fn attach_context(&mut self, context: Context) {
        let previous = std::mem::replace(&mut self.effects, context.effects().clone());
        previous.dispose_sync();
        self.context = Some(context);
    }

    /// Register a cleanup owned by this Fiber.
    pub fn own(&mut self, cleanup: impl FnOnce() + Send + 'static) {
        self.own_named("fiber.own", cleanup);
    }

    /// Register a labelled synchronous cleanup owned by this Fiber.
    pub fn own_named(&mut self, label: impl Into<String>, cleanup: impl FnOnce() + Send + 'static) {
        self.effects.push_sync(label, cleanup);
    }

    /// Register an asynchronous cleanup owned by this Fiber.
    pub fn own_async(
        &mut self,
        label: impl Into<String>,
        cleanup: impl FnOnce() -> BoxFuture + Send + 'static,
    ) {
        self.effects.push_async(label, cleanup);
    }

    /// Move to the next state if the transition is legal.
    pub fn transition(&mut self, to: FiberState) -> Result<(), FiberTransitionError> {
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
            return Err(FiberTransitionError {
                from: self.state,
                to,
            });
        }
        self.state = to;
        if to == FiberState::Disposed {
            if let Some(context) = &self.context {
                context.deactivate();
            }
            self.effects.dispose_sync();
        }
        Ok(())
    }

    /// Unload and revoke every effect owned by this Fiber.
    ///
    /// Async cleanups are spawned rather than awaited; use dispose_async when
    /// the caller can await.
    pub fn unload(&mut self) -> Result<usize, FiberTransitionError> {
        if self.state == FiberState::Disposed {
            return Ok(0);
        }
        if self.state != FiberState::Unloading {
            self.transition(FiberState::Unloading)?;
        }
        if let Some(context) = &self.context {
            context.deactivate();
        }
        let attempted = self.effects.dispose_sync().attempted;
        self.transition(FiberState::Disposed)?;
        Ok(attempted)
    }

    /// Unload and await every effect owned by this Fiber.
    pub async fn dispose_async(&mut self) -> Result<DisposalReport, FiberTransitionError> {
        if self.state == FiberState::Disposed {
            return Ok(DisposalReport::default());
        }
        if self.state != FiberState::Unloading {
            self.transition(FiberState::Unloading)?;
        }
        if let Some(context) = &self.context {
            context.deactivate();
        }
        let report = self.effects.dispose().await;
        self.transition(FiberState::Disposed)?;
        Ok(report)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[test]
    fn a_name_survives_revoking_one_of_two_concrete_types() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Session("s1".into());
        let number = registry.provide(scope.clone(), "mixed", Arc::new(7_u64));
        let text = registry.provide(scope.clone(), "mixed", Arc::new("seven".to_string()));
        assert!(registry.has_name("mixed"));
        assert_eq!(
            registry.generation(&scope, "mixed"),
            Some(text.generation),
            "the newest live provider is the reported generation"
        );
        assert!(registry.revoke(&number));
        assert!(
            registry.has_name("mixed"),
            "revoking u64 must not hide the live String provider"
        );
        assert!(registry.get::<String>(&scope, "mixed").is_some());
        assert!(registry.get::<u64>(&scope, "mixed").is_none());
        assert_eq!(registry.generation(&scope, "mixed"), Some(text.generation));
        assert!(registry.revoke(&text));
        assert!(!registry.has_name("mixed"));
        assert_eq!(registry.generation(&scope, "mixed"), None);
    }
    #[test]
    fn service_replacement_is_generation_safe() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Session("s1".into());
        let old = registry.provide(scope.clone(), "clock", Arc::new(1_u64));
        let new = registry.provide(scope.clone(), "clock", Arc::new(2_u64));

        assert_eq!(*registry.get::<u64>(&scope, "clock").unwrap(), 2);
        assert!(
            !registry.revoke(&old),
            "stale cleanup must not revoke replacement"
        );
        assert_eq!(*registry.get::<u64>(&scope, "clock").unwrap(), 2);
        assert!(registry.revoke(&new));
        assert!(!registry.contains::<u64>(&scope, "clock"));
    }

    #[test]
    fn concurrent_provider_replacements_publish_monotonic_generations() {
        let registry = Arc::new(ServiceRegistry::new());
        let mut changes = registry.subscribe_changes();
        let mut workers = Vec::new();
        for value in 0..8_u64 {
            let registry = Arc::clone(&registry);
            workers.push(std::thread::spawn(move || {
                registry.provide(Scope::Host, "clock", Arc::new(value))
            }));
        }
        let tokens: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("provider thread"))
            .collect();
        let newest = tokens
            .iter()
            .map(|token| token.generation)
            .max()
            .expect("at least one provider");
        let current = registry.acquire::<u64>(&Scope::Host, "clock").unwrap();
        assert_eq!(current.generation(), newest);

        let generations: Vec<_> = (0..8)
            .map(|_| {
                changes
                    .try_recv()
                    .expect("provider change event")
                    .generation
            })
            .collect();
        assert!(
            generations.windows(2).all(|pair| pair[0] < pair[1]),
            "broadcast events must follow committed generation order: {generations:?}"
        );
    }

    #[tokio::test]
    async fn replacement_keeps_old_generation_alive_until_leases_drain() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Workspace("w1".into());
        registry.provide(scope.clone(), "index", Arc::new("old".to_string()));

        let old_lease = registry.acquire::<String>(&scope, "index").unwrap();
        let old_generation = old_lease.generation();
        let (new_token, retirement) =
            registry.provide_with_retirement(scope.clone(), "index", Arc::new("new".to_string()));
        let retirement = retirement.expect("replaced generation");
        assert_eq!(retirement.generation(), old_generation);
        assert_eq!(retirement.in_flight(), 1);
        assert_eq!(&**old_lease, "old");

        let new_lease = registry.acquire::<String>(&scope, "index").unwrap();
        assert_eq!(&**new_lease, "new");
        assert_eq!(new_lease.generation(), new_token.generation);
        assert!(!retirement.is_drained());

        drop(old_lease);
        assert!(
            retirement.wait_drained(Duration::from_millis(100)).await,
            "retired provider must drain after its last lease is dropped"
        );
        drop(new_lease);
        assert!(registry.revoke(&new_token));
    }

    #[tokio::test]
    async fn revoked_service_rejects_new_leases_and_waits_for_existing_calls() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Session("s1".into());
        let token = registry.provide(scope.clone(), "session", Arc::new(7_u64));
        let lease = registry.acquire::<u64>(&scope, "session").unwrap();

        let retirement = registry
            .revoke_with_retirement(&token)
            .expect("current provider revoked");
        assert!(registry.acquire::<u64>(&scope, "session").is_none());
        assert!(!retirement.is_drained());
        assert!(!retirement.wait_drained(Duration::from_millis(1)).await);

        drop(lease);
        assert!(retirement.wait_drained(Duration::from_millis(100)).await);
        assert!(registry.revoke_with_retirement(&token).is_none());
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

        assert!(
            !registry.revoke(&first),
            "stale token cannot revoke the replacement"
        );
        assert!(
            changes.try_recv().is_err(),
            "stale cleanup emits no unavailable event"
        );
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

        registry.provide_owned(scope.clone(), "clock", Arc::new(1_u64), &mut fiber);
        registry.provide(scope.clone(), "clock", Arc::new(2_u64));

        fiber.unload().unwrap();
        assert_eq!(
            *registry.get::<u64>(&scope, "clock").unwrap(),
            2,
            "stale Fiber cleanup must not revoke the replacement provider"
        );
    }

    #[test]
    fn service_contracts_keep_same_name_and_type_apart() {
        struct Search(&'static str);
        let registry = ServiceRegistry::new();
        let scope = Scope::Host;
        let version = semver::Version::new(1, 2, 0);
        registry.provide_contract(
            scope.clone(),
            "search",
            "search-a",
            version.clone(),
            Arc::new(Search("a")),
        );
        registry.provide_contract(
            scope.clone(),
            "search",
            "search-b",
            version.clone(),
            Arc::new(Search("b")),
        );

        let requirement = semver::VersionReq::parse("^1").expect("requirement");
        let a = registry
            .acquire_contract::<Search>(&scope, "search", "search-a", &requirement)
            .expect("contract a");
        let b = registry
            .acquire_contract::<Search>(&scope, "search", "search-b", &requirement)
            .expect("contract b");
        assert_eq!((a.0, b.0), ("a", "b"));
        assert!(
            registry
                .acquire_contract::<Search>(&scope, "search", "search-c", &requirement)
                .is_none(),
            "an unknown contract is not a candidate"
        );
    }

    #[test]
    fn a_service_version_must_satisfy_the_requirement() {
        struct Search;
        let registry = ServiceRegistry::new();
        let scope = Scope::Host;
        registry.provide_contract(
            scope.clone(),
            "search",
            "search-a",
            semver::Version::new(1, 2, 0),
            Arc::new(Search),
        );
        let one = semver::VersionReq::parse("^1").expect("one");
        let two = semver::VersionReq::parse("^2").expect("two");
        assert!(registry
            .acquire_contract::<Search>(&scope, "search", "search-a", &one)
            .is_some());
        assert!(
            registry
                .acquire_contract::<Search>(&scope, "search", "search-a", &two)
                .is_none(),
            "a matching type does not prove a matching version"
        );
    }

    #[test]
    fn replacing_one_contract_does_not_hide_another() {
        struct Search(&'static str);
        let registry = ServiceRegistry::new();
        let scope = Scope::Host;
        let first = registry.provide_contract(
            scope.clone(),
            "search",
            "search-a",
            semver::Version::new(1, 0, 0),
            Arc::new(Search("a1")),
        );
        registry.provide_contract(
            scope.clone(),
            "search",
            "search-b",
            semver::Version::new(1, 0, 0),
            Arc::new(Search("b")),
        );
        let second = registry.provide_contract(
            scope.clone(),
            "search",
            "search-a",
            semver::Version::new(2, 0, 0),
            Arc::new(Search("a2")),
        );
        assert!(!registry.revoke(&first), "the old generation was replaced");
        let two = semver::VersionReq::parse("^2").expect("two");
        assert_eq!(
            registry
                .acquire_contract::<Search>(&scope, "search", "search-a", &two)
                .expect("new a")
                .0,
            "a2"
        );
        assert_eq!(
            registry
                .acquire_contract::<Search>(
                    &scope,
                    "search",
                    "search-b",
                    &semver::VersionReq::parse("^1").expect("one")
                )
                .expect("b still live")
                .0,
            "b"
        );
        assert!(registry.revoke(&second));
    }
    #[test]
    fn service_scope_isolation_is_strict() {
        let registry = ServiceRegistry::new();
        registry.provide(Scope::Workspace("w1".into()), "index", Arc::new(7_u32));
        assert!(registry
            .get::<u32>(&Scope::Workspace("w1".into()), "index")
            .is_some());
        assert!(registry
            .get::<u32>(&Scope::Workspace("w2".into()), "index")
            .is_none());
        assert!(registry.get::<u32>(&Scope::Host, "index").is_none());
    }

    fn manifest_with_service_contract(id: &str, provides: &str, requires: &str) -> Manifest {
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
            Err(ServiceContractError::Dependency(
                DependencyError::Cycle { .. }
            ))
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
        assert_eq!(
            plan.order,
            vec!["com.example.consumer", "com.example.provider"]
        );
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
        assert!(matches!(
            missing.plan(),
            Err(DependencyError::Missing { .. })
        ));

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
        let effects = EffectSet::new();
        let first = count.clone();
        effects.push(move || {
            first.fetch_add(1, Ordering::SeqCst);
        });
        effects.push(|| panic!("cleanup failure"));
        let last = count.clone();
        effects.push(move || {
            last.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(effects.rollback(), 3);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(effects.rollback(), 0);
    }

    #[test]
    #[ignore = "release benchmark; run with --release --ignored"]
    fn gold_standard_release_lookup_budget() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Host;
        registry.provide(scope.clone(), "clock", Arc::new(1_u64));
        let start = std::time::Instant::now();
        for _ in 0..1_000_000 {
            assert!(registry.acquire::<u64>(&scope, "clock").is_some());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(1),
            "1M exact lookups took {elapsed:?} in release"
        );
    }

    #[test]
    #[ignore = "release benchmark; run with --release --ignored"]
    fn gold_standard_release_effect_budget() {
        let scope = EffectScope::new();
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            scope.push_sync("bench", || {});
        }
        let report = scope.dispose_sync();
        let elapsed = start.elapsed();
        assert_eq!(report.attempted, 100_000);
        assert!(
            elapsed < Duration::from_secs(1),
            "100k effect registrations took {elapsed:?} in release"
        );
    }

    #[test]
    fn gold_standard_exact_service_lookup_budget() {
        let registry = ServiceRegistry::new();
        let scope = Scope::Host;
        registry.provide(scope.clone(), "clock", Arc::new(1_u64));
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            assert!(registry.acquire::<u64>(&scope, "clock").is_some());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "100k exact lookups took {elapsed:?}"
        );
    }

    #[test]
    fn gold_standard_effect_registration_budget() {
        let scope = EffectScope::new();
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            scope.push_sync("bench", || {});
        }
        let report = scope.dispose_sync();
        let elapsed = start.elapsed();
        assert_eq!(report.attempted, 10_000);
        assert!(
            elapsed < Duration::from_secs(2),
            "10k effect registrations took {elapsed:?}"
        );
    }
}
