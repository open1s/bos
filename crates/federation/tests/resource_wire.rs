//! Resource bytes move as stream messages over the same transport and identity.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::{CapabilityError, CapabilityId, FederationTransport, NodeId};
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
    caller_id: NodeId,
    provider_id: NodeId,
    provider_identity: NodeIdentity,
    network: Arc<InMemoryNetwork>,
    audit: Arc<federation::MemoryAudit>,
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

    let audit = Arc::new(federation::MemoryAudit::new());
    let caller = Arc::new(
        FederationNode::new(caller_identity, Arc::new(caller_transport)).with_audit(audit.clone()),
    );
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
        caller_id,
        provider_id,
        provider_identity: identity(52),
        network,
        audit,
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

async fn inject(
    transport: &federation::InMemoryTransport,
    from: &NodeIdentity,
    to: &NodeId,
    message: ResourceStreamMessage,
) {
    let payload = serde_json::to_vec(&message).expect("bytes");
    let message = federation::FederationMessage::new("resource", payload);
    let envelope = federation::Envelope::seal(&message, from, to).expect("sealed");
    let frame = envelope.encode().expect("encoded");
    transport.send(to, frame).await.expect("sent");
}

/// A different trusted peer that knows the live lease cannot inject a chunk, a
/// close, an error, or credit into someone else's stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_peer_cannot_inject_stream_messages() {
    let now = capability::unix_ms();
    let lease_id = LeaseId::new("l-inject");
    let holder = identity(51).node_id();
    let setup = setup(b"0123456789", lease("l-inject", &holder, None, None, now)).await;
    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &lease_id, "file:///demo", 0, 4)
        .await
        .expect("the lease is trusted");
    let first = next(&mut stream, 2_000).await;
    assert!(matches!(first, ResourceStreamMessage::Chunk { seq: 0, .. }));

    // The stranger joins the same network and forges every stream variant with
    // the lease id it guessed.
    let stranger_identity = identity(53);
    let stranger_id = stranger_identity.node_id();
    let (stranger_transport, stranger_inbox) = setup.network.join(stranger_id.clone());
    let stranger = Arc::new(FederationNode::new(
        stranger_identity.clone(),
        Arc::new(stranger_transport.clone()),
    ));
    let stranger_task = stranger.start(stranger_inbox);

    for message in [
        ResourceStreamMessage::Chunk {
            lease: lease_id.clone(),
            seq: 1,
            bytes: b"evil".to_vec(),
        },
        ResourceStreamMessage::Close {
            lease: lease_id.clone(),
            total: 0,
            checksum: String::new(),
        },
        ResourceStreamMessage::Error {
            lease: lease_id.clone(),
            message: "injected stop".to_string(),
        },
        ResourceStreamMessage::Credit {
            lease: lease_id.clone(),
            bytes: 1_000,
        },
    ] {
        inject(
            &stranger_transport,
            &stranger_identity,
            &setup.caller_id,
            message,
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(80)).await;

    // The stream is neither corrupted nor closed: the provider's next chunk is
    // still the one the consumer sees.
    stream.grant(4).await.expect("credit");
    let second = next(&mut stream, 2_000).await;
    match second {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 1, "the sequence was not advanced by the injection");
            assert_eq!(bytes, b"4567".to_vec());
        }
        other => panic!("expected the provider's chunk, got {other:?}"),
    }

    stranger_task.abort();
}

/// Even the right peer's stream is ordered: a duplicate or a jump is dropped
/// instead of reaching the consumer, and a chunk after close is not delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_and_out_of_order_chunks_are_rejected() {
    let now = capability::unix_ms();
    let lease_id = LeaseId::new("l-order");
    let holder = identity(51).node_id();
    let setup = setup(b"0123456789", lease("l-order", &holder, None, None, now)).await;
    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &lease_id, "file:///demo", 0, 4)
        .await
        .expect("the lease is trusted");
    assert!(matches!(
        next(&mut stream, 2_000).await,
        ResourceStreamMessage::Chunk { seq: 0, .. }
    ));

    // Forge as the provider itself. The transport joins under a throwaway id so
    // it does not replace the provider's real inbox; the signature, not the
    // transport, is what names the sender.
    let (provider_transport, _keepalive) = setup.network.join(identity(54).node_id());
    for message in [
        ResourceStreamMessage::Chunk {
            lease: lease_id.clone(),
            seq: 0,
            bytes: b"duplicate".to_vec(),
        },
        ResourceStreamMessage::Chunk {
            lease: lease_id.clone(),
            seq: 9,
            bytes: b"out-of-order".to_vec(),
        },
    ] {
        inject(
            &provider_transport,
            &setup.provider_identity,
            &setup.caller_id,
            message,
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    stream.grant(8).await.expect("credit");
    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 1, "the duplicate did not advance the stream");
            assert_eq!(bytes, b"456789".to_vec());
        }
        other => panic!("expected seq 1, got {other:?}"),
    }

    // Drain to the close.
    loop {
        match next(&mut stream, 2_000).await {
            ResourceStreamMessage::Close { .. } => break,
            ResourceStreamMessage::Chunk { .. } => {}
            other => panic!("unexpected mid-stream message: {other:?}"),
        }
    }
    inject(
        &provider_transport,
        &setup.provider_identity,
        &setup.caller_id,
        ResourceStreamMessage::Chunk {
            lease: lease_id.clone(),
            seq: 3,
            bytes: b"after-close".to_vec(),
        },
    )
    .await;
    match tokio::time::timeout(Duration::from_millis(120), stream.next()).await {
        // Nothing arrived, or the settled stream's queue closed: either way the
        // injected chunk did not reach the consumer.
        Err(_) | Ok(None) => {}
        Ok(Some(message)) => panic!("a chunk after close was delivered: {message:?}"),
    }
}

/// A resource larger than any single buffer streams in chunks and closes with
/// the same SHA-256 the in-memory helper would produce.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_large_stream_hashes_incrementally() {
    let data: Vec<u8> = (0..262_144u32).map(|index| (index % 251) as u8).collect();
    let now = capability::unix_ms();
    let lease_id = LeaseId::new("l-large");
    let setup = setup(
        &data,
        lease("l-large", &identity(51).node_id(), None, None, now),
    )
    .await;
    let mut stream = setup
        .caller
        .open_resource(&setup.provider_id, &lease_id, "file:///large", 0, 1_000_000)
        .await
        .expect("the lease is trusted");

    let mut received = Vec::with_capacity(data.len());
    let mut expected_seq = 0u64;
    loop {
        match next(&mut stream, 5_000).await {
            ResourceStreamMessage::Chunk { seq, bytes, .. } => {
                assert_eq!(seq, expected_seq, "chunks stay ordered");
                expected_seq += 1;
                received.extend_from_slice(&bytes);
            }
            ResourceStreamMessage::Close {
                total, checksum, ..
            } => {
                assert_eq!(total, data.len() as u64);
                assert_eq!(checksum, federation::checksum(&data));
                break;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
    assert_eq!(
        received.len(),
        data.len(),
        "byte count: received {}, data {}; events {:?}",
        received.len(),
        data.len(),
        setup.audit.events()
    );
    let mismatch = received
        .iter()
        .zip(&data)
        .position(|(actual, expected)| actual != expected);
    assert!(
        mismatch.is_none(),
        "first mismatch at {mismatch:?} (received {} bytes, data {} bytes)",
        received.len(),
        data.len()
    );
}
