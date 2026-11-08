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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use capability::transport::{unix_ms, FederationMessage, FederationTransport};
use capability::{
    CancelToken, CapabilityDescriptor, CapabilityError, CapabilityId, CapabilityOutput,
    DelegationHop, DeliveryState, InvocationContext, InvocationId, NodeId, Principal, TaskId,
    TraceContext, WorkspaceRuntime,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::artifacts::{
    ArtifactCleanupRequest, ArtifactCleanupResponse, ArtifactFailure, ArtifactStore,
};
use crate::audit::{Direction, FederationAudit, FederationEvent};
use crate::delegation::{
    DelegationId, DelegationRecord, DelegationRequest, DelegationState, DelegationStore,
};
use crate::discovery::DiscoveryRecord;
use crate::envelope::Envelope;
use crate::identity::NodeIdentity;
use crate::knowledge::{
    KnowledgeAuthorizer, KnowledgeItem, KnowledgePublication, KnowledgeQuery, KnowledgeResponse,
    KnowledgeService,
};
use crate::membership::FederationId;
use crate::peers::PeerStore;
use crate::resource::{LeaseId, LeaseStore, ResourceStreamMessage};
use crate::rotation::RotationRecord;
use crate::task::{FederatedTask, TaskCreateRequest, TaskIdRequest, TaskQueue};

/// How long a caller waits for a result when the context has no deadline.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How many inbound stream messages buffer ahead of the consumer.
const RESOURCE_QUEUE: usize = 32;

/// One inbound stream the caller owns.
struct InboundStream {
    /// The peer this stream was opened with; only it may send stream messages.
    peer: NodeId,
    /// The consumer's queue.
    sender: tokio::sync::mpsc::Sender<ResourceStreamMessage>,
    /// The next chunk sequence this stream accepts.
    next_seq: u64,
    /// False after Close or Error; only the queue's consumer may see those.
    open: bool,
}

/// How many accepted message identities are remembered at once.
const REPLAY_CAPACITY: usize = 4096;

/// Bounded, expiry-aware replay protection for inbound frames.
///
/// The key is scoped to the authenticated sender: a caller-supplied idempotency
/// key is never trusted as globally unique. A message is remembered until the
/// envelope that carried it expires, and the oldest entry is evicted when the
/// bound is reached. The cache is in memory: a restart forgets it, which is safe
/// because envelopes carry a short validity window and cannot be replayed after
/// it.
#[derive(Clone)]
struct ReplayCache {
    inner: Arc<Mutex<ReplayInner>>,
}

struct ReplayInner {
    seen: HashMap<(NodeId, String), u64>,
    order: std::collections::VecDeque<(NodeId, String)>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ReplayFile {
    entries: Vec<(NodeId, String, u64)>,
}

impl ReplayCache {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ReplayInner {
                seen: HashMap::new(),
                order: std::collections::VecDeque::new(),
            })),
        }
    }

    fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let cache = Self::new();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(cache),
            Err(error) => return Err(error),
        };
        let file: ReplayFile = serde_json::from_str(&text)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let now = unix_ms();
        let mut inner = cache
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (sender, id, expires_at_ms) in file.entries {
            if expires_at_ms <= now {
                continue;
            }
            inner
                .seen
                .insert((sender.clone(), id.clone()), expires_at_ms);
            inner.order.push_back((sender, id));
        }
        while inner.seen.len() > REPLAY_CAPACITY {
            if let Some(oldest) = inner.order.pop_front() {
                inner.seen.remove(&oldest);
            }
        }
        drop(inner);
        Ok(cache)
    }

    fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let entries: Vec<(NodeId, String, u64)> = {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner
                .seen
                .iter()
                .map(|((sender, id), expiry)| (sender.clone(), id.clone(), *expiry))
                .collect()
        };
        let text = serde_json::to_string_pretty(&ReplayFile { entries })
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path)
    }

    /// Return true when no key was seen before, recording all of them.
    fn admit(&self, keys: &[(NodeId, String)], expires_at_ms: u64, now_ms: u64) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let duplicate = keys
            .iter()
            .any(|key| inner.seen.get(key).is_some_and(|expiry| *expiry > now_ms));
        if duplicate {
            return false;
        }
        // Capacity pressure must never reopen a still-valid window: refuse the
        // new identities instead of evicting an earlier one, so a flood cannot
        // turn a previously accepted request into a fresh one. The caller sees
        // a denial; the sender can retry once the window has moved on.
        if inner.seen.len().saturating_add(keys.len()) > REPLAY_CAPACITY {
            return false;
        }
        for key in keys {
            inner.seen.insert(key.clone(), expires_at_ms);
            inner.order.push_back(key.clone());
        }
        true
    }
}

/// Removes one in-flight invocation when the provider returns, panics, or its
/// handler is aborted.
struct InFlightGuard {
    in_flight: Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
    key: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

/// Where to send a cancellation when the caller's wait ends without a reply.
struct CancelNotice {
    transport: Arc<dyn FederationTransport>,
    identity: NodeIdentity,
    peer: NodeId,
    correlation: String,
}

/// One outstanding request: who should answer it, and where the answer goes.
struct PendingReply {
    peer: NodeId,
    sender: oneshot::Sender<Envelope>,
}

/// Removes one pending waiter when its caller's future ends for any reason, and
/// asks the target to cancel when the caller gave up rather than succeeded.
struct PendingGuard {
    pending: Arc<Mutex<HashMap<String, PendingReply>>>,
    correlation: String,
    cancel: Option<CancelNotice>,
    armed: bool,
}

impl PendingGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.correlation);
        if self.armed {
            if let Some(notice) = self.cancel.take() {
                request_cancel(notice);
            }
        }
    }
}

/// Send a signed, idempotent cancellation for one invocation.
fn request_cancel(notice: CancelNotice) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let payload = json!({ "correlation": notice.correlation });
        let Ok(bytes) = serde_json::to_vec(&payload) else {
            return;
        };
        let message =
            FederationMessage::new("invoke_cancel", bytes).with_correlation(&notice.correlation);
        let Ok(envelope) = Envelope::seal(&message, &notice.identity, &notice.peer) else {
            return;
        };
        let Ok(frame) = envelope.encode() else {
            return;
        };
        let _ = notice.transport.send(&notice.peer, frame).await;
    });
}

/// The most resource streams one Node produces at once.
pub const MAX_CONCURRENT_STREAMS: usize = 128;

/// The longest one resource stream may run before the producer ends it.
pub const MAX_STREAM_DURATION: Duration = Duration::from_secs(600);

/// Finalize an incremental stream hash as the wire's hex SHA-256.
fn stream_checksum(hasher: &Sha256) -> String {
    hasher
        .clone()
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The default global cap for capability invocation handlers.
pub const MAX_IN_FLIGHT_HANDLERS: usize = 64;
/// The default per-peer cap for capability invocation handlers.
pub const MAX_IN_FLIGHT_PER_PEER: usize = 8;
/// The default global cap for control handlers (delegation, discovery, cancel).
pub const MAX_IN_FLIGHT_CONTROL: usize = 32;
/// The default per-peer cap for control handlers.
pub const MAX_IN_FLIGHT_CONTROL_PER_PEER: usize = 4;
/// The default number of concurrent inbound resource streams.
pub const MAX_RESOURCE_STREAMS: usize = 128;

/// Independent admission budgets for the handler classes.
///
/// Invocations do expensive provider work, control messages are cheap but
/// numerous, and resource streams have their own long-lived ceiling. One class
/// saturating must not consume another class's capacity. Zero is treated as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionLimits {
    /// Global invocation handlers.
    pub invocation_global: usize,
    /// Invocation handlers per peer.
    pub invocation_per_peer: usize,
    /// Global control handlers.
    pub control_global: usize,
    /// Control handlers per peer.
    pub control_per_peer: usize,
    /// Concurrent inbound resource streams.
    pub resource_streams: usize,
    /// Aggregate bytes of in-flight frames per peer.
    pub queued_bytes_per_peer: u64,
}

impl Default for AdmissionLimits {
    fn default() -> Self {
        Self {
            invocation_global: MAX_IN_FLIGHT_HANDLERS,
            invocation_per_peer: MAX_IN_FLIGHT_PER_PEER,
            control_global: MAX_IN_FLIGHT_CONTROL,
            control_per_peer: MAX_IN_FLIGHT_CONTROL_PER_PEER,
            resource_streams: MAX_RESOURCE_STREAMS,
            queued_bytes_per_peer: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandlerClass {
    Invocation,
    Control,
}

fn handler_class(kind: &str) -> HandlerClass {
    if kind == "invoke" {
        HandlerClass::Invocation
    } else {
        HandlerClass::Control
    }
}

/// The largest frame one message kind may carry.
///
/// The frame ceiling in the transport bounds every message; these are the
/// tighter per-kind budgets, so a resource chunk may be large while an
/// invoke or a control message stays small.
pub fn kind_payload_limit(kind: &str) -> u64 {
    match kind {
        "invoke" | "result" => 256 * 1024,
        "resource" => 1024 * 1024,
        "knowledge_query" | "knowledge_publication" => 512 * 1024,
        "task_create" => 128 * 1024,
        "delegate" => 64 * 1024,
        _ => 64 * 1024,
    }
}

#[derive(Clone)]
struct ByteAdmission {
    per_peer: Arc<Mutex<HashMap<NodeId, u64>>>,
    limit: u64,
}

impl ByteAdmission {
    fn new(limit: u64) -> Self {
        Self {
            per_peer: Arc::new(Mutex::new(HashMap::new())),
            limit: limit.max(1),
        }
    }

    fn try_reserve(&self, peer: &NodeId, bytes: u64) -> Option<BytePermit> {
        let mut used = self
            .per_peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = used.get(peer).copied().unwrap_or(0);
        if current.saturating_add(bytes) > self.limit {
            return None;
        }
        used.insert(peer.clone(), current.saturating_add(bytes));
        Some(BytePermit {
            per_peer: Arc::clone(&self.per_peer),
            peer: peer.clone(),
            bytes,
        })
    }
}

struct BytePermit {
    per_peer: Arc<Mutex<HashMap<NodeId, u64>>>,
    peer: NodeId,
    bytes: u64,
}

impl Drop for BytePermit {
    fn drop(&mut self) {
        let mut used = self
            .per_peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(current) = used.get_mut(&self.peer) {
            *current = current.saturating_sub(self.bytes);
            if *current == 0 {
                used.remove(&self.peer);
            }
        }
    }
}

#[derive(Clone)]
struct ClassAdmission {
    global: Arc<tokio::sync::Semaphore>,
    per_peer: Arc<Mutex<HashMap<NodeId, usize>>>,
    per_peer_limit: usize,
}

impl ClassAdmission {
    fn new(global: usize, per_peer: usize) -> Self {
        Self {
            global: Arc::new(tokio::sync::Semaphore::new(global.max(1))),
            per_peer: Arc::new(Mutex::new(HashMap::new())),
            per_peer_limit: per_peer.max(1),
        }
    }

    fn try_admit(&self, peer: &NodeId) -> Option<ClassPermit> {
        let mut counts = self
            .per_peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if counts.get(peer).copied().unwrap_or(0) >= self.per_peer_limit {
            return None;
        }
        let permit = Arc::clone(&self.global).try_acquire_owned().ok()?;
        *counts.entry(peer.clone()).or_insert(0) += 1;
        Some(ClassPermit {
            per_peer: Arc::clone(&self.per_peer),
            peer: peer.clone(),
            _global: permit,
        })
    }
}

/// Admission control for spawned request handlers.
///
/// The inbox is bounded before dispatch; this bounds what happens after it, so
/// a burst of valid frames cannot grow task count and retained payloads without
/// a ceiling. A permit is held by the task and released on completion, panic,
/// or abort. Invocation and control budgets are independent.
#[derive(Clone)]
struct Admission {
    invocation: ClassAdmission,
    control: ClassAdmission,
    bytes: ByteAdmission,
}

impl Admission {
    fn new(limits: AdmissionLimits) -> Self {
        Self {
            invocation: ClassAdmission::new(limits.invocation_global, limits.invocation_per_peer),
            control: ClassAdmission::new(limits.control_global, limits.control_per_peer),
            bytes: ByteAdmission::new(limits.queued_bytes_per_peer),
        }
    }

    fn try_admit(&self, peer: &NodeId, kind: &str, bytes: u64) -> Option<AdmissionPermit> {
        let class = match handler_class(kind) {
            HandlerClass::Invocation => self.invocation.try_admit(peer),
            HandlerClass::Control => self.control.try_admit(peer),
        }?;
        let reserved = self.bytes.try_reserve(peer, bytes)?;
        Some(AdmissionPermit {
            _class: class,
            _bytes: reserved,
        })
    }
}

struct ClassPermit {
    per_peer: Arc<Mutex<HashMap<NodeId, usize>>>,
    peer: NodeId,
    _global: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for ClassPermit {
    fn drop(&mut self) {
        let mut counts = self
            .per_peer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = counts.get_mut(&self.peer) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.peer);
            }
        }
    }
}

/// Holds both the handler count and the byte reservation for one frame.
struct AdmissionPermit {
    _class: ClassPermit,
    _bytes: BytePermit,
}

/// The operation and resource a call touches, for a scoped grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InvocationScope {
    /// The operation below the capability id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// The resource selector the operation touches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// The workspace the call belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

/// One Node's federation endpoint.
pub struct FederationNode {
    identity: NodeIdentity,
    transport: Arc<dyn FederationTransport>,
    peers: Arc<RwLock<PeerStore>>,
    runtime: Option<Arc<WorkspaceRuntime>>,
    pending: Arc<Mutex<HashMap<String, PendingReply>>>,
    delegations: Arc<RwLock<DelegationStore>>,
    leases: Arc<RwLock<LeaseStore>>,
    /// Host-governed knowledge, when this Node offers the knowledge protocol.
    knowledge: Option<Arc<RwLock<KnowledgeService>>>,
    /// The host's authorization decision for knowledge queries and publications.
    knowledge_authorizer: Option<Arc<dyn KnowledgeAuthorizer>>,
    streams: Arc<Mutex<HashMap<LeaseId, InboundStream>>>,
    /// Transfer checkpoints, when persistence is configured.
    transfers: Option<Arc<Mutex<crate::resource::TransferStore>>>,
    /// Where the checkpoints are saved.
    transfer_path: Option<PathBuf>,
    /// The optional coordinator membership, when this node runs one.
    coordinator: Option<Arc<Mutex<crate::coordinator::Coordinator>>>,
    /// Where the membership document is saved.
    coordinator_path: Option<PathBuf>,
    sessions: Arc<Mutex<HashMap<LeaseId, Arc<ServerSession>>>>,
    source: Option<Arc<dyn ResourceSource>>,
    public_capabilities: Arc<Vec<CapabilityDescriptor>>,
    audit: Option<Arc<dyn FederationAudit>>,
    /// The federation this Node advertises, when it has one.
    federation: Option<FederationId>,
    /// Where peer state is written after a rotation, when configured.
    peer_path: Option<PathBuf>,
    /// Where the local knowledge store is written after a mutation.
    knowledge_path: Option<PathBuf>,
    /// The host store that makes artifact deletion real.
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    /// The policy revision attached to discovery publications.
    policy_revision: u64,
    /// Independent admission budgets for handler classes and streams.
    admission_limits: AdmissionLimits,
    /// A per-process counter that makes every correlation unique even when two
    /// requests are created in the same millisecond.
    next_correlation: AtomicU64,
    /// In-flight invocations, keyed by correlation, with their caller and
    /// cancellation token so a remote cancel can stop the provider.
    in_flight: Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
    /// The newest discovery publication accepted from each peer, so a replayed
    /// older record cannot replace a newer catalogue.
    catalogue_seen: Arc<RwLock<HashMap<NodeId, u64>>>,
    /// Message ids and idempotency keys already accepted, with the expiry of
    /// the frame that carried them.
    replay: ReplayCache,
    /// Where the replay cache is persisted, when it is.
    replay_path: Option<PathBuf>,
    /// Where delegation records are persisted, when they are.
    delegation_path: Option<PathBuf>,
    /// Peer-owned tasks this node records for other nodes.
    tasks: Arc<RwLock<TaskQueue>>,
    /// Where the task queue persists, when persistence was requested.
    task_path: Option<PathBuf>,
    /// The revision last written, so a quiet loop writes nothing.
    delegation_revision: Arc<AtomicU64>,
    /// Serializes delegation-file writes from concurrent handler tasks.
    delegation_write: Arc<Mutex<()>>,
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
            knowledge: None,
            knowledge_authorizer: None,
            streams: Arc::new(Mutex::new(HashMap::new())),
            transfers: None,
            transfer_path: None,
            coordinator: None,
            coordinator_path: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            source: None,
            public_capabilities: Arc::new(Vec::new()),
            audit: None,
            federation: None,
            peer_path: None,
            knowledge_path: None,
            artifact_store: None,
            policy_revision: 0,
            admission_limits: AdmissionLimits::default(),
            next_correlation: AtomicU64::new(0),
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            catalogue_seen: Arc::new(RwLock::new(HashMap::new())),
            replay: ReplayCache::new(),
            replay_path: None,
            delegation_path: None,
            tasks: Arc::new(RwLock::new(TaskQueue::new())),
            task_path: None,
            delegation_revision: Arc::new(AtomicU64::new(0)),
            delegation_write: Arc::new(Mutex::new(())),
        }
    }

    /// Load and persistently bind the resource-lease store.
    ///
    /// Every later grant, revoke, and charge is written before it is
    /// acknowledged; a startup load drops leases that already expired.
    pub fn with_lease_persistence(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match LeaseStore::open(&path) {
            Ok(store) => {
                if let Ok(mut slot) = self.leases.write() {
                    *slot = store;
                }
            }
            Err(error) => tracing::warn!(
                "federation: lease store {} was not loaded: {error}",
                path.display()
            ),
        }
        self
    }

    /// Load and persist delegation records at `path`.
    ///
    /// A malformed file is reported and the node starts with an empty store
    /// rather than refusing to run; the file is left untouched for inspection.
    pub fn with_delegation_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match DelegationStore::load(&path) {
            Ok(store) => {
                self.delegation_revision = Arc::new(AtomicU64::new(store.revision()));
                if let Ok(mut slot) = self.delegations.write() {
                    *slot = store;
                }
            }
            Err(error) => tracing::warn!(
                "federation: delegation store {} was not loaded: {error}",
                path.display()
            ),
        }
        self.delegation_path = Some(path);
        self
    }

    /// Load and persist the peer task queue at `path`.
    ///
    /// A malformed file is reported and the node starts with an empty queue
    /// rather than refusing to run; the file is left untouched for inspection.
    pub fn with_task_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match TaskQueue::load(&path) {
            Ok(queue) => {
                if let Ok(mut slot) = self.tasks.write() {
                    *slot = queue;
                }
            }
            Err(error) => tracing::warn!(
                "federation: task queue {} was not loaded: {error}",
                path.display()
            ),
        }
        self.task_path = Some(path);
        self
    }

    /// Send federation events to `audit`.
    pub fn with_audit(mut self, audit: Arc<dyn FederationAudit>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Replace the independent invocation, control, and resource-stream budgets.
    pub fn with_admission_limits(mut self, limits: AdmissionLimits) -> Self {
        self.admission_limits = limits;
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

    /// Run the optional coordinator on this node.
    pub fn with_coordinator(mut self, coordinator: crate::coordinator::Coordinator) -> Self {
        self.coordinator = Some(Arc::new(Mutex::new(coordinator)));
        self
    }

    /// Load the coordinator membership from a file and keep it there.
    pub fn with_coordinator_persistence(
        mut self,
        path: impl Into<PathBuf>,
        admin: Option<NodeId>,
    ) -> Self {
        let path = path.into();
        match crate::coordinator::Coordinator::load(&path) {
            Ok(coordinator) => {
                let coordinator = if coordinator.admin().is_none() && admin.is_some() {
                    crate::coordinator::Coordinator::new(admin)
                } else {
                    coordinator
                };
                self.coordinator = Some(Arc::new(Mutex::new(coordinator)));
                self.coordinator_path = Some(path);
            }
            Err(error) => {
                tracing::warn!("federation: coordinator membership did not load: {error}")
            }
        }
        self
    }

    /// The coordinator this node runs, when it runs one.
    pub fn coordinator(&self) -> Option<Arc<Mutex<crate::coordinator::Coordinator>>> {
        self.coordinator.clone()
    }

    /// Persist resource-transfer checkpoints, so a restart resumes at the last
    /// acknowledged byte.
    pub fn with_transfer_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match crate::resource::TransferStore::load(&path) {
            Ok(store) => {
                self.transfers = Some(Arc::new(Mutex::new(store)));
                self.transfer_path = Some(path);
            }
            Err(error) => {
                tracing::warn!("federation: transfer checkpoints did not load: {error}");
            }
        }
        self
    }

    /// The transfer checkpoints this node keeps, when configured.
    pub fn transfers(&self) -> Option<Arc<Mutex<crate::resource::TransferStore>>> {
        self.transfers.clone()
    }

    /// Load and persist the replay/idempotency cache across restarts.
    ///
    /// Expired entries are dropped at load. A write failure is logged and the
    /// frame is still processed: replay protection weakens after a write error,
    /// but an authenticated request is not silently dropped because the disk
    /// failed. The file is written only for request kinds, after admission.
    pub fn with_replay_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match ReplayCache::load(&path) {
            Ok(cache) => self.replay = cache,
            Err(error) => tracing::warn!(
                "federation: replay store {} was not loaded: {error}",
                path.display()
            ),
        }
        self.replay_path = Some(path);
        self
    }

    /// Advertise federation membership and the policy revision peers should see.
    pub fn with_federation(mut self, federation: FederationId, policy_revision: u64) -> Self {
        self.federation = Some(federation);
        self.policy_revision = policy_revision;
        self
    }

    /// The federation this Node advertises, if configured.
    pub fn federation(&self) -> Option<&FederationId> {
        self.federation.as_ref()
    }

    /// Persist a peer rotation back to the peer file.
    pub fn with_peer_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        self.peer_path = Some(path.into());
        self
    }

    /// Install the host store used by artifact_cleanup.
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Persist the local knowledge store after each publication or revocation.
    pub fn with_knowledge_persistence(mut self, path: impl Into<PathBuf>) -> Self {
        self.knowledge_path = Some(path.into());
        self
    }

    /// Enable the knowledge protocol with a host-governed store and policy.
    pub fn with_knowledge(
        mut self,
        knowledge: Arc<RwLock<KnowledgeService>>,
        authorizer: Arc<dyn KnowledgeAuthorizer>,
    ) -> Self {
        self.knowledge = Some(knowledge);
        self.knowledge_authorizer = Some(authorizer);
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

    /// Revoke a peer and cancel the delegations it submitted.
    ///
    /// Revocation is immediate for new calls (every authorization re-reads the
    /// shared store); this also stops the work the peer already started, and
    /// persists the cancelled records for the audit trail.
    pub fn revoke_peer(&self, peer: &NodeId, now_ms: u64) -> usize {
        {
            let mut peers = self
                .peers
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            peers.revoke(peer);
        }
        let mut store = self
            .delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cancelled = store.cancel_from(peer, now_ms);
        if cancelled > 0 {
            self.persist_delegations(&store);
        }
        cancelled
    }

    /// Read the coordinator's current topology.
    pub async fn coordinator_topology(
        &self,
        coordinator: &NodeId,
    ) -> Result<crate::coordinator::Topology, CapabilityError> {
        let reply = self
            .round_trip(coordinator, "coordinator_topology", json!({}), None)
            .await?;
        decode_topology(reply)
    }

    /// Ask the coordinator to register (or update) one member.
    ///
    /// Only the coordinator's configured administrator may do this; the
    /// authenticated sender is the actor the coordinator checks.
    pub async fn coordinator_register(
        &self,
        coordinator: &NodeId,
        member: crate::coordinator::MemberRecord,
    ) -> Result<crate::coordinator::Topology, CapabilityError> {
        let reply = self
            .round_trip(
                coordinator,
                "coordinator_register",
                json!({ "member": member }),
                None,
            )
            .await?;
        decode_topology(reply)
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

    /// The peer-owned task queue this node records for other nodes.
    pub fn tasks(&self) -> Arc<RwLock<TaskQueue>> {
        Arc::clone(&self.tasks)
    }

    /// Claim accepted objective-only delegations for this node's shell to run.
    ///
    /// A delegation with a structured contract executes in the node itself; an
    /// objective alone needs a model, so the peer's shell claims it here. The
    /// claim moves the record to Running and persists it, so a crash after the
    /// claim is visible rather than silently re-run.
    pub fn claim_objective_delegations(&self, now_ms: u64) -> Vec<DelegationRecord> {
        let mut store = self
            .delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ids: Vec<DelegationId> = store
            .iter()
            .filter(|record| {
                record.state == DelegationState::Accepted
                    && record.request.result_contract.is_none()
            })
            .map(|record| record.request.id.clone())
            .collect();
        let mut claimed = Vec::new();
        let mut changed = false;
        for id in ids {
            // An expired bound must not be claimed: the record fails here
            // instead of executing without its deadline.
            if store
                .record(&id)
                .and_then(|record| record.request.deadline_ms)
                .is_some_and(|deadline| deadline <= now_ms)
            {
                let _ = store.fail(
                    &id,
                    "the delegation deadline has expired".to_string(),
                    now_ms,
                );
                changed = true;
                continue;
            }
            if store
                .transition(&id, DelegationState::Running, now_ms)
                .is_ok()
            {
                changed = true;
                if let Some(record) = store.record(&id) {
                    claimed.push(record.clone());
                }
            }
        }
        if changed {
            self.persist_delegations(&store);
        }
        claimed
    }

    /// Record an objective delegation's outcome and persist it.
    pub fn settle_objective_delegation(
        &self,
        id: &str,
        result: Option<Value>,
        error: Option<String>,
    ) -> bool {
        let now = unix_ms();
        let delegation = DelegationId::new(id.to_string());
        let mut store = self
            .delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let done = match error {
            Some(error) => store.fail(&delegation, error, now).is_ok(),
            None => store
                .succeed(&delegation, result.unwrap_or(Value::Null), now)
                .is_ok(),
        };
        if done {
            self.persist_delegations(&store);
        }
        done
    }

    /// Remove finished delegations, or every record when none is active.
    pub fn clear_delegations(&self, all: bool) -> Result<usize, CapabilityError> {
        let mut store = self
            .delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = if all {
            store.clear_all()
        } else {
            store.clear_finished()
        };
        if removed > 0 {
            self.persist_delegations(&store);
        }
        Ok(removed)
    }

    /// Remove one delegation record and persist the result.
    pub fn clear_delegation(&self, id: &DelegationId) -> bool {
        let mut store = self
            .delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = store.clear_one(id);
        if removed {
            self.persist_delegations(&store);
        }
        removed
    }

    /// Write the delegation store through the same gate the request handlers use.
    fn persist_delegations(&self, store: &DelegationStore) {
        let Some(path) = &self.delegation_path else {
            return;
        };
        let _gate = self
            .delegation_write
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match store.save(path) {
            Ok(()) => {
                self.delegation_revision
                    .store(store.revision(), std::sync::atomic::Ordering::Relaxed);
            }
            Err(error) => tracing::warn!("federation: delegation store was not written: {error}"),
        }
    }
    /// Persist the peer-task queue now, for a direct create or edit.
    pub fn persist_tasks(&self) {
        let queue = self
            .tasks
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(path) = &self.task_path {
            if let Err(error) = queue.save(path) {
                tracing::warn!("federation: task queue was not written: {error}");
            }
        }
    }

    /// Claim the peer tasks due now, persisting the Running marks.
    pub fn claim_due_tasks(&self, now_ms: u64) -> Vec<FederatedTask> {
        let mut queue = self
            .tasks
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let due = queue.claim_due(now_ms);
        if !due.is_empty() {
            if let Some(path) = &self.task_path {
                if let Err(error) = queue.save(path) {
                    tracing::warn!("federation: task queue was not written: {error}");
                }
            }
        }
        due
    }

    /// Settle one claimed peer task and persist the queue.
    pub fn settle_task(
        &self,
        id: &str,
        ok: bool,
        result: Option<String>,
        error: Option<String>,
    ) -> Result<(), crate::task::TaskQueueError> {
        let now = unix_ms();
        let mut queue = self
            .tasks
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.settle(id, ok, result, error, now)?;
        if let Some(path) = &self.task_path {
            if let Err(error) = queue.save(path) {
                tracing::warn!("federation: task queue was not written: {error}");
            }
        }
        Ok(())
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
        let knowledge = self.knowledge.clone();
        let knowledge_authorizer = self.knowledge_authorizer.clone();
        let knowledge_path = self.knowledge_path.clone();
        let artifact_store = self.artifact_store.clone();
        let streams = Arc::clone(&self.streams);
        let coordinator = self.coordinator.clone();
        let coordinator_path = self.coordinator_path.clone();
        let sessions = Arc::clone(&self.sessions);
        let source = self.source.clone();
        let public_capabilities = Arc::clone(&self.public_capabilities);
        let federation = self.federation.clone();
        let peer_path = self.peer_path.clone();
        let policy_revision = self.policy_revision;
        let audit = self.audit.clone();
        let in_flight = Arc::clone(&self.in_flight);
        let replay = self.replay.clone();
        let replay_path = self.replay_path.clone();
        let admission_limits = self.admission_limits;
        let admission = Admission::new(admission_limits);
        let stream_limit = admission_limits.resource_streams.max(1);
        let delegation_path = self.delegation_path.clone();
        let delegation_revision = Arc::clone(&self.delegation_revision);
        let delegation_write = Arc::clone(&self.delegation_write);
        let tasks = Arc::clone(&self.tasks);
        let task_path = self.task_path.clone();
        tokio::spawn(async move {
            while let Some(frame) = inbox.recv().await {
                let Ok(envelope) = Envelope::decode(&frame) else {
                    continue;
                };
                let now = unix_ms();
                if envelope.open(now).is_err() {
                    continue;
                }
                if envelope.recipient != identity.node_id() {
                    record(
                        &audit,
                        FederationEvent::new(
                            Direction::Inbound,
                            envelope.sender.as_str(),
                            envelope.kind.as_str(),
                            None,
                            "denied",
                        )
                        .with_subject(envelope.message_id.clone())
                        .with_reason("the envelope is addressed to another Node"),
                    );
                    continue;
                }
                if envelope.kind != "result" {
                    let mut keys = vec![(envelope.sender.clone(), envelope.message_id.clone())];
                    if let Some(idempotency) = &envelope.idempotency_key {
                        keys.push((
                            envelope.sender.clone(),
                            format!("idem:{}:{idempotency}", envelope.kind),
                        ));
                    }
                    let expires = envelope
                        .expires_at_ms
                        .unwrap_or_else(|| now.saturating_add(60_000));
                    if !replay.admit(&keys, expires, now) {
                        record(
                            &audit,
                            FederationEvent::new(
                                Direction::Inbound,
                                envelope.sender.as_str(),
                                "replay",
                                None,
                                "duplicate",
                            )
                            .with_subject(envelope.message_id.clone()),
                        );
                        if matches!(
                            envelope.kind.as_str(),
                            "invoke"
                                | "delegate"
                                | "delegation_status"
                                | "delegation_cancel"
                                | "discover"
                                | "invoke_cancel"
                                | "task_create"
                                | "task_status"
                                | "task_cancel"
                        ) {
                            let reply = seal_reply(
                                &identity,
                                &envelope,
                                error_payload(CapabilityError::Duplicate {
                                    message: format!(
                                        "message {} was already accepted",
                                        envelope.message_id
                                    ),
                                }),
                            );
                            let _ = transport.send(&envelope.sender, reply).await;
                        }
                        continue;
                    }
                    if matches!(
                        envelope.kind.as_str(),
                        "invoke"
                            | "delegate"
                            | "delegation_status"
                            | "delegation_cancel"
                            | "discover"
                            | "invoke_cancel"
                            | "knowledge_query"
                            | "knowledge_publication"
                            | "task_create"
                            | "task_status"
                            | "task_cancel"
                            | "coordinator_register"
                            | "coordinator_topology"
                    ) {
                        if let Some(path) = &replay_path {
                            if let Err(error) = replay.save(path) {
                                tracing::warn!(
                                    "federation: replay cache {} was not written: {error}",
                                    path.display()
                                );
                            }
                        }
                    }
                }
                let handled = matches!(
                    envelope.kind.as_str(),
                    "invoke"
                        | "delegate"
                        | "delegation_status"
                        | "delegation_cancel"
                        | "invoke_cancel"
                        | "discover"
                        | "knowledge_query"
                        | "knowledge_publication"
                        | "artifact_cleanup"
                        | "rotation"
                        | "task_create"
                        | "task_status"
                        | "task_cancel"
                        | "coordinator_register"
                        | "coordinator_topology"
                );
                let frame_bytes = frame.len() as u64;
                if handled && frame_bytes > kind_payload_limit(&envelope.kind) {
                    let reply = seal_reply(
                        &identity,
                        &envelope,
                        error_payload(CapabilityError::Invalid {
                            message: format!(
                                "a {} frame may carry at most {} bytes",
                                envelope.kind,
                                kind_payload_limit(&envelope.kind)
                            ),
                        }),
                    );
                    let _ = transport.send(&envelope.sender, reply).await;
                    continue;
                }
                let permit = if handled {
                    admission.try_admit(&envelope.sender, &envelope.kind, frame_bytes)
                } else {
                    None
                };
                if handled && permit.is_none() {
                    let reply = seal_reply(
                        &identity,
                        &envelope,
                        error_payload(CapabilityError::RateLimited {
                            retry_after_ms: Some(50),
                        }),
                    );
                    let _ = transport.send(&envelope.sender, reply).await;
                    continue;
                }
                match envelope.kind.as_str() {
                    "result" => {
                        let Some(correlation) = envelope.correlation.clone() else {
                            continue;
                        };
                        let waiter = {
                            let mut pending = pending
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            match pending.get(&correlation) {
                                Some(waiter) if waiter.peer == envelope.sender => {
                                    pending.remove(&correlation)
                                }
                                _ => None,
                            }
                        };
                        match waiter {
                            Some(waiter) => {
                                let _ = waiter.sender.send(envelope);
                            }
                            None => {
                                record(
                                    &audit,
                                    FederationEvent::new(
                                        Direction::Inbound,
                                        envelope.sender.as_str(),
                                        "result",
                                        None,
                                        "denied",
                                    )
                                    .with_subject(correlation)
                                    .with_reason("no pending request expects this peer"),
                                );
                            }
                        }
                    }
                    "invoke" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let in_flight = Arc::clone(&in_flight);
                        let runtime = runtime.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply =
                                serve(&identity, runtime, &peers, &in_flight, &audit, &envelope)
                                    .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "invoke_cancel" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let in_flight = Arc::clone(&in_flight);
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_invoke_cancel(
                                &identity, &peers, &in_flight, &audit, &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "delegate" | "delegation_status" | "delegation_cancel" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let delegations = Arc::clone(&delegations);
                        let audit = audit.clone();
                        let journal = DelegationJournal {
                            path: delegation_path.clone(),
                            revision: Arc::clone(&delegation_revision),
                            gate: Arc::clone(&delegation_write),
                        };
                        let runtime = runtime.clone();
                        let kind = envelope.kind.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = match kind.as_str() {
                                "delegate" => {
                                    serve_delegate(
                                        &identity,
                                        &peers,
                                        &delegations,
                                        &runtime,
                                        &journal,
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
                            journal.persist(&delegations);
                        });
                    }
                    "task_create" | "task_status" | "task_cancel" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let tasks = Arc::clone(&tasks);
                        let task_path = task_path.clone();
                        let audit = audit.clone();
                        let kind = envelope.kind.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = match kind.as_str() {
                                "task_create" => {
                                    serve_task_create(
                                        &identity, &peers, &tasks, &task_path, &audit, &envelope,
                                    )
                                    .await
                                }
                                "task_status" => {
                                    serve_task_status(&identity, &peers, &tasks, &audit, &envelope)
                                        .await
                                }
                                _ => {
                                    serve_task_cancel(
                                        &identity, &peers, &tasks, &task_path, &audit, &envelope,
                                    )
                                    .await
                                }
                            };
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "coordinator_register" | "coordinator_topology" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let coordinator = coordinator.clone();
                        let coordinator_path = coordinator_path.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_coordinator(
                                &identity,
                                &peers,
                                &coordinator,
                                &coordinator_path,
                                &audit,
                                &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "discover" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let public_capabilities = Arc::clone(&public_capabilities);
                        let federation = federation.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_discover(
                                &identity,
                                &peers,
                                &public_capabilities,
                                &federation,
                                policy_revision,
                                &audit,
                                &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "artifact_cleanup" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let store = artifact_store.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_artifact_cleanup(
                                &identity, &peers, &store, &audit, &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "rotation" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let peer_path = peer_path.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply =
                                serve_rotation(&identity, &peers, &peer_path, &audit, &envelope)
                                    .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "knowledge_query" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let knowledge = knowledge.clone();
                        let authorizer = knowledge_authorizer.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_knowledge_query(
                                &identity,
                                &peers,
                                &knowledge,
                                &authorizer,
                                &audit,
                                &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "knowledge_publication" => {
                        let identity = identity.clone();
                        let transport = Arc::clone(&transport);
                        let peers = Arc::clone(&peers);
                        let knowledge = knowledge.clone();
                        let authorizer = knowledge_authorizer.clone();
                        let knowledge_path = knowledge_path.clone();
                        let audit = audit.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let reply = serve_knowledge_publication(
                                &identity,
                                &peers,
                                &knowledge,
                                &authorizer,
                                &knowledge_path,
                                &audit,
                                &envelope,
                            )
                            .await;
                            let _ = transport.send(&envelope.sender, reply).await;
                        });
                    }
                    "resource" => {
                        // Stream frames are routed inline: the stream queue is
                        // already bounded, and dropping one for handler admission
                        // would corrupt the stream.
                        route_resource(
                            identity.clone(),
                            Arc::clone(&transport),
                            Arc::clone(&leases),
                            Arc::clone(&streams),
                            Arc::clone(&sessions),
                            source.clone(),
                            &audit,
                            stream_limit,
                            envelope,
                        )
                        .await;
                    }
                    _ => {}
                }
            }
        })
    }

    /// How many replies this Node is still waiting for.
    ///
    /// Every terminal path of a round trip removes its own entry, so this is
    /// also the observable that a timeout, a failed send, or a cancelled caller
    /// did not leave a waiter behind.
    pub fn pending_requests(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// How many remote invocations this Node is running right now.
    pub fn in_flight_invocations(&self) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Invoke one capability on a peer.
    pub async fn invoke(
        &self,
        peer: &NodeId,
        capability: &CapabilityId,
        input: Value,
        context: &InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        self.invoke_scoped(peer, capability, input, None, context)
            .await
    }

    /// Invoke with an explicit operation/resource scope.
    ///
    /// A scoped grant on either side narrows the call to these values; the
    /// scope travels in the payload so the provider can enforce its own grant.
    pub async fn invoke_scoped(
        &self,
        peer: &NodeId,
        capability: &CapabilityId,
        input: Value,
        scope: Option<&InvocationScope>,
        context: &InvocationContext,
    ) -> Result<CapabilityOutput, CapabilityError> {
        let started = Instant::now();
        let capability_text = capability.to_string();
        // Caller-side decision: this kernel must allow its own principal to
        // use the peer for this capability.
        let operation = scope.and_then(|scope| scope.operation.as_deref());
        let resource = scope.and_then(|scope| scope.resource.as_deref());
        let workspace = scope.and_then(|scope| scope.workspace.as_deref());
        let authorized = self
            .peers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authorize_scoped(peer, capability, unix_ms(), operation, resource, workspace);
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
            self.audit(correlate_context(event, context));
            return Err(authorized.expect_err("checked above"));
        }

        let deadline_ms = context.deadline.map(|deadline| {
            unix_ms().saturating_add(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64,
            )
        });
        let mut payload = json!({
            "capability": capability_text,
            "input": input,
            "deadline_ms": deadline_ms,
            "task": context.task.as_ref().map(TaskId::as_str),
            "trace": {
                "trace_id": context.trace.trace_id,
                "span_id": context.trace.span_id,
            },
            "delegation": context
                .delegation
                .iter()
                .map(|hop| json!({ "node": hop.node, "delegation": hop.delegation }))
                .collect::<Vec<_>>(),
            "principal": principal_label(&context.principal),
            "workspace": context.workspace.to_string(),
        });
        if let Some(scope) = scope {
            if let Some(operation) = &scope.operation {
                payload["operation"] = json!(operation);
            }
            if let Some(resource) = &scope.resource {
                payload["resource"] = json!(resource);
            }
        }
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
                self.audit(correlate_context(
                    event.with_elapsed_ms(started.elapsed().as_millis() as u64),
                    context,
                ));
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
        self.audit(correlate_context(event, context));
        decoded
    }

    /// Query a peer's governed knowledge.
    ///
    /// The peer's authorizer decides; a refusal is a Denied, a stale query is a
    /// Timeout, and the returned items keep their remote provenance.
    pub async fn query_knowledge(
        &self,
        peer: &NodeId,
        query: &KnowledgeQuery,
        context: &InvocationContext,
    ) -> Result<KnowledgeResponse, CapabilityError> {
        query.validate().map_err(CapabilityError::from)?;
        let payload = serde_json::to_value(query).map_err(|error| CapabilityError::Invalid {
            message: error.to_string(),
        })?;
        let reply = self
            .round_trip(peer, "knowledge_query", payload, context.deadline)
            .await?;
        decode_knowledge_response(&reply)
    }

    /// Publish knowledge to a peer that authorizes this publisher.
    pub async fn publish_knowledge(
        &self,
        peer: &NodeId,
        publication: &KnowledgePublication,
        context: &InvocationContext,
    ) -> Result<usize, CapabilityError> {
        publication.validate().map_err(CapabilityError::from)?;
        let payload =
            serde_json::to_value(publication).map_err(|error| CapabilityError::Invalid {
                message: error.to_string(),
            })?;
        let reply = self
            .round_trip(peer, "knowledge_publication", payload, context.deadline)
            .await?;
        decode_knowledge_accepted(&reply)
    }

    /// Ask a peer to remove the bytes behind artifact references.
    pub async fn cleanup_artifacts(
        &self,
        peer: &NodeId,
        request: &ArtifactCleanupRequest,
        context: &InvocationContext,
    ) -> Result<ArtifactCleanupResponse, CapabilityError> {
        request.validate()?;
        let payload = serde_json::to_value(request).map_err(|error| CapabilityError::Invalid {
            message: error.to_string(),
        })?;
        let reply = self
            .round_trip(peer, "artifact_cleanup", payload, context.deadline)
            .await?;
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(wire_error(&reply, None));
        }
        let response = reply
            .get("response")
            .cloned()
            .ok_or_else(|| CapabilityError::Invalid {
                message: "artifact cleanup reply carries no response".to_string(),
            })?;
        serde_json::from_value(response).map_err(|error| CapabilityError::Invalid {
            message: error.to_string(),
        })
    }

    /// Send a signed key-rotation record to a peer.
    ///
    /// The envelope is signed by the successor key; the record itself is signed
    /// by the old key, which is what the peer verifies before moving grants.
    pub async fn publish_rotation(
        &self,
        peer: &NodeId,
        record: &RotationRecord,
        context: &InvocationContext,
    ) -> Result<(), CapabilityError> {
        let payload = serde_json::to_value(record).map_err(|error| CapabilityError::Invalid {
            message: error.to_string(),
        })?;
        let reply = self
            .round_trip(peer, "rotation", payload, context.deadline)
            .await?;
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(wire_error(&reply, None))
        }
    }

    /// The live knowledge items this Node serves, for a local management view.
    pub fn knowledge_items(&self) -> Vec<KnowledgeItem> {
        let Some(service) = &self.knowledge else {
            return Vec::new();
        };
        service
            .read()
            .map(|service| service.items().cloned().collect())
            .unwrap_or_default()
    }

    /// Revoke one item this Node published, and persist the result.
    pub fn revoke_knowledge(&self, id: &str) -> Result<bool, CapabilityError> {
        let Some(service) = &self.knowledge else {
            return Err(CapabilityError::Unavailable {
                reason: "this Node serves no knowledge".to_string(),
            });
        };
        let mut guard = service
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let revoked = guard.revoke(&self.node(), id)?;
        if revoked {
            if let Some(path) = &self.knowledge_path {
                persist_knowledge(&guard, path);
            }
        }
        Ok(revoked)
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
        if peer == &self.node() {
            return Err(CapabilityError::Invalid {
                message: "a Node cannot delegate to itself".to_string(),
            });
        }
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

    /// Ask a peer to own a task. The peer records it; its shell runs it.
    pub async fn create_task(
        &self,
        peer: &NodeId,
        request: TaskCreateRequest,
        context: &InvocationContext,
    ) -> Result<FederatedTask, CapabilityError> {
        let started = Instant::now();
        let subject = request.id.clone();
        let result = self.create_task_inner(peer, request, context).await;
        self.record_outcome("task_create", peer, Some(subject), started, &result);
        result
    }

    async fn create_task_inner(
        &self,
        peer: &NodeId,
        request: TaskCreateRequest,
        context: &InvocationContext,
    ) -> Result<FederatedTask, CapabilityError> {
        self.require_trusted(peer)?;
        let payload = self
            .round_trip(
                peer,
                "task_create",
                json!({ "request": request }),
                context.deadline,
            )
            .await?;
        decode_task(&payload)
    }

    /// Read a peer-owned task.
    pub async fn task_status(
        &self,
        peer: &NodeId,
        id: &str,
        context: &InvocationContext,
    ) -> Result<FederatedTask, CapabilityError> {
        let request = TaskIdRequest { id: id.to_string() };
        self.require_trusted(peer)?;
        let payload = self
            .round_trip(
                peer,
                "task_status",
                json!({ "request": request }),
                context.deadline,
            )
            .await?;
        decode_task(&payload)
    }

    /// Cancel a peer-owned task.
    pub async fn cancel_task(
        &self,
        peer: &NodeId,
        id: &str,
        context: &InvocationContext,
    ) -> Result<FederatedTask, CapabilityError> {
        let request = TaskIdRequest { id: id.to_string() };
        self.require_trusted(peer)?;
        let payload = self
            .round_trip(
                peer,
                "task_cancel",
                json!({ "request": request }),
                context.deadline,
            )
            .await?;
        decode_task(&payload)
    }

    /// Refuse a peer this node does not trust.
    fn require_trusted(&self, peer: &NodeId) -> Result<(), CapabilityError> {
        let now = unix_ms();
        let peers = self
            .peers
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if peers.is_trusted(peer, now) {
            Ok(())
        } else {
            Err(CapabilityError::Denied {
                reason: format!("peer {peer} is not enrolled"),
                policy: None,
            })
        }
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

        // A restart resumes from the checkpoint unless the caller named an
        // explicit nonzero offset.
        let (resume, transfers) = match &self.transfers {
            Some(store) => {
                let stored = store
                    .lock()
                    .map(|store| store.position(lease, uri))
                    .unwrap_or(0);
                (
                    crate::resource::resume_offset(offset, stored),
                    Some(Arc::clone(store)),
                )
            }
            None => (offset, None),
        };
        if let Some(store) = &transfers {
            if let Ok(mut store) = store.lock() {
                store.checkpoint(crate::resource::TransferRecord {
                    lease: lease.as_str().to_string(),
                    uri: uri.to_string(),
                    peer: peer.to_string(),
                    position: resume,
                    updated_at_ms: now,
                });
                if let Some(path) = &self.transfer_path {
                    let _ = store.save(path);
                }
            }
        }

        let (sender, receiver) = tokio::sync::mpsc::channel(RESOURCE_QUEUE);
        {
            let mut streams = self
                .streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let entry = InboundStream {
                peer: peer.clone(),
                sender,
                next_seq: 0,
                open: true,
            };
            if streams.insert(lease.clone(), entry).is_some() {
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
                offset: resume,
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
            streams: Arc::clone(&self.streams),
            audit: self.audit.clone(),
            received: 0,
            settled: false,
            offset: resume,
            uri: uri.to_string(),
            transfers,
            transfer_path: self.transfer_path.clone(),
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
        if &record.node != peer {
            return Err(CapabilityError::Denied {
                reason: format!("the record names {}, not {peer}", record.node),
                policy: None,
            });
        }
        let known = self
            .catalogue_seen
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(peer)
            .copied();
        record
            .check(unix_ms(), known)
            .map_err(|error| CapabilityError::StaleDiscovery {
                reason: error.to_string(),
            })?;
        self.catalogue_seen
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(peer.clone(), record.published_at_ms);
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
        let correlation = format!(
            "{}-{}-{}",
            self.identity.node_id(),
            unix_ms(),
            self.next_correlation.fetch_add(1, Ordering::Relaxed)
        );
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
            .insert(
                correlation.clone(),
                PendingReply {
                    peer: peer.clone(),
                    sender,
                },
            );
        // One guard owns cleanup for every exit: a failed send, a timeout, a
        // dropped caller, and the success path (where routing already removed
        // the entry, so the second removal is a no-op).
        let mut guard = PendingGuard {
            pending: Arc::clone(&self.pending),
            correlation: correlation.clone(),
            cancel: Some(CancelNotice {
                transport: Arc::clone(&self.transport),
                identity: self.identity.clone(),
                peer: peer.clone(),
                correlation: correlation.clone(),
            }),
            armed: true,
        };

        let delivered = self.transport.send(peer, frame).await?;
        if delivered == DeliveryState::NotSent {
            guard.disarm();
            return Err(CapabilityError::Transport {
                message: "the request never left this Node".to_string(),
                delivered,
            });
        }

        let wait = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(DEFAULT_REPLY_TIMEOUT);
        match tokio::time::timeout(wait, receiver).await {
            Ok(Ok(envelope)) => {
                guard.disarm();
                Ok(envelope.payload.clone())
            }
            Ok(Err(_)) => {
                guard.disarm();
                Err(CapabilityError::UnknownOutcome {
                    message: "the reply channel closed".to_string(),
                })
            }
            Err(_) => Err(CapabilityError::UnknownOutcome {
                message: "no reply within the deadline; the call may have executed".to_string(),
            }),
        }
    }
}

async fn serve(
    identity: &NodeIdentity,
    runtime: Option<Arc<WorkspaceRuntime>>,
    peers: &Arc<RwLock<PeerStore>>,
    in_flight: &Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
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
            correlate_payload(
                FederationEvent::new(Direction::Inbound, caller, "invoke", None, "invalid")
                    .with_reason("the request does not name a capability"),
                payload,
            ),
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
    // A scoped grant narrows the capability to operations and resources; a
    // scoped grant refuses a call that does not say what it touches.
    let operation = payload.get("operation").and_then(Value::as_str);
    let resource = payload.get("resource").and_then(Value::as_str);
    let workspace = payload.get("workspace").and_then(Value::as_str);
    let authorized = peers
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .authorize_scoped(
            &envelope.sender,
            &capability,
            unix_ms(),
            operation,
            resource,
            workspace,
        );
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
        record(audit, correlate_payload(event, payload));
        return seal_reply(
            identity,
            envelope,
            error_payload(authorized.expect_err("checked")),
        );
    }

    let Some(runtime) = runtime else {
        record(
            audit,
            correlate_payload(
                FederationEvent::new(
                    Direction::Inbound,
                    caller,
                    "invoke",
                    Some(&capability.to_string()),
                    "unavailable",
                )
                .with_reason("this Node serves no capabilities"),
                payload,
            ),
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
    let mut context = InvocationContext::local(
        InvocationId::new(envelope.message_id.clone()),
        Principal::Remote {
            node: envelope.sender.clone(),
        },
        runtime.workspace().clone(),
    );
    if let Some(task) = payload.get("task").and_then(Value::as_str) {
        context.task = Some(TaskId::new(task));
    }
    if let Some(trace) = payload.get("trace") {
        let trace_id = trace
            .get("trace_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let span_id = trace
            .get("span_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !trace_id.is_empty() || !span_id.is_empty() {
            context.trace = TraceContext::new(trace_id, span_id);
        }
    }
    if let Some(hops) = payload.get("delegation").and_then(Value::as_array) {
        for hop in hops {
            let Some(node) = hop
                .get("node")
                .and_then(Value::as_str)
                .and_then(|value| NodeId::new(value).ok())
            else {
                continue;
            };
            let delegation = hop
                .get("delegation")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            context.delegation.push(DelegationHop { node, delegation });
        }
    }
    if let Some(deadline) = payload.get("deadline_ms").and_then(Value::as_u64) {
        let now = unix_ms();
        if deadline <= now {
            record(
                audit,
                correlate_payload(
                    FederationEvent::new(
                        Direction::Inbound,
                        caller,
                        "invoke",
                        Some(&capability.to_string()),
                        "timeout",
                    )
                    .with_reason("the caller's deadline had already passed"),
                    payload,
                ),
            );
            return seal_reply(
                identity,
                envelope,
                error_payload(CapabilityError::Timeout {
                    elapsed_ms: 0,
                    partial: None,
                }),
            );
        }
        context.deadline = Some(Instant::now() + Duration::from_millis(deadline - now));
    }
    // Register before the provider runs, with the caller that may cancel it.
    let key = envelope
        .correlation
        .clone()
        .unwrap_or_else(|| envelope.message_id.clone());
    let token = CancelToken::new();
    context.cancel = token.clone();
    in_flight
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key.clone(), (envelope.sender.clone(), token));
    let _flight = InFlightGuard {
        in_flight: Arc::clone(in_flight),
        key,
    };
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
    event = correlate_payload(event, payload);
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
        CapabilityError::StaleDiscovery { reason } => ("stale_discovery", reason),
        CapabilityError::Duplicate { message } => ("duplicate", message),
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
        crate::resource::ResourceError::Storage { .. }
        | crate::resource::ResourceError::Checkpoint { .. } => "storage",
    }
}

fn error_kind(error: &CapabilityError) -> &'static str {
    match error {
        CapabilityError::Invalid { .. } => "invalid",
        CapabilityError::Denied { .. } => "denied",
        CapabilityError::NotFound { .. } => "not_found",
        CapabilityError::DelegationNotFound { .. } => "delegation_not_found",
        CapabilityError::Unavailable { .. } => "unavailable",
        CapabilityError::StaleDiscovery { .. } => "stale_discovery",
        CapabilityError::Duplicate { .. } => "duplicate",
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
        | CapabilityError::StaleDiscovery { reason }
        | CapabilityError::Duplicate { message: reason }
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

fn persist_knowledge(service: &KnowledgeService, path: &std::path::Path) {
    let Ok(text) = service.to_json() else {
        tracing::warn!("federation: knowledge store did not serialize");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    let written = std::fs::write(&temporary, text).and_then(|()| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temporary, path)
    });
    if let Err(error) = written {
        tracing::warn!(
            "federation: knowledge store {} was not written: {error}",
            path.display()
        );
    }
}

/// Attach the correlation a request payload carries to an audit event.
///
/// Delegation frames nest their fields under `request`; both shapes are read so
/// a delegation row joins the same trace as the invoke that started it.
fn correlate_payload(mut event: FederationEvent, payload: &Value) -> FederationEvent {
    let payload = if payload.get("trace").is_some() || payload.get("task").is_some() {
        payload
    } else {
        payload.get("request").unwrap_or(payload)
    };
    if let Some(trace) = payload.get("trace") {
        if let (Some(trace_id), Some(span_id)) = (
            trace.get("trace_id").and_then(Value::as_str),
            trace.get("span_id").and_then(Value::as_str),
        ) {
            event = event.with_trace(trace_id, span_id);
        }
    }
    if let Some(task) = payload.get("task").and_then(Value::as_str) {
        event = event.with_task(task);
    }
    if let Some(trace_id) = payload.get("trace_id").and_then(Value::as_str) {
        let span = payload
            .get("span_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        event = event.with_trace(trace_id, span);
    }
    if let Some(delegation) = payload
        .get("delegation")
        .and_then(Value::as_array)
        .and_then(|hops| hops.last())
        .and_then(|hop| hop.get("delegation"))
        .and_then(Value::as_str)
    {
        event = event.with_delegation(delegation);
    }
    if let Some(principal) = payload.get("principal") {
        let label = principal
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| principal.to_string());
        event = event.with_principal(label);
    }
    event
}

/// Attach a local invocation's correlation to an audit event.
fn correlate_context(mut event: FederationEvent, context: &InvocationContext) -> FederationEvent {
    event = event.with_trace(
        context.trace.trace_id.clone(),
        context.trace.span_id.clone(),
    );
    if let Some(task) = &context.task {
        event = event.with_task(task.as_str());
    }
    if let Some(hop) = context.delegation.last() {
        event = event.with_delegation(hop.delegation.clone());
    }
    event = event.with_principal(principal_label(&context.principal));
    event
}

/// A stable, non-secret label for a principal.
fn principal_label(principal: &Principal) -> String {
    match principal {
        Principal::Local { session, preset } => format!("local:{session}/{preset}"),
        Principal::Remote { node } => format!("remote:{node}"),
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

fn decode_task(payload: &Value) -> Result<FederatedTask, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return serde_json::from_value(payload.get("task").cloned().unwrap_or(Value::Null))
            .map_err(|error| CapabilityError::Invalid {
                message: format!("the peer returned an unreadable task: {error}"),
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
        "stale_discovery" => CapabilityError::StaleDiscovery { reason: message },
        "duplicate" => CapabilityError::Duplicate { message },
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

async fn serve_invoke_cancel(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    in_flight: &Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_invoke_cancel_inner(identity, peers, in_flight, envelope).await;
    record_reply(audit, envelope, "invoke_cancel", started, &reply);
    reply
}

async fn serve_invoke_cancel_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    in_flight: &Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
    envelope: &Envelope,
) -> Vec<u8> {
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let key = envelope
        .payload
        .get("correlation")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| envelope.correlation.clone())
        .unwrap_or_else(|| envelope.message_id.clone());
    let found = in_flight
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    match found {
        Some((peer, token)) if peer == envelope.sender => {
            token.cancel();
            seal_reply(identity, envelope, json!({ "ok": true, "cancelled": true }))
        }
        Some(_) => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: "the invocation belongs to another peer".to_string(),
                policy: None,
            }),
        ),
        // Cancellation is idempotent: a late or duplicate cancel is acknowledged.
        None => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "cancelled": false }),
        ),
    }
}

fn decode_knowledge_response(payload: &Value) -> Result<KnowledgeResponse, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        let response = payload.get("response").cloned().unwrap_or(Value::Null);
        return serde_json::from_value(response).map_err(|error| CapabilityError::Invalid {
            message: format!("the peer returned an unreadable knowledge response: {error}"),
        });
    }
    Err(wire_error(payload, None))
}

fn decode_knowledge_accepted(payload: &Value) -> Result<usize, CapabilityError> {
    if payload.get("ok").and_then(Value::as_bool) == Some(true) {
        return payload
            .get("accepted")
            .and_then(Value::as_u64)
            .map(|count| count as usize)
            .ok_or_else(|| CapabilityError::Invalid {
                message: "the peer did not report an accepted count".to_string(),
            });
    }
    Err(wire_error(payload, None))
}

async fn serve_rotation(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    peer_path: &Option<PathBuf>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let record: RotationRecord = match serde_json::from_value(envelope.payload.clone()) {
        Ok(record) => record,
        Err(error) => {
            let error = CapabilityError::Invalid {
                message: format!("unreadable rotation record: {error}"),
            };
            return seal_reply(identity, envelope, error_payload(error));
        }
    };
    if !record.verify() {
        let error = CapabilityError::Denied {
            reason: "the rotation record was not signed by the old key".to_string(),
            policy: None,
        };
        record_federation_event(
            audit,
            envelope,
            &record,
            "denied",
            Some(error.to_string()),
            started,
        );
        return seal_reply(identity, envelope, error_payload(error));
    }
    let outcome = {
        let mut store = peers
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store.rotate(&record, unix_ms()).map(|()| {
            if let Some(path) = peer_path {
                if let Err(error) = store.save(path) {
                    tracing::warn!(
                        "federation: rotated peer store {} was not written: {error}",
                        path.display()
                    );
                }
            }
        })
    };
    match outcome {
        Ok(()) => {
            record_federation_event(audit, envelope, &record, "ok", None, started);
            seal_reply(identity, envelope, json!({ "ok": true }))
        }
        Err(error) => {
            let reason = error.to_string();
            record_federation_event(
                audit,
                envelope,
                &record,
                "denied",
                Some(reason.clone()),
                started,
            );
            seal_reply(
                identity,
                envelope,
                error_payload(CapabilityError::Denied {
                    reason,
                    policy: None,
                }),
            )
        }
    }
}

fn record_federation_event(
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
    rotation: &RotationRecord,
    outcome: &str,
    reason: Option<String>,
    started: Instant,
) {
    let mut event = FederationEvent::new(
        Direction::Inbound,
        envelope.sender.as_str(),
        "rotation",
        Some(&rotation.old.to_string()),
        outcome,
    )
    .with_elapsed_ms(started.elapsed().as_millis() as u64);
    if let Some(reason) = reason {
        event = event.with_reason(reason);
    }
    record(audit, event);
}

async fn serve_knowledge_query(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    knowledge: &Option<Arc<RwLock<KnowledgeService>>>,
    authorizer: &Option<Arc<dyn KnowledgeAuthorizer>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_query",
                None,
                error_kind(&error),
            )
            .with_reason(error.to_string()),
        );
        return seal_reply(identity, envelope, error_payload(error));
    }
    let (Some(service), Some(policy)) = (knowledge, authorizer) else {
        let error = CapabilityError::Unavailable {
            reason: "this Node does not offer knowledge federation".to_string(),
        };
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_query",
                None,
                error_kind(&error),
            ),
        );
        return seal_reply(identity, envelope, error_payload(error));
    };
    let query: KnowledgeQuery = match serde_json::from_value(envelope.payload.clone()) {
        Ok(query) => query,
        Err(error) => {
            let error = CapabilityError::Invalid {
                message: format!("unreadable knowledge query: {error}"),
            };
            record(
                audit,
                FederationEvent::new(
                    Direction::Inbound,
                    envelope.sender.as_str(),
                    "knowledge_query",
                    None,
                    error_kind(&error),
                ),
            );
            return seal_reply(identity, envelope, error_payload(error));
        }
    };
    let mut rows = Vec::new();
    let result = service
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .query(
            &identity.node_id(),
            &envelope.sender,
            &query,
            unix_ms(),
            |peer, class, purpose, scope| policy.may_query(peer, class, purpose, scope),
            &mut rows,
        );
    let event = match &result {
        Ok(_) => FederationEvent::new(
            Direction::Inbound,
            envelope.sender.as_str(),
            "knowledge_query",
            Some(&query.query_id),
            "ok",
        )
        .with_elapsed_ms(started.elapsed().as_millis() as u64),
        Err(error) => {
            let wire = CapabilityError::from(error.clone());
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_query",
                Some(&query.query_id),
                error_kind(&wire),
            )
            .with_reason(error.to_string())
        }
    };
    record(audit, event);
    match result {
        Ok(response) => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "response": response }),
        ),
        Err(error) => seal_reply(identity, envelope, error_payload(error.into())),
    }
}

async fn serve_artifact_cleanup(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    store: &Option<Arc<dyn ArtifactStore>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "artifact_cleanup",
                None,
                error_kind(&error),
            )
            .with_reason(error.to_string()),
        );
        return seal_reply(identity, envelope, error_payload(error));
    }
    let request: ArtifactCleanupRequest = match serde_json::from_value(envelope.payload.clone()) {
        Ok(request) => request,
        Err(error) => {
            let error = CapabilityError::Invalid {
                message: format!("unreadable artifact cleanup request: {error}"),
            };
            return seal_reply(identity, envelope, error_payload(error));
        }
    };
    if let Err(error) = request.validate() {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let Some(store) = store else {
        let error = CapabilityError::Unavailable {
            reason: "this Node has no artifact store".to_string(),
        };
        return seal_reply(identity, envelope, error_payload(error));
    };
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for artifact in &request.artifacts {
        match store.remove(&envelope.sender, artifact).await {
            Ok(true) => removed.push(artifact.uri.clone()),
            Ok(false) => failed.push(ArtifactFailure {
                uri: artifact.uri.clone(),
                error: "the store declined to remove it".to_string(),
            }),
            Err(error) => failed.push(ArtifactFailure {
                uri: artifact.uri.clone(),
                error,
            }),
        }
    }
    let response = ArtifactCleanupResponse {
        schema: crate::artifacts::ARTIFACT_CLEANUP_SCHEMA,
        request_id: request.request_id,
        removed,
        failed,
    };
    record(
        audit,
        FederationEvent::new(
            Direction::Inbound,
            envelope.sender.as_str(),
            "artifact_cleanup",
            None,
            if response.failed.is_empty() {
                "ok"
            } else {
                "partial"
            },
        )
        .with_elapsed_ms(started.elapsed().as_millis() as u64),
    );
    seal_reply(
        identity,
        envelope,
        json!({ "ok": true, "response": response }),
    )
}

async fn serve_knowledge_publication(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    knowledge: &Option<Arc<RwLock<KnowledgeService>>>,
    authorizer: &Option<Arc<dyn KnowledgeAuthorizer>>,
    knowledge_path: &Option<PathBuf>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_publication",
                None,
                error_kind(&error),
            )
            .with_reason(error.to_string()),
        );
        return seal_reply(identity, envelope, error_payload(error));
    }
    let (Some(service), Some(policy)) = (knowledge, authorizer) else {
        let error = CapabilityError::Unavailable {
            reason: "this Node does not accept knowledge publications".to_string(),
        };
        return seal_reply(identity, envelope, error_payload(error));
    };
    if !policy.may_publish(&envelope.sender) {
        let error = CapabilityError::Denied {
            reason: format!(
                "peer {} may not publish knowledge to this Node",
                envelope.sender
            ),
            policy: None,
        };
        record(
            audit,
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_publication",
                None,
                error_kind(&error),
            )
            .with_reason(error.to_string()),
        );
        return seal_reply(identity, envelope, error_payload(error));
    }
    let publication: KnowledgePublication = match serde_json::from_value(envelope.payload.clone()) {
        Ok(publication) => publication,
        Err(error) => {
            let error = CapabilityError::Invalid {
                message: format!("unreadable knowledge publication: {error}"),
            };
            return seal_reply(identity, envelope, error_payload(error));
        }
    };
    let result = {
        let mut guard = service
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let outcome = guard.publish(&envelope.sender, &publication, unix_ms());
        if outcome.is_ok() {
            if let Some(path) = knowledge_path {
                persist_knowledge(&guard, path);
            }
        }
        outcome
    };
    let event = match &result {
        Ok(count) => FederationEvent::new(
            Direction::Inbound,
            envelope.sender.as_str(),
            "knowledge_publication",
            None,
            "ok",
        )
        .with_elapsed_ms(started.elapsed().as_millis() as u64)
        .with_bytes(*count as u64),
        Err(error) => {
            let wire = CapabilityError::from(error.clone());
            FederationEvent::new(
                Direction::Inbound,
                envelope.sender.as_str(),
                "knowledge_publication",
                None,
                error_kind(&wire),
            )
            .with_reason(error.to_string())
        }
    };
    record(audit, event);
    match result {
        Ok(accepted) => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "accepted": accepted }),
        ),
        Err(error) => seal_reply(identity, envelope, error_payload(error.into())),
    }
}

/// The JSON reply for a task.
fn ok_task(task: &FederatedTask) -> Value {
    json!({ "ok": true, "task": task })
}

/// A task-create request: validate the sender, record the task, answer with it.
async fn serve_task_create(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    task_path: &Option<PathBuf>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_task_create_inner(identity, peers, tasks, task_path, envelope).await;
    record_reply(audit, envelope, "task_create", started, &reply);
    reply
}

async fn serve_task_create_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    task_path: &Option<PathBuf>,
    envelope: &Envelope,
) -> Vec<u8> {
    let Some(request) = envelope
        .payload
        .get("request")
        .cloned()
        .and_then(|value| serde_json::from_value::<TaskCreateRequest>(value).ok())
    else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not carry a task".to_string(),
            }),
        );
    };
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let now = unix_ms();
    let mut queue = tasks
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let outcome = queue.create(
        request.id,
        envelope.sender.as_str().to_string(),
        request.objective,
        request.schedule,
        request.budget_steps,
        request.budget_tokens,
        request.deadline_ms,
        now,
    );
    match outcome {
        Ok(task) => {
            if let Some(path) = task_path {
                if let Err(error) = queue.save(path) {
                    tracing::warn!("federation: task queue was not written: {error}");
                }
            }
            seal_reply(identity, envelope, ok_task(&task))
        }
        Err(error) => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: error.to_string(),
            }),
        ),
    }
}

/// Read one task; only its origin may see it.
async fn serve_task_status(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_task_status_inner(identity, peers, tasks, envelope).await;
    record_reply(audit, envelope, "task_status", started, &reply);
    reply
}

async fn serve_task_status_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let Some(request) = envelope
        .payload
        .get("request")
        .cloned()
        .and_then(|value| serde_json::from_value::<TaskIdRequest>(value).ok())
    else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not carry a task id".to_string(),
            }),
        );
    };
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let queue = tasks
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match queue.get(&request.id) {
        Some(task) if task.origin == envelope.sender.as_str() => {
            seal_reply(identity, envelope, ok_task(task))
        }
        Some(_) => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: "the task belongs to another node".to_string(),
                policy: None,
            }),
        ),
        None => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: format!("task {} is unknown", request.id),
            }),
        ),
    }
}

/// Cancel one task; only its origin may cancel it.
async fn serve_task_cancel(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    task_path: &Option<PathBuf>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_task_cancel_inner(identity, peers, tasks, task_path, envelope).await;
    record_reply(audit, envelope, "task_cancel", started, &reply);
    reply
}

async fn serve_task_cancel_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    tasks: &Arc<RwLock<TaskQueue>>,
    task_path: &Option<PathBuf>,
    envelope: &Envelope,
) -> Vec<u8> {
    let Some(request) = envelope
        .payload
        .get("request")
        .cloned()
        .and_then(|value| serde_json::from_value::<TaskIdRequest>(value).ok())
    else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: "the request does not carry a task id".to_string(),
            }),
        );
    };
    if let Err(error) = authorize_sender(peers, &envelope.sender, &[]) {
        return seal_reply(identity, envelope, error_payload(error));
    }
    let now = unix_ms();
    let mut queue = tasks
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match queue.get(&request.id) {
        Some(task) if task.origin == envelope.sender.as_str() => {}
        Some(_) => {
            return seal_reply(
                identity,
                envelope,
                error_payload(CapabilityError::Denied {
                    reason: "the task belongs to another node".to_string(),
                    policy: None,
                }),
            );
        }
        None => {
            return seal_reply(
                identity,
                envelope,
                error_payload(CapabilityError::Invalid {
                    message: format!("task {} is unknown", request.id),
                }),
            );
        }
    }
    match queue.cancel(&request.id, now) {
        Ok(()) => {
            if let Some(path) = task_path {
                if let Err(error) = queue.save(path) {
                    tracing::warn!("federation: task queue was not written: {error}");
                }
            }
            match queue.get(&request.id) {
                Some(task) => seal_reply(identity, envelope, ok_task(task)),
                None => seal_reply(
                    identity,
                    envelope,
                    error_payload(CapabilityError::Invalid {
                        message: format!("task {} is unknown", request.id),
                    }),
                ),
            }
        }
        Err(error) => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Invalid {
                message: error.to_string(),
            }),
        ),
    }
}

async fn serve_delegate(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    runtime: &Option<Arc<WorkspaceRuntime>>,
    journal: &DelegationJournal,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_delegate_inner(
        identity,
        peers,
        delegations,
        runtime,
        journal,
        audit,
        envelope,
    )
    .await;
    record_reply(audit, envelope, "delegate", started, &reply);
    reply
}

async fn serve_delegate_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    delegations: &Arc<RwLock<DelegationStore>>,
    runtime: &Option<Arc<WorkspaceRuntime>>,
    journal: &DelegationJournal,
    audit: &Option<Arc<dyn FederationAudit>>,
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

    let contract = request.result_contract.clone();
    let capabilities = request.capabilities.clone();
    let deadline_ms = request.deadline_ms;
    let sender = envelope.sender.clone();
    let now = unix_ms();
    let mut store = delegations
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let parent = request.parent.clone();
    let origin = Some(envelope.sender.clone());
    let outcome = match parent {
        Some(parent) => store.delegate_for(&parent, request, origin, now),
        None => store.create_for(request, origin, now),
    };
    match outcome {
        Ok(id) => {
            let _ = store.transition(&id, DelegationState::Accepted, now);
            // Only a structured contract executes; an objective alone stays
            // accepted, because this Node has no language-model executor.
            let mut execute = None;
            let failure = match execution_spec(contract.as_deref(), &capabilities) {
                Ok(Some(spec)) => match runtime {
                    Some(_) => {
                        execute = Some(spec);
                        None
                    }
                    None => Some("this Node serves no capabilities".to_string()),
                },
                Ok(None) => None,
                Err(reason) => Some(reason),
            };
            if let Some(reason) = failure {
                let _ = store.fail(&id, reason, unix_ms());
            }
            let record = store.record(&id).cloned();
            drop(store);
            if let (Some((capability, input)), Some(runtime)) = (execute, runtime) {
                let runtime = Arc::clone(runtime);
                let delegations = Arc::clone(delegations);
                let journal = journal.clone();
                let audit = audit.clone();
                let sender = sender.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    run_delegation(
                        runtime,
                        delegations,
                        journal,
                        audit,
                        sender,
                        id,
                        capability,
                        input,
                        deadline_ms,
                    )
                    .await;
                });
            }
            match record {
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
        Some(record) if record.origin.as_ref() == Some(&envelope.sender) => seal_reply(
            identity,
            envelope,
            json!({ "ok": true, "delegation": record }),
        ),
        Some(_) => seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: "the delegation belongs to another peer".to_string(),
                policy: None,
            }),
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
    let Some((state, owned)) = store.record(&id).map(|record| {
        (
            record.state,
            record.origin.as_ref() == Some(&envelope.sender),
        )
    }) else {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::DelegationNotFound {
                delegation: id.to_string(),
            }),
        );
    };
    if !owned {
        return seal_reply(
            identity,
            envelope,
            error_payload(CapabilityError::Denied {
                reason: "the delegation belongs to another peer".to_string(),
                policy: None,
            }),
        );
    }
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

/// Write the delegation store when a handler changed it.
///
/// The loop is the only writer for inbound changes, so a revision compare is
/// enough to keep a quiet node from touching the disk.
fn persist_delegations(
    path: &Option<PathBuf>,
    delegations: &Arc<RwLock<DelegationStore>>,
    saved_revision: &Arc<AtomicU64>,
    write_gate: &Arc<Mutex<()>>,
) {
    let Some(path) = path else {
        return;
    };
    let Ok(_guard) = write_gate.lock() else {
        return;
    };
    let store = delegations
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let revision = store.revision();
    if revision == saved_revision.load(Ordering::Relaxed) {
        return;
    }
    match store.save(path) {
        Ok(()) => saved_revision.store(revision, Ordering::Relaxed),
        Err(error) => tracing::warn!(
            "federation: delegations were not saved to {}: {error}",
            path.display()
        ),
    }
}

/// The write side of the delegation file, shared by the handler and the
/// executor so both persist through one lock and one revision counter.
#[derive(Clone)]
struct DelegationJournal {
    path: Option<PathBuf>,
    revision: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
}

impl DelegationJournal {
    fn persist(&self, delegations: &Arc<RwLock<DelegationStore>>) {
        persist_delegations(&self.path, delegations, &self.revision, &self.gate);
    }
}

/// The capability a result contract asks the target to run, when it asks.
///
/// Only a structured contract executes: an objective alone is recorded and
/// left `accepted`, because this Node has no language-model executor. A
/// capability outside the delegated list is refused rather than escalated.
fn execution_spec(
    contract: Option<&str>,
    capabilities: &[CapabilityId],
) -> Result<Option<(CapabilityId, Value)>, String> {
    let Some(contract) = contract else {
        return Ok(None);
    };
    // A contract that is prose, not JSON, describes the result rather than
    // asking for one call; it stays queued like any objective-only delegation.
    let Ok(contract) = serde_json::from_str::<Value>(contract) else {
        return Ok(None);
    };
    let Some(raw) = contract.get("capability").and_then(Value::as_str) else {
        return Ok(None);
    };
    let capability: CapabilityId = raw
        .parse()
        .map_err(|error| format!("invalid capability id {raw:?}: {error}"))?;
    if capabilities.is_empty() || !capabilities.contains(&capability) {
        return Err(format!("{capability} is outside the delegated scope"));
    }
    Ok(Some((
        capability,
        contract.get("input").cloned().unwrap_or(Value::Null),
    )))
}

/// Run one structured delegation and settle its record.
#[allow(clippy::too_many_arguments)]
async fn run_delegation(
    runtime: Arc<WorkspaceRuntime>,
    delegations: Arc<RwLock<DelegationStore>>,
    journal: DelegationJournal,
    audit: Option<Arc<dyn FederationAudit>>,
    sender: NodeId,
    id: DelegationId,
    capability: CapabilityId,
    input: Value,
    deadline_ms: Option<u64>,
) {
    let started = Instant::now();
    // Re-check the bound before the transition: a request that expired after
    // acceptance must not start running.
    if let Some(deadline) = deadline_ms {
        if deadline <= unix_ms() {
            let mut store = delegations
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = store.fail(
                &id,
                "the delegation deadline has expired".to_string(),
                unix_ms(),
            );
            drop(store);
            journal.persist(&delegations);
            return;
        }
    }
    {
        let mut store = delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let still_accepted = store
            .record(&id)
            .map(|record| record.state == DelegationState::Accepted)
            .unwrap_or(false);
        if !still_accepted {
            // A cancel (or another executor) won the race before this task ran.
            return;
        }
        let _ = store.transition(&id, DelegationState::Running, unix_ms());
    }
    journal.persist(&delegations);

    let mut context = InvocationContext::local(
        InvocationId::new(format!("delegation:{}", id.as_str())),
        Principal::Remote {
            node: sender.clone(),
        },
        runtime.workspace().clone(),
    );
    if let Some(deadline) = deadline_ms {
        let now = unix_ms();
        if deadline <= now {
            // The window closed between acceptance and dispatch.
            let mut store = delegations
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = store.fail(&id, "the delegation deadline has expired".to_string(), now);
            drop(store);
            journal.persist(&delegations);
            return;
        }
        context.deadline = Some(Instant::now() + Duration::from_millis(deadline - now));
    }

    let result = runtime.invoke(&capability, input, &context).await;
    let now = unix_ms();
    let outcome = {
        let mut store = delegations
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &result {
            Ok(output) => match serde_json::to_value(output) {
                Ok(value) => store.succeed(&id, value, now),
                Err(error) => store.fail(&id, error.to_string(), now),
            },
            Err(error) => store.fail(&id, error.to_string(), now),
        }
        .map_err(|error| error.to_string())
    };
    journal.persist(&delegations);

    let mut event = FederationEvent::new(
        Direction::Inbound,
        sender.as_str(),
        "delegate_exec",
        Some(&capability.to_string()),
        match &result {
            Ok(_) => "ok",
            Err(error) => error_kind(error),
        },
    )
    .with_subject(id.as_str())
    .with_elapsed_ms(started.elapsed().as_millis() as u64);
    if let Err(error) = &result {
        if let Some(reason) = error_reason(error) {
            event = event.with_reason(reason);
        }
    }
    if let Err(reason) = outcome {
        event = event.with_reason(reason);
    }
    record(&audit, event);
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
    // A delegation frame carries its trace and id inside `request`; the reply
    // names the accepted delegation, which is the id an operator follows.
    event = correlate_payload(event, &envelope.payload);
    if operation == "delegate" {
        // The reply is a sealed envelope; its payload names the record.
        if let Ok(sealed) = Envelope::decode(reply) {
            let value = &sealed.payload;
            let id = value.get("id").and_then(Value::as_str).or_else(|| {
                value
                    .pointer("/delegation/request/id")
                    .and_then(Value::as_str)
            });
            if let Some(id) = id {
                event = event.with_delegation(id);
            }
        }
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
        DelegationError::DeadlineExpired => CapabilityError::Denied {
            reason: "the delegation's deadline has expired".to_string(),
            policy: None,
        },
        DelegationError::ScopeExceedsParent => CapabilityError::Denied {
            reason: "the delegation's scope exceeds its parent's".to_string(),
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
    receiver: tokio::sync::mpsc::Receiver<ResourceStreamMessage>,
    streams: Arc<Mutex<HashMap<LeaseId, InboundStream>>>,
    audit: Option<Arc<dyn FederationAudit>>,
    received: u64,
    settled: bool,
    /// The start offset this stream opened at.
    offset: u64,
    /// The resource being read.
    uri: String,
    /// Transfer checkpoints, when configured.
    transfers: Option<Arc<Mutex<crate::resource::TransferStore>>>,
    /// Where the checkpoints are saved.
    transfer_path: Option<PathBuf>,
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
                self.checkpoint();
            }
            ResourceStreamMessage::Close { .. } => self.settle("ok", None),
            ResourceStreamMessage::Error { message, .. } => {
                self.settle("error", Some(message.clone()))
            }
            _ => {}
        }
        Some(message)
    }

    /// Record the position a resumed stream would continue from.
    fn checkpoint(&self) {
        let Some(store) = &self.transfers else {
            return;
        };
        if let Ok(mut store) = store.lock() {
            store.checkpoint(crate::resource::TransferRecord {
                lease: self.lease.as_str().to_string(),
                uri: self.uri.clone(),
                peer: self.peer.to_string(),
                position: self.offset + self.received,
                updated_at_ms: unix_ms(),
            });
            if let Some(path) = &self.transfer_path {
                let _ = store.save(path);
            }
        }
    }

    /// Record how this stream ended, once.
    fn settle(&mut self, outcome: &str, reason: Option<String>) {
        if self.settled {
            return;
        }
        self.settled = true;
        self.streams
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.lease);
        if let Some(store) = &self.transfers {
            if let Ok(mut store) = store.lock() {
                // Only a completed transfer forgets its checkpoint. An error or
                // a reader that walked away leaves the position for a resume.
                if outcome == "ok" {
                    store.remove(&self.lease, &self.uri);
                }
                if let Some(path) = &self.transfer_path {
                    let _ = store.save(path);
                }
            }
        }
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
    peer: NodeId,
    credit: AtomicU64,
    open: AtomicBool,
    notify: tokio::sync::Notify,
}

impl ServerSession {
    fn new(peer: NodeId, credit: u64) -> Self {
        Self {
            peer,
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
    streams: Arc<Mutex<HashMap<LeaseId, InboundStream>>>,
    sessions: Arc<Mutex<HashMap<LeaseId, Arc<ServerSession>>>>,
    source: Option<Arc<dyn ResourceSource>>,
    audit: &Option<Arc<dyn FederationAudit>>,
    stream_limit: usize,
    envelope: Envelope,
) {
    let Ok(message) = serde_json::from_value::<ResourceStreamMessage>(envelope.payload.clone())
    else {
        return;
    };
    let lease = message.lease().clone();

    // A locally registered stream owns this lease: the message is inbound data,
    // and only the peer the stream was opened with may move it forward.
    let sender =
        {
            let mut streams = streams
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match streams.get_mut(&lease) {
                Some(entry) => {
                    if entry.peer != envelope.sender {
                        record(
                            audit,
                            FederationEvent::new(
                                Direction::Inbound,
                                envelope.sender.as_str(),
                                "resource",
                                None,
                                "denied",
                            )
                            .with_subject(lease.to_string())
                            .with_reason("the stream belongs to another peer"),
                        );
                        return;
                    }
                    let accepted = match &message {
                        ResourceStreamMessage::Chunk { seq, .. } => {
                            entry.open && *seq == entry.next_seq
                        }
                        ResourceStreamMessage::Close { .. }
                        | ResourceStreamMessage::Error { .. } => entry.open,
                        _ => false,
                    };
                    if !accepted {
                        record(
                            audit,
                            FederationEvent::new(
                                Direction::Inbound,
                                envelope.sender.as_str(),
                                "resource",
                                None,
                                "rejected",
                            )
                            .with_subject(lease.to_string())
                            .with_reason("the message is not valid for this stream's state"),
                        );
                        return;
                    }
                    match &message {
                        ResourceStreamMessage::Chunk { .. } => entry.next_seq += 1,
                        ResourceStreamMessage::Close { .. }
                        | ResourceStreamMessage::Error { .. } => entry.open = false,
                        _ => {}
                    }
                    Some(entry.sender.clone())
                }
                None => None,
            }
        };
    if let Some(sender) = sender {
        let _ = sender.send(message).await;
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
            let existing = sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&lease)
                .cloned();
            if let Some(session) = existing {
                if session.peer != envelope.sender || session.open.load(Ordering::SeqCst) {
                    record(
                        audit,
                        FederationEvent::new(
                            Direction::Inbound,
                            envelope.sender.as_str(),
                            "resource",
                            None,
                            "rejected",
                        )
                        .with_subject(lease.to_string())
                        .with_reason("a stream for this lease is already open"),
                    );
                    send_stream(
                        &identity,
                        &transport,
                        &envelope.sender,
                        ResourceStreamMessage::Error {
                            lease,
                            message: "a stream for this lease is already open".to_string(),
                        },
                    )
                    .await;
                    return;
                }
            }
            let open_streams = sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len();
            if open_streams >= stream_limit {
                record(
                    audit,
                    FederationEvent::new(
                        Direction::Inbound,
                        envelope.sender.as_str(),
                        "resource",
                        None,
                        "rate_limited",
                    )
                    .with_subject(lease.to_string())
                    .with_reason("too many concurrent resource streams"),
                );
                send_stream(
                    &identity,
                    &transport,
                    &envelope.sender,
                    ResourceStreamMessage::Error {
                        lease,
                        message: "too many concurrent resource streams".to_string(),
                    },
                )
                .await;
                return;
            }
            let session = Arc::new(ServerSession::new(envelope.sender.clone(), credit));
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
            let session = sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&lease)
                .cloned();
            if let Some(session) = session {
                if session.peer != envelope.sender {
                    record(
                        audit,
                        FederationEvent::new(
                            Direction::Inbound,
                            envelope.sender.as_str(),
                            "resource",
                            None,
                            "denied",
                        )
                        .with_subject(lease.to_string())
                        .with_reason("credit came from a peer that does not own the stream"),
                    );
                    return;
                }
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
    // The checksum is updated per chunk; memory stays bounded by the chunk,
    // not by the total resource length.
    let mut streamed = Sha256::new();
    let started = Instant::now();
    let mut outcome = "ok";
    let mut reason: Option<String> = None;
    loop {
        if started.elapsed() > MAX_STREAM_DURATION {
            outcome = "timeout";
            reason = Some(format!("the stream exceeded {MAX_STREAM_DURATION:?}"));
            send_stream(
                &identity,
                &transport,
                &peer,
                ResourceStreamMessage::Error {
                    lease: lease.clone(),
                    message: reason.clone().unwrap_or_default(),
                },
            )
            .await;
            break;
        }
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
                        checksum: stream_checksum(&streamed),
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
                streamed.update(&bytes);
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
                        checksum: stream_checksum(&streamed),
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

fn decode_topology(reply: Value) -> Result<crate::coordinator::Topology, CapabilityError> {
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        serde_json::from_value(reply.get("topology").cloned().unwrap_or(Value::Null)).map_err(
            |error| CapabilityError::Invalid {
                message: format!("the coordinator returned an unreadable topology: {error}"),
            },
        )
    } else {
        Err(wire_error(&reply, None))
    }
}

async fn serve_coordinator(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    coordinator: &Option<Arc<Mutex<crate::coordinator::Coordinator>>>,
    path: &Option<PathBuf>,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let trusted = peers
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_trusted(&envelope.sender, unix_ms());
    let reply = if !trusted {
        error_payload(CapabilityError::Denied {
            reason: format!("peer {} is not enrolled", envelope.sender),
            policy: None,
        })
    } else if let Some(coordinator) = coordinator {
        let mut state = coordinator
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if envelope.kind == "coordinator_topology" {
            json!({ "ok": true, "topology": state.topology() })
        } else {
            let member: Option<crate::coordinator::MemberRecord> = envelope
                .payload
                .get("member")
                .and_then(|value| serde_json::from_value(value.clone()).ok());
            match member {
                Some(member) => {
                    match state.register(&envelope.sender, member) {
                        Ok(()) => {
                            if let Some(path) = path {
                                if let Err(error) = state.save(path) {
                                    tracing::warn!("federation: coordinator membership was not written: {error}");
                                }
                            }
                            json!({ "ok": true, "topology": state.topology() })
                        }
                        Err(error) => json!({
                            "ok": false,
                            "error": { "kind": "denied", "message": error.to_string() },
                        }),
                    }
                }
                None => json!({
                    "ok": false,
                    "error": { "kind": "invalid", "message": "the request names no member" },
                }),
            }
        }
    } else {
        json!({
            "ok": false,
            "error": { "kind": "unavailable", "message": "this node does not run a coordinator" },
        })
    };
    let outcome = if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        "ok"
    } else {
        "denied"
    };
    let mut event = FederationEvent::new(
        Direction::Inbound,
        envelope.sender.as_str(),
        "coordinator",
        None,
        outcome,
    );
    if let Some(reason) = reply
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        event = event.with_reason(reason);
    }
    record(audit, correlate_payload(event, &envelope.payload));
    seal_reply(identity, envelope, reply)
}

async fn serve_discover(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    capabilities: &[CapabilityDescriptor],
    federation: &Option<FederationId>,
    policy_revision: u64,
    audit: &Option<Arc<dyn FederationAudit>>,
    envelope: &Envelope,
) -> Vec<u8> {
    let started = Instant::now();
    let reply = serve_discover_inner(
        identity,
        peers,
        capabilities,
        federation,
        policy_revision,
        envelope,
    )
    .await;
    record_reply(audit, envelope, "discover", started, &reply);
    reply
}

async fn serve_discover_inner(
    identity: &NodeIdentity,
    peers: &Arc<RwLock<PeerStore>>,
    capabilities: &[CapabilityDescriptor],
    federation: &Option<FederationId>,
    policy_revision: u64,
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
    let record = DiscoveryRecord::publish_federated(
        identity,
        None,
        federation.clone(),
        policy_revision,
        capabilities.to_vec(),
    );
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

#[cfg(test)]
mod admission_tests {
    use super::{
        Admission, AdmissionLimits, ReplayCache, MAX_IN_FLIGHT_HANDLERS, MAX_IN_FLIGHT_PER_PEER,
        REPLAY_CAPACITY,
    };
    use crate::identity::NodeIdentity;

    fn node(seed: u8) -> capability::NodeId {
        NodeIdentity::from_seed([seed; 32]).node_id()
    }

    /// Capacity pressure refuses new identities instead of evicting a
    /// still-valid one, so a flood cannot reopen a replay window.
    #[test]
    fn replay_capacity_fails_closed_instead_of_evicting() {
        let cache = ReplayCache::new();
        let sender = node(21);
        for index in 0..REPLAY_CAPACITY {
            assert!(
                cache.admit(&[(sender.clone(), format!("m-{index}"))], 1_000_000, 0),
                "entry {index} fits"
            );
        }
        assert!(
            !cache.admit(&[(sender.clone(), "fresh".to_string())], 1_000_000, 0),
            "a full cache refuses the new identity"
        );
        assert!(
            !cache.admit(&[(sender, "m-0".to_string())], 1_000_000, 0),
            "the oldest identity is still protected"
        );
    }
    #[test]
    fn a_fresh_cache_forgets_like_a_restart() {
        let sender = node(13);
        let key = vec![(sender, "m-1".to_string())];
        assert!(ReplayCache::new().admit(&key, 10_000, 0));
        assert!(
            ReplayCache::new().admit(&key, 10_000, 0),
            "the replay cache is in memory, so a restart forgets it"
        );
    }

    #[test]
    fn replay_cache_survives_a_restart() {
        let path = std::env::temp_dir().join(format!(
            "bos-replay-{}-{}.json",
            std::process::id(),
            capability::unix_ms()
        ));
        let cache = ReplayCache::new();
        let sender = node(21);
        let now = capability::unix_ms();
        assert!(cache.admit(
            &[(sender.clone(), "m-restart".to_string())],
            now + 60_000,
            now
        ));
        cache.save(&path).expect("saved");
        drop(cache);

        let loaded = ReplayCache::load(&path).expect("loaded");
        assert!(
            !loaded.admit(&[(sender, "m-restart".to_string())], now + 60_000, now),
            "a duplicate is refused across a restart"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replay_admission_is_bounded_and_expiry_aware() {
        let cache = ReplayCache::new();
        let sender = node(11);
        let key = vec![(sender.clone(), "m-1".to_string())];
        assert!(cache.admit(&key, 1_000, 100), "first sight is admitted");
        assert!(!cache.admit(&key, 1_000, 100), "a duplicate is refused");
        assert!(
            cache.admit(&key, 1_000, 1_001),
            "after expiry the identity may be used again"
        );

        let cache = ReplayCache::new();
        let sender = node(12);
        for index in 0..REPLAY_CAPACITY {
            assert!(cache.admit(&[(sender.clone(), format!("m-{index}"))], 10_000, 0));
        }
        assert!(
            !cache.admit(&[(sender.clone(), "m-fresh".to_string())], 10_000, 0),
            "a full cache refuses a new identity instead of evicting"
        );
        assert!(
            !cache.admit(&[(sender, "m-0".to_string())], 10_000, 0),
            "the oldest identity is still protected"
        );
    }
    #[test]
    fn admission_is_per_peer_and_releases_on_drop() {
        let admission = Admission::new(AdmissionLimits::default());
        let peer = node(1);
        let mut permits: Vec<_> = (0..MAX_IN_FLIGHT_PER_PEER)
            .map(|_| {
                admission
                    .try_admit(&peer, "invoke", 1)
                    .expect("within the peer budget")
            })
            .collect();
        assert!(
            admission.try_admit(&peer, "invoke", 1).is_none(),
            "the peer budget is finite"
        );
        assert!(
            admission.try_admit(&node(2), "invoke", 1).is_some(),
            "another peer still gets its own share"
        );
        permits.pop();
        assert!(
            admission.try_admit(&peer, "invoke", 1).is_some(),
            "dropping a permit releases it"
        );
    }

    #[test]
    fn invocation_and_control_have_separate_budgets() {
        let admission = Admission::new(AdmissionLimits {
            invocation_global: 1,
            invocation_per_peer: 1,
            control_global: 1,
            control_per_peer: 1,
            resource_streams: 1,
            queued_bytes_per_peer: 1024,
        });
        let peer = node(3);

        let invoke = admission
            .try_admit(&peer, "invoke", 1)
            .expect("the invocation budget");
        assert!(
            admission.try_admit(&peer, "invoke", 1).is_none(),
            "the invocation class is saturated"
        );
        let control = admission
            .try_admit(&peer, "discover", 1)
            .expect("control has its own budget");
        assert!(
            admission.try_admit(&peer, "delegate", 1).is_none(),
            "the control class is saturated independently"
        );

        drop(invoke);
        drop(control);
        assert!(admission.try_admit(&peer, "invoke", 1).is_some());
        assert!(admission.try_admit(&peer, "discover", 1).is_some());
    }

    #[test]
    fn zero_limits_are_treated_as_one() {
        let admission = Admission::new(AdmissionLimits {
            invocation_global: 0,
            invocation_per_peer: 0,
            control_global: 0,
            control_per_peer: 0,
            resource_streams: 0,
            queued_bytes_per_peer: 1024,
        });
        let peer = node(4);
        let first = admission.try_admit(&peer, "invoke", 1).expect("one permit");
        assert!(admission.try_admit(&peer, "invoke", 1).is_none());
        drop(first);
        assert!(admission.try_admit(&peer, "invoke", 1).is_some());
    }

    #[test]
    fn admission_has_a_global_ceiling() {
        let admission = Admission::new(AdmissionLimits::default());
        let per_peer = MAX_IN_FLIGHT_PER_PEER;
        let peers = MAX_IN_FLIGHT_HANDLERS / per_peer;
        let mut permits = Vec::new();
        for seed in 0..peers {
            for _ in 0..per_peer {
                permits.push(
                    admission
                        .try_admit(&node(seed as u8 + 10), "invoke", 1)
                        .expect("within the global ceiling"),
                );
            }
        }
        assert_eq!(permits.len(), MAX_IN_FLIGHT_HANDLERS);
        assert!(
            admission.try_admit(&node(200), "invoke", 1).is_none(),
            "the global ceiling is finite"
        );
    }
}

#[cfg(test)]
mod invoke_cancel_tests {
    use super::*;
    use crate::peers::PeerStore;

    fn identity(seed: u8) -> NodeIdentity {
        NodeIdentity::from_seed([seed; 32])
    }

    fn cancel_envelope(caller: &NodeIdentity, provider: &NodeId, correlation: &str) -> Envelope {
        let payload = serde_json::to_vec(&json!({ "correlation": correlation })).expect("payload");
        let message =
            FederationMessage::new("invoke_cancel", payload).with_correlation(correlation);
        Envelope::seal(&message, caller, provider).expect("sealed")
    }

    async fn cancel(
        provider: &NodeIdentity,
        peers: &Arc<RwLock<PeerStore>>,
        in_flight: &Arc<Mutex<HashMap<String, (NodeId, CancelToken)>>>,
        caller: &NodeIdentity,
        correlation: &str,
    ) -> Value {
        let envelope = cancel_envelope(caller, &provider.node_id(), correlation);
        let reply = serve_invoke_cancel_inner(provider, peers, in_flight, &envelope).await;
        let reply = Envelope::decode(&reply).expect("reply envelope");
        reply.payload.clone()
    }

    #[tokio::test]
    async fn a_late_or_duplicate_cancel_is_acknowledged_without_effect() {
        let provider = identity(31);
        let caller = identity(32);
        let mut store = PeerStore::new();
        store.enroll(caller.node_id(), unix_ms(), None);
        let peers = Arc::new(RwLock::new(store));
        let in_flight = Arc::new(Mutex::new(HashMap::new()));

        let first = cancel(&provider, &peers, &in_flight, &caller, "corr-late").await;
        assert_eq!(first["ok"], true);
        assert_eq!(first["cancelled"], false);
        let duplicate = cancel(&provider, &peers, &in_flight, &caller, "corr-late").await;
        assert_eq!(duplicate["cancelled"], false);
    }

    #[tokio::test]
    async fn a_cancel_stops_the_matching_in_flight_invocation() {
        let provider = identity(41);
        let caller = identity(42);
        let mut store = PeerStore::new();
        store.enroll(caller.node_id(), unix_ms(), None);
        let peers = Arc::new(RwLock::new(store));
        let token = CancelToken::new();
        let in_flight = Arc::new(Mutex::new(HashMap::from([(
            "corr-match".to_string(),
            (caller.node_id(), token.clone()),
        )])));

        let reply = cancel(&provider, &peers, &in_flight, &caller, "corr-match").await;

        assert_eq!(reply["ok"], true);
        assert_eq!(reply["cancelled"], true);
        assert!(token.is_cancelled());
        // A second cancel is idempotent, not an error.
        let again = cancel(&provider, &peers, &in_flight, &caller, "corr-match").await;
        assert_eq!(again["cancelled"], true);
    }

    #[tokio::test]
    async fn another_peer_cannot_cancel_someone_elses_invocation() {
        let provider = identity(51);
        let caller = identity(52);
        let owner = identity(53);
        let mut store = PeerStore::new();
        store.enroll(caller.node_id(), unix_ms(), None);
        store.enroll(owner.node_id(), unix_ms(), None);
        let peers = Arc::new(RwLock::new(store));
        let token = CancelToken::new();
        let in_flight = Arc::new(Mutex::new(HashMap::from([(
            "corr-owned".to_string(),
            (owner.node_id(), token.clone()),
        )])));

        let reply = cancel(&provider, &peers, &in_flight, &caller, "corr-owned").await;

        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "denied");
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn an_unenrolled_peer_cannot_send_cancel() {
        let provider = identity(61);
        let stranger = identity(62);
        let peers = Arc::new(RwLock::new(PeerStore::new()));
        let in_flight = Arc::new(Mutex::new(HashMap::new()));

        let reply = cancel(&provider, &peers, &in_flight, &stranger, "corr").await;

        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"]["kind"], "denied");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    fn node(value: &str) -> NodeId {
        NodeId::new(value).expect("a node id")
    }

    #[test]
    fn a_peer_cannot_exceed_its_aggregate_byte_budget() {
        let budget = ByteAdmission::new(8);
        let peer = node("node-a");
        let first = budget.try_reserve(&peer, 6).expect("the first frame fits");
        assert!(
            budget.try_reserve(&peer, 6).is_none(),
            "6 + 6 is over the 8-byte budget"
        );
        drop(first);
        assert!(
            budget.try_reserve(&peer, 6).is_some(),
            "released bytes are reusable"
        );
    }

    #[test]
    fn a_budget_is_per_peer() {
        let budget = ByteAdmission::new(8);
        assert!(budget.try_reserve(&node("node-a"), 8).is_some());
        assert!(
            budget.try_reserve(&node("node-b"), 8).is_some(),
            "one peer's frames do not consume another's budget"
        );
    }

    #[test]
    fn message_kinds_have_tighter_limits_than_the_frame_ceiling() {
        assert_eq!(kind_payload_limit("invoke"), 256 * 1024);
        assert!(kind_payload_limit("resource") > kind_payload_limit("invoke"));
        assert!(kind_payload_limit("delegate") <= kind_payload_limit("invoke"));
        assert_eq!(kind_payload_limit("unknown-kind"), 64 * 1024);
    }

    #[test]
    fn a_delegation_payload_correlates_its_request() {
        let payload = serde_json::json!({
            "request": {
                "id": "d-1",
                "objective": "audit",
                "trace_id": "trace-1",
                "span_id": "span-1",
                "task": "task-1"
            }
        });
        let event = correlate_payload(
            FederationEvent::new(Direction::Inbound, "node-b", "delegate", None, "ok"),
            &payload,
        );
        assert_eq!(event.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(event.span_id.as_deref(), Some("span-1"));
        assert_eq!(event.task.as_deref(), Some("task-1"));
    }
}
