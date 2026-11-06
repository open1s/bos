//! The federation node: dual authorization, sealed invocation, and reply routing.
//!
//! One call crosses two independent kernels. The caller authorizes first
//! against its own peer store; the provider authorizes again against its own.
//! Neither decision substitutes for the other, and each Denied names which side
//! refused.
//!
//! A reply that never arrives is an unknown outcome, not a failure: the request
//! may have executed. Retrying is the caller's decision, and only an
//! idempotency key makes it safe.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::transport::{unix_ms, FederationMessage, FederationTransport};
use capability::{
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityOutput, DeliveryState,
    InvocationContext, InvocationId, NodeId, Principal, WorkspaceRuntime,
};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::audit::{Direction, FederationAudit, FederationEvent};
use crate::delegation::{
    DelegationId, DelegationRecord, DelegationRequest, DelegationState, DelegationStore,
};
use crate::discovery::DiscoveryRecord;
use crate::envelope::Envelope;
use crate::identity::NodeIdentity;
use crate::peers::PeerStore;
use crate::resource::{checksum, LeaseId, LeaseStore, ResourceStreamMessage};

/// How long a caller waits for a result when the context has no deadline.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// One Node's federation endpoint.
pub struct FederationNode {
    identity: NodeIdentity,
    transport: Arc<dyn FederationTransport>,
    peers: Arc<RwLock<PeerStore>>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Envelope>>>>,
    delegations: Arc<RwLock<DelegationStore>>,
    leases: Arc<RwLock<LeaseStore>>,
    streams:
        Arc<Mutex<HashMap<LeaseId, tokio::sync::mpsc::UnboundedSender<ResourceStreamMessage>>>>,
    sessions: Arc<Mutex<HashMap<LeaseId, Arc<ServerSession>>>>,
    source: Option<Arc<dyn ResourceSource>>,
    public_capabilities: Arc<Vec<CapabilityDescriptor>>,
    audit: Option<Arc<dyn FederationAudit>>,
}

impl FederationNode {
    /// The identity this node signs with.
    pub fn identity(&self) -> &NodeIdentity {
        &self.identity
    }

    /// Build a node that can call peers and, when a runtime is attached, serve
    /// capabilities to them.
    pub fn new(identity: NodeIdentity, transport: Arc<dyn FederationTransport>) -> Self {
        Self {
            identity,
            transport,
            peers: Arc::new(RwLock::new(PeerStore::new())),
            runtime: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            delegations: Arc::new(RwLock::new(DelegationStore::new())),
            leases: Arc::new(RwLock::new(LeaseStore::new())),
            streams: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            source: None,
            public_capabilities: Arc::new(Vec::new()),
            audit: None,
        }
    }

    /// Send federation events to `audit`.
    pub fn with_audit(mut self, audit: Arc<dyn FederationAudit>) -> Self {
        self.audit = Some(audit);
        self
    }

    fn audit(&self, event: FederationEvent) {
        record(&self.audit, event);
    }

    /// Record the outcome of one outbound coordination operation.
    fn record_outcome<T>(
        &self,
        operation: &str,
        peer: &NodeId,
        subject: Option<String>,
        started: Instant,
        result: &Result<T, CapabilityError>,
    ) {
        let mut event = FederationEvent::new(
            Direction::Outbound,
            peer.as_str(),
            operation,
            None,
            match result {
                Ok(_) => "ok",
                Err(error) => error_kind(error),
            },
        )
        .with_elapsed_ms(started.elapsed().as_millis() as u64);
        if let Some(subject) = subject {
            event = event.with_subject(subject);
        }
        if let Err(error) = result {
            if let Some(reason) = error_reason(error) {
                event = event.with_reason(reason);
            }
        }
        self.audit(event);
    }

    /// Seed the node with persisted peer state instead of an empty store.
    pub fn with_peer_store(mut self, peers: Arc<RwLock<PeerStore>>) -> Self {
        self.peers = peers;
        self
    }

    /// Attach the workspace whose capabilities this node serves.
    pub fn with_runtime(mut self, runtime: Arc<WorkspaceRuntime>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Attach the byte source resource streams read from.
    pub fn with_resource_source(mut self, source: Arc<dyn ResourceSource>) -> Self {
        self.source = Some(source);
        self
    }

    /// Declare what this Node publishes to authenticated peers.
    ///
    /// Only descriptors marked Public survive into a discovery record; passing
    /// a Granted descriptor here is not a way to publish it.
    pub fn with_public_capabilities(mut self, capabilities: Vec<CapabilityDescriptor>) -> Self {
        self.public_capabilities = Arc::new(capabilities);
        self
    }

    /// The leases this Node granted.
    pub fn leases(&self) -> Arc<RwLock<LeaseStore>> {
        Arc::clone(&self.leases)
    }

    /// The Node id this endpoint answers to.
    pub fn node(&self) -> NodeId {
        self.identity.node_id()
    }

    /// The shared peer store, so an administrator (or a test) can enroll and
    /// revoke while the node is live.
    pub fn peers(&self) -> Arc<RwLock<PeerStore>> {
        Arc::clone(&self.peers)
    }

    /// The delegation records this Node owns, so an administrator (or a test)
    /// can advance a task's state while the node is live.
    pub fn delegations(&self) -> Arc<RwLock<DelegationStore>> {
        Arc::clone(&self.delegations)
    }

    /// Start routing frames from this Node's inbox.
    ///
    /// The task owns the inbox; dropping it closes the inbox and stops routing.
    pub fn start(
        &self,
        mut inbox: tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) -> tokio::task::JoinHandle<()> {
        let identity = self.identity.clone();
        let transport = Arc::clone(&self.transport);
        let peers = Arc::clone(&self.peers);
        let runtime = self.runtime.clone();
        let pending = Arc::clone(&self.pending);
        let delegations = Arc::clone(&self.delegations);
        let leases = Arc::clone(&self.leases);
        let streams = Arc::clone(&self.streams);
        let sessions = Arc::clone(&self.sessions);
        let source = self.source.clone();
        let public_capabilities = Arc::clone(&self.public_capabilities);
        let audit = self.audit.clone();
        tokio::spawn(async move {
            while let Some(frame) = inbox.recv().await {
                let Ok(envelope) = Envelope::decode(&frame) else {
                    continue;
                };
                if envelope.open(unix_ms()).is_err() {
                    continue;
                }
                match envelope.kind.as_str() {
                    "result" => {
                        let Some(correlation) = envelope.correlation.clone() else {
                            continue;
                        };
                        let waiter = pending
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .remove(&correlation);
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(envelope);
                        }
                    }
                    "invoke" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let runtime = runtime.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let reply = serve(&identity, runtime, &peers, &audit, &envelope).await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "delegate" | "delegation_status" | "delegation_cancel" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let delegations = Arc::clone(&delegations);
                        let audit = audit.clone();
                        let kind = envelope.kind.clone();
                        tokio::spawn(async move {
                            let reply = match kind.as_str() {
                                "delegate" => {
                                    serve_delegate(
                                        &identity,
                                        &peers,
                                        &delegations,
                                        &audit,
                                        &envelope,
                                    )
                                    .await
                                }
                                "delegation_status" => {
                                    serve_status(&identity, &peers, &delegations, &audit, &envelope)
                                        .await
                                }
                                _ => {
                                    serve_cancel(&identity, &peers, &delegations, &audit, &envelope)
                                        .await
                                }
                            };
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "discover" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let public_capabilities = Arc::clone(&public_capabilities);
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let reply = serve_discover(
                                &identity,
                                &peers,
                                &public_capabilities,
                                &audit,
                                &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "resource" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let leases = Arc::clone(&leases);
                        let streams = Arc::clone(&streams);
                        let sessions = Arc::clone(&sessions);
                        let source = source.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            route_resource(
                                identity, transport, leases, streams, sessions, source, &audit,
                                envelope,
                            )
                            .await;
                        });
                    }
                    _ => {}
                }
            }
        })
    }

    /// Invoke one capability on a peer.
    pub async fn invoke(
        &self,
        peer: &NodeId,
        capability: &CapabilityId,
        input: Value,
        context: &InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        let started = Instant::now();
        let capability_text = capability.to_string();
        // Caller-side decision: this kernel must allow its own principal to
        // use the peer for this capability.
        let authorized = self
            .peers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authorize(peer, capability, unix_ms());
        if let Err(error) = &authorized {
            let mut event = FederationEvent::new(
                Direction::Outbound,
                peer.as_str(),
                "invoke",
                Some(&capability_text),
                error_kind(error),
            );
            if let Some(reason) = error_reason(error) {
                event = event.with_reason(reason);
            }
            self.audit(event);
            return Err(authorized.expect_err("checked above"));
        }

        let payload = json!({
            "capability": capability_text,
            "input": input,
        });
        let reply = match self
            .round_trip(peer, "invoke", payload, context.deadline)
            .await
        {
            Ok(reply) => reply,
            Err(error) => {
                let mut event = FederationEvent::new(
                    Direction::Outbound,
                    peer.as_str(),
                    "invoke",
                    Some(&capability_text),
                    error_kind(&error),
                );
                if let Some(reason) = error_reason(&error) {
                    event = event.with_reason(reason);
                }
                self.audit(event.with_elapsed_ms(started.elapsed().as_millis() as u64));
                return Err(error);
            }
        };
        let decoded = decode_result_value(&reply, capability);
        let elapsed = started.elapsed().as_millis() as u64;
        let mut event = FederationEvent::new(
            Direction::Outbound,
            peer.as_str(),
            "invoke",
            Some(&capability_text),
            match &decoded {
                Ok(_) => "ok",
                Err(error) => error_kind(error),
            },
        )
        .with_elapsed_ms(elapsed);
        if let Err(error) = &decoded {
            if let Some(reason) = error_reason(error) {
                event = event.with_reason(reason);
            }
        }
        self.audit(event);
        decoded
    }

    /// Submit a delegation to a peer and return the record it created.
    pub async fn delegate(
        &self,
        peer: &NodeId,
        request: DelegationRequest,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        let started = Instant::now();
        let subject = request.id.to_string();
        let result = self.delegate_inner(peer, request, context).await;
        self.record_outcome("delegate", peer, Some(subject), started, &result);
        result
    }

    async fn delegate_inner(
        &self,
        peer: &NodeId,
        request: DelegationRequest,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        if &request.target != peer {
            return Err(CapabilityError::Invalid {
                message: format!(
                    "the request targets {} but is sent to {peer}",
                    request.target
                ),
            });
        }
        let now = unix_ms();
        {
            let peers = self
                .peers
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if request.capabilities.is_empty() {
                if !peers.is_trusted(peer, now) {
                    return Err(CapabilityError::Denied {
                        reason: format!("peer {peer} is not enrolled"),
                        policy: None,
                    });
                }
            } else {
                for capability in &request.capabilities {
                    peers.authorize(peer, capability, now)?;
                }
            }
        }
        let reply = self
            .round_trip(
                peer,
                "delegate",
                json!({ "request": request }),
                context.deadline,
            )
            .await?;
        decode_delegation(&reply)
    }

    /// Read the authoritative state of one delegation from its owner.
    pub async fn delegation_status(
        &self,
        peer: &NodeId,
        id: &DelegationId,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        let started = Instant::now();
        let subject = id.to_string();
        let result = self.delegation_status_inner(peer, id, context).await;
        self.record_outcome("delegation_status", peer, Some(subject), started, &result);
        result
    }

    async fn delegation_status_inner(
        &self,
        peer: &NodeId,
        id: &DelegationId,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        let reply = self
            .round_trip(
                peer,
                "delegation_status",
                json!({ "delegation": id.as_str() }),
                context.deadline,
            )
            .await?;
        decode_delegation(&reply)
    }

    /// Ask the owner to cancel one delegation.
    pub async fn cancel_delegation(
        &self,
        peer: &NodeId,
        id: &DelegationId,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        let started = Instant::now();
        let subject = id.to_string();
        let result = self.cancel_delegation_inner(peer, id, context).await;
        self.record_outcome("delegation_cancel", peer, Some(subject), started, &result);
        result
    }

    async fn cancel_delegation_inner(
        &self,
        peer: &NodeId,
        id: &DelegationId,
        context: &InvocationContext,
    ) -> Result<DelegationRecord, CapabilityError> {
        let reply = self
            .round_trip(
                peer,
                "delegation_cancel",
                json!({ "delegation": id.as_str() }),
                context.deadline,
            )
            .await?;
        decode_delegation(&reply)
    }

    /// Open a resource stream from a peer.
    ///
    /// The caller must hold the lease the peer granted; the peer enforces that
    /// on the first chunk, so an unheld lease fails there rather than here.
    pub async fn open_resource(
        &self,
        peer: &NodeId,
        lease: &LeaseId,
        uri: &str,
        offset: u64,
        credit: u64,
    ) -> Result<RemoteStream, CapabilityError> {
        let started = Instant::now();
        let subject = lease.to_string();
        let result = self
            .open_resource_inner(peer, lease, uri, offset, credit)
            .await;
        self.record_outcome("resource", peer, Some(subject), started, &result);
        result
    }

    async fn open_resource_inner(
        &self,
        peer: &NodeId,
        lease: &LeaseId,
        uri: &str,
        offset: u64,
        credit: u64,
    ) -> Result<RemoteStream, CapabilityError> {
        let now = unix_ms();
        {
            let peers = self
                .peers
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !peers.is_trusted(peer, now) {
                return Err(CapabilityError::Denied {
                    reason: format!("peer {peer} is not enrolled"),
                    policy: None,
                });
            }
        }

        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        {
            let mut streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if streams.insert(lease.clone(), sender).is_some() {
                return Err(CapabilityError::Invalid {
                    message: format!("a stream for lease {lease} is already open"),
                });
            }
        }

        send_stream(
            &self.identity,
            &self.transport,
            peer,
            ResourceStreamMessage::Open {
                lease: lease.clone(),
                uri: uri.to_string(),
                offset,
                credit,
            },
        )
        .await;

        Ok(RemoteStream {
            lease: lease.clone(),
            peer: peer.clone(),
            identity: self.identity.clone(),
            transport: Arc::clone(&self.transport),
            receiver,
            audit: self.audit.clone(),
            received: 0,
            settled: false,
        })
    }

    /// Ask a peer for its signed discovery record.
    pub async fn discover(&self, peer: &NodeId) -> Result<DiscoveryRecord, CapabilityError> {
        let started = Instant::now();
        let result = self.discover_inner(peer).await;
        self.record_outcome("discover", peer, None, started, &result);
        result
    }

    async fn discover_inner(&self, peer: &NodeId) -> Result<DiscoveryRecord, CapabilityError> {
        let now = unix_ms();
        {
            let peers = self
                .peers
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !peers.is_trusted(peer, now) {
                return Err(CapabilityError::Denied {
                    reason: format!("peer {peer} is not enrolled"),
                    policy: None,
                });
            }
        }
        let reply = self.round_trip(peer, "discover", json!({}), None).await?;
        let record = decode_discovery(&reply)?;
        if !record.verify() {
            return Err(CapabilityError::Denied {
                reason: "the discovery record signature does not verify".to_string(),
                policy: None,
            });
        }
        if &record.node != peer {
            return Err(CapabilityError::Denied {
                reason: format!("the record names {}, not {peer}", record.node),
                policy: None,
            });
        }
        Ok(record)
    }

    /// The capabilities a peer publishes, as a catalogue.
    ///
    /// The record is verified by [Self::discover] before this builds anything,
    /// so every descriptor here was signed by the peer that names it.
    pub async fn peer_catalogue(
        &self,
        peer: &NodeId,
    ) -> Result<capability::Catalogue, CapabilityError> {
        let record = self.discover(peer).await?;
        let mut catalogue = capability::Catalogue::new();
        for descriptor in record.visible() {
            catalogue.insert(descriptor.clone())?;
        }
        Ok(catalogue)
    }

    async fn round_trip(
        &self,
        peer: &NodeId,
        kind: &str,
        payload: Value,
        deadline: Option<Instant>,
    ) -> Result<Value, CapabilityError> {
        let correlation = format!("{}-{}", self.identity.node_id(), unix_ms());
        let message = FederationMessage::new(
            kind,
            serde_json::to_vec(&payload).map_err(|error| CapabilityError::Invalid {
                message: error.to_string(),
            })?,
        )
        .with_correlation(&correlation)
        .with_idempotency_key(&correlation);
        let envelope = Envelope::seal(&message, &self.identity, peer).map_err(|error| {
            CapabilityError::Invalid {
                message: error.to_string(),
            }
        })?;
        let frame = envelope
            .encode()
            .map_err(|error| CapabilityError::Invalid {
                message: error.to_string(),
            })?;

        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(correlation.clone(), sender);

        let delivered = self.transport.send(peer, frame).await?;
        if delivered == DeliveryState::NotSent {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&correlation);
            return Err(CapabilityError::Transport {
                message: "the request never left this Node".to_string(),
                delivered,
            });
        }

        let wait = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(DEFAULT_REPLY_TIMEOUT);
        match tokio::time::timeout(wait, receiver).await {
            Ok(Ok(envelope)) => Ok(envelope.payload.clone()),
            Ok(Err(_)) => Err(CapabilityError::UnknownOutcome {
                message: "the reply channel closed".to_string(),
            }),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&correlation);
                Err(CapabilityError::UnknownOutcome {
                    message: "no reply within the deadline; the call may have executed".to_string(),
                })
            }
        }
    }
}

async fn serve(
    identity: &NodeIdentity,
    runtime: Option<Arc<WorkspaceRuntime>>,
    peers: &Arc<RwLock<PeerStore>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let caller = envelope.sender.as_str();
    let payload = &envelope.payload;
    let capability = payload
        .get("capability")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<CapabilityId>().ok());
    let Some(capability) = capability else {
        record(
            audit,
            FederationEvent::new(Direction::Inbound, caller, "invoke", None, "invalid")
                .with_reason("the request does not name a capability"),
        );
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not name a capability".to_string(),
            }),
        );
    };

    // Provider-side decision: the receiving kernel checks the caller, with its
    // own grant state, regardless of what the caller already decided.
    let authorized = peers
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .authorize(&envelope.sender, &capability, unix_ms());
    if let Err(error) = &authorized {
        let mut event = FederationEvent::new(
            Direction::Inbound,
            caller,
            "invoke",
            Some(&capability.to_string()),
            error_kind(error),
        );
        if let Some(reason) = error_reason(error) {
            event = event.with_reason(reason);
        }
        record(audit, event);
        return seal_reply(
            identity,
            envelope,
            error_payload(authorized.expect_err("checked")),
        );
    }

    let Some(runtime) = runtime else {
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                caller,
                "invoke",
                Some(&capability.to_string()),
                "unavailable",
            )
            .with_reason("this Node serves no capabilities"),
        );
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Unavailable {
                reason: "this Node serves no capabilities".to_string(),
            }),
        );
    };

    let input = payload.get("input").cloned().unwrap_or(Value::Null);
    let context = InvocationContext::local(
        InvocationId::new(envelope.message_id.clone()),
        Principal::Remote {
            node: envelope.sender.clone(),
        },
        runtime.workspace().clone(),
    );
    let result = runtime.invoke(&capability, input, &context).await;
    let elapsed = started.elapsed().as_millis() as u64;
    let mut event = FederationEvent::new(
        Direction::Inbound,
        caller,
        "invoke",
        Some(&capability.to_string()),
        match &result {
            Ok(_) => "ok",
            Err(error) => error_kind(error),
        },
    )
    .with_elapsed_ms(elapsed);
    if let Err(error) = &result {
        if let Some(reason) = error_reason(error) {
            event = event.with_reason(reason);
        }
    }
    record(audit, event);
    let payload = match result {
        Ok(output) => json!({ "ok": true, "output": output }),
        Err(error) => error_payload(error),
    };
    seal_reply(identity, envelope, payload)
}

fn seal_reply(identity: &NodeIdentity, request: &Envelope, payload: Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    // Correlate on the caller's correlation, which is what it is waiting on;
    // the message id is only the fallback for a request that carried none.
    let correlation = request
        .correlation
        .clone()
        .unwrap_or_else(|| request.message_id.clone());
    let message = FederationMessage::new("result", bytes)
        .with_correlation(correlation.clone())
        .with_idempotency_key(correlation);
    let envelope = Envelope::seal(&message, identity, &request.sender)
        .expect("a reply envelope for a verified request seals");
    envelope.encode().unwrap_or_default()
}

fn error_payload(error: CapabilityError) -> Value {
    let (kind, message) = match error {
        CapabilityError::Invalid { message } => ("invalid", message),
        CapabilityError::Denied { reason, .. } => ("denied", reason),
        CapabilityError::NotFound { capability } => ("not_found", capability.to_string()),
        CapabilityError::DelegationNotFound { delegation } => ("delegation_not_found", delegation),
        CapabilityError::Unavailable { reason } => ("unavailable", reason),
        CapabilityError::RateLimited { retry_after_ms } => (
            "rate_limited",
            retry_after_ms.map_or_else(String::new, |ms| ms.to_string()),
        ),
        CapabilityError::Timeout { elapsed_ms, .. } => ("timeout", elapsed_ms.to_string()),
        CapabilityError::Cancelled => ("cancelled", String::new()),
        CapabilityError::ProviderFailed { message } => ("provider_failed", message),
        CapabilityError::Transport { message, .. } => ("transport", message),
        CapabilityError::UnknownOutcome { message } => ("unknown_outcome", message),
    };
    json!({ "ok": false, "error": { "kind": kind, "message": message } })
}

fn resource_kind(error: &crate::resource::ResourceError) -> &'static str {
    match error {
        crate::resource::ResourceError::UnknownLease { .. } => "not_found",
        crate::resource::ResourceError::WrongHolder { .. }
        | crate::resource::ResourceError::Expired { .. } => "denied",
        crate::resource::ResourceError::QuotaExceeded { .. }
        | crate::resource::ResourceError::CreditExhausted { .. } => "quota_exceeded",
        crate::resource::ResourceError::OutOfOrder { .. }
        | crate::resource::ResourceError::ChecksumMismatch { .. }
        | crate::resource::ResourceError::Invalid { .. } => "invalid",
    }
}

fn error_kind(error: &CapabilityError) -> &'static str {
    match error {
        CapabilityError::Invalid { .. } => "invalid",
        CapabilityError::Denied { .. } => "denied",
        CapabilityError::NotFound { .. } => "not_found",
        CapabilityError::DelegationNotFound { .. } => "delegation_not_found",
        CapabilityError::Unavailable { .. } => "unavailable",
        CapabilityError::RateLimited { .. } => "rate_limited",
        CapabilityError::Timeout { .. } => "timeout",
        CapabilityError::Cancelled => "cancelled",
        CapabilityError::ProviderFailed { .. } => "provider_failed",
        CapabilityError::Transport { .. } => "transport",
        CapabilityError::UnknownOutcome { .. } => "unknown_outcome",
    }
}

fn error_reason(error: &CapabilityError) -> Option<String> {
    match error {
        CapabilityError::Denied { reason, .. }
        | CapabilityError::Unavailable { reason, .. }
        | CapabilityError::ProviderFailed { message: reason }
        | CapabilityError::Invalid { message: reason }
        | CapabilityError::Transport {
            message: reason, ..
        }
        | CapabilityError::UnknownOutcome { message: reason } => Some(reason.clone()),
        CapabilityError::Timeout { elapsed_ms, .. } => {
            Some(format!("timed out after {elapsed_ms} ms"))
        }
        CapabilityError::RateLimited { .. }
        | CapabilityError::Cancelled
        | CapabilityError::NotFound { .. }
        | CapabilityError::DelegationNotFound { .. } => None,
    }
}

fn record(audit: &Option<Arc<dyn FederationAudit>>, event: FederationEvent) {
    if let Some(audit) = audit {
        audit.record(event);
    }
}

fn decode_result_value(
    payload: &Value,
    capability: &CapabilityId,
) -> Result<CapabilityOutput, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        let output = payload.get("output").cloned().unwrap_or(Value::Null);
        return serde_json::from_value(output).map_err(|error| CapabilityError::Invalid {
            message: format!("the peer returned an unreadable result: {error}"),
        });
    }
    Err(wire_error(payload, Some(capability)))
}

fn decode_delegation(payload: &Value) -> Result<DelegationRecord, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return serde_json::from_value(payload.get("delegation").cloned().unwrap_or(Value::Null))
            .map_err(|error| CapabilityError::Invalid {
                message: format!("the peer returned an unreadable delegation: {error}"),
            });
    }
    Err(wire_error(payload, None))
}

fn wire_error(payload: &Value, capability: Option<&CapabilityId>) -> CapabilityError {
    let error = payload.get("error");
    let kind = error
        .and_then(|error| error.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("provider_failed");
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match kind {
        "invalid" => CapabilityError::Invalid { message },
        "denied" => CapabilityError::Denied {
            reason: message,
            policy: None,
        },
        "not_found" => CapabilityError::NotFound {
            capability: capability
                .cloned()
                .unwrap_or_else(|| "remote/unknown".parse().expect("a valid fallback id")),
        },
        "delegation_not_found" => CapabilityError::DelegationNotFound {
            delegation: message,
        },
        "unavailable" => CapabilityError::Unavailable { reason: message },
        "rate_limited" => CapabilityError::RateLimited {
            retry_after_ms: message.parse().ok(),
        },
        "timeout" => CapabilityError::Timeout {
            elapsed_ms: message.parse().unwrap_or_default(),
            partial: None,
        },
        "cancelled" => CapabilityError::Cancelled,
        "unknown_outcome" => CapabilityError::UnknownOutcome { message },
        _ => CapabilityError::ProviderFailed { message },
    }
}

async fn serve_delegate(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_delegate_inner(identity, peers, delegations, envelope).await;
    record_reply(audit, envelope, "delegate", started, &reply);
    reply
}

async fn serve_delegate_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let Some(request) = envelope
        .payload
        .get("request")
        .cloned()
        .and_then(|value| serde_json::from_value::<DelegationRequest>(value).ok())
    else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not carry a delegation".to_string(),
            }),
        );
    };
    if request.target != identity.node_id() {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: "the request is addressed to another Node".to_string(),
                policy: None,
            }),
        );
    }
    if let Err(error) = authorize_sender(peers, &envelope.sender, &request.capabilities) {
        return seal_reply(identity, envelope, error_payload(error));
    }

    let now = unix_ms();
    let mut store = delegations
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let parent = request.parent.clone();
    let outcome = match parent {
        Some(parent) => store.delegate(&parent, request, now),
        None => store.create(request, now),
    };
    match outcome {
        Ok(id) => {
            let _ = store.transition(&id, DelegationState::Accepted, now);
            match store.record(&id).cloned() {
                Some(record) => seal_reply(
                    identity,
                    envelope,
                    json!({ "ok": true, "delegation": record }),
                ),
                None => seal_reply(
                    identity,
                    envelope,
                    error_payload(CapabilityError::ProviderFailed {
                        message: "the accepted delegation disappeared".to_string(),
                    }),
                ),
            }
        }
        Err(error) => seal_reply(identity, envelope, error_payload(delegation_error(error))),
    }
}

async fn serve_status(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_status_inner(identity, peers, delegations, envelope).await;
    record_reply(audit, envelope, "delegation_status", started, &reply);
    reply
}

async fn serve_status_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    envelope: &Envelope,
) -> Vec<u8> {
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let Some(id) = delegation_id(envelope) else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not name a delegation".to_string(),
            }),
        );
    };
    let record = delegations
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .record(&id)
        .cloned();
    match record {
        Some(record) => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "delegation": record }),
        ),
        None => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::DelegationNotFound {
                delegation: id.to_string(),
            }),
        ),
    }
}

async fn serve_cancel(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_cancel_inner(identity, peers, delegations, envelope).await;
    record_reply(audit, envelope, "delegation_cancel", started, &reply);
    reply
}

async fn serve_cancel_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    envelope: &Envelope,
) -> Vec<u8> {
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let Some(id) = delegation_id(envelope) else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not name a delegation".to_string(),
            }),
        );
    };
    let now = unix_ms();
    let mut store = delegations
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(state) = store.record(&id).map(|record| record.state) else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::DelegationNotFound {
                delegation: id.to_string(),
            }),
        );
    };
    // Cancellation is idempotent: a terminal delegation is returned as it is,
    // so a late cancel cannot overwrite a result the owner already recorded.
    if !state.is_terminal() {
        let _ = store.transition(&id, DelegationState::Cancelled, now);
    }
    match store.record(&id).cloned() {
        Some(record) => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "delegation": record }),
        ),
        None => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::DelegationNotFound {
                delegation: id.to_string(),
            }),
        ),
    }
}

fn reply_outcome(reply: &[u8]) -> (String, Option<String>) {
    let Ok(envelope) = Envelope::decode(reply) else {
        return ("unknown_outcome".to_string(), None);
    };
    let payload = &envelope.payload;
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return ("ok".to_string(), None);
    }
    let kind = payload
        .pointer("/error/kind")
        .and_then(Value::as_str)
        .unwrap_or("unknown_outcome")
        .to_string();
    let message = payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .map(str::to_string);
    (kind, message)
}

/// The delegation a coordination message acts on: a cancel or status names it
/// directly, a submit nests it in the request.
fn delegation_subject(envelope: &Envelope) -> Option<String> {
    if let Some(id) = delegation_id(envelope) {
        return Some(id.to_string());
    }
    envelope
        .payload
        .get("request")?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

fn record_reply(
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
    operation: &str,
    started: Instant,
    reply: &[u8],
) {
    let (outcome, reason) = reply_outcome(reply);
    let mut event = FederationEvent::new(
        Direction::Inbound,
        envelope.sender.as_str(),
        operation,
        None,
        &outcome,
    )
    .with_elapsed_ms(started.elapsed().as_millis() as u64);
    if let Some(subject) = delegation_subject(envelope) {
        event = event.with_subject(subject);
    }
    if let Some(reason) = reason {
        event = event.with_reason(reason);
    }
    record(audit, event);
}

fn authorize_sender(
    peers: &Arc<RwLock<PeerStore>>,
    sender: &capability::NodeId,
    capabilities: &[CapabilityId],
) -> Result<(), CapabilityError> {
    let now = unix_ms();
    let peers = peers
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !peers.is_trusted(sender, now) {
        return Err(CapabilityError::Denied {
            reason: format!("peer {sender} is not enrolled"),
            policy: None,
        });
    }
    for capability in capabilities {
        peers.authorize(sender, capability, now)?;
    }
    Ok(())
}

fn delegation_id(envelope: &Envelope) -> Option<DelegationId> {
    envelope
        .payload
        .get("delegation")
        .and_then(Value::as_str)
        .map(DelegationId::new)
}

fn delegation_error(error: crate::delegation::DelegationError) -> CapabilityError {
    use crate::delegation::DelegationError;
    match error {
        DelegationError::Unknown { id } => CapabilityError::DelegationNotFound {
            delegation: id.to_string(),
        },
        DelegationError::Invalid { message } => CapabilityError::Invalid { message },
        DelegationError::InvalidTransition { id, from, to } => CapabilityError::Denied {
            reason: format!("delegation {id} cannot go from {from:?} to {to:?}"),
            policy: None,
        },
        DelegationError::SubDelegationDisabled { id } => CapabilityError::Denied {
            reason: format!("delegation {id} does not allow sub-delegation"),
            policy: None,
        },
        DelegationError::NotAttenuated { capability } => CapabilityError::Denied {
            reason: format!("the delegation would widen authority with {capability}"),
            policy: None,
        },
        DelegationError::RecipientNotAllowed { node } => CapabilityError::Denied {
            reason: format!("the delegation would forward to {node}"),
            policy: None,
        },
        DelegationError::DeadlineExceedsParent => CapabilityError::Denied {
            reason: "the delegation outlives its parent".to_string(),
            policy: None,
        },
    }
}

/// A caller's view of an incoming resource stream.
pub struct RemoteStream {
    lease: LeaseId,
    peer: NodeId,
    identity: NodeIdentity,
    transport: Arc<dyn FederationTransport>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<ResourceStreamMessage>,
    audit: Option<Arc<dyn FederationAudit>>,
    received: u64,
    settled: bool,
}

impl RemoteStream {
    /// The lease this stream runs under.
    pub fn lease(&self) -> &LeaseId {
        &self.lease
    }

    /// The next message: a chunk, a close, or an error.
    pub async fn next(&mut self) -> Option<ResourceStreamMessage> {
        let message = self.receiver.recv().await?;
        match &message {
            ResourceStreamMessage::Chunk { bytes, .. } => {
                self.received += bytes.len() as u64;
            }
            ResourceStreamMessage::Close { .. } => self.settle("ok", None),
            ResourceStreamMessage::Error { message, .. } => {
                self.settle("error", Some(message.clone()))
            }
            _ => {}
        }
        Some(message)
    }

    /// Record how this stream ended, once.
    fn settle(&mut self, outcome: &str, reason: Option<String>) {
        if self.settled {
            return;
        }
        self.settled = true;
        let mut event = FederationEvent::new(
            Direction::Outbound,
            self.peer.as_str(),
            "resource",
            None,
            outcome,
        )
        .with_subject(self.lease.to_string())
        .with_bytes(self.received);
        if let Some(reason) = reason {
            event = event.with_reason(reason);
        }
        record(&self.audit, event);
    }

    /// Grant the sender more credit.
    pub async fn grant(&self, bytes: u64) -> Result<(), CapabilityError> {
        send_stream(
            &self.identity,
            &self.transport,
            &self.peer,
            ResourceStreamMessage::Credit {
                lease: self.lease.clone(),
                bytes,
            },
        )
        .await;
        Ok(())
    }
}

impl Drop for RemoteStream {
    fn drop(&mut self) {
        // A reader that walks away before the stream ends leaves a record too:
        // otherwise the provider's "0 bytes left" would look like the only fact.
        self.settle(
            "cancelled",
            Some("the reader dropped the stream".to_string()),
        );
    }
}

/// The bytes a Node offers to resource streams.
#[async_trait]
pub trait ResourceSource: Send + Sync {
    /// Read up to `max` bytes at `offset`. None means end of data.
    async fn read(
        &self,
        uri: &str,
        offset: u64,
        max: usize,
    ) -> Result<Option<Vec<u8>>, CapabilityError>;
}

/// How many bytes one chunk may carry before the receiver grants more.
pub const STREAM_CHUNK: usize = 8 * 1024;

struct ServerSession {
    credit: AtomicU64,
    open: AtomicBool,
    notify: tokio::sync::Notify,
}

impl ServerSession {
    fn new(credit: u64) -> Self {
        Self {
            credit: AtomicU64::new(credit),
            open: AtomicBool::new(true),
            notify: tokio::sync::Notify::new(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn route_resource(
    identity: NodeIdentity,
    transport: Arc<dyn FederationTransport>,
    leases: Arc<RwLock<LeaseStore>>,
    streams: Arc<
        Mutex<HashMap<LeaseId, tokio::sync::mpsc::UnboundedSender<ResourceStreamMessage>>>,
    >,
    sessions: Arc<Mutex<HashMap<LeaseId, Arc<ServerSession>>>>,
    source: Option<Arc<dyn ResourceSource>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: Envelope,
) {
    let Ok(message) = serde_json::from_value::<ResourceStreamMessage>(envelope.payload.clone())
    else {
        return;
    };
    let lease = message.lease().clone();

    // A locally registered stream owns this lease: the message is inbound data.
    if let Some(sender) = streams
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&lease)
        .cloned()
    {
        let _ = sender.send(message);
        return;
    }

    match message {
        ResourceStreamMessage::Open {
            uri,
            offset,
            credit,
            ..
        } => {
            let Some(source) = source else {
                record(
                    audit,
                    FederationEvent::new(
                        Direction::Inbound,
                        envelope.sender.as_str(),
                        "resource",
                        None,
                        "unavailable",
                    )
                    .with_subject(lease.to_string())
                    .with_reason("this Node serves no resources"),
                );
                send_stream(
                    &identity,
                    &transport,
                    &envelope.sender,
                    ResourceStreamMessage::Error {
                        lease,
                        message: "this Node serves no resources".to_string(),
                    },
                )
                .await;
                return;
            };
            let authorized = leases
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .authorize(&lease, &envelope.sender, unix_ms(), 0);
            if let Err(error) = &authorized {
                record(
                    audit,
                    FederationEvent::new(
                        Direction::Inbound,
                        envelope.sender.as_str(),
                        "resource",
                        None,
                        resource_kind(error),
                    )
                    .with_subject(lease.to_string())
                    .with_reason(error.to_string()),
                );
                send_stream(
                    &identity,
                    &transport,
                    &envelope.sender,
                    ResourceStreamMessage::Error {
                        lease,
                        message: authorized.expect_err("checked").to_string(),
                    },
                )
                .await;
                return;
            }
            record(
                audit,
                FederationEvent::new(
                    Direction::Inbound,
                    envelope.sender.as_str(),
                    "resource",
                    None,
                    "ok",
                )
                .with_subject(lease.to_string()),
            );
            let session = Arc::new(ServerSession::new(credit));
            sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(lease.clone(), Arc::clone(&session));
            tokio::spawn(produce(
                identity,
                transport,
                leases,
                sessions,
                source,
                envelope.sender.clone(),
                lease,
                uri,
                offset,
                session,
                audit.clone(),
            ));
        }
        ResourceStreamMessage::Credit { bytes, .. } => {
            if let Some(session) = sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&lease)
                .cloned()
            {
                session.credit.fetch_add(bytes, Ordering::SeqCst);
                session.notify.notify_waiters();
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn produce(
    identity: NodeIdentity,
    transport: Arc<dyn FederationTransport>,
    leases: Arc<RwLock<LeaseStore>>,
    sessions: Arc<Mutex<HashMap<LeaseId, Arc<ServerSession>>>>,
    source: Arc<dyn ResourceSource>,
    peer: NodeId,
    lease: LeaseId,
    uri: String,
    mut offset: u64,
    session: Arc<ServerSession>,
    audit: Option<Arc<dyn FederationAudit>>,
) {
    let mut seq = 0u64;
    let mut streamed: Vec<u8> = Vec::new();
    let mut outcome = "ok";
    let mut reason: Option<String> = None;
    loop {
        let credit = session.credit.load(Ordering::SeqCst);
        if credit == 0 {
            if !session.open.load(Ordering::SeqCst) {
                break;
            }
            session.notify.notified().await;
            continue;
        }
        // Read no further than both the credit and the lease allow, so a
        // bounded lease streams its quota instead of failing the first chunk
        // that happens to be larger.
        let remaining = leases
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&lease)
            .and_then(|lease| lease.remaining());
        if remaining == Some(0) {
            outcome = "quota_exceeded";
            reason = Some(format!("lease {lease} has 0 bytes left"));
            send_stream(
                &identity,
                &transport,
                &peer,
                ResourceStreamMessage::Error {
                    lease: lease.clone(),
                    message: format!("lease {lease} has 0 bytes left"),
                },
            )
            .await;
            break;
        }
        let limit = remaining
            .map_or(credit, |remaining| remaining.min(credit))
            .min(STREAM_CHUNK as u64);
        match source.read(&uri, offset, limit as usize).await {
            Ok(Some(bytes)) if bytes.is_empty() => {
                send_stream(
                    &identity,
                    &transport,
                    &peer,
                    ResourceStreamMessage::Close {
                        lease: lease.clone(),
                        total: offset,
                        checksum: checksum(&streamed),
                    },
                )
                .await;
                break;
            }
            Ok(Some(bytes)) => {
                let length = bytes.len() as u64;
                let charged = leases
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .charge(&lease, &peer, unix_ms(), length);
                if let Err(error) = &charged {
                    outcome = resource_kind(error);
                    reason = Some(error.to_string());
                    send_stream(
                        &identity,
                        &transport,
                        &peer,
                        ResourceStreamMessage::Error {
                            lease: lease.clone(),
                            message: error.to_string(),
                        },
                    )
                    .await;
                    break;
                }
                session.credit.fetch_sub(length, Ordering::SeqCst);
                streamed.extend_from_slice(&bytes);
                offset += length;
                send_stream(
                    &identity,
                    &transport,
                    &peer,
                    ResourceStreamMessage::Chunk {
                        lease: lease.clone(),
                        seq,
                        bytes,
                    },
                )
                .await;
                seq += 1;
            }
            Ok(None) => {
                send_stream(
                    &identity,
                    &transport,
                    &peer,
                    ResourceStreamMessage::Close {
                        lease: lease.clone(),
                        total: offset,
                        checksum: checksum(&streamed),
                    },
                )
                .await;
                break;
            }
            Err(error) => {
                outcome = "provider_failed";
                reason = Some(error.to_string());
                send_stream(
                    &identity,
                    &transport,
                    &peer,
                    ResourceStreamMessage::Error {
                        lease: lease.clone(),
                        message: error.to_string(),
                    },
                )
                .await;
                break;
            }
        }
    }
    // One record when the stream ends, not one per chunk: the byte count and
    // the stopping reason are the facts a reader needs, and no payload rides
    // along.
    let mut event =
        FederationEvent::new(Direction::Inbound, peer.as_str(), "resource", None, outcome)
            .with_subject(lease.to_string())
            .with_bytes(offset);
    if let Some(reason) = reason {
        event = event.with_reason(reason);
    }
    record(&audit, event);
    session.open.store(false, Ordering::SeqCst);
    sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&lease);
}

async fn send_stream(
    identity: &NodeIdentity,
    transport: &Arc<dyn FederationTransport>,
    peer: &NodeId,
    message: ResourceStreamMessage,
) {
    let Ok(bytes) = serde_json::to_vec(&message) else {
        return;
    };
    let message = FederationMessage::new("resource", bytes);
    if let Ok(envelope) = Envelope::seal(&message, identity, peer) {
        if let Ok(frame) = envelope.encode() {
            let _ = transport.send(peer, frame).await;
        }
    }
}

async fn serve_discover(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    capabilities: &[CapabilityDescriptor],
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_discover_inner(identity, peers, capabilities, envelope).await;
    record_reply(audit, envelope, "discover", started, &reply);
    reply
}

async fn serve_discover_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    capabilities: &[CapabilityDescriptor],
    envelope: &Envelope,
) -> Vec<u8> {
    let trusted = peers
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_trusted(&envelope.sender, unix_ms());
    if !trusted {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: format!("peer {} is not enrolled", envelope.sender),
                policy: None,
            }),
        );
    }
    let record = DiscoveryRecord::publish(identity, capabilities.to_vec());
    seal_reply(identity, envelope, json!({ "ok": true, "record": record }))
}

fn decode_discovery(payload: &Value) -> Result<DiscoveryRecord, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return serde_json::from_value(payload.get("record").cloned().unwrap_or(Value::Null))
            .map_err(|error| CapabilityError::Invalid {
                message: format!("the peer returned an unreadable discovery record: {error}"),
            });
    }
    Err(wire_error(payload, None))
}
