//! The acceptance matrix: the federation's promises, exercised on more than
//! one node at a time.
//!
//! Rows:
//! - three nodes publish, resolve a remote provider, and invoke through it
//!   (federated composition);
//! - a node restart keeps its queued tasks (restart-safe governance);
//! - an unreachable peer fails closed instead of hanging (fault containment).
//!
//! The real-transport case lives in `task_tcp_e2e.rs`; these run on the
//! deterministic in-memory wire.

use std::sync::Arc;
use std::time::{Duration, Instant};

use capability::catalogue::Catalogue;
use capability::composition::{
    resolve_host, Binding, CapabilityRequirement, CompositionSpec, GrantRef, LocalScope,
    PeerCatalogue, PeerRef, RemotePolicy,
};
use capability::{
    CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilityOutput, InvocationContext,
    InvocationId, NodeId, Principal, VersionSource, WorkspaceId,
};
use federation::{
    DiscoveryRecord, FederationNode, InMemoryNetwork, NodeIdentity, RemoteCapabilityProvider,
    TaskSchedule,
};
use semver::{Version, VersionReq};
use serde_json::json;

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a capability")
}

fn descriptor(name: &str) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    );
    descriptor.visibility = capability::Visibility::Public;
    descriptor
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:acceptance").expect("a workspace")
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:acceptance"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(5));
    context
}

fn echo_runtime(descriptor: CapabilityDescriptor) -> Arc<capability::WorkspaceRuntime> {
    use capability::composition::{Binding as B, Budgets, ResolvedCapability, ResolvedComposition};
    let plan = ResolvedComposition {
        id: "compose:acceptance".into(),
        version: Version::new(1, 0, 0),
        workspace: workspace(),
        capabilities: vec![ResolvedCapability {
            descriptor: descriptor.clone(),
            binding: B::Local {
                provider: capability::ProviderId::builtin(),
            },
            decision: None,
        }],
        skipped: Vec::new(),
        budgets: Budgets::default(),
    };
    let provider: Arc<dyn capability::CapabilityProvider> = Arc::new(Echo {
        provider: capability::ProviderId::builtin(),
        descriptor,
    });
    Arc::new(capability::WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

struct Echo {
    provider: capability::ProviderId,
    descriptor: CapabilityDescriptor,
}

#[async_trait::async_trait]
impl capability::CapabilityProvider for Echo {
    fn id(&self) -> &capability::ProviderId {
        &self.provider
    }

    fn kind(&self) -> capability::ProviderKind {
        capability::ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, capability::CapabilityError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn invoke(
        &self,
        call: capability::CapabilityCall<'_>,
    ) -> Result<CapabilityOutput, capability::CapabilityError> {
        Ok(CapabilityOutput::json(call.input))
    }

    async fn health(&self) -> capability::ProviderHealth {
        capability::ProviderHealth::Available
    }
}

struct Node {
    node: Arc<FederationNode>,
    id: NodeId,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn mesh(size: usize, seeds: &[u8]) -> Vec<Node> {
    let network = Arc::new(InMemoryNetwork::new());
    let identities: Vec<NodeIdentity> = seeds.iter().map(|seed| identity(*seed)).collect();
    let ids: Vec<NodeId> = identities
        .iter()
        .map(|identity| identity.node_id())
        .collect();
    let mut transports = Vec::new();
    let mut inboxes = Vec::new();
    for id in &ids {
        let (transport, inbox) = network.join(id.clone());
        transports.push(transport);
        inboxes.push(inbox);
    }
    let mut nodes = Vec::new();
    for (index, identity) in identities.into_iter().enumerate() {
        let node = Arc::new(
            FederationNode::new(identity, Arc::new(transports[index].clone()))
                .with_runtime(echo_runtime(descriptor("echo")))
                .with_public_capabilities(vec![descriptor("echo")]),
        );
        let task = node.start(inboxes.remove(0));
        nodes.push(Node {
            node,
            id: ids[index].clone(),
            _tasks: vec![task],
        });
    }
    debug_assert_eq!(nodes.len(), size);
    let now = capability::unix_ms();
    for (left_index, left) in nodes.iter().enumerate() {
        for (right_index, right) in ids.iter().enumerate() {
            if left_index == right_index {
                continue;
            }
            let store = left.node.peers();
            let mut store = store.write().expect("peers");
            store.enroll(right.clone(), now, None);
            store
                .grant(right, capability("echo"), now, None)
                .expect("granted");
        }
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    nodes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_nodes_publish_resolve_and_invoke() {
    let nodes = mesh(3, &[71, 72, 73]).await;
    let caller = &nodes[0];
    let peers = &nodes[1..];

    // Discovery advertises the protocol and schema.
    let mut catalogues = Vec::new();
    for peer in peers {
        let record: DiscoveryRecord = caller.node.discover(&peer.id).await.expect("discovered");
        assert!(
            !record.descriptors.is_empty(),
            "the public provider is advertised"
        );
        assert!(!record.protocol.is_empty());
        assert_eq!(record.schema_version, 1);
        assert!(
            record.features.iter().any(|feature| feature == "invoke"),
            "the publisher advertises its features"
        );
        let mut catalogue = Catalogue::new();
        for descriptor in &record.descriptors {
            catalogue.insert(descriptor.clone()).expect("unique");
        }
        catalogues.push(PeerCatalogue::new(peer.id.clone(), catalogue));
    }

    let local = Catalogue::new();
    let scope = LocalScope::admitting(workspace(), &local);
    let spec = CompositionSpec::new(
        "compose:acceptance",
        Version::new(1, 0, 0),
        "resolve echo remotely",
    )
    .with_requirement(
        CapabilityRequirement::new(capability("echo"), VersionReq::parse("^1").expect("range"))
            .with_remote(RemotePolicy::Allowed),
    );
    let mut spec = spec;
    spec.peers = peers
        .iter()
        .map(|peer| {
            PeerRef::new(peer.id.clone()).with_grant(GrantRef::new(format!("g:{}", peer.id)))
        })
        .collect();
    let plan = resolve_host(&spec, &local, &catalogues, &scope).expect("a plan");
    let Binding::Remote { node, .. } = &plan.capabilities[0].binding else {
        panic!("a remote binding");
    };
    assert!(peers.iter().any(|peer| &peer.id == node));

    let provider: Arc<dyn capability::CapabilityProvider> =
        Arc::new(RemoteCapabilityProvider::new(
            node.clone(),
            Arc::clone(&caller.node),
            vec![descriptor("echo")],
        ));
    let runtime = capability::WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime");
    let output = runtime
        .invoke(&capability("echo"), json!({ "matrix": 3 }), &context())
        .await
        .expect("the peer answers");
    assert_eq!(output.structured.unwrap()["matrix"], 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_keeps_the_task_queue() {
    let dir = std::env::temp_dir().join(format!("federation-acceptance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let path = dir.join("tasks.json");

    let network = Arc::new(InMemoryNetwork::new());
    let first_identity = identity(81);
    let id = first_identity.node_id();
    let (transport, inbox) = network.join(id.clone());
    let node = FederationNode::new(first_identity.clone(), Arc::new(transport))
        .with_task_persistence(&path);
    let router = node.start(inbox);
    {
        let queue = node.tasks();
        let mut queue = queue.write().expect("queue");
        queue
            .create(
                "task-restart",
                id.as_str(),
                "Audit the matrix",
                TaskSchedule::At { at_ms: 0 },
                10,
                50_000,
                300_000,
                capability::unix_ms(),
            )
            .expect("queued");
    }
    node.persist_tasks();
    drop(router);
    drop(node);

    // A new process would build a new node from the same identity and paths.
    let network = Arc::new(InMemoryNetwork::new());
    let second_identity = identity(81);
    let id = second_identity.node_id();
    let (transport, inbox) = network.join(id.clone());
    let restarted =
        FederationNode::new(second_identity, Arc::new(transport)).with_task_persistence(&path);
    let _router = restarted.start(inbox);
    let due = restarted.claim_due_tasks(capability::unix_ms());
    assert_eq!(due.len(), 1, "the queued task survived the restart");
    assert_eq!(due[0].id, "task-restart");
    drop(_router);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_peer_fails_closed() {
    let nodes = mesh(1, &[91]).await;
    let caller = &nodes[0];
    let ghost = identity(92).node_id();
    {
        let store = caller.node.peers();
        let mut store = store.write().expect("peers");
        let now = capability::unix_ms();
        store.enroll(ghost.clone(), now, None);
        store
            .grant(&ghost, capability("echo"), now, None)
            .expect("granted");
    }
    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(300));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        caller
            .node
            .invoke(&ghost, &capability("echo"), json!({}), &short),
    )
    .await;
    let error = result
        .expect("the call must not hang")
        .expect_err("unreachable");
    assert!(
        matches!(
            error,
            capability::CapabilityError::Transport { .. }
                | capability::CapabilityError::Timeout { .. }
                | capability::CapabilityError::UnknownOutcome { .. }
        ),
        "{error:?}"
    );
}

async fn wait_event(
    audit: &federation::MemoryAudit,
    predicate: impl Fn(&federation::FederationEvent) -> bool,
) -> federation::FederationEvent {
    for _ in 0..100 {
        if let Some(event) = audit
            .events()
            .iter()
            .find(|event| predicate(event))
            .cloned()
        {
            return event;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the expected audit event did not arrive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegation_correlates_its_trace_across_nodes() {
    use federation::{DelegationId, DelegationRequest};

    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(51);
    let provider_identity = identity(52);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let caller_audit = Arc::new(federation::MemoryAudit::new());
    let provider_audit = Arc::new(federation::MemoryAudit::new());
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let caller = Arc::new(
        FederationNode::new(caller_identity, Arc::new(caller_transport))
            .with_audit(caller_audit.clone()),
    );
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(echo_runtime(descriptor("echo")))
            .with_audit(provider_audit.clone()),
    );
    let mut tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    let now = capability::unix_ms();
    {
        let peers = caller.peers();
        let mut peers = peers.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let peers = provider.peers();
        let mut peers = peers.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let request = DelegationRequest {
        id: DelegationId::new("d-correlate"),
        parent: None,
        target: provider_id.clone(),
        objective: "echo the value".to_string(),
        capabilities: vec![capability("echo")],
        operations: Vec::new(),
        resources: Vec::new(),
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: Some(
            json!({ "capability": "builtin/echo", "input": { "matrix": true } }).to_string(),
        ),
        trace_id: Some("trace-matrix".to_string()),
        span_id: Some("span-caller".to_string()),
    };
    let record = caller
        .delegate(&provider_id, request, &context())
        .await
        .expect("the target accepts");
    let event = wait_event(&provider_audit, |event| event.operation == "delegate").await;
    assert_eq!(event.trace_id.as_deref(), Some("trace-matrix"));
    assert_eq!(event.span_id.as_deref(), Some("span-caller"));
    assert_eq!(
        event.delegation.as_deref(),
        Some(record.request.id.as_str()),
        "the row names the accepted delegation"
    );
    tasks.retain(|task| !task.is_finished());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workspace_scoped_grant_is_enforced_in_the_mesh() {
    use federation::InvocationScope;

    let nodes = mesh(2, &[53, 54]).await;
    let caller = &nodes[0];
    let provider = &nodes[1];
    let now = capability::unix_ms();
    {
        let peers = provider.node.peers();
        let mut peers = peers.write().expect("peers");
        peers
            .grant_scoped_in(
                &caller.id,
                capability("echo"),
                vec!["echo".to_string()],
                Vec::new(),
                vec!["ws:research".to_string()],
                now,
                None,
            )
            .expect("a workspace-scoped grant");
    }
    {
        let peers = caller.node.peers();
        let mut peers = peers.write().expect("peers");
        peers
            .grant_scoped_in(
                &provider.id,
                capability("echo"),
                vec!["echo".to_string()],
                Vec::new(),
                vec!["ws:research".to_string()],
                now,
                None,
            )
            .expect("a workspace-scoped grant");
    }
    let research = capability::WorkspaceId::new("ws:research").expect("a workspace");
    let mut context = context();
    context.workspace = research.clone();
    let scope = InvocationScope {
        operation: Some("echo".to_string()),
        resource: None,
        workspace: Some(research.to_string()),
    };
    caller
        .node
        .invoke_scoped(
            &provider.id,
            &capability("echo"),
            json!({ "matrix": true }),
            Some(&scope),
            &context,
        )
        .await
        .expect("the matching workspace is allowed");
    let wrong = InvocationScope {
        workspace: Some("ws:other".to_string()),
        ..scope
    };
    let error = caller
        .node
        .invoke_scoped(
            &provider.id,
            &capability("echo"),
            json!({ "matrix": true }),
            Some(&wrong),
            &context,
        )
        .await
        .expect_err("another workspace is denied");
    assert!(
        matches!(error, capability::CapabilityError::Denied { .. }),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_a_peer_stops_calls_immediately() {
    let nodes = mesh(2, &[55, 56]).await;
    let caller = &nodes[0];
    let provider = &nodes[1];
    {
        let peers = provider.node.peers();
        let mut peers = peers.write().expect("peers");
        peers.revoke(&caller.id);
    }
    let error = caller
        .node
        .invoke(&provider.id, &capability("echo"), json!({}), &context())
        .await
        .expect_err("a revoked peer is refused");
    assert!(
        matches!(error, capability::CapabilityError::Denied { .. }),
        "{error:?}"
    );
}
