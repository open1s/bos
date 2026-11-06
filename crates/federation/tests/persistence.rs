//! Node identity and peer grants survive a restart.

use std::fs;
use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use capability::{CapabilityId, NodeId};
use federation::{NodeIdentity, PeerError, PeerStore, TrustState};

fn scratch(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "bsh-federation-persist-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a scratch root");
    root
}

fn node(value: &str) -> NodeId {
    NodeId::new(value).expect("a valid node id")
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

#[test]
fn an_identity_file_is_created_once_and_reused() {
    let root = scratch("identity");
    let path = root.join("federation").join("node.key");

    let first = NodeIdentity::load_or_create(&path).expect("created");
    let second = NodeIdentity::load_or_create(&path).expect("reused");

    assert_eq!(first.node_id(), second.node_id());
    let seed = fs::read_to_string(&path).expect("the seed file exists");
    let decoded = STANDARD.decode(seed.trim()).expect("base64");
    assert_eq!(decoded.len(), 32, "an Ed25519 seed is 32 bytes");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_peer_store_survives_a_round_trip() {
    let root = scratch("peers");
    let path = root.join("peers.json");
    let now = capability::unix_ms();
    let mut store = PeerStore::new();
    let trusted = node("node:trusted");
    store.enroll(trusted.clone(), now, Some("builder".to_string()));
    store
        .grant(&trusted, capability("build"), now, Some(now + 60_000))
        .expect("granted");
    let revoked = node("node:revoked");
    store.enroll(revoked.clone(), now, None);
    store.revoke(&revoked);

    store.save(&path).expect("saved");
    let loaded = PeerStore::load(&path).expect("loaded");

    assert_eq!(loaded, store);
    assert!(loaded.is_trusted(&trusted, now));
    assert!(loaded
        .authorize(&trusted, &capability("build"), now)
        .is_ok());
    assert_eq!(
        loaded.record(&revoked).map(|record| record.state),
        Some(TrustState::Revoked)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_missing_peer_file_is_an_empty_store() {
    let root = scratch("missing");
    let store = PeerStore::load(&root.join("absent.json")).expect("empty");

    assert_eq!(store, PeerStore::new());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_corrupt_peer_file_is_reported() {
    let root = scratch("corrupt");
    let path = root.join("peers.json");
    fs::write(&path, "{not json").expect("write");

    let error = PeerStore::load(&path).expect_err("the file is unreadable");

    assert!(matches!(error, PeerError::Json { .. }), "{error:?}");
    let _ = fs::remove_dir_all(&root);
}
