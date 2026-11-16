//! Resource-stream lifecycle: cancellation, deadlines, and attempt correlation
//! (issues #347 and #348).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use capability::{CapabilityError, CapabilityId, FederationTransport, NodeId};
use federation::audit::stream_subject;
use federation::{
    FederationEvent, FederationMessage, FederationNode, InMemoryNetwork, LeaseId, MemoryAudit,
    NodeIdentity, RemoteStream, ResourceLease, ResourceSource, ResourceStreamMessage,
};

const URI: &str = "file:///artifact";

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability() -> CapabilityId {
    CapabilityId::new("resource", "fs.read").expect("a valid id")
}

fn lease(id: &str, holder: &NodeId, max_bytes: Option<u64>) -> ResourceLease {
    ResourceLease {
        id: LeaseId::new(id),
        capability: capability(),
        holder: holder.clone(),
        granted_at_ms: capability::unix_ms(),
        expires_at_ms: None,
        max_bytes,
        used_bytes: 0,
    }
}

/// Bytes, optionally stalling every read past the data.
///
/// A stalled read never completes, so only a cancellation or the injected
/// deadline can end the producer.
struct GatedBytes {
    data: Vec<u8>,
    entered: AtomicUsize,
    completed: AtomicUsize,
    stall: AtomicBool,
}

impl GatedBytes {
    fn new(data: &[u8], stall: bool) -> Self {
        Self {
            data: data.to_vec(),
            entered: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            stall: AtomicBool::new(stall),
        }
    }

    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    fn completed(&self) -> usize {
        self.completed.load(Ordering::SeqCst)
    }

    fn unstall(&self) {
        self.stall.store(false, Ordering::SeqCst);
    }
}

#[async_trait]
impl ResourceSource for GatedBytes {
    async fn read(
        &self,
        _uri: &str,
        offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let start = offset as usize;
        if self.stall.load(Ordering::SeqCst) && start >= self.data.len() {
            std::future::pending::<()>().await;
        }
        if start >= self.data.len() {
            return Ok(None);
        }
        let end = (start + max).min(self.data.len());
        self.completed.fetch_add(1, Ordering::SeqCst);
        Ok(Some(self.data[start..end].to_vec()))
    }
}

/// A read that never returns at all.
struct Stalled;

#[async_trait]
impl ResourceSource for Stalled {
    async fn read(
        &self,
        _uri: &str,
        _offset: u64,
        _max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        std::future::pending::<Result<Option<Vec<u8>>, CapabilityError>>().await
    }
}

/// A source whose second read waits for the test, so a cancel can race it.
struct GatedReads {
    first: Vec<u8>,
    tail: Option<Vec<u8>>,
    entered: AtomicUsize,
    gate: tokio::sync::Notify,
}

impl GatedReads {
    fn new(first: &[u8], tail: Option<&[u8]>) -> Self {
        Self {
            first: first.to_vec(),
            tail: tail.map(<[u8]>::to_vec),
            entered: AtomicUsize::new(0),
            gate: tokio::sync::Notify::new(),
        }
    }

    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    /// Release the waiting read; a permit survives if no read is waiting yet.
    fn release(&self) {
        self.gate.notify_one();
    }
}

#[async_trait]
impl ResourceSource for GatedReads {
    async fn read(
        &self,
        _uri: &str,
        offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        if offset == 0 {
            let end = self.first.len().min(max);
            return Ok(Some(self.first[..end].to_vec()));
        }
        if offset as usize == self.first.len() {
            self.gate.notified().await;
            return Ok(self.tail.clone());
        }
        Ok(None)
    }
}

struct Setup {
    caller: Arc<FederationNode>,
    provider: Arc<FederationNode>,
    caller_identity: NodeIdentity,
    provider_id: NodeId,
    network: Arc<InMemoryNetwork>,
    caller_audit: Arc<MemoryAudit>,
    provider_audit: Arc<MemoryAudit>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn setup(
    source: Arc<dyn ResourceSource>,
    lease_id: &str,
    max_bytes: Option<u64>,
    max_duration: Option<Duration>,
    transfers: Option<std::path::PathBuf>,
) -> Setup {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(61);
    let provider_identity = identity(62);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();

    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());

    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let mut caller = FederationNode::new(caller_identity.clone(), Arc::new(caller_transport))
        .with_audit(caller_audit.clone());
    if let Some(path) = transfers {
        caller = caller.with_transfer_persistence(path);
    }
    let mut provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_resource_source(source)
        .with_audit(provider_audit.clone());
    if let Some(duration) = max_duration {
        provider = provider.with_max_stream_duration(duration);
    }
    let caller = Arc::new(caller);
    let provider = Arc::new(provider);
    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    provider
        .leases()
        .write()
        .expect("leases")
        .grant(lease(lease_id, &caller_id, max_bytes))
        .expect("the lease is granted");
    caller
        .peers()
        .write()
        .expect("peers")
        .enroll(provider_id.clone(), capability::unix_ms(), None);
    tokio::time::sleep(Duration::from_millis(20)).await;

    Setup {
        caller,
        provider,
        caller_identity,
        provider_id,
        network,
        caller_audit,
        provider_audit,
        _tasks: tasks,
    }
}

async fn open(setup: &Setup, lease: &str, credit: u64) -> RemoteStream {
    setup
        .caller
        .open_resource(&setup.provider_id, &LeaseId::new(lease), URI, 0, credit)
        .await
        .expect("the peer is enrolled and the lease is trusted")
}

async fn next(stream: &mut RemoteStream, millis: u64) -> ResourceStreamMessage {
    tokio::time::timeout(Duration::from_millis(millis), stream.next())
        .await
        .expect("a stream message arrives")
        .expect("the stream channel is open")
}

async fn wait_for_event(
    audit: &MemoryAudit,
    predicate: impl Fn(&FederationEvent) -> bool,
) -> FederationEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(event) = audit.events().into_iter().find(|event| predicate(event)) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the expected audit event arrives")
}

/// A provider terminal record: every ended stream carries its byte count.
fn terminal(event: &FederationEvent) -> bool {
    event.operation == "resource" && event.bytes.is_some()
}

async fn wait_for_sessions(provider: &FederationNode, expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.active_resource_sessions() != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the provider still has {} sessions, expected {expected}",
            provider.active_resource_sessions()
        )
    });
}

/// The lease usage on the provider side, for asserting that nothing was charged.
fn used_bytes(setup: &Setup, lease: &str) -> Option<u64> {
    setup
        .provider
        .leases()
        .read()
        .expect("leases")
        .get(&LeaseId::new(lease))
        .map(|lease| lease.used_bytes)
}

/// Forge a cancel as the caller: the signature names the sender, so a
/// throwaway transport cannot impersonate a different peer.
async fn inject_cancel(setup: &Setup, lease: &str, attempt: u64) {
    let message = ResourceStreamMessage::Cancel {
        lease: LeaseId::new(lease),
        attempt,
    };
    let payload = serde_json::to_vec(&message).expect("bytes");
    let message = FederationMessage::new("resource", payload);
    let envelope = federation::Envelope::seal(&message, &setup.caller_identity, &setup.provider_id)
        .expect("sealed");
    let frame = envelope.encode().expect("encoded");
    let (transport, _keepalive) = setup.network.join(identity(63).node_id());
    transport
        .send(&setup.provider_id, frame)
        .await
        .expect("the forged cancel is sent");
}

fn scratch(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "federation-stream-{label}-{}-{}",
        std::process::id(),
        capability::unix_ms()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// Issue #347: dropping the stream stops the producer at its pending read. No
/// further read, charge, or chunk happens, the provider slot is released, and a
/// new stream opens immediately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_stream_stops_the_producer_and_releases_its_slot() {
    let source = Arc::new(GatedBytes::new(b"abcd", true));
    let setup = setup(
        source.clone(),
        "lease-stop",
        None,
        Some(Duration::from_secs(30)),
        None,
    )
    .await;

    let mut stream = open(&setup, "lease-stop", 4).await;
    let attempt = stream.attempt();
    let lease = LeaseId::new("lease-stop");
    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 0);
            assert_eq!(bytes, b"abcd".to_vec());
        }
        other => panic!("expected the first chunk, got {other:?}"),
    }
    assert_eq!(used_bytes(&setup, "lease-stop"), Some(4));

    // Grant one more window: the producer starts a read that never returns.
    stream.grant(4).await.expect("credit");
    tokio::time::timeout(Duration::from_secs(2), async {
        while source.entered() != 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the producer is stalled in its second read");

    drop(stream);

    let expected = stream_subject(&lease, attempt);
    let terminal_event = wait_for_event(&setup.provider_audit, terminal).await;
    assert_eq!(terminal_event.outcome, "cancelled", "{terminal_event:?}");
    assert_eq!(terminal_event.subject.as_deref(), Some(expected.as_str()));
    assert_eq!(terminal_event.bytes, Some(4));
    wait_for_sessions(&setup.provider, 0).await;

    // Nothing more was read, charged, or sent after the cancel was accepted.
    assert_eq!(source.entered(), 2, "the stalled read is not retried");
    assert_eq!(source.completed(), 1, "the stalled read never completed");
    assert_eq!(
        used_bytes(&setup, "lease-stop"),
        Some(4),
        "nothing was charged after the cancellation"
    );

    // A cancel the peer transport accepted is recorded as confirmed, and the
    // caller record names the same attempt as the provider record.
    let caller = wait_for_event(&setup.caller_audit, terminal).await;
    assert_eq!(caller.outcome, "cancelled");
    assert_eq!(caller.subject.as_deref(), Some(expected.as_str()));
    assert!(
        caller
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("accepted"),
        "{caller:?}"
    );

    // The slot is free: the same lease opens again right away.
    source.unstall();
    let mut second = open(&setup, "lease-stop", 4).await;
    match next(&mut second, 2_000).await {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 0, "the new stream is its own sequence");
            assert_eq!(bytes, b"abcd".to_vec());
        }
        other => panic!("expected a fresh chunk, got {other:?}"),
    }
    second.grant(4).await.expect("credit");
    match next(&mut second, 2_000).await {
        ResourceStreamMessage::Close { total, .. } => assert_eq!(total, 4),
        other => panic!("expected the close, got {other:?}"),
    }
}

/// Issue #347: a producer waiting for credit (no read pending) wakes and exits
/// promptly on cancel, not at its deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_producer_waiting_for_credit_exits_promptly_on_cancel() {
    let source = Arc::new(GatedBytes::new(b"abcd", true));
    let setup = setup(
        source.clone(),
        "lease-credit",
        None,
        // Far beyond the test: only the cancel can end this stream in time.
        Some(Duration::from_secs(30)),
        None,
    )
    .await;

    let stream = open(&setup, "lease-credit", 0).await;
    let attempt = stream.attempt();
    wait_for_sessions(&setup.provider, 1).await;

    let started = std::time::Instant::now();
    drop(stream);
    wait_for_sessions(&setup.provider, 0).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the cancel woke the producer in {:?}",
        started.elapsed()
    );
    assert_eq!(source.entered(), 0, "a stream with no credit never reads");

    let terminal_event = wait_for_event(&setup.provider_audit, terminal).await;
    assert_eq!(terminal_event.outcome, "cancelled");
    assert_eq!(
        terminal_event.subject.as_deref(),
        Some(stream_subject(LeaseId::new("lease-credit"), attempt).as_str())
    );
    assert_eq!(terminal_event.bytes, Some(0));
}

/// Issue #348: a read that never returns is dropped at the injected deadline,
/// and the timeout outcome is distinct from a cancellation.
#[tokio::test(start_paused = true)]
async fn a_pending_read_exits_at_the_injected_deadline() {
    let setup = setup(
        Arc::new(Stalled),
        "lease-deadline",
        None,
        Some(Duration::from_millis(30)),
        None,
    )
    .await;
    let mut stream = open(&setup, "lease-deadline", 64).await;
    let attempt = stream.attempt();

    let event = wait_for_event(&setup.provider_audit, |event| {
        terminal(event) && event.outcome == "timeout"
    })
    .await;
    assert_eq!(
        event.subject.as_deref(),
        Some(stream_subject(LeaseId::new("lease-deadline"), attempt).as_str())
    );
    assert_eq!(event.bytes, Some(0), "the pending read was not accepted");
    assert!(
        event
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("exceeded"),
        "{event:?}"
    );
    wait_for_sessions(&setup.provider, 0).await;
    assert_eq!(used_bytes(&setup, "lease-deadline"), Some(0));

    match next(&mut stream, 1_000).await {
        ResourceStreamMessage::Error { message, .. } => {
            assert!(message.contains("exceeded"), "{message}");
        }
        other => panic!("expected the timeout error, got {other:?}"),
    }
    assert!(
        !setup
            .provider_audit
            .events()
            .iter()
            .any(|event| terminal(event) && event.outcome == "cancelled"),
        "a timeout is not recorded as a cancellation"
    );
}
/// Issue #347: a cancel racing end-of-file, or a chunk that was already read,
/// produces exactly one terminal outcome and one terminal provider record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_racing_eof_or_a_chunk_yields_one_terminal_outcome() {
    for (label, tail, cancel_first) in [
        ("eof", None, true),
        ("eof", None, false),
        ("chunk", Some(&b"efgh"[..]), true),
        ("chunk", Some(&b"efgh"[..]), false),
    ] {
        let source = Arc::new(GatedReads::new(b"abcd", tail));
        let setup = setup(
            source.clone(),
            "lease-race",
            None,
            Some(Duration::from_secs(2)),
            None,
        )
        .await;
        let mut stream = open(&setup, "lease-race", 4).await;
        let attempt = stream.attempt();
        let lease = LeaseId::new("lease-race");
        match next(&mut stream, 2_000).await {
            ResourceStreamMessage::Chunk { seq, .. } => assert_eq!(seq, 0),
            other => panic!("expected the first chunk, got {other:?}"),
        }
        stream.grant(8).await.expect("credit");
        tokio::time::timeout(Duration::from_secs(2), async {
            while source.entered() < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the second read is pending");

        // Race the read result against the cancellation in both orders.
        if cancel_first {
            drop(stream);
            source.release();
        } else {
            source.release();
            drop(stream);
        }

        wait_for_event(&setup.provider_audit, terminal).await;
        // Let both racers settle, then insist the stream still ended once.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let terminals: Vec<FederationEvent> = setup
            .provider_audit
            .events()
            .into_iter()
            .filter(terminal)
            .collect();
        assert_eq!(terminals.len(), 1, "{label}/{cancel_first}: {terminals:?}");
        let event = &terminals[0];
        assert!(
            matches!(event.outcome.as_str(), "ok" | "cancelled"),
            "{label}/{cancel_first}: {event:?}"
        );
        assert_eq!(
            event.subject.as_deref(),
            Some(stream_subject(&lease, attempt).as_str()),
            "{label}/{cancel_first}"
        );
        if label == "eof" {
            assert_eq!(event.bytes, Some(4), "{label}/{cancel_first}");
        } else if event.outcome == "ok" {
            assert_eq!(event.bytes, Some(8), "both chunks were accepted");
        } else {
            assert!(
                matches!(event.bytes, Some(4) | Some(8)),
                "{label}/{cancel_first}: {event:?}"
            );
        }
        wait_for_sessions(&setup.provider, 0).await;
    }
}

/// Issue #347: a duplicate cancel is idempotent, and a cancel naming an older
/// attempt never stops the newer stream on the same lease.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_cancel_does_not_stop_a_newer_stream_and_duplicates_are_idempotent() {
    // The stream stalls after its last byte, so only the cancel can end it.
    let source = Arc::new(GatedBytes::new(b"abcdefgh", true));
    let setup = setup(
        source,
        "lease-generation",
        None,
        Some(Duration::from_secs(30)),
        None,
    )
    .await;
    let lease = LeaseId::new("lease-generation");

    let mut first = open(&setup, "lease-generation", 4).await;
    let first_attempt = first.attempt();
    match next(&mut first, 2_000).await {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 0);
            assert_eq!(bytes, b"abcd".to_vec());
        }
        other => panic!("expected the first chunk, got {other:?}"),
    }
    drop(first);
    wait_for_sessions(&setup.provider, 0).await;

    let mut second = open(&setup, "lease-generation", 4).await;
    let second_attempt = second.attempt();
    assert!(
        second_attempt > first_attempt,
        "a later open is a newer generation"
    );
    match next(&mut second, 2_000).await {
        ResourceStreamMessage::Chunk { seq, .. } => assert_eq!(seq, 0),
        other => panic!("expected the new stream first chunk, got {other:?}"),
    }

    // A cancel for the previous generation is ignored, and the live stream
    // still produces once it has credit.
    inject_cancel(&setup, "lease-generation", first_attempt).await;
    let stale = wait_for_event(&setup.provider_audit, |event| {
        event.operation == "resource"
            && event.outcome == "cancel_ignored"
            && event.subject.as_deref() == Some(stream_subject(&lease, first_attempt).as_str())
    })
    .await;
    assert!(
        stale
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("names attempt"),
        "{stale:?}"
    );
    second.grant(8).await.expect("credit");
    match next(&mut second, 2_000).await {
        ResourceStreamMessage::Chunk { seq, bytes, .. } => {
            assert_eq!(seq, 1, "the stale cancel did not stop the stream");
            assert_eq!(bytes, b"efgh".to_vec());
        }
        other => panic!("expected the second chunk, got {other:?}"),
    }

    // Duplicate cancels for the live generation: one stop, one terminal record.
    let expected = stream_subject(&lease, second_attempt);
    inject_cancel(&setup, "lease-generation", second_attempt).await;
    inject_cancel(&setup, "lease-generation", second_attempt).await;
    wait_for_sessions(&setup.provider, 0).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let events = setup.provider_audit.events();
    let terminals: Vec<&FederationEvent> = events
        .iter()
        .filter(|event| terminal(event) && event.subject.as_deref() == Some(expected.as_str()))
        .collect();
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].outcome, "cancelled");
    let accepted = events
        .iter()
        .filter(|event| {
            event.outcome == "cancel_accepted"
                && event.subject.as_deref() == Some(expected.as_str())
        })
        .count();
    assert_eq!(
        accepted, 1,
        "the duplicate cancel is idempotent: {events:?}"
    );
}

/// Issue #347: only a verified completion removes the checkpoint; a cancelled
/// stream resumes at the last acknowledged byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_stream_keeps_the_checkpoint_at_the_last_acknowledged_byte() {
    let dir = scratch("checkpoint");
    let path = dir.join("transfers.json");
    let source = Arc::new(GatedBytes::new(b"0123456789", false));
    let setup = setup(
        source,
        "lease-resume",
        None,
        Some(Duration::from_secs(30)),
        Some(path.clone()),
    )
    .await;
    let lease = LeaseId::new("lease-resume");

    let mut stream = open(&setup, "lease-resume", 4).await;
    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Chunk { bytes, .. } => assert_eq!(bytes, b"0123".to_vec()),
        other => panic!("expected the first chunk, got {other:?}"),
    }
    let store = federation::resource::TransferStore::load(&path).expect("checkpoint file");
    assert_eq!(store.position(&lease, URI), 4);

    drop(stream);
    wait_for_event(&setup.provider_audit, terminal).await;
    wait_for_sessions(&setup.provider, 0).await;

    let kept = federation::resource::TransferStore::load(&path).expect("checkpoint survives");
    assert_eq!(
        kept.position(&lease, URI),
        4,
        "a cancellation keeps the last acknowledged byte"
    );

    // The resume continues at that byte, and only the verified close removes
    // the checkpoint.
    let mut resumed = open(&setup, "lease-resume", 64).await;
    let mut collected = Vec::new();
    loop {
        match next(&mut resumed, 2_000).await {
            ResourceStreamMessage::Chunk { bytes, .. } => collected.extend_from_slice(&bytes),
            ResourceStreamMessage::Close { total, .. } => {
                assert_eq!(total, 10, "the close counts from the resource start");
                break;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
    assert_eq!(
        collected, b"456789",
        "the resumed stream continued at byte 4"
    );
    let cleared = federation::resource::TransferStore::load(&path).expect("checkpoint file");
    assert_eq!(
        cleared.position(&lease, URI),
        0,
        "a verified completion removes the checkpoint"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #347: a cancel the transport never accepted is recorded as an unknown
/// provider outcome, never as a stopped producer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_that_does_not_leave_the_node_is_recorded_as_unknown() {
    let source = Arc::new(GatedBytes::new(b"abcd", true));
    let setup = setup(
        source,
        "lease-unknown",
        None,
        Some(Duration::from_secs(30)),
        None,
    )
    .await;
    let mut stream = open(&setup, "lease-unknown", 4).await;
    let attempt = stream.attempt();
    match next(&mut stream, 2_000).await {
        ResourceStreamMessage::Chunk { seq, .. } => assert_eq!(seq, 0),
        other => panic!("expected the first chunk, got {other:?}"),
    }

    // The provider leaves the network, so the cancel frame cannot be accepted.
    setup.network.leave(&setup.provider_id);
    drop(stream);

    let caller = wait_for_event(&setup.caller_audit, terminal).await;
    assert_eq!(caller.outcome, "cancel_unconfirmed", "{caller:?}");
    assert_eq!(
        caller.subject.as_deref(),
        Some(stream_subject(LeaseId::new("lease-unknown"), attempt).as_str())
    );
    assert!(
        caller
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("unknown"),
        "{caller:?}"
    );
    assert!(
        !setup.provider_audit.events().iter().any(terminal),
        "the provider recorded nothing it was not told"
    );
}
