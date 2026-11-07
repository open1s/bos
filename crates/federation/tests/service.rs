//! Dual authorization: the caller's kernel and the provider's kernel both decide.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::composition::{Binding, ResolvedCapability, ResolvedComposition};
use capability::{
    CapabilityCall, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityKind,
    CapabilityOutput, CapabilityProvider, DeliveryState, FederationTransport, InvocationContext,
    InvocationId, NodeId, Principal, ProviderHealth, ProviderId, ProviderKind, VersionSource,
    WorkspaceId, WorkspaceRuntime,
};
use federation::{
    AdmissionLimits, Direction, FederationAudit, FederationEvent, FederationMessage,
    FederationNode, InMemoryNetwork, LeaseId, MemoryAudit, NodeIdentity, ResourceLease,
    ResourceSource,
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
    runtime_with(descriptor, provider)
}

fn runtime_with(
    descriptor: CapabilityDescriptor,
    provider: Arc<dyn CapabilityProvider>,
) -> Arc<WorkspaceRuntime> {
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
        budgets: capability::composition::Budgets::default(),
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
    pair_with_limits(caller_audit, provider_audit, AdmissionLimits::default()).await
}

async fn pair_with_limits(
    caller_audit: Option<Arc<dyn FederationAudit>>,
    provider_audit: Option<Arc<dyn FederationAudit>>,
    limits: AdmissionLimits,
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
        .with_resource_source(Arc::new(EchoSource))
        .with_admission_limits(limits);
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

/// A structured delegation runs on the target's runtime and settles the record
/// the caller can then read back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_structured_delegation_is_executed_by_the_target() {
    use federation::{DelegationId, DelegationRequest, DelegationState};

    let pair = pair().await;
    let request = DelegationRequest {
        id: DelegationId::new("d-exec"),
        parent: None,
        target: pair.provider_id.clone(),
        objective: "echo the value".to_string(),
        capabilities: vec![capability("echo")],
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: Some(
            json!({ "capability": "builtin/echo", "input": { "value": 9 } }).to_string(),
        ),
    };

    let accepted = pair
        .caller
        .delegate(&pair.provider_id, request, &context())
        .await
        .expect("the target accepts");
    assert_eq!(accepted.state, DelegationState::Accepted);

    let id = DelegationId::new("d-exec");
    let mut record = accepted.clone();
    for _ in 0..100 {
        record = pair
            .caller
            .delegation_status(&pair.provider_id, &id, &context())
            .await
            .expect("status answers");
        if record.state.is_terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(record.state, DelegationState::Succeeded, "{record:?}");
    let result = record.result.expect("a stored result");
    assert_eq!(result["structured"], json!({ "value": 9 }));
    assert_eq!(pair.calls.load(Ordering::SeqCst), 1);
}

/// A contract may not reach past the capabilities the caller delegated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contract_outside_the_delegated_scope_is_refused() {
    use federation::{DelegationId, DelegationRequest, DelegationState};

    let pair = pair().await;
    let request = DelegationRequest {
        id: DelegationId::new("d-scope"),
        parent: None,
        target: pair.provider_id.clone(),
        objective: "try to escape the scope".to_string(),
        capabilities: vec![capability("echo")],
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: Some(json!({ "capability": "builtin/other", "input": {} }).to_string()),
    };

    let record = pair
        .caller
        .delegate(&pair.provider_id, request, &context())
        .await
        .expect("the target accepts and then refuses the contract");

    assert_eq!(record.state, DelegationState::Failed);
    assert!(
        record
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("outside the delegated scope"),
        "{record:?}"
    );
    assert_eq!(pair.calls.load(Ordering::SeqCst), 0);
}

/// An objective without a structured contract is recorded and left queued; a
/// node with no language-model executor must not pretend it ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_objective_only_delegation_stays_accepted() {
    use federation::{DelegationId, DelegationRequest, DelegationState};

    let pair = pair().await;
    let request = DelegationRequest {
        id: DelegationId::new("d-prose"),
        parent: None,
        target: pair.provider_id.clone(),
        objective: "write a poem".to_string(),
        capabilities: vec![capability("echo")],
        deadline_ms: None,
        allow_sub_delegation: false,
        result_contract: None,
    };

    let record = pair
        .caller
        .delegate(&pair.provider_id, request, &context())
        .await
        .expect("the target accepts");

    assert_eq!(record.state, DelegationState::Accepted);
    assert_eq!(pair.calls.load(Ordering::SeqCst), 0);
}

/// A provider that never finishes, so its handler stays admitted.
struct Hanging;

#[async_trait]
impl CapabilityProvider for Hanging {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![descriptor("echo")])
    }

    async fn invoke(&self, _call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        std::future::pending().await
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

/// The ninth concurrent request from one peer is refused with a retryable
/// overload instead of spawning a ninth handler.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_beyond_its_handler_budget_gets_a_retryable_refusal() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(81);
    let provider_identity = identity(82);
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
            .with_runtime(runtime_with(descriptor("echo"), Arc::new(Hanging))),
    );
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut hung = Vec::new();
    for _ in 0..federation::MAX_IN_FLIGHT_PER_PEER {
        let caller = Arc::clone(&caller);
        let peer = provider_id.clone();
        hung.push(tokio::spawn(async move {
            caller
                .invoke(&peer, &capability("echo"), json!({}), &context())
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(80)).await;

    let refused = caller
        .invoke(&provider_id, &capability("echo"), json!({}), &context())
        .await;
    assert!(
        matches!(refused, Err(CapabilityError::RateLimited { .. })),
        "the over-budget request is refused, not spawned: {refused:?}"
    );

    for task in hung {
        task.abort();
    }
    caller_task.abort();
    provider_task.abort();
}

/// A caller and a provider whose invoke never returns, for timeout/cancel tests.
async fn hanging_pair() -> (
    Arc<FederationNode>,
    Arc<FederationNode>,
    NodeId,
    Vec<tokio::task::JoinHandle<()>>,
) {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(93);
    let provider_identity = identity(94);
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
            .with_runtime(runtime_with(descriptor("echo"), Arc::new(Hanging))),
    );
    let tasks = vec![caller.start(caller_inbox), provider.start(provider_inbox)];
    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    (caller, provider, provider_id, tasks)
}

/// Concurrent requests created in the same millisecond must not overwrite one
/// another's waiter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_round_trips_do_not_collide() {
    let pair = pair().await;
    let provider_id = pair.provider_id.clone();
    let caller = Arc::new(pair.caller);
    let mut tasks = Vec::new();
    for value in 0..8 {
        let caller = Arc::clone(&caller);
        let provider = provider_id.clone();
        tasks.push(tokio::spawn(async move {
            caller
                .invoke(
                    &provider,
                    &capability("echo"),
                    json!({ "value": value }),
                    &context(),
                )
                .await
        }));
    }
    for task in tasks {
        let output = task.await.expect("joined").expect("answered");
        assert!(output.structured.is_some(), "{output:?}");
    }
    assert_eq!(caller.pending_requests(), 0);
}

/// A send failure removes the waiter instead of leaking it.
#[tokio::test]
async fn a_failed_send_leaves_no_pending_waiter() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(95);
    let caller_id = caller_identity.node_id();
    let (transport, inbox) = network.join(caller_id.clone());
    let caller = FederationNode::new(caller_identity, Arc::new(transport));
    let task = caller.start(inbox);

    let absent = identity(96).node_id();
    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(absent.clone(), now, None);
        peers
            .grant(&absent, capability("echo"), now, None)
            .expect("granted");
    }

    let error = caller
        .invoke(&absent, &capability("echo"), json!({}), &context())
        .await
        .expect_err("the peer is not on the network");
    assert!(
        matches!(error, CapabilityError::Transport { .. }),
        "{error:?}"
    );
    assert_eq!(caller.pending_requests(), 0);
    task.abort();
}

/// A timeout removes the waiter; the remote call may still run, which the error
/// states explicitly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timeout_leaves_no_pending_waiter() {
    let (caller, _provider, provider_id, tasks) = hanging_pair().await;
    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(50));

    let error = caller
        .invoke(&provider_id, &capability("echo"), json!({}), &short)
        .await
        .expect_err("the provider never answers");
    // The provider enforces the same absolute deadline, so the call may end
    // with a definitive Timeout instead of the caller's UnknownOutcome; both
    // are failures and both must clear the waiter.
    assert!(
        matches!(
            error,
            CapabilityError::UnknownOutcome { .. } | CapabilityError::Timeout { .. }
        ),
        "{error:?}"
    );
    assert_eq!(caller.pending_requests(), 0);
    for task in tasks {
        task.abort();
    }
}

/// Dropping the caller's future removes its waiter too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_caller_leaves_no_pending_waiter() {
    let (caller, _provider, provider_id, tasks) = hanging_pair().await;
    let future = {
        let caller = Arc::clone(&caller);
        tokio::spawn(async move {
            caller
                .invoke(&provider_id, &capability("echo"), json!({}), &context())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    future.abort();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(caller.pending_requests(), 0);
    for task in tasks {
        task.abort();
    }
}

/// A provider that watches its invocation's cancellation and records the
/// deadline it was handed.
struct Watching {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    remaining_ms: Arc<std::sync::Mutex<Option<u64>>>,
}

#[async_trait]
impl CapabilityProvider for Watching {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(vec![descriptor("echo")])
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        let remaining = call.context.deadline.map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as u64
        });
        *self.remaining_ms.lock().expect("remaining") = remaining;
        loop {
            if call.context.cancelled() {
                self.cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(CapabilityError::Cancelled);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

/// The caller's absolute deadline reaches the provider, and giving up sends a
/// cancellation the provider observes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_call_cancels_the_remote_provider() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(97);
    let provider_identity = identity(98);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());

    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let remaining = Arc::new(std::sync::Mutex::new(None));
    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport)).with_runtime(
            runtime_with(
                descriptor("echo"),
                Arc::new(Watching {
                    cancelled: Arc::clone(&cancelled),
                    remaining_ms: Arc::clone(&remaining),
                }),
            ),
        ),
    );
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(80));
    let error = caller
        .invoke(&provider_id, &capability("echo"), json!({}), &short)
        .await
        .expect_err("the deadline expires first");
    // The provider's workspace enforces the propagated absolute deadline, so
    // the provider future can be stopped by that timeout before the caller's
    // cancellation frame arrives. Either outcome stops the work; the deadline
    // itself must still have reached the provider.
    assert!(
        matches!(
            error,
            CapabilityError::UnknownOutcome { .. } | CapabilityError::Timeout { .. }
        ),
        "{error:?}"
    );

    let seen = *remaining.lock().expect("remaining");
    assert!(
        seen.is_some_and(|ms| ms <= 150),
        "the provider was handed the caller's deadline: {seen:?}"
    );
    for _ in 0..200 {
        if provider.in_flight_invocations() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(provider.in_flight_invocations(), 0);
    assert_eq!(caller.pending_requests(), 0);

    caller_task.abort();
    provider_task.abort();
}

/// A captured signed frame is dispatched once; a replay gets a deterministic
/// duplicate outcome and the provider does not run again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_frame_is_not_dispatched_twice() {
    use capability::MessageId;

    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(101);
    let provider_identity = identity(102);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, _caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let audit = Arc::new(MemoryAudit::new());
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(runtime(descriptor("echo"), Arc::clone(&calls)))
            .with_audit(audit.clone()),
    );
    let task = provider.start(provider_inbox);
    let now = capability::unix_ms();
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let payload = json!({ "capability": "builtin/echo", "input": { "value": 1 } });
    let message = |id: &str| {
        let mut message =
            FederationMessage::new("invoke", serde_json::to_vec(&payload).expect("bytes"));
        message.id = MessageId::new(id);
        message
    };
    let frame = |message: FederationMessage| {
        federation::Envelope::seal(&message, &caller_identity, &provider_id)
            .expect("sealed")
            .encode()
            .expect("encoded")
    };

    let duplicate = frame(message("m-once"));
    caller_transport
        .send(&provider_id, duplicate.clone())
        .await
        .expect("first send");
    caller_transport
        .send(&provider_id, duplicate)
        .await
        .expect("replay send");

    // A retry with a fresh id but the same idempotency key is the same logical
    // operation and is refused too.
    let mut keyed = message("m-key-1");
    keyed.idempotency_key = Some("op-1".to_string());
    let mut retry = message("m-key-2");
    retry.idempotency_key = Some("op-1".to_string());
    caller_transport
        .send(&provider_id, frame(keyed))
        .await
        .expect("keyed send");
    caller_transport
        .send(&provider_id, frame(retry))
        .await
        .expect("key retry send");

    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one call per logical request"
    );
    let duplicates = audit
        .events()
        .iter()
        .filter(|event| event.outcome == "duplicate")
        .count();
    assert_eq!(
        duplicates,
        2,
        "both replays were audited: {:?}",
        audit.events()
    );

    task.abort();
}

/// A transport that remembers the last frame it sent, so a test can learn the
/// correlation of a request that is still in flight.
struct Capturing {
    inner: federation::InMemoryTransport,
    captured: Arc<std::sync::Mutex<Option<Vec<u8>>>>,
}

#[async_trait]
impl FederationTransport for Capturing {
    async fn connect(&self, peer: &NodeId) -> Result<federation::PeerSession, CapabilityError> {
        self.inner.connect(peer).await
    }

    async fn send(&self, peer: &NodeId, frame: Vec<u8>) -> Result<DeliveryState, CapabilityError> {
        if let Ok(mut slot) = self.captured.lock() {
            *slot = Some(frame.clone());
        }
        self.inner.send(peer, frame).await
    }

    async fn close(&self, peer: &NodeId) -> Result<(), CapabilityError> {
        self.inner.close(peer).await
    }
}

/// A signed frame addressed to another Node is rejected before dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_recipient_frame_is_rejected() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(111);
    let provider_identity = identity(112);
    let stranger = identity(113).node_id();
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, _caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let audit = Arc::new(MemoryAudit::new());
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(runtime(descriptor("echo"), Arc::clone(&calls)))
            .with_audit(audit.clone()),
    );
    let task = provider.start(provider_inbox);
    let now = capability::unix_ms();
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let payload = json!({ "capability": "builtin/echo", "input": {} });
    let message = FederationMessage::new("invoke", serde_json::to_vec(&payload).expect("bytes"));
    let envelope =
        federation::Envelope::seal(&message, &caller_identity, &stranger).expect("sealed");
    caller_transport
        .send(&provider_id, envelope.encode().expect("encoded"))
        .await
        .expect("sent to the wrong inbox");
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the frame was not dispatched"
    );
    assert!(
        audit.events().iter().any(|event| {
            event.outcome == "denied"
                && event
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("another Node"))
        }),
        "the wrong recipient is audited: {:?}",
        audit.events()
    );
    task.abort();
}

/// A signed result from a Node other than the one asked cannot satisfy a
/// pending request, and does not consume its waiter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_result_from_another_peer_does_not_satisfy_a_request() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(121);
    let provider_identity = identity(122);
    let attacker_identity = identity(123);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let attacker_id = attacker_identity.node_id();
    let (caller_inner, caller_inbox) = network.join(caller_id.clone());
    let captured = Arc::new(std::sync::Mutex::new(None));
    let caller_transport = Arc::new(Capturing {
        inner: caller_inner,
        captured: Arc::clone(&captured),
    });
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let (attacker_transport, attacker_inbox) = network.join(attacker_id.clone());

    let caller = Arc::new(FederationNode::new(caller_identity, caller_transport));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport))
            .with_runtime(runtime_with(descriptor("echo"), Arc::new(Hanging))),
    );
    let attacker = Arc::new(FederationNode::new(
        attacker_identity.clone(),
        Arc::new(attacker_transport),
    ));
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);
    let attacker_task = attacker.start(attacker_inbox);

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(300));
    let call = {
        let caller = Arc::clone(&caller);
        tokio::spawn(async move {
            caller
                .invoke(&provider_id, &capability("echo"), json!({}), &short)
                .await
        })
    };

    // Learn the live correlation from the request the caller sent.
    let correlation = loop {
        if let Some(frame) = captured.lock().expect("captured").clone() {
            let envelope = federation::Envelope::decode(&frame).expect("decoded");
            break envelope.correlation.clone().expect("a correlation");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };

    // The attacker is enrolled and can sign, but it is not the requested peer.
    let payload =
        json!({ "ok": true, "output": { "content": [], "structured": { "forged": true } } });
    let message = FederationMessage::new("result", serde_json::to_vec(&payload).expect("bytes"))
        .with_correlation(&correlation);
    let forged =
        federation::Envelope::seal(&message, &attacker_identity, &caller_id).expect("sealed");
    let (attacker_out, _keepalive) = network.join(attacker_id.clone());
    attacker_out
        .send(&caller_id, forged.encode().expect("encoded"))
        .await
        .expect("sent");
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        caller.pending_requests(),
        1,
        "the legitimate waiter was not consumed"
    );
    let outcome = call
        .await
        .expect("joined")
        .expect_err("still no real reply");
    // The forged result must not satisfy the waiter. Once the deadline also
    // reaches the provider, the failure may be a definitive Timeout rather
    // than the caller's UnknownOutcome; either way the forged result is not
    // the outcome.
    assert!(
        matches!(
            outcome,
            CapabilityError::UnknownOutcome { .. } | CapabilityError::Timeout { .. }
        ),
        "{outcome:?}"
    );
    assert_eq!(caller.pending_requests(), 0);

    caller_task.abort();
    provider_task.abort();
    attacker_task.abort();
}

struct Forward {
    target: std::sync::Weak<FederationNode>,
    target_id: NodeId,
    capability: CapabilityId,
}

#[async_trait]
impl CapabilityProvider for Forward {
    fn id(&self) -> &capability::ProviderId {
        static ID: std::sync::OnceLock<capability::ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(capability::ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        let target = self
            .target
            .upgrade()
            .ok_or_else(|| CapabilityError::ProviderFailed {
                message: "the forward target is gone".to_string(),
            })?;
        target
            .invoke(
                &self.target_id,
                &self.capability,
                call.input.clone(),
                call.context,
            )
            .await
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

/// A call through a proxy Node must hand the final provider the same absolute
/// deadline, never a freshly started one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forwarded_call_preserves_the_earliest_deadline() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(140);
    let proxy_identity = identity(141);
    let provider_identity = identity(142);
    let caller_id = caller_identity.node_id();
    let proxy_id = proxy_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (proxy_transport, proxy_inbox) = network.join(proxy_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());

    let remaining = Arc::new(std::sync::Mutex::new(None));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport)).with_runtime(
            runtime_with(
                descriptor("echo"),
                Arc::new(Watching {
                    cancelled: Arc::clone(&cancelled),
                    remaining_ms: Arc::clone(&remaining),
                }),
            ),
        ),
    );
    let proxy = Arc::new_cyclic(|weak| {
        FederationNode::new(proxy_identity, Arc::new(proxy_transport)).with_runtime(runtime_with(
            descriptor("echo"),
            Arc::new(Forward {
                target: weak.clone(),
                target_id: provider_id.clone(),
                capability: capability("echo"),
            }),
        ))
    });
    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));

    let caller_task = caller.start(caller_inbox);
    let proxy_task = proxy.start(proxy_inbox);
    let provider_task = provider.start(provider_inbox);

    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(proxy_id.clone(), now, None);
        peers
            .grant(&proxy_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = proxy.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(proxy_id.clone(), now, None);
        peers
            .grant(&proxy_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut short = context();
    short.deadline = Some(Instant::now() + Duration::from_millis(250));
    let error = caller
        .invoke(&proxy_id, &capability("echo"), json!({}), &short)
        .await
        .expect_err("the deadline expires at the provider");
    assert!(
        matches!(
            error,
            CapabilityError::UnknownOutcome { .. } | CapabilityError::Timeout { .. }
        ),
        "{error:?}"
    );

    let seen = *remaining.lock().expect("remaining");
    assert!(
        seen.is_some_and(|ms| ms > 0 && ms <= 260),
        "the forwarded deadline was attenuated: {seen:?}"
    );

    for _ in 0..200 {
        if provider.in_flight_invocations() == 0 && proxy.in_flight_invocations() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(provider.in_flight_invocations(), 0);
    assert_eq!(proxy.in_flight_invocations(), 0);

    caller_task.abort();
    proxy_task.abort();
    provider_task.abort();
}

/// Resource streams have their own budget: one stream is admitted, the second
/// is refused with a retryable error while the first stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_resource_stream_budget_is_separate_and_configurable() {
    let pair = pair_with_limits(
        None,
        None,
        AdmissionLimits {
            resource_streams: 1,
            ..AdmissionLimits::default()
        },
    )
    .await;
    let now = capability::unix_ms();
    for id in ["lease-one", "lease-two"] {
        pair.provider
            .leases()
            .write()
            .expect("leases")
            .grant(ResourceLease {
                id: LeaseId::new(id),
                capability: capability("echo"),
                holder: pair.caller_id.clone(),
                granted_at_ms: now,
                expires_at_ms: None,
                max_bytes: Some(64),
                used_bytes: 0,
            })
            .expect("granted");
    }

    let mut first = pair
        .caller
        .open_resource(
            &pair.provider_id,
            &LeaseId::new("lease-one"),
            "file:///artifact",
            0,
            16,
        )
        .await
        .expect("the first open is sent");
    // The first open must be admitted before the second reaches the provider.
    let first_message = tokio::time::timeout(Duration::from_secs(1), first.next())
        .await
        .expect("the first stream answers");
    assert!(matches!(
        first_message,
        Some(federation::ResourceStreamMessage::Chunk { .. })
            | Some(federation::ResourceStreamMessage::Close { .. })
    ));

    let mut second = pair
        .caller
        .open_resource(
            &pair.provider_id,
            &LeaseId::new("lease-two"),
            "file:///artifact",
            0,
            16,
        )
        .await
        .expect("the second open is sent");
    let refusal = tokio::time::timeout(Duration::from_secs(1), second.next())
        .await
        .expect("the second stream answers");
    match refusal {
        Some(federation::ResourceStreamMessage::Error { message, .. }) => {
            assert!(
                message.contains("too many concurrent resource streams"),
                "{message}"
            );
        }
        other => panic!("unexpected resource response: {other:?}"),
    }
}

#[derive(Default)]
struct SeenContext {
    task: Option<String>,
    trace: (String, String),
    delegation: Vec<(String, String)>,
    remote_principal: bool,
}

struct ContextProbe {
    seen: Arc<std::sync::Mutex<SeenContext>>,
}

#[async_trait]
impl CapabilityProvider for ContextProbe {
    fn id(&self) -> &ProviderId {
        static ID: std::sync::OnceLock<ProviderId> = std::sync::OnceLock::new();
        ID.get_or_init(ProviderId::builtin)
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Builtin
    }

    async fn describe(&self) -> Result<Vec<CapabilityDescriptor>, CapabilityError> {
        Ok(Vec::new())
    }

    async fn invoke(&self, call: CapabilityCall<'_>) -> Result<CapabilityOutput, CapabilityError> {
        let mut seen = self.seen.lock().expect("seen");
        seen.task = call
            .context
            .task
            .as_ref()
            .map(|task| task.as_str().to_string());
        seen.trace = (
            call.context.trace.trace_id.clone(),
            call.context.trace.span_id.clone(),
        );
        seen.delegation = call
            .context
            .delegation
            .iter()
            .map(|hop| (hop.node.to_string(), hop.delegation.clone()))
            .collect();
        seen.remote_principal = matches!(call.context.principal, Principal::Remote { .. });
        Ok(CapabilityOutput::text("probed"))
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Available
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_trace_and_delegation_reach_the_remote_provider() {
    let network = Arc::new(InMemoryNetwork::new());
    let caller_identity = identity(160);
    let provider_identity = identity(161);
    let caller_id = caller_identity.node_id();
    let provider_id = provider_identity.node_id();
    let (caller_transport, caller_inbox) = network.join(caller_id.clone());
    let (provider_transport, provider_inbox) = network.join(provider_id.clone());
    let seen = Arc::new(std::sync::Mutex::new(SeenContext::default()));
    let provider = Arc::new(
        FederationNode::new(provider_identity, Arc::new(provider_transport)).with_runtime(
            runtime_with(
                descriptor("echo"),
                Arc::new(ContextProbe {
                    seen: Arc::clone(&seen),
                }),
            ),
        ),
    );
    let caller = Arc::new(FederationNode::new(
        caller_identity,
        Arc::new(caller_transport),
    ));
    let caller_task = caller.start(caller_inbox);
    let provider_task = provider.start(provider_inbox);
    let now = capability::unix_ms();
    {
        let store = caller.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(provider_id.clone(), now, None);
        peers
            .grant(&provider_id, capability("echo"), now, None)
            .expect("granted");
    }
    {
        let store = provider.peers();
        let mut peers = store.write().expect("peers");
        peers.enroll(caller_id.clone(), now, None);
        peers
            .grant(&caller_id, capability("echo"), now, None)
            .expect("granted");
    }
    tokio::time::sleep(Duration::from_millis(20)).await;

    let mut context = context();
    context.task = Some(capability::TaskId::new("task-1"));
    context.trace = capability::TraceContext::new("trace-1", "span-1");
    context.delegation = vec![capability::DelegationHop {
        node: caller_id.clone(),
        delegation: "delegation-1".to_string(),
    }];
    caller
        .invoke(&provider_id, &capability("echo"), json!({}), &context)
        .await
        .expect("the provider answers");

    let seen = seen.lock().expect("seen");
    assert_eq!(seen.task.as_deref(), Some("task-1"));
    assert_eq!(seen.trace, ("trace-1".to_string(), "span-1".to_string()));
    assert_eq!(
        seen.delegation,
        vec![(caller_id.to_string(), "delegation-1".to_string())]
    );
    assert!(seen.remote_principal);

    caller_task.abort();
    provider_task.abort();
}
