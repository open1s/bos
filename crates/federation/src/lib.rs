#![warn(missing_docs)]
//! BSH federation: node identity, the signed JSON envelope, and the transport
//! seam the protocol is defined above.
//!
//! The three pieces are deliberately separate:
//!
//! - [NodeIdentity] is an Ed25519 key; a NodeId is its public key.
//! - [Envelope] is the wire format: JSON, versioned, signed over a canonical
//!   form, with an expiry.
//! - [FederationTransport] carries opaque frames and reports whether one left,
//!   so retry safety is decidable rather than assumed.
//!
//! Nothing here decides authorization. A valid signature proves who sent a
//! message; it never proves that the sender may invoke what it asks for.

pub mod artifacts;
pub mod audit;
pub mod collaboration;
pub mod compensation;
pub mod delegation;
pub mod discovery;
pub mod envelope;
pub mod identity;
pub mod knowledge;
pub mod membership;
pub mod memory;
pub mod peers;
pub mod provider;
pub mod resource;
pub mod rotation;
pub mod service;
pub mod task;
pub mod zenoh;

pub use artifacts::{
    ArtifactCleanupRequest, ArtifactCleanupResponse, ArtifactFailure, ArtifactStore,
    FileArtifactStore, ARTIFACT_CLEANUP_KIND, ARTIFACT_CLEANUP_SCHEMA, MAX_CLEANUP_ARTIFACTS,
};
pub use audit::{Direction, FederationAudit, FederationEvent, MemoryAudit};
pub use capability::{FederationMessage, FederationTransport, MessageId, PeerSession};
pub use collaboration::{
    ArtifactRef, CollaborationError, DelegationExecutor, Orchestrator, ReceiptAuditor,
    ReceiptMismatch, TaskExecutor, TaskGraph, TaskNode, TaskNodeId, TaskOutcome, TaskProgress,
    TaskState, COLLABORATION_SCHEMA,
};
pub use compensation::{CompensationQueue, PendingCompensation};
pub use delegation::{
    DelegationError, DelegationId, DelegationRecord, DelegationRequest, DelegationState,
    DelegationStore,
};
pub use discovery::DiscoveryRecord;
pub use envelope::{Envelope, EnvelopeError, PROTOCOL, SCHEMA_VERSION};
pub use identity::{IdentityError, NodeIdentity};
pub use knowledge::{
    knowledge_hash, KnowledgeAudit, KnowledgeAuthorizer, KnowledgeClass, KnowledgeError,
    KnowledgeItem, KnowledgeOrigin, KnowledgePublication, KnowledgePurpose, KnowledgeQuery,
    KnowledgeResponse, KnowledgeService, Provenance, KNOWLEDGE_PROTOCOL, KNOWLEDGE_SCHEMA,
    MAX_KNOWLEDGE_BYTES, MAX_KNOWLEDGE_ITEMS,
};
pub use membership::FederationId;
pub use memory::{InMemoryNetwork, InMemoryTransport};
pub use peers::{Grant, PeerError, PeerRecord, PeerStore, TrustState};
pub use provider::RemoteCapabilityProvider;
pub use resource::{
    checksum, LeaseId, LeaseStore, ResourceError, ResourceLease, ResourceStreamMessage,
    StreamReceiver, StreamSender,
};
pub use rotation::RotationRecord;
pub use service::{
    AdmissionLimits, FederationNode, RemoteStream, ResourceSource, MAX_CONCURRENT_STREAMS,
    MAX_IN_FLIGHT_CONTROL, MAX_IN_FLIGHT_CONTROL_PER_PEER, MAX_IN_FLIGHT_HANDLERS,
    MAX_IN_FLIGHT_PER_PEER, MAX_RESOURCE_STREAMS, MAX_STREAM_DURATION, STREAM_CHUNK,
};
pub use task::{
    FederatedTask, FederatedTaskState, TaskCreateRequest, TaskIdRequest, TaskQueue, TaskQueueError,
    TaskSchedule,
};
pub use zenoh::{inbox_key, node_key, Inbox, ZenohTransport};
