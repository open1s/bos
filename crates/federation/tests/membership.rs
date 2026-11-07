//! Federation membership metadata is explicit, signed, and never authority.

use std::sync::Arc;

use capability::{unix_ms, NodeId};
use federation::{FederationId, InMemoryNetwork, NodeIdentity, PeerRecord, PeerStore};

fn node(seed: u8) -> NodeId {
    NodeIdentity::from_seed([seed; 32]).node_id()
}

#[test]
fn a_federation_id_round_trips() {
    let id = FederationId::new("fed:team");
    assert_eq!(id.as_str(), "fed:team");
    assert_eq!(id.to_string(), "fed:team");
    let text = serde_json::to_string(&id).expect("json");
    assert_eq!(serde_json::from_str::<FederationId>(&text).expect("id"), id);
}

#[test]
fn peers_record_their_membership_and_a_member_list_is_sorted() {
    let mut store = PeerStore::new();
    let federation = FederationId::new("fed:team");
    let first = node(3);
    let second = node(4);
    let outside = node(5);
    store.enroll_in(second.clone(), Some(federation.clone()), unix_ms(), None);
    store.enroll_in(first.clone(), Some(federation.clone()), unix_ms(), None);
    store.enroll(outside.clone(), unix_ms(), None);

    assert_eq!(store.federation_of(&first), Some(&federation));
    assert_eq!(store.federation_of(&outside), None);
    let members: Vec<&NodeId> = store
        .members_of(&federation)
        .into_iter()
        .map(|record| &record.node)
        .collect();
    assert_eq!(members, vec![&first, &second]);
}

#[test]
fn an_old_peer_record_without_membership_still_loads() {
    let node = node(6);
    let text = format!(
        r#"{{"node":"{node}","state":"Trusted","grants":[],"enrolled_at_ms":1,"expires_at_ms":null,"note":null}}"#
    );
    let record: PeerRecord = serde_json::from_str(&text).expect("old record");
    assert_eq!(record.federation, None);
    let mut store = PeerStore::new();
    store.upsert(record);
    assert_eq!(store.federation_of(&node), None);
}

#[tokio::test]
async fn discovery_advertises_the_configured_federation_and_revision() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = NodeIdentity::from_seed([81; 32]);
    let provider_identity = NodeIdentity::from_seed([82; 32]);
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
            .with_federation(FederationId::new("fed:team"), 7),
    );
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);
    let now = unix_ms();
    caller.peers().write().expect("peers").enroll_in(
        provider_id.clone(),
        Some(FederationId::new("fed:team")),
        now,
        None,
    );
    provider
        .peers()
        .write()
        .expect("peers")
        .enroll(caller_id.clone(), now, None);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let record = caller.discover(&provider_id).await.expect("discovered");

    assert_eq!(record.federation, Some(FederationId::new("fed:team")));
    assert_eq!(record.policy_revision, 7);
    assert!(record.verify());

    caller_task.abort();
    provider_task.abort();
}

#[test]
fn a_signed_rotation_moves_grants_and_revokes_the_old_key() {
    use capability::{unix_ms, CapabilityId};

    let old = NodeIdentity::from_seed([91; 32]);
    let new = NodeIdentity::from_seed([92; 32]);
    let old_id = old.node_id();
    let new_id = new.node_id();
    let capability: CapabilityId = "builtin/read_file".parse().expect("capability");
    let mut store = PeerStore::new();
    let now = unix_ms();
    store.enroll_in(
        old_id.clone(),
        Some(FederationId::new("fed:team")),
        now,
        Some("laptop".to_string()),
    );
    store
        .grant(&old_id, capability.clone(), now, None)
        .expect("granted");

    let record =
        federation::RotationRecord::sign(&old, &new, now, Some("disk replacement".to_string()));
    let text = record.to_json().expect("json");
    let decoded = federation::RotationRecord::from_json(&text).expect("round trip");
    assert!(decoded.verify());
    store.rotate(&decoded, now).expect("rotation");

    assert!(!store.is_trusted(&old_id, now));
    assert!(store.is_trusted(&new_id, now));
    assert_eq!(
        store.record(&new_id).expect("new").federation,
        Some(FederationId::new("fed:team"))
    );
    store
        .authorize(&new_id, &capability, now)
        .expect("the grant moved");
    assert!(store.authorize(&old_id, &capability, now).is_err());
}

#[test]
fn a_forged_or_unenrolled_rotation_is_refused() {
    use capability::unix_ms;

    let old = NodeIdentity::from_seed([93; 32]);
    let new = NodeIdentity::from_seed([94; 32]);
    let imitator = NodeIdentity::from_seed([95; 32]);
    let mut store = PeerStore::new();
    let now = unix_ms();
    store.enroll(old.node_id(), now, None);

    let mut record = federation::RotationRecord::sign(&old, &new, now, None);
    record.signature = imitator.sign(b"not the statement");
    assert!(matches!(
        store.rotate(&record, now),
        Err(federation::PeerError::Rotation { .. })
    ));

    let stranger = NodeIdentity::from_seed([96; 32]);
    let record = federation::RotationRecord::sign(&stranger, &new, now, None);
    assert!(matches!(
        store.rotate(&record, now),
        Err(federation::PeerError::Rotation { .. })
    ));
}

#[tokio::test]
async fn a_rotation_record_is_delivered_and_applied_over_the_wire() {
    use std::sync::Arc;

    let network = Arc::new(InMemoryNetwork::new());
    let provider_identity = NodeIdentity::from_seed([111; 32]);
    let old = NodeIdentity::from_seed([112; 32]);
    let new = NodeIdentity::from_seed([113; 32]);
    let provider_id = provider_identity.node_id();
    let old_id = old.node_id();
    let new_id = new.node_id();
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let (sender_transport, sender_inbox) = network.join(new_id.clone());
    let peers_path =
        std::env::temp_dir().join(format!("bos-rotation-wire-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&peers_path);
    let mut store = PeerStore::new();
    let capability: capability::CapabilityId = "builtin/read_file".parse().expect("capability");
    store.enroll(old_id.clone(), capability::unix_ms(), None);
    store
        .grant(&old_id, capability.clone(), capability::unix_ms(), None)
        .expect("grant");
    store.save(&peers_path).expect("saved");
    let provider = Arc::new(
        federation::FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_peer_store(Arc::new(std::sync::RwLock::new(store)))
            .with_peer_persistence(&peers_path),
    );
    let sender = Arc::new(federation::FederationNode::new(
        new.clone(),
        Arc::new(sender_transport),
    ));
    let provider_task = provider.start(provider_inbox);
    let sender_task = sender.start(sender_inbox);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let record = federation::RotationRecord::sign(&old, &new, capability::unix_ms(), None);
    let context = capability::InvocationContext::local(
        capability::InvocationId::new("inv:rotation"),
        capability::Principal::Local {
            session: "s".to_string(),
            preset: "p".to_string(),
        },
        capability::WorkspaceId::new("ws:rotation").expect("workspace"),
    );
    sender
        .publish_rotation(&provider_id, &record, &context)
        .await
        .expect("the record is accepted");

    let peers = provider.peers();
    let store = peers.read().expect("peers");
    assert!(store.is_trusted(&new_id, capability::unix_ms()));
    assert!(!store.is_trusted(&old_id, capability::unix_ms()));
    drop(store);
    let reloaded = PeerStore::load(&peers_path).expect("persisted");
    assert!(reloaded.is_trusted(&new_id, capability::unix_ms()));
    assert!(reloaded
        .authorize(&new_id, &capability, capability::unix_ms())
        .is_ok());

    provider_task.abort();
    sender_task.abort();
    let _ = std::fs::remove_file(peers_path);
}
