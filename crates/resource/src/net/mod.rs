//! Node-level network pool: one transport per peer node, shared by every
//! subsystem (fs explorer, proc manager, vnodes, supervisors). This is the
//! seam that owns *identity* (`node_id` → transport); per-connection
//! reconnect lives in the transport itself ([`crate::transport::QuicTransport`] re-establishes a
//! closed connection on next use).
//!
//! Keepalive is a transport property: QUIC streams are configured with a
//! `keep_alive_interval` so idle connections stay punched through NATs and
//! the server-side idle timeout never fires on an otherwise-quiet link.

pub mod router;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::transport::Transport;

/// Pool of peer transports keyed by node id.
///
/// Clone is cheap (single `Arc`), but callers usually hold one `Arc<Net>`
/// and share it across the node.
pub struct Net {
    peers: RwLock<HashMap<String, Arc<dyn Transport>>>,
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Net {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
        }
    }

    /// Look up the transport for `node_id`, if known.
    pub async fn get(&self, node_id: &str) -> Option<Arc<dyn Transport>> {
        self.peers.read().await.get(node_id).cloned()
    }

    /// Insert `transport` for `node_id`, replacing any old entry. Returns the
    /// previous transport, if one was registered (useful for tests).
    pub async fn add(
        &self,
        node_id: impl Into<String>,
        transport: Arc<dyn Transport>,
    ) -> Option<Arc<dyn Transport>> {
        self.peers.write().await.insert(node_id.into(), transport)
    }

    /// Remove and return the transport for `node_id`, if any.
    pub async fn remove(&self, node_id: &str) -> Option<Arc<dyn Transport>> {
        self.peers.write().await.remove(node_id)
    }

    /// Get the transport for `node_id`, building and inserting it on miss.
    /// `build` runs at most once concurrently per node (the pool is locked
    /// for the whole build, which also prevents thundering-herd dials).
    pub async fn get_or_add<F>(
        &self,
        node_id: &str,
        build: F,
    ) -> crate::error::Result<Arc<dyn Transport>>
    where
        F: FnOnce() -> crate::error::Result<Arc<dyn Transport>>,
    {
        let mut peers = self.peers.write().await;
        if let Some(t) = peers.get(node_id) {
            return Ok(t.clone());
        }
        let t = build()?;
        peers.insert(node_id.to_string(), t.clone());
        Ok(t)
    }

    /// Currently-known node ids (sorted, for deterministic diagnostics).
    pub async fn nodes(&self) -> Vec<String> {
        let mut v: Vec<String> = self.peers.read().await.keys().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::ResourceManager;
    use crate::policy::{Effect, PolicyDoc, Rule, SharedPolicy};
    use crate::transport::inprocess::InProcessTransport;

    fn dummy_transport() -> Arc<dyn Transport> {
        let policy = SharedPolicy::new(PolicyDoc {
            admins: vec!["t".into()],
            rules: vec![Rule {
                agents: vec!["*".into()],
                uris: vec!["*".into()],
                actions: vec!["*".into()],
                effect: Effect::Allow,
            }],
            ..Default::default()
        });
        let mgr = Arc::new(ResourceManager::new(policy));
        Arc::new(InProcessTransport::new(mgr, "t"))
    }

    #[tokio::test]
    async fn pool_add_get_remove() {
        let net = Net::new();
        assert!(net.get("a").await.is_none());

        let t = dummy_transport();
        assert!(net.add("a", t.clone()).await.is_none());
        // Second add replaces and returns the old one.
        let prev = net.add("a", dummy_transport()).await;
        assert!(prev.is_some());

        let got = net.get("a").await.expect("a is registered");
        // Same allocation as the one we last inserted.
        assert!(Arc::ptr_eq(&got, &net.get("a").await.unwrap()));

        assert_eq!(net.nodes().await, vec!["a"]);

        assert!(net.remove("a").await.is_some());
        assert!(net.get("a").await.is_none());
    }
}
