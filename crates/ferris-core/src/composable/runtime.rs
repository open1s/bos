//! A small live composition runtime.
//!
//! This is the driver that turns the primitives (context, effects, epochs,
//! dependency index) into behavior. It owns fibers, activates components when
//! their hard dependencies are available, reloads them when a provider changes,
//! and suspends them when a provider disappears.
//!
//! It is deliberately host-agnostic: a component is a closure over a Context.
//! The BSH host can adapt its install path to this runtime without moving the
//! package, trust, or capability layers.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use super::context::Context;
use super::effects::DisposalReport;
use super::reactivity::{Inject, Reconciler, RefreshAction};
use super::{Fiber, FiberState, FiberTransitionError, Scope, ServiceChange, ServiceRegistry};

/// A unit of live composition. Applying it registers effects on the context.
pub trait Component: Send + Sync + 'static {
    /// Apply the component. Every registration should go through the context.
    fn apply(&self, ctx: &Context) -> Result<(), String>;
}

impl<F> Component for F
where
    F: Fn(&Context) -> Result<(), String> + Send + Sync + 'static,
{
    fn apply(&self, ctx: &Context) -> Result<(), String> {
        self(ctx)
    }
}

/// Why a runtime operation failed.
#[derive(Debug)]
pub enum RuntimeError {
    /// The id is already active or suspended.
    AlreadyRegistered(String),
    /// The id is unknown.
    NotRegistered(String),
    /// A state transition was illegal.
    Transition(FiberTransitionError),
    /// The component rejected activation.
    Apply(String),
    /// One or more fibers failed during a reconciliation batch; the rest of
    /// the batch still ran and the failed ids were requeued.
    Batch(Vec<String>),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered(id) => write!(f, "component {id} is already registered"),
            Self::NotRegistered(id) => write!(f, "component {id} is not registered"),
            Self::Transition(error) => fmt::Display::fmt(error, f),
            Self::Apply(message) => write!(f, "component apply failed: {message}"),
            Self::Batch(errors) => write!(f, "reconciliation failed: {}", errors.join("; ")),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<FiberTransitionError> for RuntimeError {
    fn from(error: FiberTransitionError) -> Self {
        Self::Transition(error)
    }
}

/// Live state of one node in a runtime snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    /// Applied and running.
    Active,
    /// Waiting for a hard dependency.
    Suspended,
}

/// One node in a runtime snapshot.
#[derive(Clone, Debug)]
pub struct RuntimeNode {
    /// Fiber id.
    pub id: String,
    /// Live state.
    pub state: RuntimeState,
    /// Declared scope.
    pub scope: Scope,
    /// Available service generations, sorted by name.
    pub dependencies: Vec<(String, u64)>,
    /// Effects still registered by an active fiber.
    pub pending_effects: usize,
}

/// A point-in-time view of the live graph.
#[derive(Clone, Debug, Default)]
pub struct RuntimeSnapshot {
    /// Nodes sorted by id.
    pub nodes: Vec<RuntimeNode>,
}

struct ManagedFiber {
    fiber: Fiber,
    component: Arc<dyn Component>,
}

struct SuspendedFiber {
    component: Arc<dyn Component>,
    inject: Inject,
    scope: Scope,
}

/// A live graph of components, fibers, and service dependencies.
pub struct CompositionRuntime {
    services: Arc<ServiceRegistry>,
    fibers: Mutex<BTreeMap<String, ManagedFiber>>,
    suspended: Mutex<BTreeMap<String, SuspendedFiber>>,
    reconciler: Mutex<Reconciler>,
    /// One async lifecycle gate per component id. Activate, unload and
    /// reconcile take it before touching that id, so remove-then-await cannot
    /// overlap another lifecycle transition on the same id.
    lifecycle: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Default for CompositionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositionRuntime {
    /// Create an empty runtime over a fresh registry.
    pub fn new() -> Self {
        Self::with_registry(Arc::new(ServiceRegistry::new()))
    }

    /// Create an empty runtime over an existing registry.
    pub fn with_registry(services: Arc<ServiceRegistry>) -> Self {
        Self {
            services,
            fibers: Mutex::new(BTreeMap::new()),
            suspended: Mutex::new(BTreeMap::new()),
            reconciler: Mutex::new(Reconciler::new()),
            lifecycle: Mutex::new(BTreeMap::new()),
        }
    }

    /// The per-id lifecycle gate, created on first use.
    fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.lifecycle.lock().expect("lifecycle map poisoned");
        Arc::clone(gates.entry(id.to_owned()).or_default())
    }

    /// The service registry this runtime composes against.
    pub fn services(&self) -> Arc<ServiceRegistry> {
        Arc::clone(&self.services)
    }

    /// Activate a component, or leave it suspended until its hard dependencies
    /// are available.
    pub async fn activate(
        &self,
        id: impl Into<String>,
        scope: Scope,
        component: Arc<dyn Component>,
        inject: Inject,
    ) -> Result<(), RuntimeError> {
        let id = id.into();
        let gate = self.gate(&id);
        let _lifecycle = gate.lock().await;
        if self.is_active(&id) || self.is_suspended(&id) {
            return Err(RuntimeError::AlreadyRegistered(id));
        }
        if let Ok(reconciler) = self.reconciler.lock() {
            reconciler.register(&id, &inject);
        }
        let inject_for_cleanup = inject.clone();
        if let Err(error) = self.start(&id, scope, component, inject).await {
            if let Ok(reconciler) = self.reconciler.lock() {
                reconciler.unregister(&id, &inject_for_cleanup);
            }
            return Err(error);
        }
        Ok(())
    }

    async fn start(
        &self,
        id: &str,
        scope: Scope,
        component: Arc<dyn Component>,
        inject: Inject,
    ) -> Result<(), RuntimeError> {
        if !inject.hard_ready(&self.services) {
            self.suspended
                .lock()
                .expect("suspended map poisoned")
                .insert(
                    id.to_owned(),
                    SuspendedFiber {
                        component,
                        inject,
                        scope,
                    },
                );
            return Ok(());
        }

        let context = Context::root(Arc::clone(&self.services), scope);
        let mut fiber = Fiber::with_context(id, context.clone());
        fiber.set_inject(inject);
        // Snapshot the dependency generations before applying. Without this the
        // new fiber would start with an empty epoch and a later provider
        // removal would look like Noop.
        let _ = fiber.refresh(&self.services);
        fiber.transition(FiberState::Loading)?;
        if let Err(message) = component.apply(&context) {
            let _ = fiber.transition(FiberState::Failed);
            return Err(RuntimeError::Apply(message));
        }
        fiber.transition(FiberState::Active)?;
        self.fibers
            .lock()
            .expect("fiber map poisoned")
            .insert(id.to_owned(), ManagedFiber { fiber, component });
        Ok(())
    }

    /// Unload one component and await its effects.
    pub async fn unload(&self, id: &str) -> Result<DisposalReport, RuntimeError> {
        let gate = self.gate(id);
        let _lifecycle = gate.lock().await;
        let managed = self.fibers.lock().expect("fiber map poisoned").remove(id);
        let suspended = self
            .suspended
            .lock()
            .expect("suspended map poisoned")
            .remove(id);

        let (inject, report) = if let Some(mut managed) = managed {
            let inject = managed.fiber.inject().clone();
            let report = managed.fiber.dispose_async().await?;
            (Some(inject), report)
        } else if let Some(suspended) = suspended {
            (Some(suspended.inject), DisposalReport::default())
        } else {
            (None, DisposalReport::default())
        };

        let Some(inject) = inject else {
            return Err(RuntimeError::NotRegistered(id.to_owned()));
        };
        if let Ok(reconciler) = self.reconciler.lock() {
            reconciler.unregister(id, &inject);
        }
        Ok(report)
    }

    /// Apply one service availability change, reloading or suspending exactly
    /// the affected fibers. Returns how many fibers changed state.
    pub async fn on_change(&self, change: &ServiceChange) -> Result<usize, RuntimeError> {
        let affected = {
            let mut reconciler = self.reconciler.lock().expect("reconciler poisoned");
            reconciler.on_change(change);
            reconciler.drain()
        };
        let mut changed = 0;
        let mut failed = Vec::new();
        for id in affected {
            match self.reconcile_one(&id).await {
                Ok(count) => changed += count,
                Err(error) => failed.push((id, error)),
            }
        }
        if !failed.is_empty() {
            // Keep the failed ids dirty for a later service change instead of
            // dropping work that a partial batch could not finish.
            if let Ok(mut reconciler) = self.reconciler.lock() {
                reconciler.requeue(failed.iter().map(|(id, _)| id.clone()));
            }
            return Err(RuntimeError::Batch(
                failed
                    .into_iter()
                    .map(|(id, error)| format!("{id}: {error}"))
                    .collect(),
            ));
        }
        Ok(changed)
    }

    async fn reconcile_one(&self, id: &str) -> Result<usize, RuntimeError> {
        let gate = self.gate(id);
        let _lifecycle = gate.lock().await;
        let managed = { self.fibers.lock().expect("fiber map poisoned").remove(id) };
        if let Some(managed) = managed {
            return self.reconcile_active(id, managed).await;
        }
        let suspended = {
            self.suspended
                .lock()
                .expect("suspended map poisoned")
                .remove(id)
        };
        if let Some(suspended) = suspended {
            if suspended.inject.hard_ready(&self.services) {
                let inject = suspended.inject.clone();
                let component = Arc::clone(&suspended.component);
                let scope = suspended.scope.clone();
                match self
                    .start(id, scope.clone(), Arc::clone(&component), inject.clone())
                    .await
                {
                    Ok(()) => return Ok(1),
                    Err(error) => {
                        // Keep the fiber retryable and let on_change requeue it.
                        self.suspended
                            .lock()
                            .expect("suspended map poisoned")
                            .insert(
                                id.to_owned(),
                                SuspendedFiber {
                                    component,
                                    inject,
                                    scope,
                                },
                            );
                        return Err(error);
                    }
                }
            }
            self.suspended
                .lock()
                .expect("suspended map poisoned")
                .insert(id.to_owned(), suspended);
        }
        Ok(0)
    }

    async fn reconcile_active(
        &self,
        id: &str,
        mut managed: ManagedFiber,
    ) -> Result<usize, RuntimeError> {
        let scope = managed
            .fiber
            .context()
            .map(|context| context.scope().clone())
            .unwrap_or(Scope::Host);
        let action = managed.fiber.refresh(&self.services);
        match action {
            RefreshAction::Reload => {
                let inject = managed.fiber.inject().clone();
                let component = Arc::clone(&managed.component);
                managed.fiber.dispose_async().await?;
                match self
                    .start(id, scope.clone(), Arc::clone(&component), inject.clone())
                    .await
                {
                    Ok(()) => Ok(1),
                    Err(error) => {
                        // The old fiber is gone; keep the component retryable
                        // rather than dropping it on a transient apply error.
                        self.suspended
                            .lock()
                            .expect("suspended map poisoned")
                            .insert(
                                id.to_owned(),
                                SuspendedFiber {
                                    component,
                                    inject,
                                    scope,
                                },
                            );
                        Err(error)
                    }
                }
            }
            RefreshAction::Suspend => {
                let inject = managed.fiber.inject().clone();
                let component = Arc::clone(&managed.component);
                managed.fiber.dispose_async().await?;
                self.suspended
                    .lock()
                    .expect("suspended map poisoned")
                    .insert(
                        id.to_owned(),
                        SuspendedFiber {
                            component,
                            inject,
                            scope,
                        },
                    );
                Ok(1)
            }
            RefreshAction::Noop | RefreshAction::Notify => {
                self.fibers
                    .lock()
                    .expect("fiber map poisoned")
                    .insert(id.to_owned(), managed);
                Ok(0)
            }
        }
    }

    /// Whether a component is currently active.
    pub fn is_active(&self, id: &str) -> bool {
        self.fibers
            .lock()
            .map(|fibers| fibers.contains_key(id))
            .unwrap_or(false)
    }

    /// Whether a component is waiting for a hard dependency.
    pub fn is_suspended(&self, id: &str) -> bool {
        self.suspended
            .lock()
            .map(|suspended| suspended.contains_key(id))
            .unwrap_or(false)
    }

    /// Number of active fibers.
    pub fn active_count(&self) -> usize {
        self.fibers.lock().map(|fibers| fibers.len()).unwrap_or(0)
    }

    /// Number of suspended fibers.
    pub fn suspended_count(&self) -> usize {
        self.suspended
            .lock()
            .map(|suspended| suspended.len())
            .unwrap_or(0)
    }

    /// Point-in-time view of every active and suspended node.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        let mut nodes = Vec::new();

        if let Ok(fibers) = self.fibers.lock() {
            for (id, managed) in fibers.iter() {
                let scope = managed
                    .fiber
                    .context()
                    .map(|context| context.scope().clone())
                    .unwrap_or(Scope::Host);
                let mut dependencies: Vec<(String, u64)> = managed
                    .fiber
                    .epoch()
                    .deps
                    .iter()
                    .map(|((_, _, name), generation)| (name.clone(), *generation))
                    .collect();
                dependencies.sort();
                nodes.push(RuntimeNode {
                    id: id.clone(),
                    state: RuntimeState::Active,
                    scope,
                    dependencies,
                    pending_effects: managed.fiber.pending_effects(),
                });
            }
        }

        if let Ok(suspended) = self.suspended.lock() {
            for (id, entry) in suspended.iter() {
                let mut dependencies: Vec<(String, u64)> = entry
                    .inject
                    .all()
                    .filter_map(|requirement| {
                        self.services
                            .generation(&requirement.scope, &requirement.name)
                            .map(|generation| (requirement.name.clone(), generation))
                    })
                    .collect();
                dependencies.sort();
                nodes.push(RuntimeNode {
                    id: id.clone(),
                    state: RuntimeState::Suspended,
                    scope: entry.scope.clone(),
                    dependencies,
                    pending_effects: 0,
                });
            }
        }

        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        RuntimeSnapshot { nodes }
    }

    /// Explain why a node is active, suspended, or unknown.
    pub fn why(&self, id: &str) -> Option<String> {
        if self.is_active(id) {
            return Some("active".to_string());
        }
        let suspended = self.suspended.lock().ok()?;
        let entry = suspended.get(id)?;
        let missing = entry.inject.missing_hard(&self.services);
        if missing.is_empty() {
            return Some("suspended: waiting for update".to_string());
        }
        let names = missing
            .into_iter()
            .map(|(scope, realm, name)| match realm {
                Some(realm) => format!("{name}@{realm} [{scope:?}]"),
                None => format!("{name} [{scope:?}]"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("suspended: missing {names}"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::context::EffectCleanup;
    use super::*;
    use crate::composable::{ServiceChangeKind, ServiceToken};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn change(kind: ServiceChangeKind, name: &str, generation: u64) -> ServiceChange {
        ServiceChange {
            name: name.to_owned(),
            scope: Scope::Host,
            realm: None,
            generation,
            kind,
        }
    }

    #[tokio::test]
    #[ignore = "release benchmark; run with --release --ignored"]
    async fn gold_standard_release_activation_budget() {
        let runtime = CompositionRuntime::new();
        let component = |_ctx: &Context| Ok(());
        let start = std::time::Instant::now();
        for index in 0..1_000 {
            runtime
                .activate(
                    format!("c{index}"),
                    Scope::Host,
                    Arc::new(component),
                    Inject::default(),
                )
                .await
                .expect("activate");
        }
        let elapsed = start.elapsed();
        assert_eq!(runtime.active_count(), 1_000);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "1k activations took {elapsed:?} in release"
        );
    }

    #[tokio::test]
    #[ignore = "release benchmark; run with --release --ignored"]
    async fn gold_standard_release_reload_budget() {
        let runtime = CompositionRuntime::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let component = {
            let runs = Arc::clone(&runs);
            move |_ctx: &Context| {
                runs.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        };
        runtime
            .activate(
                "consumer",
                Scope::Host,
                Arc::new(component),
                Inject::default().require(Scope::Host, "clock"),
            )
            .await
            .expect("activate");
        let _first = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(0_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Available, "clock", 1))
            .await
            .expect("available");

        let start = std::time::Instant::now();
        for generation in 1..1_000 {
            let _token = runtime
                .services()
                .provide(Scope::Host, "clock", Arc::new(generation));
            runtime
                .on_change(&change(ServiceChangeKind::Replaced, "clock", generation))
                .await
                .expect("replaced");
        }
        let elapsed = start.elapsed();
        assert!(runs.load(Ordering::Relaxed) >= 1_000);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "1k reloads took {elapsed:?} in release"
        );
    }

    #[tokio::test]
    async fn consumer_waits_then_reloads_on_provider_change() {
        let runtime = CompositionRuntime::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let cleanups = Arc::new(AtomicUsize::new(0));

        let component = {
            let seen = Arc::clone(&seen);
            let cleanups = Arc::clone(&cleanups);
            move |ctx: &Context| {
                let clock = ctx.acquire::<u64>("clock").ok_or("clock missing")?;
                seen.lock().expect("seen").push(*clock);
                let cleanups = Arc::clone(&cleanups);
                ctx.effect("consumer.cleanup", move |_| {
                    EffectCleanup::sync(move || {
                        cleanups.fetch_add(1, Ordering::SeqCst);
                    })
                })
                .map_err(|error| error.to_string())?;
                Ok(())
            }
        };

        runtime
            .activate(
                "consumer",
                Scope::Host,
                Arc::new(component),
                Inject::default().require(Scope::Host, "clock"),
            )
            .await
            .expect("activate");
        assert!(!runtime.is_active("consumer"));
        assert!(runtime.is_suspended("consumer"));

        let first = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(1_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Available, "clock", 1))
            .await
            .expect("available");
        assert!(runtime.is_active("consumer"));
        assert_eq!(*seen.lock().expect("seen"), vec![1]);
        assert_eq!(cleanups.load(Ordering::SeqCst), 0);

        let second = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(2_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Replaced, "clock", 2))
            .await
            .expect("replaced");
        assert!(runtime.is_active("consumer"));
        assert_eq!(*seen.lock().expect("seen"), vec![1, 2]);
        assert_eq!(cleanups.load(Ordering::SeqCst), 1);

        let _ = first;
        runtime.services().revoke(&second);
        runtime
            .on_change(&change(ServiceChangeKind::Unavailable, "clock", 2))
            .await
            .expect("unavailable");
        assert!(!runtime.is_active("consumer"));
        assert!(runtime.is_suspended("consumer"));
        assert_eq!(cleanups.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn one_failing_dependent_does_not_skip_the_rest_of_the_batch() {
        let runtime = CompositionRuntime::new();
        let inject = Inject::default().require(Scope::Host, "clock");
        for (id, fails) in [("a", true), ("b", false), ("c", false)] {
            let component = move |_ctx: &Context| {
                if fails {
                    Err("boom".to_string())
                } else {
                    Ok(())
                }
            };
            runtime
                .activate(id, Scope::Host, Arc::new(component), inject.clone())
                .await
                .expect("activate");
            assert!(runtime.is_suspended(id));
        }
        let _clock = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(1_u64));
        let error = match runtime
            .on_change(&change(ServiceChangeKind::Available, "clock", 1))
            .await
        {
            Ok(_) => panic!("the failing dependent must be reported"),
            Err(error) => error,
        };
        match error {
            RuntimeError::Batch(errors) => assert_eq!(errors.len(), 1, "{errors:?}"),
            other => panic!("expected a batch error, got {other:?}"),
        }
        assert!(runtime.is_active("b"), "the second dependent still ran");
        assert!(runtime.is_active("c"), "the third dependent still ran");
        assert!(
            runtime.is_suspended("a"),
            "the failed dependent stays retryable"
        );
    }

    #[tokio::test]
    async fn unload_does_not_forget_a_fiber_activated_during_its_disposal() {
        let runtime = Arc::new(CompositionRuntime::new());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let cleanup_done = Arc::new(AtomicBool::new(false));
        let first = {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let cleanup_done = Arc::clone(&cleanup_done);
            move |ctx: &Context| {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                let cleanup_done = Arc::clone(&cleanup_done);
                ctx.effect("blocking", move |_| {
                    EffectCleanup::async_cleanup(async move {
                        started.notify_waiters();
                        release.notified().await;
                        cleanup_done.store(true, Ordering::SeqCst);
                    })
                })
                .map_err(|error| error.to_string())?;
                Ok(())
            }
        };
        let inject = Inject::default().require(Scope::Host, "clock");
        let _clock = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(1_u64));
        runtime
            .activate("x", Scope::Host, Arc::new(first), inject.clone())
            .await
            .expect("activate");

        let unload = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move { runtime.unload("x").await })
        };
        started.notified().await;

        let second = {
            let cleanup_done = Arc::clone(&cleanup_done);
            move |_ctx: &Context| {
                if cleanup_done.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err("activated before the old disposal finished".to_string())
                }
            }
        };
        let activate = {
            let runtime = Arc::clone(&runtime);
            tokio::spawn(async move {
                runtime
                    .activate("x", Scope::Host, Arc::new(second), inject.clone())
                    .await
            })
        };
        release.notify_waiters();
        unload.await.expect("join unload").expect("unload");
        activate
            .await
            .expect("join activate")
            .expect("activate after unload");

        let token = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(2_u64));
        runtime.services().revoke(&token);
        runtime
            .on_change(&change(ServiceChangeKind::Unavailable, "clock", 2))
            .await
            .expect("unavailable");
        assert!(
            runtime.is_suspended("x"),
            "the new fiber's reconciler registration must survive the old unload"
        );
    }
    #[tokio::test]
    async fn unload_disposes_effects_and_forgets_the_fiber() {
        let runtime = CompositionRuntime::new();
        let cleanups = Arc::new(AtomicUsize::new(0));
        let component = {
            let cleanups = Arc::clone(&cleanups);
            move |ctx: &Context| {
                let marker = Arc::clone(&cleanups);
                ctx.effect("cleanup", move |_| {
                    EffectCleanup::sync(move || {
                        marker.fetch_add(1, Ordering::SeqCst);
                    })
                })
                .map_err(|error| error.to_string())?;
                Ok(())
            }
        };

        runtime
            .activate("idle", Scope::Host, Arc::new(component), Inject::default())
            .await
            .expect("activate");
        assert!(runtime.is_active("idle"));
        let report = runtime.unload("idle").await.expect("unload");
        assert_eq!(report.attempted, 1);
        assert_eq!(cleanups.load(Ordering::SeqCst), 1);
        assert!(!runtime.is_active("idle"));
        assert!(matches!(
            runtime.unload("idle").await,
            Err(RuntimeError::NotRegistered(_))
        ));
    }

    #[tokio::test]
    async fn soft_notify_keeps_the_fiber_active() {
        let runtime = CompositionRuntime::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let component = {
            let runs = Arc::clone(&runs);
            move |_ctx: &Context| {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };

        runtime
            .activate(
                "observer",
                Scope::Host,
                Arc::new(component),
                Inject::default().optional(
                    Scope::Host,
                    "telemetry",
                    super::super::reactivity::Reaction::Notify,
                ),
            )
            .await
            .expect("activate");
        assert!(runtime.is_active("observer"));

        let token = runtime
            .services()
            .provide(Scope::Host, "telemetry", Arc::new(1_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Available, "telemetry", 1))
            .await
            .expect("available");
        assert!(runtime.is_active("observer"));
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        runtime.services().revoke(&token);
        runtime
            .on_change(&change(ServiceChangeKind::Unavailable, "telemetry", 1))
            .await
            .expect("unavailable");
        assert!(runtime.is_active("observer"));
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn replacement_can_activate_a_suspended_consumer() {
        let runtime = CompositionRuntime::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let component = {
            let runs = Arc::clone(&runs);
            move |_ctx: &Context| {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };

        runtime
            .activate(
                "consumer",
                Scope::Host,
                Arc::new(component),
                Inject::default().require(Scope::Host, "clock"),
            )
            .await
            .expect("activate");
        assert!(runtime.is_suspended("consumer"));

        let token: ServiceToken = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(1_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Available, "clock", 1))
            .await
            .expect("available");
        assert!(runtime.is_active("consumer"));
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        runtime.services().revoke(&token);
        runtime
            .on_change(&change(ServiceChangeKind::Unavailable, "clock", 1))
            .await
            .expect("unavailable");
        assert!(runtime.is_suspended("consumer"));
    }

    #[tokio::test]
    async fn snapshot_and_why_explain_the_live_graph() {
        let runtime = CompositionRuntime::new();
        let component = |_ctx: &Context| Ok(());
        runtime
            .activate(
                "consumer",
                Scope::Host,
                Arc::new(component),
                Inject::default().require(Scope::Host, "clock"),
            )
            .await
            .expect("activate");

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].state, RuntimeState::Suspended);
        assert!(runtime.why("consumer").unwrap().contains("clock"));

        let _token = runtime
            .services()
            .provide(Scope::Host, "clock", Arc::new(1_u64));
        runtime
            .on_change(&change(ServiceChangeKind::Available, "clock", 1))
            .await
            .expect("available");

        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.nodes[0].state, RuntimeState::Active);
        assert_eq!(
            snapshot.nodes[0].dependencies,
            vec![("clock".to_string(), 1)]
        );
        assert_eq!(runtime.why("consumer").unwrap(), "active");
        assert!(runtime.why("missing").is_none());
    }
}
