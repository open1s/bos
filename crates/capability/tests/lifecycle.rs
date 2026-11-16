//! Transactional workspace activation, drain, suspension, and teardown.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::composition::{Binding, Budgets, ResolvedCapability, ResolvedComposition};
use capability::descriptor::{CapabilityDescriptor, OutputContract};
use capability::id::{CapabilityId, CapabilityKind, ProviderId, VersionSource, WorkspaceId};
use capability::provider::{
    CapabilityCall, CapabilityOutput, CapabilityProvider, ProviderHealth, ProviderKind,
};
use capability::{
    CapabilityError, InvocationContext, InvocationId, Principal, WorkspaceError,
    WorkspaceLifecycle, WorkspaceState,
};
use semver::Version;
use serde_json::json;

struct LifecycleProvider {
    id: ProviderId,
    label: String,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    fail_activate: bool,
    fail_deactivate: bool,
    gate: Option<Arc<tokio::sync::Notify>>,
}

#[async_trait]
impl CapabilityProvider for LifecycleProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn activate(&self) -> Result<(), CapabilityError> {
        self.events
            .lock()
            .expect("events")
            .push(format!("activate:{}", self.label));
        if self.fail_activate {
            return Err(CapabilityError::ProviderFailed {
                message: format!("{} cannot activate", self.label),
            });
        }
        Ok(())
    }

    async fn deactivate(&self) -> Result<(), CapabilityError> {
        self.events
            .lock()
            .expect("events")
            .push(format!("deactivate:{}", self.label));
        if self.fail_deactivate {
            return Err(CapabilityError::ProviderFailed {
                message: format!("{} cannot deactivate", self.label),
            });
        }
        Ok(())
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.events
            .lock()
            .expect("events")
            .push(format!("invoke:{}", self.label));
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        Ok(CapabilityOutput::text(&self.label))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn descriptor(id: &str) -> CapabilityDescriptor {
    let id: CapabilityId = id.parse().expect("a capability id");
    CapabilityDescriptor::new(
        id,
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_output(OutputContract::json())
}

fn plan(entries: &[(&str, &str)]) -> ResolvedComposition {
    ResolvedComposition {
        id: "compose:lifecycle".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new("ws:lifecycle").expect("a workspace"),
        capabilities: entries
            .iter()
            .map(|(capability, provider)| ResolvedCapability {
                descriptor: descriptor(capability),
                binding: Binding::Local {
                    provider: ProviderId::new(*provider),
                },
                decision: None,
            })
            .collect(),
        skipped: Vec::new(),
        budgets: Budgets::default(),
    }
}

fn context() -> InvocationContext {
    InvocationContext::local(
        InvocationId::new("inv:lifecycle"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        WorkspaceId::new("ws:lifecycle").expect("a workspace"),
    )
}

fn provider(
    label: &str,
    events: &Arc<std::sync::Mutex<Vec<String>>>,
    fail_activate: bool,
    fail_deactivate: bool,
    gate: Option<Arc<tokio::sync::Notify>>,
) -> Arc<LifecycleProvider> {
    Arc::new(LifecycleProvider {
        id: ProviderId::new(format!("p:{label}")),
        label: label.to_string(),
        events: Arc::clone(events),
        fail_activate,
        fail_deactivate,
        gate,
    })
}

fn recorded(events: &Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
    events.lock().expect("events").clone()
}

async fn wait_for(events: &Arc<std::sync::Mutex<Vec<String>>>, needle: &str) {
    for _ in 0..200 {
        if events
            .lock()
            .expect("events")
            .iter()
            .any(|event| event == needle)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("did not observe {needle}: {:?}", recorded(events));
}

#[tokio::test]
async fn partial_activation_rolls_back_in_reverse_order() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let a = provider("a", &events, false, false, None);
    let b = provider("b", &events, false, false, None);
    let c = provider("c", &events, true, false, None);
    let mut lifecycle = WorkspaceLifecycle::with_order(
        plan(&[
            ("builtin/a", "p:a"),
            ("builtin/b", "p:b"),
            ("builtin/c", "p:c"),
        ]),
        &[
            a as Arc<dyn CapabilityProvider>,
            b as Arc<dyn CapabilityProvider>,
            c as Arc<dyn CapabilityProvider>,
        ],
        &[
            "builtin/a".parse().unwrap(),
            "builtin/b".parse().unwrap(),
            "builtin/c".parse().unwrap(),
        ],
    )
    .expect("a lifecycle");

    let error = lifecycle.activate().await.expect_err("c fails");

    assert!(matches!(error, WorkspaceError::Activation { .. }));
    assert_eq!(lifecycle.state(), WorkspaceState::Failed);
    assert_eq!(
        recorded(&events),
        vec![
            "activate:a",
            "activate:b",
            "activate:c",
            "deactivate:b",
            "deactivate:a",
        ]
    );
}

#[tokio::test]
async fn a_rollback_failure_is_reported() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let a = provider("a", &events, false, true, None);
    let b = provider("b", &events, true, false, None);
    let mut lifecycle = WorkspaceLifecycle::with_order(
        plan(&[("builtin/a", "p:a"), ("builtin/b", "p:b")]),
        &[
            a as Arc<dyn CapabilityProvider>,
            b as Arc<dyn CapabilityProvider>,
        ],
        &["builtin/a".parse().unwrap(), "builtin/b".parse().unwrap()],
    )
    .expect("a lifecycle");

    let error = lifecycle.activate().await.expect_err("b fails");
    match error {
        WorkspaceError::Activation {
            rollback_failures, ..
        } => {
            assert_eq!(rollback_failures.len(), 1);
            assert!(rollback_failures[0].contains("a"), "{rollback_failures:?}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(lifecycle.state(), WorkspaceState::Failed);
}

#[tokio::test]
async fn an_activation_order_must_cover_every_binding() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let a = provider("a", &events, false, false, None);
    let error = WorkspaceLifecycle::with_order(
        plan(&[("builtin/a", "p:a")]),
        &[a as Arc<dyn CapabilityProvider>],
        &[],
    )
    .expect_err("the order is incomplete");
    assert!(matches!(error, WorkspaceError::InvalidOrder { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_closes_admission_and_waits_for_in_flight_calls() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let gate = Arc::new(tokio::sync::Notify::new());
    let read = provider("read", &events, false, false, Some(Arc::clone(&gate)));
    let mut lifecycle = WorkspaceLifecycle::new(
        plan(&[("builtin/read_file", "p:read")]),
        &[read as Arc<dyn CapabilityProvider>],
    )
    .expect("a lifecycle");
    lifecycle.activate().await.expect("active");
    let lifecycle = Arc::new(lifecycle);
    let id: CapabilityId = "builtin/read_file".parse().expect("an id");

    let call = {
        let lifecycle = Arc::clone(&lifecycle);
        let id = id.clone();
        tokio::spawn(async move { lifecycle.invoke(&id, json!({}), &context()).await })
    };
    wait_for(&events, "invoke:read").await;

    let shutdown = {
        let lifecycle = Arc::clone(&lifecycle);
        tokio::spawn(async move { lifecycle.shutdown(Duration::from_secs(1)).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(lifecycle.state(), WorkspaceState::Draining);
    let denied = lifecycle
        .invoke(&id, json!({}), &context())
        .await
        .expect_err("admission is closed");
    assert!(matches!(denied, CapabilityError::Denied { .. }));

    gate.notify_one();
    call.await.expect("joined").expect("the call completes");
    shutdown.await.expect("joined").expect("drained");
    assert_eq!(lifecycle.state(), WorkspaceState::Stopped);
    assert!(recorded(&events).contains(&"deactivate:read".to_string()));
}

#[tokio::test]
async fn shutdown_is_idempotent_and_releases_resources_once() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let read = provider("read", &events, false, false, None);
    let mut lifecycle = WorkspaceLifecycle::new(
        plan(&[("builtin/read_file", "p:read")]),
        &[read as Arc<dyn CapabilityProvider>],
    )
    .expect("a lifecycle");
    lifecycle.activate().await.expect("active");
    lifecycle.own_resource("file:/tmp/a").expect("owned");
    assert!(matches!(
        lifecycle.own_resource("file:/tmp/a"),
        Err(WorkspaceError::DuplicateResource { .. })
    ));

    lifecycle
        .shutdown(Duration::from_millis(100))
        .await
        .expect("stopped");
    assert_eq!(lifecycle.state(), WorkspaceState::Stopped);
    assert!(lifecycle.resources().is_empty());
    lifecycle
        .shutdown(Duration::from_millis(100))
        .await
        .expect("idempotent");
    assert_eq!(
        recorded(&events)
            .iter()
            .filter(|event| *event == "deactivate:read")
            .count(),
        1,
        "deactivation happens exactly once"
    );
}

#[tokio::test]
async fn suspend_closes_admission_and_resume_reopens_it() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let read = provider("read", &events, false, false, None);
    let mut lifecycle = WorkspaceLifecycle::new(
        plan(&[("builtin/read_file", "p:read")]),
        &[read as Arc<dyn CapabilityProvider>],
    )
    .expect("a lifecycle");
    let id: CapabilityId = "builtin/read_file".parse().expect("an id");
    lifecycle.activate().await.expect("active");

    lifecycle
        .suspend(Duration::from_millis(100))
        .await
        .expect("suspended");
    assert_eq!(lifecycle.state(), WorkspaceState::Suspended);
    assert!(lifecycle.invoke(&id, json!({}), &context()).await.is_err());

    lifecycle.resume().expect("resumed");
    assert_eq!(lifecycle.state(), WorkspaceState::Active);
    lifecycle
        .invoke(&id, json!({}), &context())
        .await
        .expect("calls flow again");
    lifecycle
        .shutdown(Duration::from_millis(100))
        .await
        .expect("stopped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_drain_timeout_can_be_retried() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let gate = Arc::new(tokio::sync::Notify::new());
    let read = provider("read", &events, false, false, Some(Arc::clone(&gate)));
    let mut lifecycle = WorkspaceLifecycle::new(
        plan(&[("builtin/read_file", "p:read")]),
        &[read as Arc<dyn CapabilityProvider>],
    )
    .expect("a lifecycle");
    lifecycle.activate().await.expect("active");
    let lifecycle = Arc::new(lifecycle);
    let id: CapabilityId = "builtin/read_file".parse().expect("an id");

    let call = {
        let lifecycle = Arc::clone(&lifecycle);
        let id = id.clone();
        tokio::spawn(async move { lifecycle.invoke(&id, json!({}), &context()).await })
    };
    wait_for(&events, "invoke:read").await;

    let error = lifecycle
        .shutdown(Duration::from_millis(20))
        .await
        .expect_err("the call is still running");
    assert!(matches!(error, WorkspaceError::DrainTimeout { .. }));
    assert_eq!(lifecycle.state(), WorkspaceState::Draining);

    gate.notify_one();
    call.await.expect("joined").expect("the call completes");
    lifecycle
        .shutdown(Duration::from_secs(1))
        .await
        .expect("the retry drains");
    assert_eq!(lifecycle.state(), WorkspaceState::Stopped);
}
