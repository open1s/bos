//! The transport contract the federation protocol is defined above.
//!
//! A transport carries messages between Nodes and reports whether a message
//! left this Node. It never decides authorization and never changes what a
//! message means (ADR-005, ADR-009). Zenoh implements this trait in the
//! federation crate; the protocol types here do not know that.

use async_trait::async_trait;

use crate::error::{CapabilityError, DeliveryState};
use crate::id::NodeId;

/// Milliseconds since the Unix epoch.
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// One message between two Nodes.
#[derive(Debug, Clone, PartialEq)]
pub struct FederationMessage {
    /// Message identity, unique per sender.
    pub id: MessageId,
    /// The message kind, for example invoke, result, event, cancel, discover.
    pub kind: String,
    /// Opaque payload bytes.
    pub payload: Vec<u8>,
    /// Correlation id shared by a request and its result.
    pub correlation: Option<String>,
    /// Deduplication key, when the operation supports one.
    pub idempotency_key: Option<String>,
    /// Absolute expiry in milliseconds, when the message has one.
    pub expires_at_ms: Option<u64>,
}

impl FederationMessage {
    /// Build a message.
    pub fn new(kind: impl Into<String>, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            id: MessageId::new(uuid_like()),
            kind: kind.into(),
            payload: payload.into(),
            correlation: None,
            idempotency_key: None,
            expires_at_ms: None,
        }
    }

    /// Attach a correlation id.
    pub fn with_correlation(mut self, correlation: impl Into<String>) -> Self {
        self.correlation = Some(correlation.into());
        self
    }

    /// Attach an idempotency key.
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Attach an absolute expiry.
    pub fn with_expiry_ms(mut self, expires_at_ms: u64) -> Self {
        self.expires_at_ms = Some(expires_at_ms);
        self
    }
}

/// A message identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(String);

impl MessageId {
    /// Build a message id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An authenticated session with one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSession {
    /// The peer.
    pub peer: NodeId,
    /// When the session was opened, in milliseconds since the Unix epoch.
    pub opened_at_ms: u64,
}

/// The transport seam.
///
/// The three operations are deliberately small, and send carries an opaque
/// frame: the protocol seals a message into one before it reaches a transport,
/// so a transport cannot alter or fabricate a message. Streaming, chunking, and
/// progress are message kinds inside the frame, not additional methods that
/// every implementation would then have to fake.
#[async_trait]
pub trait FederationTransport: Send + Sync {
    /// Open or reuse a session with a peer.
    async fn connect(&self, peer: &NodeId) -> Result<PeerSession, CapabilityError>;

    /// Send one sealed frame, reporting whether it left this Node.
    async fn send(&self, peer: &NodeId, frame: Vec<u8>) -> Result<DeliveryState, CapabilityError>;

    /// Close the session with a peer.
    async fn close(&self, peer: &NodeId) -> Result<(), CapabilityError>;

    /// Publish an announcement to every Node sharing this transport.
    ///
    /// An announcement carries presence and public descriptors; it is not a
    /// request and is never part of the invoke path. A transport that cannot
    /// broadcast says so, and the runtime runs without discovery.
    async fn announce(&self, _frame: Vec<u8>) -> Result<(), CapabilityError> {
        Err(CapabilityError::Unavailable {
            reason: "this transport cannot announce".to_string(),
        })
    }

    /// Subscribe to announcements. `None` means the transport cannot broadcast.
    async fn subscribe_announce(
        &self,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<Vec<u8>>>, CapabilityError> {
        Ok(None)
    }
}

fn uuid_like() -> String {
    // A counter plus nanoseconds is enough for an in-process message id and
    // needs no dependency; the wire format treats it as opaque.
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let counter = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}-{counter:x}")
}
