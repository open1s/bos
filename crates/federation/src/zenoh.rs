//! The Zenoh transport: sealed frames addressed by Node id.
//!
//! Key space (ADR-009):
//!
//!   `bsh/v1/node/<node-key>/inbox`
//!
//! where node-key is the hex public key. The id is never the routing decision:
//! a peer that can publish to a key is still refused by the receiving kernel.
//! The key only says where a frame is delivered.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use capability::transport::{unix_ms, FederationTransport, PeerSession};
use capability::{CapabilityError, DeliveryState, NodeId};
use tokio::sync::mpsc;

use crate::identity;

/// The key expression prefix every federation frame lives under.
pub const KEY_PREFIX: &str = "bsh/v1/node";

/// The shared announcement key: presence and public descriptors, never writes.
pub const ANNOUNCE_KEY: &str = "bsh/v1/announce";

/// How many inbox frames the transport buffers ahead of the router.
///
/// A slow router must not let a sender grow the process without bound, so the
/// queue is bounded and overflow is dropped and logged. A frame is one sealed
/// message; the cap is far above any healthy burst.
const INGRESS_CAPACITY: usize = 1024;

/// The announcement queue is smaller: announcements are periodic presence, and
/// losing one is the same as waiting for the next tick.
const ANNOUNCE_CAPACITY: usize = 256;

/// The Zenoh transport for one Node.
#[derive(Clone)]
pub struct ZenohTransport {
    session: Arc<bus::Session>,
    node: NodeId,
}

impl ZenohTransport {
    /// Build a transport over an open session.
    pub fn new(session: Arc<bus::Session>, node: NodeId) -> Self {
        Self { session, node }
    }

    /// The Node this transport speaks for.
    pub fn node(&self) -> &NodeId {
        &self.node
    }

    /// Publish one announcement frame to the shared announce key.
    pub async fn announce(&self, frame: Vec<u8>) -> Result<(), CapabilityError> {
        self.session
            .put(ANNOUNCE_KEY, frame)
            .await
            .map_err(|error| CapabilityError::Unavailable {
                reason: format!("cannot publish an announcement: {error}"),
            })
    }

    /// Subscribe to announcements from every Node on this transport.
    pub async fn subscribe_announce(&self) -> Result<Inbox, CapabilityError> {
        let subscriber = self
            .session
            .declare_subscriber(ANNOUNCE_KEY)
            .await
            .map_err(|error| CapabilityError::Unavailable {
                reason: format!("cannot subscribe to announcements: {error}"),
            })?;
        let (sender, receiver) = mpsc::channel(ANNOUNCE_CAPACITY);
        tokio::spawn(async move {
            let mut dropped = 0u64;
            while let Ok(sample) = subscriber.recv_async().await {
                let frame = sample.payload().to_bytes().to_vec();
                match sender.try_send(frame) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        dropped += 1;
                        if dropped == 1 || dropped.is_multiple_of(1024) {
                            tracing::warn!(
                                "federation: announcement queue is full; dropped {dropped} frame(s)"
                            );
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        });
        Ok(Inbox { receiver })
    }

    /// Subscribe to this Node's inbox.
    ///
    /// The returned inbox yields raw frames; decoding and verification happen
    /// above the transport, where the envelope lives.
    pub async fn subscribe(&self) -> Result<Inbox, CapabilityError> {
        let key = inbox_key(&self.node)?;
        let subscriber = self
            .session
            .declare_subscriber(key)
            .await
            .map_err(|error| CapabilityError::Unavailable {
                reason: format!("cannot subscribe to this Node's inbox: {error}"),
            })?;
        let (sender, receiver) = mpsc::channel(INGRESS_CAPACITY);
        tokio::spawn(async move {
            let mut dropped = 0u64;
            while let Ok(sample) = subscriber.recv_async().await {
                let frame = sample.payload().to_bytes().to_vec();
                match sender.try_send(frame) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        dropped += 1;
                        if dropped == 1 || dropped.is_multiple_of(1024) {
                            tracing::warn!(
                                "federation: inbox queue is full; dropped {dropped} frame(s)"
                            );
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        });
        Ok(Inbox { receiver })
    }
}

/// A Node's inbox as a stream of frames.
pub struct Inbox {
    receiver: mpsc::Receiver<Vec<u8>>,
}

impl Inbox {
    /// Take the frame receiver, for a caller that routes frames itself.
    pub fn into_receiver(self) -> mpsc::Receiver<Vec<u8>> {
        self.receiver
    }

    /// Receive the next frame. None when the subscription ended.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.receiver.recv().await
    }

    /// Receive the next frame, failing when nothing arrives in time.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        tokio::time::timeout(timeout, self.receiver.recv())
            .await
            .ok()
            .flatten()
    }
}

#[async_trait]
impl FederationTransport for ZenohTransport {
    async fn connect(&self, peer: &NodeId) -> Result<PeerSession, CapabilityError> {
        // The address is derived from the identity, so a malformed peer id is
        // refused before any network work happens.
        let _ = inbox_key(peer)?;
        Ok(PeerSession {
            peer: peer.clone(),
            opened_at_ms: unix_ms(),
        })
    }

    async fn send(&self, peer: &NodeId, frame: Vec<u8>) -> Result<DeliveryState, CapabilityError> {
        let key = inbox_key(peer)?;
        match self.session.put(key, frame).await {
            Ok(()) => Ok(DeliveryState::Sent),
            Err(error) => Err(CapabilityError::Transport {
                message: error.to_string(),
                delivered: DeliveryState::NotSent,
            }),
        }
    }

    async fn close(&self, _peer: &NodeId) -> Result<(), CapabilityError> {
        Ok(())
    }

    async fn announce(&self, frame: Vec<u8>) -> Result<(), CapabilityError> {
        ZenohTransport::announce(self, frame).await
    }

    async fn subscribe_announce(&self) -> Result<Option<mpsc::Receiver<Vec<u8>>>, CapabilityError> {
        Ok(Some(self.subscribe_announce().await?.into_receiver()))
    }
}

/// The inbox key expression for one Node.
pub fn inbox_key(node: &NodeId) -> Result<String, CapabilityError> {
    Ok(format!("{KEY_PREFIX}/{}/inbox", node_key(node)?))
}

/// The filesystem- and key-expression-safe form of a Node id.
///
/// A base64 Node id contains "=", "+", and "/", and "/" is the key expression
/// separator; the hex form is stable and cannot be mistaken for a hierarchy.
pub fn node_key(node: &NodeId) -> Result<String, CapabilityError> {
    let bytes = STANDARD
        .decode(node.as_str())
        .map_err(|error| CapabilityError::Invalid {
            message: format!("Node id {} is not base64: {error}", node),
        })?;
    if !identity::is_valid_node_id(node) {
        return Err(CapabilityError::Invalid {
            message: format!("Node id {node} is not an Ed25519 public key"),
        });
    }
    Ok(bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}
