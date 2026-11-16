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

type Inbox = mpsc::Sender<Vec<u8>>;

/// One Node's queue holds this many frames before a sender waits.
const QUEUE_CAPACITY: usize = 1024;

/// A set of Nodes that can reach each other in one process.
pub struct InMemoryNetwork {
    peers: Mutex<HashMap<NodeId, Inbox>>,
    announce: tokio::sync::broadcast::Sender<Vec<u8>>,
}

impl Default for InMemoryNetwork {
    fn default() -> Self {
        Self {
            peers: Mutex::new(HashMap::new()),
            announce: tokio::sync::broadcast::channel(256).0,
        }
    }
}

impl InMemoryNetwork {
    /// An empty network.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach one Node and return its transport and its inbox.
    pub fn join(self: &Arc<Self>, node: NodeId) -> (InMemoryTransport, mpsc::Receiver<Vec<u8>>) {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
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
        inbox
            .send(frame)
            .await
            .map_err(|_| CapabilityError::Transport {
                message: format!("peer {peer} is no longer receiving"),
                delivered: DeliveryState::NotSent,
            })?;
        Ok(DeliveryState::Sent)
    }

    async fn close(&self, _peer: &NodeId) -> Result<(), CapabilityError> {
        Ok(())
    }

    async fn announce(&self, frame: Vec<u8>) -> Result<(), CapabilityError> {
        // Broadcast is best-effort by design: a network with no listeners has
        // nowhere to send, and that is not a delivery failure of this frame.
        let _ = self.network.announce.send(frame);
        Ok(())
    }

    async fn subscribe_announce(&self) -> Result<Option<mpsc::Receiver<Vec<u8>>>, CapabilityError> {
        let mut broadcast = self.network.announce.subscribe();
        let (sender, receiver) = mpsc::channel(256);
        tokio::spawn(async move {
            while let Ok(frame) = broadcast.recv().await {
                if sender.send(frame).await.is_err() {
                    break;
                }
            }
        });
        Ok(Some(receiver))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A receiver that stops reading stops the sender at the queue bound; the
    /// process does not grow without limit.
    #[tokio::test]
    async fn a_full_queue_blocks_the_sender_instead_of_growing() {
        let network = Arc::new(InMemoryNetwork::new());
        let peer = crate::identity::NodeIdentity::from_seed([91; 32]).node_id();
        let (transport, receiver) = network.join(peer.clone());

        for _ in 0..QUEUE_CAPACITY {
            transport.send(&peer, vec![0]).await.expect("buffered");
        }

        let blocked =
            tokio::time::timeout(Duration::from_millis(50), transport.send(&peer, vec![1])).await;
        assert!(
            blocked.is_err(),
            "the queue must not grow past its capacity"
        );
        drop(receiver);
    }
}
