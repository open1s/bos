//! A TaskGraph executes through a real TCP federation session.
//!
//! This is the integration boundary the UI uses: a capability provider on one
//! Node, a DelegationExecutor on another Node, real Zenoh sessions, and the
//! task result returning through the authority graph.

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
    DelegationExecutor, FederationNode, NodeIdentity, Orchestrator, TaskGraph, TaskNodeId,
    TaskState, ZenohTransport,
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
        id: "compose:tcp-executor".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new("ws:tcp-executor").expect("workspace"),
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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback listener")
        .local_addr()
        .expect("an address")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_graph_runs_through_a_real_tcp_delegation() {
    let port = free_port();
    let listener = Bus::from(config(Some(port), None)).await;
    let connector = Bus::from(config(None, Some(port))).await;

    let caller_identity = NodeIdentity::from_seed([151; 32]);
    let provider_identity = NodeIdentity::from_seed([152; 32]);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();

    let caller_transport = ZenohTransport::new(listener.session(), caller_id.clone());
    let provider_transport = ZenohTransport::new(connector.session(), provider_id.clone());
    let caller_inbox = caller_transport
        .subscribe()
        .await
        .expect("caller inbox")
        .into_receiver();
    let provider_inbox = provider_transport
        .subscribe()
        .await
        .expect("provider inbox")
        .into_receiver();

    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(runtime()),
    );
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);

    // Zenoh has to establish the link before the first request.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let now = capability::unix_ms();
    let capability: CapabilityId = "builtin/echo".parse().expect("capability");
    caller
        .peers()
        .write()
        .expect("caller peers")
        .enroll(provider_id.clone(), now, None);
    caller
        .peers()
        .write()
        .expect("caller peers")
        .grant(&provider_id, capability.clone(), now, None)
        .expect("caller grant");
    provider
        .peers()
        .write()
        .expect("provider peers")
        .enroll(caller_id.clone(), now, None);
    provider
        .peers()
        .write()
        .expect("provider peers")
        .grant(&caller_id, capability.clone(), now, None)
        .expect("provider grant");

    let mut context = InvocationContext::local(
        InvocationId::new("inv:tcp-executor"),
        Principal::Local {
            session: "tcp".to_string(),
            preset: "tcp".to_string(),
        },
        WorkspaceId::new("ws:tcp-executor").expect("workspace"),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(20));
    let executor =
        DelegationExecutor::new(Arc::clone(&caller), provider_id, vec![capability], context)
            .with_poll(Duration::from_millis(20))
            .with_contract(
                json!({ "capability": "builtin/echo", "input": { "value": 99 } }).to_string(),
            );

    let mut graph = TaskGraph::new();
    graph
        .add(TaskNodeId::new("tcp-task"), "echo over tcp", Vec::new())
        .expect("task");
    let mut orchestrator = Orchestrator::new(graph);
    let progress = orchestrator
        .run(Arc::new(executor), 1)
        .await
        .expect("the graph runs");

    let task = &orchestrator.graph().tasks[&TaskNodeId::new("tcp-task")];
    assert_eq!(progress.succeeded, 1, "task state: {:?}", task.error);
    assert_eq!(task.state, TaskState::Succeeded);
    assert_eq!(
        task.result
            .as_ref()
            .and_then(|value| value.get("structured")),
        Some(&json!({ "value": 99 }))
    );

    caller_task.abort();
    provider_task.abort();
}
