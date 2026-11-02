//! Reactive coeffects: dependency declarations, epochs, and the index that
//! turns a service change into the fibers that must re-evaluate.
//!
//! The primitive is deliberately pure. A fiber computes an epoch from its
//! declared services and the registry's current generations; the caller acts on
//! the returned RefreshAction. Host wiring (reload, suspend, coalescing) lives
//! in the composition runtime and can be tested against this seam.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::RwLock;

use super::{Fiber, Scope, ServiceChange, ServiceRegistry};

/// What a dependent wants when a soft dependency changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reaction {
    /// Dispose and re-apply.
    Reload,
    /// Tell the component; it decides.
    Notify,
}

/// Scope, optional realm, and service name.
pub(crate) type DependencyKey = (Scope, Option<String>, String);

/// One declared service dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceRequirement {
    /// Stable service name.
    pub(crate) name: String,
    /// Visibility scope.
    pub(crate) scope: Scope,
    /// Optional isolation realm.
    pub(crate) realm: Option<String>,
    /// Reaction for soft dependencies. Hard dependencies always reload.
    pub(crate) reaction: Reaction,
}

impl ServiceRequirement {
    pub(crate) fn key(&self) -> DependencyKey {
        (self.scope.clone(), self.realm.clone(), self.name.clone())
    }
}

/// The dependency set of one fiber.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inject {
    /// Missing a hard dependency means the fiber must suspend.
    pub(crate) hard: Vec<ServiceRequirement>,
    /// Soft dependencies participate in the epoch but never block activation.
    pub(crate) soft: Vec<ServiceRequirement>,
}

impl Inject {
    /// Declare a hard dependency.
    pub(crate) fn require(mut self, scope: Scope, name: impl Into<String>) -> Self {
        self.hard.push(ServiceRequirement {
            name: name.into(),
            scope,
            realm: None,
            reaction: Reaction::Reload,
        });
        self
    }

    /// Declare a hard dependency in a named isolation realm.
    pub(crate) fn require_in_realm(
        mut self,
        scope: Scope,
        realm: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        self.hard.push(ServiceRequirement {
            name: name.into(),
            scope,
            realm: Some(realm.into()),
            reaction: Reaction::Reload,
        });
        self
    }

    /// Declare a soft dependency.
    pub(crate) fn optional(
        mut self,
        scope: Scope,
        name: impl Into<String>,
        reaction: Reaction,
    ) -> Self {
        self.soft.push(ServiceRequirement {
            name: name.into(),
            scope,
            realm: None,
            reaction,
        });
        self
    }

    /// Declare a soft dependency in a named isolation realm.
    pub(crate) fn optional_in_realm(
        mut self,
        scope: Scope,
        realm: impl Into<String>,
        name: impl Into<String>,
        reaction: Reaction,
    ) -> Self {
        self.soft.push(ServiceRequirement {
            name: name.into(),
            scope,
            realm: Some(realm.into()),
            reaction,
        });
        self
    }

    /// Every declared requirement, hard first.
    pub(crate) fn all(&self) -> impl Iterator<Item = &ServiceRequirement> {
        self.hard.iter().chain(self.soft.iter())
    }

    /// Whether every hard requirement is currently available.
    pub(crate) fn hard_ready(&self, registry: &ServiceRegistry) -> bool {
        self.hard.iter().all(|requirement| {
            registry
                .generation_in_realm(
                    &requirement.scope,
                    requirement.realm.as_deref(),
                    &requirement.name,
                )
                .is_some()
        })
    }

    /// Hard requirements that currently have no provider.
    pub(crate) fn missing_hard(
        &self,
        registry: &ServiceRegistry,
    ) -> Vec<(Scope, Option<String>, String)> {
        self.hard
            .iter()
            .filter(|requirement| {
                registry
                    .generation_in_realm(
                        &requirement.scope,
                        requirement.realm.as_deref(),
                        &requirement.name,
                    )
                    .is_none()
            })
            .map(|requirement| {
                (
                    requirement.scope.clone(),
                    requirement.realm.clone(),
                    requirement.name.clone(),
                )
            })
            .collect()
    }
}

/// The dependency snapshot that decides whether a fiber must reload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Epoch {
    /// Configuration/implementation revision, bumped by the owner.
    pub(crate) revision: u64,
    /// Current generation of every available declared service.
    pub(crate) deps: BTreeMap<DependencyKey, u64>,
}

/// What the caller should do after a refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefreshAction {
    /// Nothing changed.
    Noop,
    /// Dispose effects and apply again.
    Reload,
    /// Dispose effects and wait for a hard dependency.
    Suspend,
    /// A soft dependency changed; notify without forcing a reload.
    Notify,
}

impl Fiber {
    /// Install the dependency declarations.
    pub(crate) fn set_inject(&mut self, inject: Inject) {
        self.inject = inject;
    }

    /// The current dependency declarations.
    pub(crate) fn inject(&self) -> &Inject {
        &self.inject
    }

    /// The last computed epoch.
    pub(crate) fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    /// Bump the configuration/implementation revision. The next refresh sees a
    /// changed epoch and asks for a reload.
    pub(crate) fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    /// Recompute the epoch and classify the change.
    ///
    /// A missing hard dependency suspends. A changed hard dependency, a changed
    /// soft dependency with Reaction::Reload, or a revision bump reloads. A
    /// changed soft dependency with Reaction::Notify only notifies.
    pub(crate) fn refresh(&mut self, registry: &ServiceRegistry) -> RefreshAction {
        let previous = self.epoch.clone();
        let next = self.compute_epoch(registry);
        if next == previous {
            return RefreshAction::Noop;
        }
        self.epoch = next;

        if self
            .inject
            .hard
            .iter()
            .any(|requirement| !self.epoch.deps.contains_key(&requirement.key()))
        {
            return RefreshAction::Suspend;
        }

        let hard_changed = self.inject.hard.iter().any(|requirement| {
            previous.deps.get(&requirement.key()) != self.epoch.deps.get(&requirement.key())
        });
        let soft_reload = self.inject.soft.iter().any(|requirement| {
            requirement.reaction == Reaction::Reload
                && previous.deps.get(&requirement.key()) != self.epoch.deps.get(&requirement.key())
        });

        if hard_changed || soft_reload || previous.revision != self.epoch.revision {
            RefreshAction::Reload
        } else {
            RefreshAction::Notify
        }
    }

    fn compute_epoch(&self, registry: &ServiceRegistry) -> Epoch {
        let mut deps = BTreeMap::new();
        for requirement in self.inject.all() {
            if let Some(generation) = registry.generation_in_realm(
                &requirement.scope,
                requirement.realm.as_deref(),
                &requirement.name,
            ) {
                deps.insert(requirement.key(), generation);
            }
        }
        Epoch {
            revision: self.revision,
            deps,
        }
    }
}

/// Maps a service key to the fibers that declared it.
#[derive(Default)]
pub(crate) struct DependencyIndex {
    entries: RwLock<BTreeMap<DependencyKey, BTreeSet<String>>>,
}

impl DependencyIndex {
    /// Create an empty index.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register every declaration of one fiber.
    pub(crate) fn register(&self, fiber: &str, inject: &Inject) {
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        for requirement in inject.all() {
            entries
                .entry(requirement.key())
                .or_default()
                .insert(fiber.to_owned());
        }
    }

    /// Remove every declaration of one fiber.
    pub(crate) fn unregister(&self, fiber: &str, inject: &Inject) {
        let Ok(mut entries) = self.entries.write() else {
            return;
        };
        for requirement in inject.all() {
            let key = requirement.key();
            if let Some(fibers) = entries.get_mut(&key) {
                fibers.remove(fiber);
                if fibers.is_empty() {
                    entries.remove(&key);
                }
            }
        }
    }

    /// Fibers affected by one service change, sorted and deduplicated.
    pub(crate) fn affected(&self, change: &ServiceChange) -> Vec<String> {
        self.affected_many(std::slice::from_ref(change))
    }

    /// Fibers affected by a batch of service changes, sorted and deduplicated.
    pub(crate) fn affected_many(&self, changes: &[ServiceChange]) -> Vec<String> {
        let Ok(entries) = self.entries.read() else {
            return Vec::new();
        };
        let mut affected = BTreeSet::new();
        for change in changes {
            let key = (
                change.scope.clone(),
                change.realm.clone(),
                change.name.clone(),
            );
            if let Some(fibers) = entries.get(&key) {
                affected.extend(fibers.iter().cloned());
            }
        }
        affected.into_iter().collect()
    }

    /// Number of distinct service keys tracked.
    pub(crate) fn len(&self) -> usize {
        self.entries
            .read()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }
}

/// Coalesces service changes into the set of fibers that need a refresh.
///
/// The reconciler owns no fibers and performs no I/O. A host drains it, calls
/// Fiber::refresh for each returned id, and acts on the RefreshAction. Coalescing
/// by fiber id is what prevents a burst of service updates from becoming a
/// reload storm.
#[derive(Default)]
pub(crate) struct Reconciler {
    index: DependencyIndex,
    dirty: BTreeSet<String>,
}

impl Reconciler {
    /// Create an empty reconciler.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a fiber's declarations.
    pub(crate) fn register(&self, fiber: &str, inject: &Inject) {
        self.index.register(fiber, inject);
    }

    /// Remove a fiber's declarations.
    pub(crate) fn unregister(&self, fiber: &str, inject: &Inject) {
        self.index.unregister(fiber, inject);
    }

    /// Add the dependents of one change to the dirty set.
    pub(crate) fn on_change(&mut self, change: &ServiceChange) {
        self.dirty.extend(self.index.affected(change));
    }

    /// Add the dependents of a batch to the dirty set.
    pub(crate) fn on_changes(&mut self, changes: &[ServiceChange]) {
        self.dirty.extend(self.index.affected_many(changes));
    }

    /// Take the current dirty set in deterministic order. The next drain starts
    /// empty, so a fiber is refreshed once per coalesced batch.
    pub(crate) fn drain(&mut self) -> Vec<String> {
        std::mem::take(&mut self.dirty).into_iter().collect()
    }

    /// Whether any fiber is waiting for a refresh.
    pub(crate) fn is_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Number of distinct service keys tracked.
    pub(crate) fn tracked(&self) -> usize {
        self.index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::composable::{ServiceChangeKind, ServiceToken};
    use std::sync::Arc;

    fn registry() -> ServiceRegistry {
        ServiceRegistry::new()
    }

    fn change(scope: Scope, name: &str, generation: u64) -> ServiceChange {
        ServiceChange {
            name: name.to_owned(),
            scope,
            realm: None,
            generation,
            kind: ServiceChangeKind::Available,
        }
    }

    #[test]
    fn missing_hard_dependency_waits_then_reloads() {
        let registry = registry();
        let mut fiber = Fiber::new("fiber");
        fiber.set_inject(Inject::default().require(Scope::Host, "clock"));

        assert_eq!(fiber.refresh(&registry), RefreshAction::Noop);

        let token = registry.provide(Scope::Host, "clock", Arc::new(1_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);
        assert!(fiber
            .epoch()
            .deps
            .contains_key(&(Scope::Host, None, "clock".into())));

        registry.revoke(&token);
        assert_eq!(fiber.refresh(&registry), RefreshAction::Suspend);
        assert!(fiber.epoch().deps.is_empty());
    }

    #[test]
    fn replacement_bumps_the_epoch() {
        let registry = registry();
        let mut fiber = Fiber::new("fiber");
        fiber.set_inject(Inject::default().require(Scope::Host, "clock"));

        let first = registry.provide(Scope::Host, "clock", Arc::new(1_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);
        let first_generation = fiber.epoch().deps[&(Scope::Host, None, "clock".into())];

        let second = registry.provide(Scope::Host, "clock", Arc::new(2_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);
        let second_generation = fiber.epoch().deps[&(Scope::Host, None, "clock".into())];
        assert_ne!(first_generation, second_generation);

        registry.revoke(&first);
        assert_eq!(fiber.refresh(&registry), RefreshAction::Noop);
        registry.revoke(&second);
        assert_eq!(fiber.refresh(&registry), RefreshAction::Suspend);
    }

    #[test]
    fn soft_dependency_can_notify_instead_of_reload() {
        let registry = registry();
        let mut fiber = Fiber::new("fiber");
        fiber.set_inject(Inject::default().optional(Scope::Host, "telemetry", Reaction::Notify));

        let token = registry.provide(Scope::Host, "telemetry", Arc::new(1_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Notify);
        registry.revoke(&token);
        assert_eq!(fiber.refresh(&registry), RefreshAction::Notify);
    }

    #[test]
    fn revision_bump_forces_reload() {
        let registry = registry();
        let mut fiber = Fiber::new("fiber");
        fiber.set_inject(Inject::default().require(Scope::Host, "clock"));
        let _token = registry.provide(Scope::Host, "clock", Arc::new(1_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);
        fiber.bump_revision();
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);
    }

    #[test]
    fn isolated_dependency_tracks_only_its_realm() {
        let registry = ServiceRegistry::new();
        let mut fiber = Fiber::new("fiber");
        fiber.set_inject(Inject::default().require_in_realm(Scope::Host, "a", "clock"));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Noop);

        let other = registry.provide_in_realm(Scope::Host, Some("b"), "clock", Arc::new(2_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Noop);

        let mine = registry.provide_in_realm(Scope::Host, Some("a"), "clock", Arc::new(1_u64));
        assert_eq!(fiber.refresh(&registry), RefreshAction::Reload);

        registry.revoke(&mine);
        assert_eq!(fiber.refresh(&registry), RefreshAction::Suspend);
        registry.revoke(&other);
    }

    #[test]
    fn index_tracks_and_removes_dependents() {
        let index = DependencyIndex::new();
        let inject = Inject::default().require(Scope::Host, "clock").optional(
            Scope::Host,
            "telemetry",
            Reaction::Notify,
        );
        index.register("a", &inject);
        index.register("b", &inject);
        assert_eq!(index.len(), 2);

        assert_eq!(
            index.affected(&change(Scope::Host, "clock", 1)),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(index.affected(&change(Scope::Host, "other", 1)).is_empty());

        index.unregister("a", &inject);
        assert_eq!(
            index.affected(&change(Scope::Host, "clock", 2)),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn batch_changes_are_deduplicated() {
        let index = DependencyIndex::new();
        let inject = Inject::default().require(Scope::Host, "clock");
        index.register("a", &inject);
        index.register("b", &inject);

        let changes = vec![
            change(Scope::Host, "clock", 1),
            change(Scope::Host, "clock", 2),
        ];
        assert_eq!(
            index.affected_many(&changes),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn service_token_is_usable_for_revocation() {
        let registry = registry();
        let token: ServiceToken = registry.provide(Scope::Host, "clock", Arc::new(1_u64));
        assert!(registry.revoke(&token));
    }

    #[test]
    fn reconciler_coalesces_a_burst_into_one_batch() {
        let mut reconciler = Reconciler::new();
        let inject = Inject::default().require(Scope::Host, "clock");
        reconciler.register("a", &inject);
        reconciler.register("b", &inject);

        reconciler.on_change(&change(Scope::Host, "clock", 1));
        reconciler.on_change(&change(Scope::Host, "clock", 2));
        assert!(reconciler.is_dirty());
        assert_eq!(reconciler.drain(), vec!["a".to_string(), "b".to_string()]);
        assert!(!reconciler.is_dirty());
        assert!(reconciler.drain().is_empty());
    }

    #[test]
    fn reconciler_forgets_an_unregistered_fiber() {
        let mut reconciler = Reconciler::new();
        let inject = Inject::default().require(Scope::Host, "clock");
        reconciler.register("a", &inject);
        reconciler.register("b", &inject);
        reconciler.unregister("a", &inject);

        reconciler.on_change(&change(Scope::Host, "clock", 3));
        assert_eq!(reconciler.drain(), vec!["b".to_string()]);
    }
}
