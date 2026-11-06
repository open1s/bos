//! Dual authorization: the caller's kernel and the provider's kernel both decide.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
use capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind,
    CapabilityOutput, CapabilityProvider, InvocationContext, InvocationId, NodeId, Principal,
    ProviderHealth, ProviderId, ProviderKind, VersionSource, WorkspaceId, WorkspaceRuntime,
};
use federation::{
    Direction, FederationAudit, FederationEvent, FederationNode, InMemoryNetwork, LeaseId,
    MemoryAudit, NodeIdentity, ResourceLease, ResourceSource,
};
use semver::Version;
use serde_json::json;

struct Echo {
    provider: ProviderId,
    descriptor: CapabilityDescriptor,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl CapabilityProvider for Echo {
    fn id(&self) -> &ProviderId {
        &self.provider
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![self.descriptor.clone()])
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(CapabilityOutput::json(call.input))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

fn identity(seed: u8) -> NodeIdentity {
    NodeIdentity::from_seed([seed; 32])
}

fn capability(name: &str) -> CapabilityId {
    CapabilityId::new("builtin", name).expect("a valid id")
}

fn descriptor(name: &str) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        capability(name),
        CapabilityKind::Tool,
        Version::new(1, 0, 0),
        VersionSource::Declared,
    )
}

fn workspace() -> WorkspaceId {
    WorkspaceId::new("ws:builder").expect("a valid workspace id")
}

fn runtime(descriptor: CapabilityDescriptor, calls: Arc<AtomicUsize>) -> Arc<WorkspaceRuntime> {
    let provider: Arc<dyn CapabilityProvider> = Arc::new(Echo {
        provider: ProviderId::builtin(),
        descriptor: descriptor.clone(),
        calls,
    });
    let plan = ResolvedComposition {
        id: "compose:builder".into(),
        version: Version::new(1, 0, 0),
        workspace: workspace(),
        capabilities: vec![ResolvedCapability {
            descriptor,
            binding: Binding::Local {
                provider: ProviderId::builtin(),
            },
        }],
        skipped: Vec::new(),
    };
    Arc::new(WorkspaceRuntime::instantiate(plan, &[provider]).expect("a runtime"))
}

struct EchoSource;

#[async_trait]
impl ResourceSource for EchoSource {
    async fn read(
        &self,
        _uri: &str,
        _offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError> {
        Ok(Some(vec![7; max.min(4)]))
    }
}

fn context() -> InvocationContext {
    let mut context = InvocationContext::local(
        InvocationId::new("inv:caller"),
        Principal::Local {
            session: "session".to_string(),
            preset: "preset".to_string(),
        },
        workspace(),
    );
    context.deadline = Some(Instant::now() + Duration::from_secs(5));
    context
}

struct Pair {
    caller: FederationNode,
    provider: FederationNode,
    calls: Arc<AtomicUsize>,
    caller_id: NodeId,
    provider_id: NodeId,
    caller_peers: Arc<std::sync::RwLock<federation::PeerStore>>,
    provider_peers: Arc<std::sync::RwLock<federation::PeerStore>>,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn pair() -> Pair {
    pair_with(None, None).await
}

async fn pair_with(
    caller_audit: Option<Arc<dyn FederationAudit>>,
    provider_audit: Option<Arc<dyn FederationAudit>>,
) -> Pair {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(21);
    let provider_identity = identity(22);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();

    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());

    let calls = Arc::new(AtomicUsize::new(0));
    let mut provider = FederationNode::new(provider_identity, Arc::new(provider_transport))
        .with_runtime(runtime(descriptor("echo"), Arc::clone(&calls)))
        .with_resource_source(Arc::new(EchoSource));
    if let Some(audit) = provider_audit {
        provider = provider.with_audit(audit);
    }
    let mut caller = FederationNode::new(caller_identity, Arc::new(caller_transport));
    if let Some(audit) = caller_audit {
        caller = caller.with_audit(audit);
    }

    let mut tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];

    let caller_peers = caller.peers();
    let provider_peers = provider.peers();
    let now = capability::unix_ms();
    {
        let mut store = caller_peers.write().expect("peer store");
        store.enroll(provider_id.clone(), now, None);
        store
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let mut store = provider_peers.write().expect("peer store");
        store.enroll(caller_id.clone(), now, None);
        store
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }

    // Let both routers enter their receive loops before the first frame.
    tokio::time::sleep(Duration::from_millis(20)).await;
    tasks.retain(|task| !task.is_finished());

    Pair {
        caller,
        provider,
        calls,
        caller_id,
        provider_id,
        caller_peers,
        provider_peers,
        _tasks: tasks,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_call_crosses_both_kernels() {
    let pair = pair().await;

    let output = pair
        .caller
        .invoke(
            &pair.provider_id,
            &capability("echo"),
            json!({ "value": 7 }),
            &context(),
        )
        .await
        .expect("both sides allow it");

    assert_eq!(output.structured, Some(json!({ "value": 7 })));
    assert_eq!(pair.calls.load(Ordering::SeqCst), 1);
    assert_eq!(pair.provider.node(), pair.provider_id);
    assert_eq!(pair.caller.node(), pair.caller_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_provider_refuses_a_caller_it_revoked() {
    let pair = pair().await;
    pair.provider_peers
        .write()
        .expect("peer store")
        .revoke(&pair.caller_id);

    let error = pair
        .caller
        .invoke(
            &pair.provider_id,
            &capability("echo"),
            json!({}),
            &context(),
        )
        .await
        .expect_err("the provider revoked the caller");

    assert!(
        matches!(error, CapabilityError::Denied { ref reason, .. } if reason.contains("revoked")),
        "{error:?}"
    );
    assert_eq!(pair.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_caller_refuses_before_sending_when_it_holds_no_grant() {
    let pair = pair().await;
    pair.caller_peers
        .write()
        .expect("peer store")
        .revoke_grant(&pair.provider_id, &capability("echo"));

    let error = pair
        .caller
        .invoke(
            &pair.provider_id,
            &capability("echo"),
            json!({}),
            &context(),
        )
        .await
        .expect_err("the caller has no grant");

    assert!(matches!(error, CapabilityError::Denied { .. }), "{error:?}");
    assert_eq!(
        pair.calls.load(Ordering::SeqCst),
        0,
        "a refused call must not reach the provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_capability_at_the_provider_is_not_found() {
    let pair = pair().await;
    let now = capability::unix_ms();
    pair.provider_peers
        .write()
        .expect("peer store")
        .grant(&pair.caller_id, capability("missing"), now, None)
        .expect("the caller is enrolled");
    pair.caller_peers
        .write()
        .expect("peer store")
        .grant(&pair.provider_id, capability("missing"), now, None)
        .expect("the peer is enrolled");

    let error = pair
        .caller
        .invoke(
            &pair.provider_id,
            &capability("missing"),
            json!({}),
            &context(),
        )
        .await
        .expect_err("the provider does not serve it");

    assert!(
        matches!(error, CapabilityError::NotFound { .. }),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_peer_is_a_transport_failure_before_send() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(23);
    let (transport, inbox) = network.join(caller_identity.node_id());
    let caller = FederationNode::new(caller_identity, Arc::new(transport));
    let task = caller.start(inbox);
    tokio::time::sleep(Duration::from_millis(10)).await;

    let ghost = identity(24).node_id();
    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(ghost.clone(), now, None);
        store
            .grant(&ghost, capability("echo"), now, None)
            .expect("granted");
    }

    let error = caller
        .invoke(&ghost, &capability("echo"), json!({}), &context())
        .await
        .expect_err("the peer is not on the network");

    assert!(
        matches!(
            error,
            CapabilityError::Transport {
                delivered: capability::DeliveryState::NotSent,
                ..
            }
        ),
        "{error:?}"
    );
    task.abort();
}

struct BlackHole;

#[async_trait]
impl capability::FederationTransport for BlackHole {
    async fn connect(&self, peer: &NodeId) -> Result<capability::PeerSession, CapabilityError> {
        Ok(capability::PeerSession {
            peer: peer.clone(),
            opened_at_ms: capability::unix_ms(),
        })
    }

    async fn send(
        &self,
        _peer: &NodeId,
        _frame: Vec<u8>,
    ) -> Result<capability::DeliveryState, CapabilityError> {
        Ok(capability::DeliveryState::Sent)
    }

    async fn close(&self, _peer: &NodeId) -> Result<(), CapabilityError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sent_request_with_no_reply_is_an_unknown_outcome() {
    let caller_identity = identity(25);
    let caller = FederationNode::new(caller_identity, Arc::new(BlackHole));
    let peer = identity(26).node_id();
    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut store = store.write().expect("peer store");
        store.enroll(peer.clone(), now, None);
        store
            .grant(&peer, capability("echo"), now, None)
            .expect("granted");
    }
    let mut context = context();
    context.deadline = Some(Instant::now() + Duration::from_millis(50));

    let error = caller
        .invoke(&peer, &capability("echo"), json!({}), &context)
        .await
        .expect_err("the frame is sent and nothing answers");

    assert!(
        matches!(error, CapabilityError::UnknownOutcome { .. }),
        "{error:?}"
    );
}

/// Every decision leaves a record on the side that made it, and a record never
/// carries the payload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_sides_audit_an_invocation_and_its_refusal() {
    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let pair = pair_with(Some(caller_audit.clone()), Some(provider_audit.clone())).await;

    pair.caller
        .invoke(
            &pair.provider_id,
            &capability("echo"),
            json!({ "value": 7 }),
            &context(),
        )
        .await
        .expect("granted");

    let outbound = caller_audit.events();
    let sent = outbound
        .iter()
        .find(|event| event.operation == "invoke")
        .expect("the caller recorded its call");
    assert_eq!(sent.direction, Direction::Outbound);
    assert_eq!(sent.outcome, "ok");
    assert_eq!(sent.capability.as_deref(), Some("builtin/echo"));
    assert!(sent.elapsed_ms.is_some());

    let inbound = provider_audit.events();
    let served = inbound
        .iter()
        .find(|event| event.operation == "invoke")
        .expect("the provider recorded its decision");
    assert_eq!(served.direction, Direction::Inbound);
    assert_eq!(served.outcome, "ok");

    // The provider revokes the caller. The refusal is recorded with a reason,
    // and neither record contains the argument that was refused.
    pair.provider_peers
        .write()
        .expect("peers")
        .revoke(&pair.caller_id);
    let denied = pair
        .caller
        .invoke(
            &pair.provider_id,
            &capability("echo"),
            json!({ "value": 99 }),
            &context(),
        )
        .await
        .expect_err("revoked");
    assert!(
        matches!(denied, CapabilityError::Denied { .. }),
        "{denied:?}"
    );

    let inbound = provider_audit.events();
    let refusal = inbound.last().expect("a record");
    assert_eq!(refusal.direction, Direction::Inbound);
    assert_eq!(refusal.outcome, "denied");
    assert!(
        refusal
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("revoked"),
        "{refusal:?}"
    );
    let serialized = serde_json::to_string(&inbound).expect("serialized");
    assert!(!serialized.contains("\"value\":99"), "{serialized}");
    assert!(!serialized.contains("\"input\""), "{serialized}");

    let outbound = caller_audit.events();
    let sent = outbound.last().expect("a record");
    assert_eq!(sent.outcome, "denied");
}

/// A delegation leaves a record on both sides, subject names the delegation,
/// and a cancelled delegation records its own decision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_sides_audit_a_delegation_and_its_cancellation() {
    use federation::{DelegationId, DelegationRequest};

    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let pair = pair_with(Some(caller_audit.clone()), Some(provider_audit.clone())).await;

    let request = DelegationRequest {
        id: DelegationId::new("d-audit"),
        parent: None,
        target: pair.provider_id.clone(),
        objective: "inspect the tree".to_string(),
        capabilities: vec![capability("echo")],
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: None,
    };
    pair.caller
        .delegate(&pair.provider_id, request, &context())
        .await
        .expect("accepted");

    let sent = caller_audit.events();
    let event = sent.last().expect("a caller record");
    assert_eq!(event.operation, "delegate");
    assert_eq!(event.outcome, "ok");
    assert_eq!(event.subject.as_deref(), Some("d-audit"));

    let served = provider_audit.events();
    let event = served.last().expect("a provider record");
    assert_eq!(event.direction, Direction::Inbound);
    assert_eq!(event.operation, "delegate");
    assert_eq!(event.outcome, "ok");
    assert_eq!(event.subject.as_deref(), Some("d-audit"));

    pair.caller
        .cancel_delegation(&pair.provider_id, &DelegationId::new("d-audit"), &context())
        .await
        .expect("cancelled");

    let sent = caller_audit.events();
    let event = sent.last().expect("a caller record");
    assert_eq!(event.operation, "delegation_cancel");
    assert_eq!(event.outcome, "ok");
    assert_eq!(event.subject.as_deref(), Some("d-audit"));

    let served = provider_audit.events();
    let event = served.last().expect("a provider record");
    assert_eq!(event.operation, "delegation_cancel");
    assert_eq!(event.outcome, "ok");

    // Revoked before a new delegation: the refusal is a record, not a silence.
    pair.provider_peers
        .write()
        .expect("peers")
        .revoke(&pair.caller_id);
    let request = DelegationRequest {
        id: DelegationId::new("d-refused"),
        parent: None,
        target: pair.provider_id.clone(),
        objective: "should be refused".to_string(),
        capabilities: vec![capability("echo")],
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: None,
    };
    pair.caller
        .delegate(&pair.provider_id, request, &context())
        .await
        .expect_err("revoked");

    let served = provider_audit.events();
    let refusal = served.last().expect("a provider record");
    assert_eq!(refusal.operation, "delegate");
    assert_eq!(refusal.outcome, "denied");
    assert_eq!(refusal.subject.as_deref(), Some("d-refused"));
    assert!(
        refusal
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("not enrolled"),
        "{refusal:?}"
    );
}

async fn wait_for<F>(audit: &MemoryAudit, predicate: F) -> FederationEvent
where
    F: Fn(&FederationEvent) -> bool,
{
    for _ in 0..100 {
        if let Some(event) = audit
            .events()
            .iter()
            .find(|event| predicate(event))
            .cloned()
        {
            return event;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no matching federation event");
}

/// A resource open leaves a record on both sides; a lease the provider does not
/// hold for the caller is recorded as not found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_sides_audit_a_resource_open() {
    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let pair = pair_with(Some(caller_audit.clone()), Some(provider_audit.clone())).await;

    let lease = ResourceLease {
        id: LeaseId::new("lease-audit"),
        capability: capability("echo"),
        holder: pair.caller_id.clone(),
        granted_at_ms: capability::unix_ms(),
        expires_at_ms: None,
        max_bytes: Some(16),
        used_bytes: 0,
    };
    pair.provider
        .leases()
        .write()
        .expect("leases")
        .grant(lease)
        .expect("granted");

    let _stream = pair
        .caller
        .open_resource(
            &pair.provider_id,
            &LeaseId::new("lease-audit"),
            "file:///artifact",
            0,
            64,
        )
        .await
        .expect("the open is sent");

    let sent = wait_for(&caller_audit, |event| event.operation == "resource").await;
    assert_eq!(sent.direction, Direction::Outbound);
    assert_eq!(sent.outcome, "ok");
    assert_eq!(sent.subject.as_deref(), Some("lease-audit"));

    let served = wait_for(&provider_audit, |event| event.operation == "resource").await;
    assert_eq!(served.direction, Direction::Inbound);
    assert_eq!(served.outcome, "ok");
    assert_eq!(served.subject.as_deref(), Some("lease-audit"));

    // A lease the provider never granted: the open is sent, the provider
    // refuses it, and the refusal is recorded with the lease id.
    let _stream = pair
        .caller
        .open_resource(
            &pair.provider_id,
            &LeaseId::new("lease-missing"),
            "file:///artifact",
            0,
            8,
        )
        .await
        .expect("the open is sent");

    let refusal = wait_for(&provider_audit, |event| {
        event.operation == "resource" && event.subject.as_deref() == Some("lease-missing")
    })
    .await;
    assert_eq!(refusal.outcome, "not_found");
}

/// A stream that stops on its lease quota records where it stopped and how
/// many bytes it moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_quota_exhausted_stream_records_its_end() {
    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let pair = pair_with(Some(caller_audit.clone()), Some(provider_audit.clone())).await;

    let lease = ResourceLease {
        id: LeaseId::new("lease-quota"),
        capability: capability("echo"),
        holder: pair.caller_id.clone(),
        granted_at_ms: capability::unix_ms(),
        expires_at_ms: None,
        max_bytes: Some(4),
        used_bytes: 0,
    };
    pair.provider
        .leases()
        .write()
        .expect("leases")
        .grant(lease)
        .expect("granted");

    let mut stream = pair
        .caller
        .open_resource(
            &pair.provider_id,
            &LeaseId::new("lease-quota"),
            "file:///artifact",
            0,
            64,
        )
        .await
        .expect("the open is sent");

    let event = wait_for(&provider_audit, |event| {
        event.operation == "resource" && event.outcome == "quota_exceeded"
    })
    .await;
    assert_eq!(event.subject.as_deref(), Some("lease-quota"));
    assert_eq!(event.bytes, Some(4), "exactly the quota moved");
    assert!(
        event
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("0 bytes left"),
        "{event:?}"
    );

    // The caller sees the same ending, with the bytes it actually collected.
    let mut collected = 0u64;
    loop {
        match stream.next().await {
            Some(federation::ResourceStreamMessage::Chunk { bytes, .. }) => {
                collected += bytes.len() as u64;
            }
            Some(federation::ResourceStreamMessage::Error { .. })
            | Some(federation::ResourceStreamMessage::Close { .. }) => break,
            Some(_) => {}
            None => break,
        }
    }
    assert_eq!(collected, 4);
    let event = wait_for(&caller_audit, |event| {
        event.operation == "resource" && event.outcome == "error"
    })
    .await;
    assert_eq!(event.direction, Direction::Outbound);
    assert_eq!(event.subject.as_deref(), Some("lease-quota"));
    assert_eq!(event.bytes, Some(4));
}

/// Discovery is audited on both sides, and a revoked caller's discovery is a
/// recorded refusal on the provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_sides_audit_a_discovery() {
    let caller_audit = Arc::new(MemoryAudit::new());
    let provider_audit = Arc::new(MemoryAudit::new());
    let pair = pair_with(Some(caller_audit.clone()), Some(provider_audit.clone())).await;

    pair.caller
        .discover(&pair.provider_id)
        .await
        .expect("the peer answers");

    let sent = caller_audit.events();
    let event = sent.last().expect("a caller record");
    assert_eq!(event.operation, "discover");
    assert_eq!(event.outcome, "ok");

    let served = provider_audit.events();
    let event = served.last().expect("a provider record");
    assert_eq!(event.direction, Direction::Inbound);
    assert_eq!(event.operation, "discover");
    assert_eq!(event.outcome, "ok");

    pair.provider_peers
        .write()
        .expect("peers")
        .revoke(&pair.caller_id);
    pair.caller
        .discover(&pair.provider_id)
        .await
        .expect_err("revoked");

    let served = provider_audit.events();
    let refusal = served.last().expect("a provider record");
    assert_eq!(refusal.operation, "discover");
    assert_eq!(refusal.outcome, "denied");
}
