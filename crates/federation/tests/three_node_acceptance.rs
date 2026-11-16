//! Three-node federation acceptance: caller -> coordinator -> worker.
//!
//! A delegates an objective to B; B selects C, delegates the concrete
//! capability there, waits for the result, and settles A's parent. The three
//! nodes use real Zenoh TCP sessions, so this covers routing, trust, grants and
//! two hops of result propagation.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bus::{Bus, BusConfig};
use capability::composition::{Binding, Budgets, ResolvedCapability, ResolvedComposition};
use capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind,
    CapabilityOutput, CapabilityProvider, InvocationContext, InvocationId, Principal,
    ProviderHealth, ProviderId, ProviderKind, VersionSource, WorkspaceId, WorkspaceRuntime,
};
use federation::{
    DelegationId, DelegationRequest, DelegationState, FederationNode, NodeIdentity, ZenohTransport,
};
use semver::Version;
use serde_json::json;

struct Echo;

#[async_trait]
impl CapabilityProvider for Echo {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![descriptor()])
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        Ok(CapabilityOutput::json(call.input))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn descriptor() -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        "builtin/echo".parse().expect("id"),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
}

fn runtime() -> Arc<WorkspaceRuntime> {
    let plan = ResolvedComposition {
        id: "compose:three-node".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new("ws:three").expect("workspace"),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor(),
            binding: Binding::Local {
                provider: ProviderId::builtin(),
            },
            decision: None,
        }],
        skipped: Vec::new(),
        budgets: Budgets::default(),
    };
    Arc::new(WorkspaceRuntime::instantiate(plan, &[Arc::new(Echo)]).expect("runtime"))
}

fn context(id: &str) -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new(id),
        Principal::Local {
            session: "three".to_string(),
            preset: "three".to_string(),
        },
        WorkspaceId::new("ws:three").expect("workspace"),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(20));
    context
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("probe")
        .local_addr()
        .expect("address")
        .port()
}

fn config(listen: Option<u16>, connect: Option<u16>) -> BusConfig {
    BusConfig {
        mode: "peer".to_string(),
        connect: connect.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        listen: listen.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        peer: None,
        tls: None,
        scouting: false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_caller_coordinator_and_worker_complete_one_task() {
    let port = free_port();
    let coordinator_bus = Bus::from(config(Some(port), None)).await;
    let caller_bus = Bus::from(config(None, Some(port))).await;
    let worker_bus = Bus::from(config(None, Some(port))).await;

    let caller_identity = NodeIdentity::from_seed([181; 32]);
    let coordinator_identity = NodeIdentity::from_seed([182; 32]);
    let worker_identity = NodeIdentity::from_seed([183; 32]);
    let caller_id = caller_identity.node_id();
    let coordinator_id = coordinator_identity.node_id();
    let worker_id = worker_identity.node_id();

    let caller_transport = ZenohTransport::new(caller_bus.session(), caller_id.clone());
    let coordinator_transport =
        ZenohTransport::new(coordinator_bus.session(), coordinator_id.clone());
    let worker_transport = ZenohTransport::new(worker_bus.session(), worker_id.clone());
    let caller_inbox = caller_transport
        .subscribe()
        .await
        .expect("caller inbox")
        .into_receiver();
    let coordinator_inbox = coordinator_transport
        .subscribe()
        .await
        .expect("coordinator inbox")
        .into_receiver();
    let worker_inbox = worker_transport
        .subscribe()
        .await
        .expect("worker inbox")
        .into_receiver();

    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let coordinator = Arc::new(FederationNode::new(
        coordinator_identity,
        Arc::new(coordinator_transport),
    ));
    let worker = Arc::new(
        FederationNode::new(worker_identity, Arc::new(worker_transport)).with_runtime(runtime()),
    );
    let caller_task = caller.start(caller_inbox);
    let coordinator_task = coordinator.start(coordinator_inbox);
    let worker_task = worker.start(worker_inbox);

    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let now = capability::unix_ms();
    let capability: CapabilityId = "builtin/echo".parse().expect("capability");
    // A trusts B; B trusts A and C; C trusts B.
    caller
        .peers()
        .write()
        .expect("peers")
        .enroll(coordinator_id.clone(), now, None);
    coordinator
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), now, None);
    coordinator
        .peers()
        .write()
        .expect("peers")
        .enroll(worker_id.clone(), now, None);
    coordinator
        .peers()
        .write()
        .expect("peers")
        .grant(&worker_id, capability.clone(), now, None)
        .expect("B grants C");
    worker
        .peers()
        .write()
        .expect("peers")
        .enroll(coordinator_id.clone(), now, None);
    worker
        .peers()
        .write()
        .expect("peers")
        .grant(&coordinator_id, capability.clone(), now, None)
        .expect("C grants B");

    // A asks B for an objective; B is free to choose a worker.
    let parent = DelegationId::new("three-parent");
    let accepted = caller
        .delegate(
            &coordinator_id,
            DelegationRequest {
                id: parent.clone(),
                parent: None,
                target: coordinator_id.clone(),
                objective: "echo a value through the best worker".to_string(),
                capabilities: Vec::new(),
                operations: Vec::new(),
                resources: Vec::new(),
                deadline_ms: Some(capability::unix_ms() + 20_000),
                allow_sub_delegation: true,
                result_contract: None,
                trace_id: None,
                span_id: None,
            },
            &context("three:parent"),
        )
        .await
        .expect("B accepts");
    assert!(accepted.state.is_active());
    let claimed = coordinator.claim_objective_delegations(capability::unix_ms());
    assert_eq!(claimed.len(), 1, "B claims A's objective");
    assert_eq!(claimed[0].state, DelegationState::Running);

    // B delegates the concrete capability to C.
    let child = DelegationId::new("three-child");
    let child_context = context("three:child");
    let mut child_record = coordinator
        .delegate(
            &worker_id,
            DelegationRequest {
                id: child.clone(),
                parent: None,
                target: worker_id.clone(),
                objective: "echo 7".to_string(),
                capabilities: vec![capability],
                operations: Vec::new(),
                resources: Vec::new(),
                deadline_ms: Some(capability::unix_ms() + 20_000),
                allow_sub_delegation: false,
                result_contract: Some(
                    json!({ "capability": "builtin/echo", "input": { "value": 7 } }).to_string(),
                ),
                trace_id: None,
                span_id: None,
            },
            &child_context,
        )
        .await
        .expect("C accepts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !child_record.state.is_terminal() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        child_record = coordinator
            .delegation_status(&worker_id, &child, &child_context)
            .await
            .expect("child status");
    }
    assert_eq!(
        child_record.state,
        DelegationState::Succeeded,
        "{child_record:?}"
    );
    let child_result = child_record.result.clone().expect("child result");

    // B settles A's parent with the worker result.
    assert!(coordinator.settle_objective_delegation(
        parent.as_str(),
        Some(child_result.clone()),
        None
    ));

    let mut parent_record = accepted;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !parent_record.state.is_terminal() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        parent_record = caller
            .delegation_status(&coordinator_id, &parent, &context("three:status"))
            .await
            .expect("parent status");
    }
    assert_eq!(parent_record.state, DelegationState::Succeeded);
    assert_eq!(parent_record.result, Some(child_result));
    assert_eq!(
        parent_record
            .result
            .as_ref()
            .and_then(|value| value.get("structured")),
        Some(&json!({ "value": 7 }))
    );

    caller_task.abort();
    coordinator_task.abort();
    worker_task.abort();
}
