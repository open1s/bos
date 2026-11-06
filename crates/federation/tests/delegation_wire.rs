//! Delegation over the wire: submit, status, cancel, and attenuation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use capability::{
    CapabilityError, CapabilityId, InvocationContext, InvocationId, NodeId, Principal, WorkspaceId,
};
use federation::{
    DelegationId, DelegationRequest, DelegationState, FederationNode, InMemoryNetwork, NodeIdentity,
};
use serde_json::json;

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:delegation").expect("a valid workspace id")
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:delegation"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(5));
    context
}

fn request(
    id: &str,
    target: &NodeId,
    capabilities: Vec<CapabilityId>,
    allow_sub: bool,
) -> DelegationRequest {
    DelegationRequest {
        id: DelegationId::new(id),
        parent: None,
        target: target.clone(),
        objective: "verify the change".to_string(),
        capabilities,
        deadline_ms: None,
        allow_sub_delegation: allow_sub,
        result_contract: None,
    }
}

struct Pair {
    caller: Arc<FederationNode>,
    target: Arc<FederationNode>,
    target_id: NodeId,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn pair(grant_scope: bool) -> Pair {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(41);
    let target_identity = identity(42);
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
        store
            .grant(&target_id, capability("read_file"), now, None)
            .unwrap();
    }
    {
        let store = target.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(caller_id.clone(), now, None);
        if grant_scope {
            store
                .grant(&caller_id, capability("read_file"), now, None)
                .unwrap();
        }
    }

    tokio::time::sleep(Duration::from_millis(20)).await;

    Pair {
        caller,
        target,
        target_id,
        _tasks: tasks,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegation_is_accepted_and_stored_by_the_target() {
    let pair = pair(true).await;

    let record = pair
        .caller
        .delegate(
            &pair.target_id,
            request("d-1", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect("the target accepts");

    assert_eq!(record.state, DelegationState::Accepted);
    assert_eq!(record.request.objective, "verify the change");
    let stored = pair
        .target
        .delegations()
        .read()
        .expect("delegations")
        .record(&DelegationId::new("d-1"))
        .cloned()
        .expect("the target stored it");
    assert_eq!(stored.state, DelegationState::Accepted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_the_target_did_not_grant_is_denied() {
    let pair = pair(false).await;

    let error = pair
        .caller
        .delegate(
            &pair.target_id,
            request("d-2", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect_err("the target holds no grant for that capability");

    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("no valid grant")),
        "{error:?}"
    );
    assert!(pair
        .target
        .delegations()
        .read()
        .expect("delegations")
        .record(&DelegationId::new("d-2"))
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_the_authoritative_state() {
    let pair = pair(true).await;
    let id = DelegationId::new("d-3");
    pair.caller
        .delegate(
            &pair.target_id,
            request("d-3", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect("accepted");
    pair.target
        .delegations()
        .write()
        .expect("delegations")
        .transition(&id, DelegationState::Running, capability::unix_ms())
        .expect("running");

    let record = pair
        .caller
        .delegation_status(&pair.target_id, &id, &context())
        .await
        .expect("the owner reports");

    assert_eq!(record.state, DelegationState::Running);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_is_idempotent_and_cannot_overwrite_a_result() {
    let pair = pair(true).await;
    let id = DelegationId::new("d-4");
    pair.caller
        .delegate(
            &pair.target_id,
            request("d-4", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect("accepted");
    pair.target
        .delegations()
        .write()
        .expect("delegations")
        .transition(&id, DelegationState::Running, capability::unix_ms())
        .expect("running");

    let first = pair
        .caller
        .cancel_delegation(&pair.target_id, &id, &context())
        .await
        .expect("cancelled");
    assert_eq!(first.state, DelegationState::Cancelled);

    let second = pair
        .caller
        .cancel_delegation(&pair.target_id, &id, &context())
        .await
        .expect("a late cancel is not an error");
    assert_eq!(second.state, DelegationState::Cancelled);

    // A result recorded first must survive a late cancel.
    let other = DelegationId::new("d-5");
    pair.caller
        .delegate(
            &pair.target_id,
            request("d-5", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect("accepted");
    pair.target
        .delegations()
        .write()
        .expect("delegations")
        .transition(&other, DelegationState::Running, capability::unix_ms())
        .expect("running");
    pair.target
        .delegations()
        .write()
        .expect("delegations")
        .succeed(&other, json!({ "ok": true }), capability::unix_ms())
        .expect("succeeded");

    let late = pair
        .caller
        .cancel_delegation(&pair.target_id, &other, &context())
        .await
        .expect("the owner reports the terminal state");
    assert_eq!(late.state, DelegationState::Succeeded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_delegation_cannot_exceed_the_parent() {
    let pair = pair(true).await;
    let parent = pair
        .caller
        .delegate(
            &pair.target_id,
            request("d-6", &pair.target_id, vec![capability("read_file")], false),
            &context(),
        )
        .await
        .expect("accepted");

    let mut child = request(
        "d-6-child",
        &pair.target_id,
        vec![capability("read_file")],
        false,
    );
    child.parent = Some(parent.request.id.clone());
    let error = pair
        .caller
        .delegate(&pair.target_id, child, &context())
        .await
        .expect_err("the parent forbids sub-delegation");

    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("sub-delegation")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_delegation_is_not_found() {
    let pair = pair(true).await;

    let error = pair
        .caller
        .delegation_status(&pair.target_id, &DelegationId::new("missing"), &context())
        .await
        .expect_err("nothing was created");

    assert!(
        matches!(error, CapabilityError::DelegationNotFound { .. }),
        "{error:?}"
    );
}

/// The target's accepted delegation outlives its process: a restart reloads the
/// store from the data directory and still answers status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delegation_survives_the_target_restarting() {
    let root = std::env::temp_dir().join(format!("bsh-delegation-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    let path = root.join("delegations.json");

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
    let target = Arc::new(
        FederationNode::new(target_identity.clone(), Arc::new(target_transport))
            .with_delegation_persistence(&path),
    );
    let caller_task = caller.start(caller_inbox);
    let target_task = target.start(target_inbox);

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(target_id.clone(), now, None);
        peers
            .grant(&target_id, capability("read_file"), now, None)
            .expect("granted");
    }
    {
        let store = target.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("read_file"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let record = caller
        .delegate(
            &target_id,
            request(
                "d-restart",
                &target_id,
                vec![capability("read_file")],
                false,
            ),
            &context(),
        )
        .await
        .expect("accepted");
    assert_eq!(record.state, DelegationState::Accepted);

    // The serve loop persists after the handler returns.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(path.is_file(), "the store was written");
    target_task.abort();
    drop(target);

    // A new process, same identity, same data directory.
    let (target_transport, target_inbox) = network.join(target_id.clone());
    let target = Arc::new(
        FederationNode::new(target_identity, Arc::new(target_transport))
            .with_delegation_persistence(&path),
    );
    let target_task = target.start(target_inbox);
    target
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), capability::unix_ms(), None);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let status = caller
        .delegation_status(&target_id, &DelegationId::new("d-restart"), &context())
        .await
        .expect("the restarted target answers");
    assert_eq!(status.state, DelegationState::Accepted);
    assert_eq!(status.request.objective, "verify the change");
    assert_eq!(
        status.origin,
        Some(caller_id),
        "ownership is part of the persisted record"
    );

    caller_task.abort();
    target_task.abort();
    let _ = std::fs::remove_dir_all(&root);
}

/// A delegation belongs to the peer that submitted it: another trusted peer
/// cannot read it or cancel it by knowing the id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_peer_cannot_read_or_cancel_a_delegation() {
    let network = Arc::new(InMemoryNetwork::new());
    let owner_identity = identity(71);
    let stranger_identity = identity(72);
    let target_identity = identity(73);
    let owner_id = owner_identity.node_id();
    let stranger_id = stranger_identity.node_id();
    let target_id = target_identity.node_id();

    let (owner_transport, owner_inbox) = network.join(owner_id.clone());
    let (stranger_transport, stranger_inbox) = network.join(stranger_id.clone());
    let (target_transport, target_inbox) = network.join(target_id.clone());
    let owner = Arc::new(FederationNode::new(
        owner_identity,
        Arc::new(owner_transport),
    ));
    let stranger = Arc::new(FederationNode::new(
        stranger_identity,
        Arc::new(stranger_transport),
    ));
    let target = Arc::new(FederationNode::new(
        target_identity,
        Arc::new(target_transport),
    ));
    let tasks = vec![
        owner.start(owner_inbox),
        stranger.start(stranger_inbox),
        target.start(target_inbox),
    ];

    let now = capability::unix_ms();
    {
        let store = target.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(owner_id.clone(), now, None);
        peers.enroll(stranger_id.clone(), now, None);
        peers
            .grant(&owner_id, capability("echo"), now, None)
            .expect("granted");
        peers
            .grant(&stranger_id, capability("echo"), now, None)
            .expect("granted");
    }
    for caller in [&owner, &stranger] {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(target_id.clone(), now, None);
        peers
            .grant(&target_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let record = owner
        .delegate(
            &target_id,
            request("d-owned", &target_id, vec![capability("echo")], false),
            &context(),
        )
        .await
        .expect("the target accepts");
    assert_eq!(record.origin, Some(owner_id.clone()));
    assert_eq!(record.state, DelegationState::Accepted);

    let status = stranger
        .delegation_status(&target_id, &DelegationId::new("d-owned"), &context())
        .await;
    assert!(
        matches!(status, Err(CapabilityError::Denied { .. })),
        "a stranger cannot read it: {status:?}"
    );

    let cancel = stranger
        .cancel_delegation(&target_id, &DelegationId::new("d-owned"), &context())
        .await;
    assert!(
        matches!(cancel, Err(CapabilityError::Denied { .. })),
        "a stranger cannot cancel it: {cancel:?}"
    );
    let still = owner
        .delegation_status(&target_id, &DelegationId::new("d-owned"), &context())
        .await
        .expect("the owner still reads it");
    assert_eq!(still.state, DelegationState::Accepted);

    let cancelled = owner
        .cancel_delegation(&target_id, &DelegationId::new("d-owned"), &context())
        .await
        .expect("the owner cancels");
    assert_eq!(cancelled.state, DelegationState::Cancelled);

    // Revoking the owner on the target ends the relationship, ownership or not.
    target.peers().write().expect("peers").revoke(&owner_id);
    let after_revoke = owner
        .delegation_status(&target_id, &DelegationId::new("d-owned"), &context())
        .await;
    assert!(
        matches!(after_revoke, Err(CapabilityError::Denied { .. })),
        "a revoked owner is refused: {after_revoke:?}"
    );

    for task in tasks {
        task.abort();
    }
}
