//! A TaskGraph whose tasks run as delegated work on another Node.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::composition::{Binding, Budgets, ResolvedCapability, ResolvedComposition};
use capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind,
    CapabilityOutput, CapabilityProvider, InvocationContext, InvocationId, Principal,
    ProviderHealth, ProviderId, ProviderKind, VersionSource, WorkspaceId, WorkspaceRuntime,
};
use federation::{
    DelegationExecutor, InMemoryNetwork, NodeIdentity, Orchestrator, TaskGraph, TaskNodeId,
    TaskState,
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
        Ok(Vec::new())
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

fn runtime(provider: Arc<dyn CapabilityProvider>, workspace: &str) -> Arc<WorkspaceRuntime> {
    let plan = ResolvedComposition {
        id: "compose:executor".into(),
        version: Version::new(1, 0, 0),
        workspace: WorkspaceId::new(workspace).expect("workspace"),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor(),
            binding: Binding::Local {
                provider: ProviderId::builtin(),
            },
        }],
        skipped: Vec::new(),
        budgets: Budgets::default(),
    };
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("runtime"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_graph_executes_through_a_delegation() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = NodeIdentity::from_seed([101; 32]);
    let provider_identity = NodeIdentity::from_seed([102; 32]);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let caller = Arc::new(federation::FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider = Arc::new(
        federation::FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(runtime(Arc::new(Echo), "ws:executor")),
    );
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);
    let now = capability::unix_ms();
    let capability: CapabilityId = "builtin/echo".parse().expect("capability");
    caller
        .peers()
        .write()
        .expect("peers")
        .enroll(provider_id.clone(), now, None);
    caller
        .peers()
        .write()
        .expect("peers")
        .grant(&provider_id, capability.clone(), now, None)
        .expect("grant");
    provider
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), now, None);
    provider
        .peers()
        .write()
        .expect("peers")
        .grant(&caller_id, capability.clone(), now, None)
        .expect("grant");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut context = InvocationContext::local(
        InvocationId::new("inv:executor"),
        Principal::Local {
            session: "s".to_string(),
            preset: "p".to_string(),
        },
        WorkspaceId::new("ws:executor").expect("workspace"),
    );
    context.deadline = Some(std::time::Instant::now() + Duration::from_secs(10));
    let executor =
        DelegationExecutor::new(Arc::clone(&caller), provider_id, vec![capability], context)
            .with_poll(Duration::from_millis(10))
            .with_contract(
                json!({ "capability": "builtin/echo", "input": { "value": 7 } }).to_string(),
            );
    let mut graph = TaskGraph::new();
    graph
        .add(TaskNodeId::new("t1"), "echo one value", Vec::new())
        .expect("task");
    let mut orchestrator = Orchestrator::new(graph);

    let progress = orchestrator
        .run(Arc::new(executor), 1)
        .await
        .expect("the graph runs");

    assert_eq!(progress.succeeded, 1);
    let task = &orchestrator.graph().tasks[&TaskNodeId::new("t1")];
    assert_eq!(task.state, TaskState::Succeeded);
    assert_eq!(
        task.result
            .as_ref()
            .and_then(|value| value.get("structured")),
        Some(&json!({ "value": 7 }))
    );

    caller_task.abort();
    provider_task.abort();
}
