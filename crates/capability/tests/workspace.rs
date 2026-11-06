//! A workspace dispatches local and remote bindings through one path.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::composition::{Binding, Budgets, ResolvedCapability, ResolvedComposition};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{
    CapabilityId, CapabilityKind, NodeId, ProviderId, VersionSource, WorkspaceId,
};
use capability::provider::{
    CapabilityCall, CapabilityOutput, CapabilityProvider, ProviderHealth, ProviderKind,
};
use capability::{
    CapabilityError, GrantRef, InvocationContext, InvocationId, Principal, WorkspaceError,
    WorkspaceRuntime,
};
use semver::Version;
use serde_json::json;

struct Fake {
    provider: ProviderId,
    kind: ProviderKind,
    label: &'static str,
}

#[async_trait]
impl CapabilityProvider for Fake {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        self.kind.clone()
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        Ok(CapabilityOutput::text(self.label))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn descriptor(id: &str) -> CapabilityDescriptor {
    let id: CapabilityId = id.parse().expect("a valid id");
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_output(OutputContract::json())
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:test").expect("a valid workspace id")
}

fn plan(binding: Binding, id: &str) -> ResolvedComposition {
    ResolvedComposition {
        id: "compose:test".into(),
        version: Version::new(1, 0, 0),
        workspace: workspace(),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor(id),
            binding,
        }],
        skipped: Vec::new(),
        budgets: Budgets::default(),
    }
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:test"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    )
}

#[tokio::test]
async fn a_local_binding_dispatches_to_its_provider() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        label: "local",
    });
    let plan = plan(
        Binding::Local {
            provider: ProviderId::builtin(),
        },
        "builtin/read_file",
    );
    let runtime = WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");

    let output = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the provider answers");

    assert_eq!(output, CapabilityOutput::text("local"));
    assert_eq!(runtime.workspace(), &workspace());
    assert!(runtime.contains(&id));
}

#[tokio::test]
async fn a_remote_binding_dispatches_to_the_peer_provider() {
    let node = NodeId::new("node-b").expect("a valid node id");
    let peer: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: capability::workspace::remote_provider_id(&node),
        kind: ProviderKind::Remote { node: node.clone() },
        label: "remote",
    });
    let plan = plan(
        Binding::Remote {
            node: node.clone(),
            grant: GrantRef::new("grant:1"),
        },
        "builtin/build",
    );
    let runtime = WorkspaceRuntime::instantiate(plan, &[peer]).expect("a runtime");
    let id = "builtin/build".parse().expect("a valid id");

    let output = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the peer provider answers");

    assert_eq!(output, CapabilityOutput::text("remote"));
    assert_eq!(runtime.workspace(), &workspace());
    assert!(matches!(
        runtime.binding(&id),
        Some(Binding::Remote { node: bound, .. }) if bound == &node
    ));
}

#[tokio::test]
async fn a_missing_provider_fails_at_instantiation_not_on_the_first_call() {
    let plan = plan(
        Binding::Local {
            provider: ProviderId::builtin(),
        },
        "builtin/read_file",
    );

    let error = WorkspaceRuntime::instantiate(plan, &[]).expect_err("no provider is registered");

    assert!(matches!(error, WorkspaceError::MissingProvider { .. }));
}

#[tokio::test]
async fn a_capability_outside_the_workspace_is_not_found() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Fake {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        label: "local",
    });
    let runtime = WorkspaceRuntime::instantiate(
        plan(
            Binding::Local {
                provider: ProviderId::builtin(),
            },
            "builtin/read_file",
        ),
        &[provider],
    )
    .expect("a runtime");
    let outside = "builtin/write_file".parse().expect("a valid id");

    let error = runtime
        .invoke(&outside, json!({}), &context())
        .await
        .expect_err("the workspace was not built with it");

    assert!(matches!(error, CapabilityError::NotFound { .. }));
}

struct Counting {
    provider: ProviderId,
    kind: ProviderKind,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl CapabilityProvider for Counting {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        self.kind.clone()
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(CapabilityOutput::text("counted"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn other_context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:other"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        WorkspaceId::new("ws:other").expect("a second workspace"),
    )
}

#[tokio::test]
async fn a_local_binding_refuses_another_workspace_context_before_dispatch() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Counting {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        calls: Arc::clone(&calls),
    });
    let runtime = WorkspaceRuntime::instantiate(
        plan(
            Binding::Local {
                provider: ProviderId::builtin(),
            },
            "builtin/read_file",
        ),
        &[provider],
    )
    .expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");

    let error = runtime
        .invoke(&id, json!({}), &other_context())
        .await
        .expect_err("the context belongs to another workspace");

    assert!(matches!(error, CapabilityError::Denied { .. }));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the owning workspace still works");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_remote_binding_refuses_another_workspace_context_before_dispatch() {
    let node = NodeId::new("node-b").expect("a valid node id");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peer: Arc<dyn CapabilityProvider> = Arc::new(Counting {
        provider: capability::workspace::remote_provider_id(&node),
        kind: ProviderKind::Remote { node: node.clone() },
        calls: Arc::clone(&calls),
    });
    let runtime = WorkspaceRuntime::instantiate(
        plan(
            Binding::Remote {
                node,
                grant: GrantRef::new("grant:1"),
            },
            "builtin/build",
        ),
        &[peer],
    )
    .expect("a runtime");
    let id = "builtin/build".parse().expect("a valid id");

    let error = runtime
        .invoke(&id, json!({}), &other_context())
        .await
        .expect_err("the context belongs to another workspace");

    assert!(matches!(error, CapabilityError::Denied { .. }));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the owning workspace reaches the peer proxy");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

struct Slow(Duration);

#[async_trait]
impl CapabilityProvider for Slow {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        tokio::time::sleep(self.0).await;
        Ok(CapabilityOutput::text("slow"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

struct Gate {
    release: tokio::sync::Notify,
    active: std::sync::atomic::AtomicUsize,
    max: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl CapabilityProvider for Gate {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        use std::sync::atomic::Ordering;
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(active, Ordering::SeqCst);
        self.release.notified().await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CapabilityOutput::text("gated"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

struct FailOnce(std::sync::atomic::AtomicBool);

#[async_trait]
impl CapabilityProvider for FailOnce {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        if !self.0.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err(CapabilityError::ProviderFailed {
                message: "first call fails".to_string(),
            });
        }
        Ok(CapabilityOutput::text("second call"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn plan_with_budgets(budgets: Budgets) -> ResolvedComposition {
    let mut plan = plan(
        Binding::Local {
            provider: ProviderId::builtin(),
        },
        "builtin/read_file",
    );
    plan.budgets = budgets;
    plan
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_effective_default_deadline_bounds_a_slow_provider() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Slow(Duration::from_millis(200)));
    let runtime = WorkspaceRuntime::instantiate(
        plan_with_budgets(Budgets {
            max_concurrent_invocations: 4,
            default_timeout: Some(Duration::from_millis(40)),
        }),
        &[provider],
    )
    .expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");
    let started = Instant::now();

    let error = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect_err("the default deadline wins");

    assert!(
        matches!(error, CapabilityError::Timeout { .. }),
        "{error:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(200), "tightened");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stricter_caller_deadline_is_not_extended_by_the_default() {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Slow(Duration::from_millis(200)));
    let runtime = WorkspaceRuntime::instantiate(
        plan_with_budgets(Budgets {
            max_concurrent_invocations: 4,
            default_timeout: Some(Duration::from_secs(1)),
        }),
        &[provider],
    )
    .expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");
    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(40));
    let started = Instant::now();

    let error = runtime
        .invoke(&id, json!({}), &short)
        .await
        .expect_err("the caller's deadline is tighter");

    assert!(
        matches!(error, CapabilityError::Timeout { .. }),
        "{error:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "not extended"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrency_is_bounded_per_workspace() {
    let provider = Arc::new(Gate {
        release: tokio::sync::Notify::new(),
        active: std::sync::atomic::AtomicUsize::new(0),
        max: std::sync::atomic::AtomicUsize::new(0),
    });
    let runtime = Arc::new(
        WorkspaceRuntime::instantiate(
            plan_with_budgets(Budgets {
                max_concurrent_invocations: 1,
                default_timeout: None,
            }),
            &[provider.clone() as Arc<dyn CapabilityProvider>],
        )
        .expect("a runtime"),
    );
    let id: CapabilityId = "builtin/read_file".parse().expect("a valid id");
    let first = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        tokio::spawn(async move { runtime.invoke(&id, json!({}), &context()).await })
    };
    for _ in 0..100 {
        if provider.active.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let second = {
        let runtime = Arc::clone(&runtime);
        let id = id.clone();
        tokio::spawn(async move { runtime.invoke(&id, json!({}), &context()).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        provider.max.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second call waits for the single permit"
    );

    provider.release.notify_one();
    first.await.expect("joined").expect("first");
    provider.release.notify_one();
    second.await.expect("joined").expect("second");
    assert_eq!(provider.max.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_failed_call_releases_its_permit() {
    let provider = Arc::new(FailOnce(std::sync::atomic::AtomicBool::new(false)));
    let runtime = WorkspaceRuntime::instantiate(
        plan_with_budgets(Budgets {
            max_concurrent_invocations: 1,
            default_timeout: None,
        }),
        &[provider as Arc<dyn CapabilityProvider>],
    )
    .expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");

    runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect_err("the first call fails");
    let output = runtime
        .invoke(&id, json!({}), &context())
        .await
        .expect("the permit was released");
    assert_eq!(output, CapabilityOutput::text("second call"));
}

#[tokio::test]
async fn a_cancelled_context_is_refused_before_dispatch() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Counting {
        provider: ProviderId::builtin(),
        kind: ProviderKind::Builtin,
        calls: Arc::clone(&calls),
    });
    let runtime = WorkspaceRuntime::instantiate(plan_with_budgets(Budgets::default()), &[provider])
        .expect("a runtime");
    let id = "builtin/read_file".parse().expect("a valid id");
    let cancelled = context();
    cancelled.cancel.cancel();

    let error = runtime
        .invoke(&id, json!({}), &cancelled)
        .await
        .expect_err("cancelled");

    assert!(matches!(error, CapabilityError::Cancelled));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
