//! A resource transfer interrupted by a restart resumes at the checkpoint.
//!
//! The caller reads one small chunk under a four-byte credit window, drops the
//! stream, and is rebuilt from the same identity and checkpoint path. The
//! provider must serve the second attempt from the stored offset, and the
//! checkpoint must disappear only when the stream completes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use capability::CapabilityId;
use federation::resource::{LeaseId, ResourceLease, TransferStore};
use federation::{FederationNode, InMemoryNetwork, NodeIdentity, ResourceSource};

struct FixedSource {
    bytes: Vec<u8>,
    first_offset: Arc<AtomicU64>,
    fail_once: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl ResourceSource for FixedSource {
    async fn read(
        &self,
        _uri: &str,
        offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, capability::CapabilityError> {
        let _ = self.first_offset.compare_exchange(
            u64::MAX,
            offset,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        // The first attempt fails after four bytes, closing the provider's
        // session; the retry serves the remainder.
        if offset > 0 && self.fail_once.swap(false, Ordering::SeqCst) {
            return Err(capability::CapabilityError::Unavailable {
                reason: "the source is briefly unavailable".to_string(),
            });
        }
        let Some(rest) = self.bytes.get(offset as usize..) else {
            return Ok(None);
        };
        Ok(Some(rest[..rest.len().min(max)].to_vec()))
    }
}

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn lease_id() -> LeaseId {
    LeaseId::new("lease-resume")
}

fn capability() -> CapabilityId {
    CapabilityId::new("builtin", "read_file").expect("a capability")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_resumes_at_the_checkpoint() {
    let root = std::env::temp_dir().join(format!("federation-transfer-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    let path = root.join("transfers.json");

    let expected: Vec<u8> = (0u8..64).collect();
    let offsets = Arc::new(AtomicU64::new(u64::MAX));
    let source = Arc::new(FixedSource {
        bytes: expected.clone(),
        first_offset: Arc::clone(&offsets),
        fail_once: std::sync::atomic::AtomicBool::new(true),
    });

    let network = Arc::new(InMemoryNetwork::new());
    let provider_identity = identity(41);
    let provider_id = provider_identity.node_id();
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_resource_source(source);
    let provider_router = provider.start(provider_inbox);

    let caller_identity = identity(42);
    let caller_id = caller_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let caller = FederationNode::new(caller_identity.clone(), Arc::new(caller_transport))
        .with_transfer_persistence(&path);
    let caller_router = caller.start(caller_inbox);

    let now = capability::unix_ms();
    {
        let peers = caller.peers();
        let mut peers = peers.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability(), now, None)
            .expect("granted");
    }
    {
        let peers = provider.peers();
        let mut peers = peers.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        let lease_store = provider.leases();
        let mut leases = lease_store.write().expect("leases");
        leases
            .grant(ResourceLease {
                id: lease_id(),
                capability: capability(),
                holder: caller_id.clone(),
                granted_at_ms: now,
                expires_at_ms: None,
                max_bytes: Some(256),
                used_bytes: 0,
            })
            .expect("a lease");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut stream = caller
        .open_resource(&provider_id, &lease_id(), "file:///artifact", 0, 4)
        .await
        .expect("opened");
    let first = stream.next().await.expect("a chunk");
    let received = match first {
        federation::resource::ResourceStreamMessage::Chunk { bytes, .. } => bytes,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(received, &expected[..4]);
    stream.grant(64).await.expect("more credit");
    let failed = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the stream must not hang")
        .expect("the error");
    assert!(matches!(
        failed,
        federation::resource::ResourceStreamMessage::Error { .. }
    ));
    drop(stream);
    drop(caller);
    drop(caller_router);

    let store = TransferStore::load(&path).expect("the checkpoint");
    assert_eq!(
        store.position(&lease_id(), "file:///artifact"),
        4,
        "an interrupted transfer keeps its position"
    );

    // The next attempt's first read is the one that proves the resume offset.
    offsets.store(u64::MAX, Ordering::SeqCst);

    // A new process: same identity, same checkpoint path, fresh transport.
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let restarted = FederationNode::new(caller_identity, Arc::new(caller_transport))
        .with_transfer_persistence(&path);
    let _caller_router = restarted.start(caller_inbox);
    {
        let peers = restarted.peers();
        let mut peers = peers.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut stream = restarted
        .open_resource(&provider_id, &lease_id(), "file:///artifact", 0, 64)
        .await
        .expect("reopened");
    let mut collected = Vec::new();
    loop {
        match stream.next().await.expect("a message") {
            federation::resource::ResourceStreamMessage::Chunk { bytes, .. } => {
                collected.extend_from_slice(&bytes);
            }
            federation::resource::ResourceStreamMessage::Close { .. } => break,
            federation::resource::ResourceStreamMessage::Error { message, .. } => {
                panic!("the resumed stream failed: {message}")
            }
            _ => {}
        }
    }
    assert_eq!(
        collected,
        expected[4..],
        "the remainder is served from offset 4"
    );
    assert_eq!(
        offsets.load(Ordering::SeqCst),
        4,
        "the provider was asked for the stored offset"
    );

    let store = TransferStore::load(&path).expect("the checkpoint");
    assert_eq!(
        store.position(&lease_id(), "file:///artifact"),
        0,
        "a completed transfer forgets its checkpoint"
    );

    drop(stream);
    drop(restarted);
    drop(_caller_router);
    drop(provider_router);
    drop(provider);
    let _ = std::fs::remove_dir_all(&root);
}
