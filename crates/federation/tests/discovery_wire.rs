//! Discovery over the wire: signed, authenticated, public-only.

use std::sync::Arc;
use std::time::Duration;

use capability::{
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind, NodeId, VersionSource,
    Visibility,
};
use federation::{FederationNode, InMemoryNetwork, NodeIdentity};
use semver::Version;

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn descriptor(name: &str, visibility: Visibility) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        CapabilityId::new("builtin", name).expect("a valid id"),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
    .with_display_name(name)
    .with_visibility(visibility)
}

struct Pair {
    caller: Arc<FederationNode>,
    provider_id: NodeId,
    provider: Arc<FederationNode>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn pair(caller_enrolled: bool, provider_enrolled: bool) -> Pair {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(61);
    let provider_identity = identity(62);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();

    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());

    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_public_capabilities(vec![
                descriptor("read_file", Visibility::Public),
                descriptor("deploy", Visibility::Granted),
            ]),
    );
    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    let now = capability::unix_ms();
    if caller_enrolled {
        caller
            .peers()
            .write()
            .expect("peer store")
            .enroll(provider_id.clone(), now, None);
    }
    if provider_enrolled {
        provider
            .peers()
            .write()
            .expect("peer store")
            .enroll(caller_id, now, None);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    Pair {
        caller,
        provider_id,
        provider,
        _tasks: tasks,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_returns_only_public_capabilities_and_verifies() {
    let pair = pair(true, true).await;

    let record = pair
        .caller
        .discover(&pair.provider_id)
        .await
        .expect("the peer answers");

    assert_eq!(record.node, pair.provider_id);
    assert!(record.verify());
    assert_eq!(
        record.visible().len(),
        1,
        "the Granted descriptor stays private"
    );
    assert_eq!(
        record.visible()[0].id,
        CapabilityId::new("builtin", "read_file").expect("a valid id")
    );
    assert_eq!(pair.provider.node(), pair.provider_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_must_trust_the_peer_before_discovery() {
    let pair = pair(false, true).await;

    let error = pair
        .caller
        .discover(&pair.provider_id)
        .await
        .expect_err("the caller never enrolled the peer");

    assert!(matches!(error, CapabilityError::Denied { .. }), "{error:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_provider_refuses_an_unenrolled_caller() {
    let pair = pair(true, false).await;

    let error = pair
        .caller
        .discover(&pair.provider_id)
        .await
        .expect_err("the provider does not know the caller");

    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("not enrolled")),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tampered_record_stops_verifying() {
    let pair = pair(true, true).await;
    let mut record = pair
        .caller
        .discover(&pair.provider_id)
        .await
        .expect("answered");
    record.descriptors[0].display_name = "something else".to_string();

    assert!(!record.verify());
}
