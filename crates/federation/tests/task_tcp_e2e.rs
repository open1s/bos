//! Two real Zenoh sessions on loopback carry a task end to end.
//!
//! The in-memory wire tests prove the protocol; this one proves it over a real
//! transport: create on the caller, claim and settle on the peer, then read the
//! outcome back from the caller.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bus::{Bus, BusConfig};
use capability::{InvocationContext, InvocationId, NodeId, Principal, WorkspaceId};
use federation::{
    DelegationId, DelegationRequest, DelegationState, FederatedTaskState, FederationNode,
    NodeIdentity, TaskCreateRequest, TaskSchedule, ZenohTransport,
};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback listener")
        .local_addr()
        .expect("an address")
        .port()
}

fn peer_config(listen: Option<u16>, connect: Option<u16>) -> BusConfig {
    BusConfig {
        mode: "peer".to_string(),
        connect: connect.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        listen: listen.map(|port| vec![format!("tcp/127.0.0.1:{port}")]),
        peer: None,
        tls: None,
        // Explicit connect only: two tests sharing a scouting network would see
        // each other's announcements and route frames to the wrong node.
        scouting: false,
    }
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:tcp"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        WorkspaceId::new("ws:tcp").expect("a valid workspace id"),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(20));
    context
}

fn request(id: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        id: id.to_string(),
        objective: "Audit the working copy".to_string(),
        schedule: TaskSchedule::At { at_ms: 0 },
        budget_steps: 10,
        budget_tokens: 50_000,
        deadline_ms: 300_000,
    }
}

struct Pair {
    caller: Arc<FederationNode>,
    caller_id: NodeId,
    target: Arc<FederationNode>,
    target_id: NodeId,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
    _buses: Vec<Bus>,
}

async fn pair(seed_a: u8, seed_b: u8) -> Pair {
    let port = free_port();
    let listener = Bus::from(peer_config(Some(port), None)).await;
    let connector = Bus::from(peer_config(None, Some(port))).await;

    let caller_identity = NodeIdentity::from_seed([seed_a; 32]);
    let target_identity = NodeIdentity::from_seed([seed_b; 32]);
    let caller_id = caller_identity.node_id();
    let target_id = target_identity.node_id();

    let caller_transport = ZenohTransport::new(listener.session(), caller_id.clone());
    let target_transport = ZenohTransport::new(connector.session(), target_id.clone());
    let caller_inbox = caller_transport
        .subscribe()
        .await
        .expect("the caller inbox")
        .into_receiver();
    let target_inbox = target_transport
        .subscribe()
        .await
        .expect("the target inbox")
        .into_receiver();

    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let target = Arc::new(FederationNode::new(
        target_identity,
        Arc::new(target_transport),
    ));
    let tasks = vec![caller.start(caller_inbox), target.start(target_inbox)];

    // Zenoh discovery needs a moment before the first frame can route.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let now = capability::unix_ms();
    {
        let peers = caller.peers();
        let mut store = peers.write().expect("peer store");
        store.enroll(target_id.clone(), now, None);
    }
    {
        let peers = target.peers();
        let mut store = peers.write().expect("peer store");
        store.enroll(caller_id.clone(), now, None);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    Pair {
        caller,
        caller_id,
        target,
        target_id,
        _tasks: tasks,
        _buses: vec![listener, connector],
    }
}

/// Create on A, claim and settle on B, read the outcome from A, then cancel a
/// second task over the same live sessions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_crosses_two_real_sessions() {
    let pair = pair(81, 82).await;

    let created = pair
        .caller
        .create_task(&pair.target_id, request("tcp-1"), &context())
        .await
        .expect("the peer accepts");
    assert_eq!(created.id, "tcp-1");
    assert_eq!(created.origin, pair.caller_id.to_string());
    assert_eq!(created.state, FederatedTaskState::Queued);

    let due = pair.target.claim_due_tasks(capability::unix_ms());
    assert_eq!(due.len(), 1, "the peer claims the due task");
    assert_eq!(due[0].id, "tcp-1");
    assert_eq!(due[0].state, FederatedTaskState::Running);
    assert!(
        pair.target
            .claim_due_tasks(capability::unix_ms())
            .is_empty(),
        "a claimed task is not claimed twice"
    );

    pair.target
        .settle_task("tcp-1", true, Some("the peer's answer".to_string()), None)
        .expect("settled");

    let status = pair
        .caller
        .task_status(&pair.target_id, "tcp-1", &context())
        .await
        .expect("status crosses back");
    assert_eq!(status.state, FederatedTaskState::Succeeded);
    assert_eq!(status.result.as_deref(), Some("the peer's answer"));

    pair.caller
        .create_task(&pair.target_id, request("tcp-2"), &context())
        .await
        .expect("the peer accepts");
    let cancelled = pair
        .caller
        .cancel_task(&pair.target_id, "tcp-2", &context())
        .await
        .expect("cancel crosses the wire");
    assert_eq!(cancelled.state, FederatedTaskState::Cancelled);
}

/// An objective delegation crosses the same sessions and settles on the peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_objective_delegation_crosses_two_real_sessions() {
    let pair = pair(83, 84).await;

    let record = pair
        .caller
        .delegate(
            &pair.target_id,
            DelegationRequest {
                id: DelegationId::new("d-tcp"),
                parent: None,
                target: pair.target_id.clone(),
                objective: "Audit the working copy".to_string(),
                capabilities: Vec::new(),
                operations: Vec::new(),
                resources: Vec::new(),
                deadline_ms: None,
                allow_sub_delegation: false,
                result_contract: None,
                trace_id: None,
                span_id: None,
            },
            &context(),
        )
        .await
        .expect("the peer accepts the delegation");
    assert_eq!(record.state, DelegationState::Accepted);

    let claimed = pair
        .target
        .claim_objective_delegations(capability::unix_ms());
    assert_eq!(claimed.len(), 1, "the peer claims the objective");
    assert_eq!(claimed[0].request.id.as_str(), "d-tcp");
    assert_eq!(claimed[0].state, DelegationState::Running);

    assert!(pair.target.settle_objective_delegation(
        "d-tcp",
        Some(serde_json::json!({ "text": "done" })),
        None,
    ));
    let stored = pair
        .target
        .delegations()
        .read()
        .expect("delegations")
        .record(&DelegationId::new("d-tcp"))
        .cloned()
        .expect("stored");
    assert_eq!(stored.state, DelegationState::Succeeded);
    assert_eq!(stored.result, Some(serde_json::json!({ "text": "done" })));
}
