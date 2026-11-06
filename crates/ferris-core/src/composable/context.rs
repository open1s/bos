//! The composition context.
//!
//! A context is the only surface a component uses. It carries the scope path,
//! the service registry it resolves against, and the effect scope that owns
//! everything the component registers.
//!
//! This is the Phase A primitive: it does not yet enforce manifest declarations
//! or capability policy at acquisition. Those checks stay at the host boundary
//! and will be added when the composition plan is wired to the agent build.

use std::any::Any;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::effects::{BoxFuture, DisposalReport, Disposer, EffectScope};
use super::{Scope, ServiceLease, ServiceRegistry, ServiceToken};

/// What kind of context this is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextKind {
    /// The host root.
    Root,
    /// One extension's activation.
    Extension(String),
    /// A child component inside an extension.
    Component {
        /// Parent fiber label.
        parent: String,
        /// Component name.
        name: String,
    },
    /// A workspace-scoped context.
    Workspace(String),
    /// A session-scoped context.
    Session(String),
}

/// The cleanup an effect body returns.
///
/// The body runs immediately when the effect is registered. Returning
/// EffectCleanup::None means the body registered its own effects through the
/// context; returning Sync or Async hands the runtime one explicit inverse.
pub enum EffectCleanup {
    /// Nothing to undo.
    None,
    /// A synchronous inverse.
    Sync(Box<dyn FnOnce() + Send + 'static>),
    /// An asynchronous inverse.
    Async(BoxFuture),
}

impl EffectCleanup {
    /// Wrap a synchronous closure.
    pub fn sync(cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self::Sync(Box::new(cleanup))
    }

    /// Wrap an asynchronous closure.
    pub fn async_cleanup(cleanup: impl Future<Output = ()> + Send + 'static) -> Self {
        Self::Async(Box::pin(cleanup))
    }
}

/// Why an effect could not be registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectError {
    /// The owning context has already been disposed.
    Inactive,
}

impl std::fmt::Display for EffectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inactive => write!(f, "cannot create an effect on an inactive context"),
        }
    }
}

impl std::error::Error for EffectError {}

/// Checks whether a consumer may acquire a service. The host supplies this so
/// service discovery never becomes authorization.
pub trait ServiceAuthorizer: Send + Sync {
    /// Allow or deny one acquisition. The error is a human-readable reason.
    fn authorize(&self, consumer: &str, scope: &Scope, name: &str) -> Result<(), String>;
}

/// A live composition context.
#[derive(Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

struct ContextInner {
    id: u64,
    parent: Option<Context>,
    kind: ContextKind,
    scope: Scope,
    path: Vec<Scope>,
    services: Arc<ServiceRegistry>,
    effects: EffectScope,
    active: Arc<AtomicBool>,
    owner: String,
    authorizer: Option<Arc<dyn ServiceAuthorizer>>,
    realms: Arc<BTreeMap<String, String>>,
}

impl Context {
    /// Create a root context for one registry and scope.
    pub fn root(services: Arc<ServiceRegistry>, scope: Scope) -> Self {
        Self::root_authorized(services, scope, String::new(), None)
    }

    /// Create a root context with an owner and an acquisition authorizer.
    pub fn root_authorized(
        services: Arc<ServiceRegistry>,
        scope: Scope,
        owner: impl Into<String>,
        authorizer: Option<Arc<dyn ServiceAuthorizer>>,
    ) -> Self {
        Self {
            inner: Arc::new(ContextInner {
                id: next_id(),
                parent: None,
                kind: ContextKind::Root,
                path: inherited_scopes(&scope),
                scope,
                services,
                effects: EffectScope::new(),
                active: Arc::new(AtomicBool::new(true)),
                owner: owner.into(),
                authorizer,
                realms: Arc::new(BTreeMap::new()),
            }),
        }
    }

    /// Create a child context. Its effects are a child scope of this context.
    pub fn child(&self, kind: ContextKind, scope: Scope) -> Self {
        let effects = self.effects().child(format!("context/{kind:?}"));
        Self {
            inner: Arc::new(ContextInner {
                id: next_id(),
                parent: Some(self.clone()),
                kind,
                path: child_scopes(&self.inner.path, &scope),
                scope,
                services: Arc::clone(&self.inner.services),
                effects,
                active: Arc::clone(&self.inner.active),
                owner: self.inner.owner.clone(),
                authorizer: self.inner.authorizer.clone(),
                realms: Arc::clone(&self.inner.realms),
            }),
        }
    }

    /// Return a view whose lookup for one service name uses a named realm.
    /// The view shares the parent's effect scope and lifetime.
    pub fn isolate(&self, name: impl Into<String>, label: impl Into<String>) -> Self {
        let mut realms = (*self.inner.realms).clone();
        realms.insert(name.into(), label.into());
        Self {
            inner: Arc::new(ContextInner {
                id: next_id(),
                parent: Some(self.clone()),
                kind: self.inner.kind.clone(),
                scope: self.inner.scope.clone(),
                path: self.inner.path.clone(),
                services: Arc::clone(&self.inner.services),
                effects: self.inner.effects.clone(),
                active: Arc::clone(&self.inner.active),
                owner: self.inner.owner.clone(),
                authorizer: self.inner.authorizer.clone(),
                realms: Arc::new(realms),
            }),
        }
    }

    /// The realm assigned to one service name, if any.
    pub fn realm_for(&self, name: &str) -> Option<&str> {
        self.inner.realms.get(name).map(String::as_str)
    }

    /// Numeric identity, unique within the process.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// Kind label.
    pub fn kind(&self) -> &ContextKind {
        &self.inner.kind
    }

    /// The service scope this context resolves in.
    pub fn scope(&self) -> &Scope {
        &self.inner.scope
    }

    /// The resolution path, most specific scope first.
    pub fn path(&self) -> &[Scope] {
        &self.inner.path
    }

    /// Parent context, if any.
    pub fn parent(&self) -> Option<&Context> {
        self.inner.parent.as_ref()
    }

    /// Whether the context can still own effects.
    pub fn is_active(&self) -> bool {
        self.inner.active.load(Ordering::Acquire)
    }

    /// Register an effect body. The body runs now; its returned cleanup is
    /// owned by this context.
    pub fn effect<F>(&self, label: impl Into<String>, body: F) -> Result<Disposer, EffectError>
    where
        F: FnOnce(&Context) -> EffectCleanup,
    {
        if !self.is_active() {
            return Err(EffectError::Inactive);
        }
        let label = label.into();
        match body(self) {
            EffectCleanup::None => Ok(self.inner.effects.push_sync(label, || {})),
            EffectCleanup::Sync(cleanup) => Ok(self.inner.effects.push_sync(label, cleanup)),
            EffectCleanup::Async(cleanup) => {
                Ok(self.inner.effects.push_async(label, move || cleanup))
            }
        }
    }

    /// Resolve a typed service along this context's scope path, most specific
    /// first. Extension scope is never inherited by another context.
    ///
    /// An authorizer, when installed, is consulted before each lookup; a denial
    /// is treated as absence, so the parent scope is still tried.
    pub fn acquire<T>(&self, name: &str) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        for scope in &self.inner.path {
            if !self.authorized(scope, name) {
                continue;
            }
            if let Some(lease) =
                self.inner
                    .services
                    .acquire_in_realm(scope, self.realm_for(name), name)
            {
                return Some(lease);
            }
        }
        None
    }

    /// Resolve only in the exact declared scope.
    pub fn acquire_exact<T>(&self, name: &str) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        if !self.authorized(&self.inner.scope, name) {
            return None;
        }
        self.inner
            .services
            .acquire_in_realm(&self.inner.scope, self.realm_for(name), name)
    }

    /// Provide a typed service in this context's exact scope.
    /// Resolve at one exact scope, still passing the authorizer. This is the
    /// path a host adapter uses when it knows the declared scope explicitly.
    pub fn acquire_at<T>(&self, scope: &Scope, name: &str) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        if !self.authorized(scope, name) {
            return None;
        }
        self.inner
            .services
            .acquire_in_realm(scope, self.realm_for(name), name)
    }

    /// Resolve one exact contract at one exact scope, passing the authorizer.
    pub fn acquire_contract_at<T>(
        &self,
        scope: &Scope,
        name: &str,
        contract: &str,
        requirement: &semver::VersionReq,
    ) -> Option<ServiceLease<T>>
    where
        T: Any + Send + Sync,
    {
        if !self.authorized(scope, name) {
            return None;
        }
        self.inner
            .services
            .acquire_contract(scope, name, contract, requirement)
    }

    /// Provide one exact contract into this context's scope.
    pub fn provide_contract<T>(
        &self,
        name: &str,
        contract: &str,
        version: semver::Version,
        service: Arc<T>,
    ) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        self.inner
            .services
            .provide_with_contract(
                self.inner.scope.clone(),
                self.realm_for(name),
                name,
                contract,
                version,
                service,
            )
            .0
    }

    pub fn provide<T>(&self, name: &str, service: Arc<T>) -> ServiceToken
    where
        T: Any + Send + Sync,
    {
        self.inner.services.provide_in_realm(
            self.inner.scope.clone(),
            self.realm_for(name),
            name,
            service,
        )
    }

    /// Whether the context's scope path can see a service.
    pub fn contains<T>(&self, name: &str) -> bool
    where
        T: Any + Send + Sync,
    {
        self.inner.path.iter().any(|scope| {
            self.authorized(scope, name)
                && self
                    .inner
                    .services
                    .contains_in_realm::<T>(scope, self.realm_for(name), name)
        })
    }

    fn authorized(&self, scope: &Scope, name: &str) -> bool {
        match &self.inner.authorizer {
            Some(authorizer) => authorizer.authorize(&self.inner.owner, scope, name).is_ok(),
            None => true,
        }
    }

    /// Mark the context inactive without disposing its effects. The owner of
    /// the effect scope (a Fiber) calls this before its own disposal.
    pub fn deactivate(&self) {
        self.inner.active.store(false, Ordering::Release);
    }

    /// Deactivate the context and dispose every effect it owns.
    pub async fn dispose(&self) -> DisposalReport {
        self.inner.active.store(false, Ordering::Release);
        self.inner.effects.dispose().await
    }

    /// Dispose synchronously and spawn async cleanups that cannot be awaited.
    pub fn dispose_sync(&self) -> DisposalReport {
        self.inner.active.store(false, Ordering::Release);
        self.inner.effects.dispose_sync()
    }

    /// The effect scope, for a fiber that needs to attach the same ownership.
    pub fn effects(&self) -> &EffectScope {
        &self.inner.effects
    }
}

/// The scopes a root context can resolve through, most specific first.
fn inherited_scopes(scope: &Scope) -> Vec<Scope> {
    match scope {
        Scope::Host => vec![Scope::Host],
        Scope::Workspace(_) | Scope::Session(_) | Scope::Extension(_) => {
            vec![scope.clone(), Scope::Host]
        }
    }
}

/// Extend an existing path with a child scope. Ancestor extension scopes are
/// not inherited: an extension's private services never leak to a sibling or to
/// a child context.
fn child_scopes(parent: &[Scope], scope: &Scope) -> Vec<Scope> {
    let mut path = Vec::with_capacity(parent.len() + 1);
    path.push(scope.clone());
    for ancestor in parent {
        if matches!(ancestor, Scope::Extension(_)) {
            continue;
        }
        if ancestor != scope {
            path.push(ancestor.clone());
        }
    }
    path
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn registry() -> Arc<ServiceRegistry> {
        Arc::new(ServiceRegistry::new())
    }

    #[tokio::test]
    async fn child_effects_dispose_with_the_parent() {
        let parent = Context::root(registry(), Scope::Host);
        let child = parent.child(
            ContextKind::Component {
                parent: "root".into(),
                name: "child".into(),
            },
            Scope::Extension("com.example.child".into()),
        );

        let count = Arc::new(AtomicUsize::new(0));
        let marker = count.clone();
        child
            .effect("child.cleanup", move |_| {
                EffectCleanup::sync(move || {
                    marker.fetch_add(1, Ordering::SeqCst);
                })
            })
            .unwrap();

        assert!(parent.is_active());
        parent.dispose().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!parent.is_active());
    }

    #[tokio::test]
    async fn effect_body_runs_immediately_and_cleanup_runs_once() {
        let ctx = Context::root(registry(), Scope::Host);
        let runs = Arc::new(AtomicUsize::new(0));
        let cleanups = Arc::new(AtomicUsize::new(0));

        let body_runs = runs.clone();
        let body_cleanups = cleanups.clone();
        ctx.effect("work", move |_| {
            body_runs.fetch_add(1, Ordering::SeqCst);
            EffectCleanup::sync(move || {
                body_cleanups.fetch_add(1, Ordering::SeqCst);
            })
        })
        .unwrap();

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(cleanups.load(Ordering::SeqCst), 0);
        ctx.dispose().await;
        assert_eq!(cleanups.load(Ordering::SeqCst), 1);
        assert_eq!(ctx.dispose().await.attempted, 0);
    }

    #[test]
    fn disposed_context_rejects_new_effects() {
        let ctx = Context::root(registry(), Scope::Host);
        ctx.dispose_sync();
        assert!(matches!(
            ctx.effect("late", |_| EffectCleanup::None),
            Err(EffectError::Inactive)
        ));
    }

    #[tokio::test]
    async fn services_are_scoped_to_the_context() {
        let ctx = Context::root(registry(), Scope::Host);
        ctx.provide("clock", Arc::new(7_u64));
        let value = ctx.acquire::<u64>("clock").unwrap();
        assert_eq!(*value, 7);
        assert!(ctx.contains::<u64>("clock"));
        assert!(!ctx.contains::<u64>("missing"));
    }

    #[tokio::test]
    async fn isolate_realms_keep_same_name_services_apart() {
        let root = Context::root(registry(), Scope::Host);
        let a = root.isolate("clock", "a");
        let b = root.isolate("clock", "b");

        a.provide("clock", Arc::new(1_u64));
        b.provide("clock", Arc::new(2_u64));

        assert_eq!(*a.acquire::<u64>("clock").unwrap(), 1);
        assert_eq!(*b.acquire::<u64>("clock").unwrap(), 2);
        assert!(root.acquire::<u64>("clock").is_none());
        assert_eq!(a.realm_for("clock"), Some("a"));
    }

    // A Tokio test, not a plain one: dropping the Context runs async cleanup
    // through tokio::time::timeout, which panics with "there is no reactor
    // running" when the benchmark is run outside a runtime.
    #[tokio::test]
    #[ignore = "release benchmark; run with --release --ignored"]
    async fn gold_standard_release_inherited_lookup_budget() {
        let registry = Arc::new(ServiceRegistry::new());
        let root = Context::root(Arc::clone(&registry), Scope::Host);
        root.provide("clock", Arc::new(1_u64));
        let workspace = root.child(
            ContextKind::Workspace("w".into()),
            Scope::Workspace("w".into()),
        );
        let session = workspace.child(ContextKind::Session("s".into()), Scope::Session("s".into()));
        let extension = session.child(
            ContextKind::Extension("e".into()),
            Scope::Extension("e".into()),
        );
        let start = std::time::Instant::now();
        for _ in 0..500_000 {
            assert!(extension.acquire::<u64>("clock").is_some());
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "500k inherited lookups took {elapsed:?} in release"
        );
    }

    #[test]
    #[ignore = "release benchmark; run with --release --ignored"]
    fn gold_standard_release_context_clone_budget() {
        let registry = Arc::new(ServiceRegistry::new());
        let context = Context::root(registry, Scope::Host);
        let start = std::time::Instant::now();
        for _ in 0..1_000_000 {
            let clone = context.clone();
            std::hint::black_box(&clone);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "1M context clones took {elapsed:?} in release"
        );
    }

    struct DenySecret;

    impl ServiceAuthorizer for DenySecret {
        fn authorize(&self, _consumer: &str, _scope: &Scope, name: &str) -> Result<(), String> {
            if name == "secret" {
                Err("secret is not granted".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn an_authorizer_can_deny_an_acquisition() {
        let registry = registry();
        let ctx = Context::root_authorized(
            Arc::clone(&registry),
            Scope::Host,
            "com.example.consumer",
            Some(Arc::new(DenySecret)),
        );
        ctx.provide("clock", Arc::new(1_u64));
        ctx.provide("secret", Arc::new(2_u64));

        assert!(ctx.acquire::<u64>("clock").is_some());
        assert!(ctx.acquire::<u64>("secret").is_none());
        assert!(ctx.acquire_exact::<u64>("secret").is_none());
        assert!(!ctx.contains::<u64>("secret"));
    }

    #[tokio::test]
    async fn host_services_are_visible_to_workspace_and_session_children() {
        let parent = Context::root(registry(), Scope::Host);
        parent.provide("clock", Arc::new(1_u64));

        let workspace = parent.child(
            ContextKind::Workspace("w".into()),
            Scope::Workspace("w".into()),
        );
        assert_eq!(*workspace.acquire::<u64>("clock").unwrap(), 1);

        let session = workspace.child(ContextKind::Session("s".into()), Scope::Session("s".into()));
        assert_eq!(*session.acquire::<u64>("clock").unwrap(), 1);
        assert_eq!(session.path().len(), 3);
    }

    #[tokio::test]
    async fn extension_private_services_do_not_leak_to_a_sibling() {
        let workspace = Context::root(registry(), Scope::Workspace("w".into()));
        let one = workspace.child(
            ContextKind::Extension("a".into()),
            Scope::Extension("a".into()),
        );
        let two = workspace.child(
            ContextKind::Extension("b".into()),
            Scope::Extension("b".into()),
        );
        one.provide("secret", Arc::new(1_u64));

        assert!(one.acquire::<u64>("secret").is_some());
        assert!(two.acquire::<u64>("secret").is_none());
        assert!(two
            .path()
            .iter()
            .all(|scope| !matches!(scope, Scope::Extension(_))
                || scope == &Scope::Extension("b".into())));
    }

    #[tokio::test]
    async fn child_effects_are_not_visible_to_siblings() {
        let parent = Context::root(registry(), Scope::Host);
        let one = parent.child(
            ContextKind::Workspace("one".into()),
            Scope::Workspace("one".into()),
        );
        let two = parent.child(
            ContextKind::Workspace("two".into()),
            Scope::Workspace("two".into()),
        );
        one.provide("clock", Arc::new(1_u64));
        assert!(one.contains::<u64>("clock"));
        assert!(!two.contains::<u64>("clock"));
    }
}
