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

/// A typed service registry. Replacing a service invalidates the old token, so
/// cleanup from an older provider cannot accidentally remove its replacement.
#[derive(Default)]
pub(crate) struct ServiceRegistry {
    entries: RwLock<HashMap<ServiceKey, (u64, Arc<dyn Any + Send + Sync>)>>,
    next_generation: AtomicU64,
}

impl ServiceRegistry {
    /// Create an empty registry.
    pub(crate) fn new() -> Self {
        Self::default()
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
        self.entries
            .write()
            .expect("service registry lock poisoned")
            .insert(key.clone(), (generation, service));
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
    fn service_scope_isolation_is_strict() {
        let registry = ServiceRegistry::new();
        registry.provide(Scope::Workspace("w1".into()), "index", Arc::new(7_u32));
        assert!(registry.get::<u32>(&Scope::Workspace("w1".into()), "index").is_some());
        assert!(registry.get::<u32>(&Scope::Workspace("w2".into()), "index").is_none());
        assert!(registry.get::<u32>(&Scope::Host, "index").is_none());
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
