//! An in-process federation transport.
//!
//! It is what the protocol tests run against, and it is the shape every real
//! transport must honor: a frame either leaves this Node or the caller is told
//! it did not. An unknown peer is a not-sent failure, never a silent drop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use capability::transport::{unix_ms, FederationTransport, PeerSession};
use capability::{CapabilityError, DeliveryState, NodeId};
use tokio::sync::mpsc;

type Inbox = mpsc::UnboundedSender<Vec<u8>>;

/// A set of Nodes that can reach each other in one process.
#[derive(Default)]
pub struct InMemoryNetwork {
    peers: Mutex<HashMap<NodeId, Inbox>>,
}

impl InMemoryNetwork {
    /// An empty network.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach one Node and return its transport and its inbox.
    pub fn join(
        self: &Arc<Self>,
        node: NodeId,
    ) -> (InMemoryTransport, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(node.clone(), sender);
        (
            InMemoryTransport {
                node,
                network: Arc::clone(self),
            },
            receiver,
        )
    }

    /// Remove one Node; later frames to it are not sent.
    pub fn leave(&self, node: &NodeId) {
        self.peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(node);
    }

    fn inbox(&self, node: &NodeId) -> Option<Inbox> {
        self.peers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(node)
            .cloned()
    }
}

/// One Node's view of an in-process network.
#[derive(Clone)]
pub struct InMemoryTransport {
    node: NodeId,
    network: Arc<InMemoryNetwork>,
}

impl InMemoryTransport {
    /// The Node this transport speaks for.
    pub fn node(&self) -> &NodeId {
        &self.node
    }
}

#[async_trait]
impl FederationTransport for InMemoryTransport {
    async fn connect(&self, peer: &NodeId) -> Result<PeerSession, CapabilityError> {
        if self.network.inbox(peer).is_none() {
            return Err(CapabilityError::Unavailable {
                reason: format!("peer {peer} is not on this network"),
            });
        }
        Ok(PeerSession {
            peer: peer.clone(),
            opened_at_ms: unix_ms(),
        })
    }

    async fn send(&self, peer: &NodeId, frame: Vec<u8>) -> Result<DeliveryState, CapabilityError> {
        let Some(inbox) = self.network.inbox(peer) else {
            return Err(CapabilityError::Transport {
                message: format!("peer {peer} is not on this network"),
                delivered: DeliveryState::NotSent,
            });
        };
        inbox.send(frame).map_err(|_| CapabilityError::Transport {
            message: format!("peer {peer} is no longer receiving"),
            delivered: DeliveryState::NotSent,
        })?;
        Ok(DeliveryState::Sent)
    }

    async fn close(&self, _peer: &NodeId) -> Result<(), CapabilityError> {
        Ok(())
    }
}
