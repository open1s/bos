//! Delegation state is authoritative, transitions are validated, authority attenuates.

use capability::{CapabilityId, NodeId};
use federation::{
    DelegationError, DelegationId, DelegationRequest, DelegationState, DelegationStore,
};

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a valid node id")
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn request(
    id: &str,
    capabilities: Vec<CapabilityId>,
    allow_sub_delegation: bool,
) -> DelegationRequest {
    DelegationRequest {
        id: DelegationId::new(id),
        parent: None,
        target: node("node:builder"),
        objective: "verify the change".to_string(),
        capabilities,
        deadline_ms: None,
        allow_sub_delegation,
        result_contract: None,
    }
}

#[test]
fn a_delegation_walks_the_happy_path() {
    let mut store = DelegationStore::new();
    let id = DelegationId::new("d-1");
    store
        .create(request("d-1", vec![capability("read_file")], false), 100)
        .expect("created");

    store
        .transition(&id, DelegationState::Accepted, 110)
        .unwrap();
    store
        .transition(&id, DelegationState::Running, 120)
        .unwrap();
    store
        .succeed(&id, serde_json::json!({ "ok": true }), 130)
        .unwrap();

    let record = store.record(&id).expect("a record");
    assert_eq!(record.state, DelegationState::Succeeded);
    assert_eq!(record.updated_at_ms, 130);
    assert_eq!(record.result, Some(serde_json::json!({ "ok": true })));
}

#[test]
fn an_invalid_transition_is_refused() {
    let mut store = DelegationStore::new();
    let id = DelegationId::new("d-2");
    store
        .create(request("d-2", vec![capability("read_file")], false), 100)
        .unwrap();

    let error = store
        .transition(&id, DelegationState::Running, 110)
        .expect_err("created cannot jump to running");

    assert!(
        matches!(
            error,
            DelegationError::InvalidTransition {
                from: DelegationState::Created,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_finished_delegation_cannot_be_cancelled() {
    let mut store = DelegationStore::new();
    let id = DelegationId::new("d-3");
    store.create(request("d-3", vec![], false), 100).unwrap();
    store
        .transition(&id, DelegationState::Accepted, 110)
        .unwrap();
    store
        .transition(&id, DelegationState::Running, 120)
        .unwrap();
    store.succeed(&id, serde_json::json!(null), 130).unwrap();

    let error = store
        .transition(&id, DelegationState::Cancelled, 140)
        .expect_err("a terminal state is final");

    assert!(matches!(error, DelegationError::InvalidTransition { .. }));
}

#[test]
fn a_created_delegation_can_be_rejected() {
    let mut store = DelegationStore::new();
    let id = DelegationId::new("d-4");
    store.create(request("d-4", vec![], false), 100).unwrap();

    store.reject(&id, "busy", 110).unwrap();

    let record = store.record(&id).expect("a record");
    assert_eq!(record.state, DelegationState::Rejected);
    assert_eq!(record.error.as_deref(), Some("busy"));
}

#[test]
fn sub_delegation_is_disabled_by_default() {
    let mut store = DelegationStore::new();
    let parent = DelegationId::new("d-5");
    store
        .create(request("d-5", vec![capability("read_file")], false), 100)
        .unwrap();
    store
        .transition(&parent, DelegationState::Accepted, 110)
        .unwrap();

    let error = store
        .delegate(
            &parent,
            request("d-5-child", vec![capability("read_file")], false),
            120,
        )
        .expect_err("the parent forbids it");

    assert!(matches!(
        error,
        DelegationError::SubDelegationDisabled { .. }
    ));
}

#[test]
fn a_sub_delegation_cannot_widen_the_parent_scope() {
    let mut store = DelegationStore::new();
    let parent = DelegationId::new("d-6");
    store
        .create(request("d-6", vec![capability("read_file")], true), 100)
        .unwrap();
    store
        .transition(&parent, DelegationState::Accepted, 110)
        .unwrap();

    let error = store
        .delegate(
            &parent,
            request(
                "d-6-child",
                vec![capability("read_file"), capability("write_file")],
                false,
            ),
            120,
        )
        .expect_err("the child asks for more than the parent holds");

    assert!(matches!(
        error,
        DelegationError::NotAttenuated { capability: widened } if widened == capability("write_file")
    ));
}

#[test]
fn a_sub_delegation_cannot_forward_to_another_node() {
    let mut store = DelegationStore::new();
    let parent = DelegationId::new("d-7");
    store
        .create(request("d-7", vec![capability("read_file")], true), 100)
        .unwrap();
    store
        .transition(&parent, DelegationState::Accepted, 110)
        .unwrap();
    let mut child = request("d-7-child", vec![capability("read_file")], false);
    child.target = node("node:other");

    let error = store
        .delegate(&parent, child, 120)
        .expect_err("not granted");

    assert!(matches!(error, DelegationError::RecipientNotAllowed { .. }));
}

#[test]
fn a_sub_delegation_cannot_outlive_the_parent() {
    let mut store = DelegationStore::new();
    let parent = DelegationId::new("d-8");
    let mut parent_request = request("d-8", vec![capability("read_file")], true);
    parent_request.deadline_ms = Some(1_000);
    store.create(parent_request, 100).unwrap();
    store
        .transition(&parent, DelegationState::Accepted, 110)
        .unwrap();
    let mut child = request("d-8-child", vec![capability("read_file")], false);
    child.deadline_ms = Some(2_000);

    let error = store
        .delegate(&parent, child, 120)
        .expect_err("the child outlives the parent");

    assert!(matches!(error, DelegationError::DeadlineExceedsParent));
}

#[test]
fn the_chain_names_every_hop() {
    let mut store = DelegationStore::new();
    let parent = DelegationId::new("d-9");
    store
        .create(request("d-9", vec![capability("read_file")], true), 100)
        .unwrap();
    store
        .transition(&parent, DelegationState::Accepted, 110)
        .unwrap();
    let child = store
        .delegate(
            &parent,
            request("d-9-child", vec![capability("read_file")], false),
            120,
        )
        .expect("attenuated");

    let chain = store.chain(&child);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].request.id, parent);
    assert_eq!(chain[1].request.id, child);
    assert_eq!(chain[1].request.parent.as_ref(), Some(&parent));
}

#[test]
fn an_unknown_delegation_is_an_error() {
    let mut store = DelegationStore::new();

    let error = store
        .transition(
            &DelegationId::new("missing"),
            DelegationState::Accepted,
            100,
        )
        .expect_err("nothing to transition");

    assert!(matches!(error, DelegationError::Unknown { .. }));
    assert!(store.record(&DelegationId::new("missing")).is_none());
}

/// The authenticated submitter is recorded, and a sub-delegation inherits the
/// parent's owner rather than taking on the sub-delegating peer's identity.
#[test]
fn ownership_is_recorded_and_inherited() {
    let mut store = DelegationStore::new();
    let owner = node("node:owner");
    let other = node("node:other");

    let mut parent = request("d-parent", vec![capability("read_file")], true);
    parent.target = node("node:builder");
    store
        .create_for(parent, Some(owner.clone()), 100)
        .expect("created");

    store
        .transition(
            &DelegationId::new("d-parent"),
            DelegationState::Accepted,
            105,
        )
        .expect("the parent is active");

    let mut child = request("d-child", vec![capability("read_file")], false);
    child.parent = Some(DelegationId::new("d-parent"));
    child.target = node("node:builder");
    store
        .delegate_for(&DelegationId::new("d-parent"), child, Some(other), 110)
        .expect("sub-delegated");

    assert_eq!(
        store
            .record(&DelegationId::new("d-parent"))
            .expect("parent")
            .origin,
        Some(owner.clone())
    );
    assert_eq!(
        store
            .record(&DelegationId::new("d-child"))
            .expect("child")
            .origin,
        Some(owner),
        "the child keeps the parent's owner"
    );
}
