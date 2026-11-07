//! Peer-owned tasks over the wire: create, status, cancel and ownership.

use std::sync::Arc;
use std::time::{Duration, Instant};

use capability::{
    CapabilityError, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId,
};
use federation::{
    DelegationId, DelegationRequest, DelegationState, FederatedTaskState, FederationNode,
    InMemoryNetwork, NodeIdentity, TaskCreateRequest, TaskSchedule,
};

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:tasks").expect("a valid workspace id")
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:task"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(5));
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
    network: Arc<InMemoryNetwork>,
    caller: Arc<FederationNode>,
    caller_id: NodeId,
    target: Arc<FederationNode>,
    target_id: NodeId,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn pair() -> Pair {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(51);
    let target_identity = identity(52);
    let caller_id = caller_identity.node_id();
    let target_id = target_identity.node_id();

    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (target_transport, target_inbox) = network.join(target_id.clone());

    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let target = Arc::new(FederationNode::new(
        target_identity,
        Arc::new(target_transport),
    ));
    let tasks = vec![caller.start(caller_inbox), target.start(target_inbox)];

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(target_id.clone(), now, None);
    }
    {
        let store = target.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(caller_id.clone(), now, None);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    Pair {
        network,
        caller,
        caller_id,
        target,
        target_id,
        _tasks: tasks,
    }
}

/// create records the task on the peer with the authenticated origin, status
/// reads it back, and cancel settles it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_is_created_read_and_cancelled_across_the_wire() {
    let pair = pair().await;
    let created = pair
        .caller
        .create_task(&pair.target_id, request("task-1"), &context())
        .await
        .expect("the peer accepts");
    assert_eq!(created.id, "task-1");
    assert_eq!(created.origin, pair.caller_id.to_string());
    assert_eq!(created.state, FederatedTaskState::Queued);
    assert_eq!(created.objective, "Audit the working copy");

    let stored = pair
        .target
        .tasks()
        .read()
        .expect("tasks")
        .get("task-1")
        .cloned()
        .expect("the peer stored it");
    assert_eq!(stored, created, "the caller sees the peer's record");

    let status = pair
        .caller
        .task_status(&pair.target_id, "task-1", &context())
        .await
        .expect("status");
    assert_eq!(status, created);

    let cancelled = pair
        .caller
        .cancel_task(&pair.target_id, "task-1", &context())
        .await
        .expect("cancel");
    assert_eq!(cancelled.state, FederatedTaskState::Cancelled);
}

/// An objective-only delegation is accepted, claimed by the peer, and settled
/// with the result the peer's shell produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_objective_delegation_is_claimed_and_settled() {
    let pair = pair().await;
    let record = pair
        .caller
        .delegate(
            &pair.target_id,
            DelegationRequest {
                id: DelegationId::new("d-obj"),
                parent: None,
                target: pair.target_id.clone(),
                objective: "Audit the working copy".to_string(),
                capabilities: Vec::new(),
                deadline_ms: None,
                allow_sub_delegation: false,
                result_contract: None,
            },
            &context(),
        )
        .await
        .expect("accepted");
    assert_eq!(record.state, DelegationState::Accepted);

    let claimed = pair
        .target
        .claim_objective_delegations(capability::unix_ms());
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].state, DelegationState::Running);
    assert!(pair
        .target
        .claim_objective_delegations(capability::unix_ms())
        .is_empty());

    assert!(pair.target.settle_objective_delegation(
        "d-obj",
        Some(serde_json::json!({ "text": "done" })),
        None
    ));
    let stored = pair
        .target
        .delegations()
        .read()
        .expect("delegations")
        .record(&DelegationId::new("d-obj"))
        .cloned()
        .expect("stored");
    assert_eq!(stored.state, DelegationState::Succeeded);
    assert_eq!(stored.result, Some(serde_json::json!({ "text": "done" })));
}

/// A claim marks the task and persists it; settle writes the result through
/// the same file so a reload sees the outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claims_and_settlements_are_durable() {
    let dir = std::env::temp_dir().join(format!("federation-task-persist-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("tasks.json");
    let network = Arc::new(InMemoryNetwork::new());
    let identity = identity(54);
    let id = identity.node_id();
    let (transport, inbox) = network.join(id);
    let node =
        Arc::new(FederationNode::new(identity, Arc::new(transport)).with_task_persistence(&path));
    let task = node.start(inbox);
    {
        let queue = node.tasks();
        let mut queue = queue.write().expect("queue");
        queue
            .create(
                "task-1",
                "origin",
                "Audit",
                TaskSchedule::At { at_ms: 0 },
                10,
                50_000,
                300_000,
                0,
            )
            .expect("created");
    }
    let due = node.claim_due_tasks(0);
    assert_eq!(due.len(), 1);
    node.settle_task("task-1", true, Some("done".to_string()), None)
        .expect("settled");
    let reloaded = federation::TaskQueue::load(&path).expect("load");
    let stored = reloaded.get("task-1").expect("stored");
    assert_eq!(stored.state, FederatedTaskState::Succeeded);
    assert_eq!(stored.result.as_deref(), Some("done"));
    drop(task);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A node that did not create the task cannot read or cancel it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_node_cannot_read_or_cancel_the_task() {
    let pair = pair().await;
    pair.caller
        .create_task(&pair.target_id, request("task-1"), &context())
        .await
        .expect("accepted");

    let intruder_identity = identity(53);
    let intruder_id = intruder_identity.node_id();
    let (transport, inbox) = pair.network.join(intruder_id.clone());
    let intruder = Arc::new(FederationNode::new(intruder_identity, Arc::new(transport)));
    let task = intruder.start(inbox);
    let now = capability::unix_ms();
    {
        let store = intruder.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(pair.target_id.clone(), now, None);
    }
    {
        let store = pair.target.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(intruder_id.clone(), now, None);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let read = intruder
        .task_status(&pair.target_id, "task-1", &context())
        .await;
    assert!(matches!(read, Err(CapabilityError::Denied { .. })));
    let cancel = intruder
        .cancel_task(&pair.target_id, "task-1", &context())
        .await;
    assert!(matches!(cancel, Err(CapabilityError::Denied { .. })));
    drop(task);
}
