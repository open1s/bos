//! Resource bytes move as stream messages over the same transport and identity.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::{CapabilityError, CapabilityId, NodeId};
use federation::{
    checksum, FederationNode, InMemoryNetwork, LeaseId, NodeIdentity, RemoteStream, ResourceLease,
    ResourceSource, ResourceStreamMessage,
};

struct Bytes(Vec<u8>);

#[async_trait]
impl ResourceSource for Bytes {
    async fn read(
        &self,
        _uri: &str,
        offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        let start = offset as usize;
        if start >= self.0.len() {
            return Ok(None);
        }
        let end = (start + max).min(self.0.len());
        Ok(Some(self.0[start..end].to_vec()))
    }
}

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability() -> CapabilityId {
    CapabilityId::new("resource", "fs.read").expect("a valid id")
}

fn lease(
    id: &str,
    holder: &NodeId,
    max_bytes: Option<u64>,
    expires_at_ms: Option<u64>,
    now: u64,
) -> ResourceLease {
    ResourceLease {
        id: LeaseId::new(id),
        capability: capability(),
        holder: holder.clone(),
        granted_at_ms: now,
        expires_at_ms,
        max_bytes,
        used_bytes: 0,
    }
}

struct Setup {
    caller: Arc<FederationNode>,
    provider: Arc<FederationNode>,
    provider_id: NodeId,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn setup(data: &[u8], granted: ResourceLease) -> Setup {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(51);
    let provider_identity = identity(52);
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
            .with_resource_source(Arc::new(Bytes(data.to_vec()))),
    );
    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    provider
        .leases()
        .write()
        .expect("leases")
        .grant(granted)
        .expect("granted");
    {
        let store = caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(provider_id.clone(), capability::unix_ms(), None);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    Setup {
        caller,
        provider,
        provider_id,
        _tasks: tasks,
    }
}

async fn next(stream: &mut RemoteStream, millis: u64) -> ResourceStreamMessage {
    tokio::time::timeout(Duration::from_millis(millis), stream.next())
        .await
        .expect("a stream message arrives")
        .expect("the stream channel is open")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_is_bounded_by_credit_and_closes_with_a_checksum() {
    let data = b"0123456789";
    let now = capability::unix_ms();
    let lease_id = LeaseId::new("l-1");
    let setup = setup(data, lease("l-1", &identity(51).node_id(), None, None, now)).await;

    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &lease_id, "file:///demo", 0, 4)
        .await
        .expect("the lease is trusted");

    let mut received = Vec::new();
    let first = next(&mut stream, 2_000).await;
    match first {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 0);
            assert_eq!(bytes, b"0123".to_vec());
            received.extend_from_slice(&bytes);
        }
        other => panic!("expected a chunk, got {other:?}"),
    }

    // The sender is out of credit: nothing more arrives until the caller grants.
    assert!(
        tokio::time::timeout(Duration::from_millis(150), stream.next())
            .await
            .is_err(),
        "the producer must block without credit"
    );

    stream.grant(4).await.expect("credit sent");
    let second = next(&mut stream, 2_000).await;
    match second {
        ResourceStreamMessage::Chunk { bytes, .. } => received.extend_from_slice(&bytes),
        other => panic!("expected a chunk, got {other:?}"),
    }
    stream.grant(64).await.expect("credit sent");

    loop {
        match next(&mut stream, 2_000).await {
            ResourceStreamMessage::Chunk { bytes, .. } => received.extend_from_slice(&bytes),
            ResourceStreamMessage::Close {
                total,
                checksum: declared,
                ..
            } => {
                assert_eq!(total, data.len() as u64);
                assert_eq!(declared, checksum(data));
                break;
            }
            other => panic!("unexpected stream message: {other:?}"),
        }
    }

    assert_eq!(received, data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_held_by_another_node_is_refused() {
    let now = capability::unix_ms();
    let setup = setup(
        b"secret",
        lease("l-2", &identity(99).node_id(), None, None, now),
    )
    .await;

    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &LeaseId::new("l-2"), "file:///x", 0, 8)
        .await
        .expect("the peer is enrolled");

    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Error { message, .. } => {
            assert!(message.contains("belongs to"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_lease_sends_an_error_instead_of_bytes() {
    let now = capability::unix_ms();
    let setup = setup(
        b"expired",
        lease("l-3", &identity(51).node_id(), None, Some(now), now),
    )
    .await;

    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &LeaseId::new("l-3"), "file:///x", 0, 8)
        .await
        .expect("the peer is enrolled");

    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Error { message, .. } => {
            assert!(message.contains("expired"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_quota_stops_the_stream_mid_way() {
    let now = capability::unix_ms();
    let setup = setup(
        b"0123456789",
        lease("l-4", &identity(51).node_id(), Some(4), None, now),
    )
    .await;

    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &LeaseId::new("l-4"), "file:///x", 0, 64)
        .await
        .expect("the peer is enrolled");

    let mut received = Vec::new();
    loop {
        match next(&mut stream, 2_000).await {
            ResourceStreamMessage::Chunk { bytes, .. } => received.extend_from_slice(&bytes),
            ResourceStreamMessage::Error { message, .. } => {
                assert!(message.contains("bytes left"), "{message}");
                break;
            }
            other => panic!("unexpected stream message: {other:?}"),
        }
    }
    assert_eq!(received.len(), 4, "the quota bounds the stream");
    assert_eq!(setup.provider_id, setup.provider.node());
}
