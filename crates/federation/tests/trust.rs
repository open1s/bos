//! Trust is explicit, revocable, and time-bounded; discovery is signed and public-only.

use capability::unix_ms;
use capability::{
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind, VersionSource, Visibility,
};
use federation::{DiscoveryRecord, NodeIdentity, PeerRecord, PeerStore, TrustState};

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn descriptor(name: &str, visibility: Visibility) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        semver::Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_visibility(visibility)
}

#[test]
fn an_untouched_store_trusts_nobody() {
    let store = PeerStore::new();
    let error = store
        .authorize(&identity(9).node_id(), &capability("read_file"), unix_ms())
        .expect_err("nothing is enrolled");

    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("not enrolled")),
        "{error:?}"
    );
}

#[test]
fn a_grant_allows_exactly_the_named_capability() {
    let mut store = PeerStore::new();
    let peer = identity(1).node_id();
    let now = unix_ms();
    store.enroll(peer.clone(), now, Some("baseline peer".to_string()));
    store
        .grant(&peer, capability("read_file"), now, None)
        .expect("the peer is enrolled");

    assert!(store.is_trusted(&peer, now));
    store
        .authorize(&peer, &capability("read_file"), now)
        .expect("granted");
    let error = store
        .authorize(&peer, &capability("write_file"), now)
        .expect_err("not granted");
    assert!(matches!(error, CapabilityError::Denied { .. }));
}

#[test]
fn revoking_a_peer_removes_every_grant() {
    let mut store = PeerStore::new();
    let peer = identity(2).node_id();
    let now = unix_ms();
    store.enroll(peer.clone(), now, None);
    store
        .grant(&peer, capability("read_file"), now, None)
        .unwrap();

    assert!(store.revoke(&peer));
    assert_eq!(
        store.record(&peer).map(|record| record.state),
        Some(TrustState::Revoked)
    );
    assert!(store
        .authorize(&peer, &capability("read_file"), now)
        .is_err());
}

#[test]
fn an_expired_grant_is_denied() {
    let mut store = PeerStore::new();
    let peer = identity(3).node_id();
    let now = unix_ms();
    store.enroll(peer.clone(), now, None);
    store
        .grant(&peer, capability("read_file"), now, Some(now + 1))
        .unwrap();

    assert!(store
        .authorize(&peer, &capability("read_file"), now)
        .is_ok());
    assert!(store
        .authorize(&peer, &capability("read_file"), now + 1)
        .is_err());
}

#[test]
fn an_expired_enrollment_is_denied_even_with_a_grant() {
    let mut store = PeerStore::new();
    let peer = identity(4).node_id();
    let now = unix_ms();
    store.enroll(peer.clone(), now, None);
    store
        .grant(&peer, capability("read_file"), now, None)
        .unwrap();
    store.upsert(PeerRecord {
        node: peer.clone(),
        state: TrustState::Trusted,
        grants: store.record(&peer).expect("a record").grants.clone(),
        enrolled_at_ms: now,
        expires_at_ms: Some(now + 1),
        note: None,
    });

    assert!(store
        .authorize(&peer, &capability("read_file"), now)
        .is_ok());
    let error = store
        .authorize(&peer, &capability("read_file"), now + 1)
        .expect_err("the enrollment expired");
    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("expired")),
        "{error:?}"
    );
}

#[test]
fn a_record_publishes_only_public_descriptors() {
    let alice = identity(5);
    let record = DiscoveryRecord::publish(
        &alice,
        vec![
            descriptor("read_file", Visibility::Public),
            descriptor("deploy", Visibility::Granted),
        ],
    );

    assert_eq!(record.visible().len(), 1);
    assert_eq!(record.visible()[0].id, capability("read_file"));
    assert!(record.verify());
}

#[test]
fn a_tampered_record_does_not_verify() {
    let alice = identity(6);
    let mut record =
        DiscoveryRecord::publish(&alice, vec![descriptor("read_file", Visibility::Public)]);
    record.descriptors[0].display_name = "something else".to_string();

    assert!(!record.verify());
}

#[test]
fn a_record_from_another_node_does_not_verify() {
    let alice = identity(7);
    let bob = identity(8);
    let mut record =
        DiscoveryRecord::publish(&alice, vec![descriptor("read_file", Visibility::Public)]);
    record.node = bob.node_id();

    assert!(!record.verify());
}

#[test]
fn a_record_round_trips_through_its_bytes() {
    let alice = identity(10);
    let record = DiscoveryRecord::publish(
        &alice,
        vec![
            descriptor("read_file", Visibility::Public),
            descriptor("grep", Visibility::Public),
        ],
    );
    let bytes = record.encode().expect("encoded");
    let decoded = DiscoveryRecord::decode(&bytes).expect("decoded");

    assert_eq!(decoded, record);
    assert!(decoded.verify());
    assert!(decoded.find(&capability("grep")).is_some());
    assert!(decoded.find(&capability("missing")).is_none());
}

/// The store decides who may invoke what; it is written private to the account.
#[cfg(unix)]
#[test]
fn the_peer_store_file_is_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = std::env::temp_dir().join(format!("bsh-peers-mode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    let path = root.join("peers.json");

    let mut store = PeerStore::new();
    store.enroll(identity(7).node_id(), unix_ms(), None);
    store.save(&path).expect("saved");

    let mode = std::fs::metadata(&path)
        .expect("the saved store")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the peer store is owner-only");
    let _ = std::fs::remove_dir_all(&root);
}
