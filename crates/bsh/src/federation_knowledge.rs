//! The opt-in knowledge surface the BSH federation node installs.
//!
//! Knowledge is off unless the settings enable it, and even then a peer needs an
//! explicit grant for the matching capability. Membership alone never suffices.

use std::sync::{Arc, RwLock};

use agent::capability::{unix_ms, CapabilityId, NodeId, WorkspaceId};
use federation::{KnowledgeAuthorizer, KnowledgeClass, KnowledgePurpose, PeerStore};

/// Query grant a peer must hold to ask this node.
pub(crate) const QUERY_CAPABILITY: &str = "builtin/knowledge_query";
/// Publication grant a peer must hold to publish here.
pub(crate) const PUBLISH_CAPABILITY: &str = "builtin/knowledge_publish";

/// Authorize knowledge traffic from the same peer store that gates capabilities.
pub(crate) struct GrantKnowledgeAuthorizer {
    peers: Arc<RwLock<PeerStore>>,
}

impl GrantKnowledgeAuthorizer {
    /// Bind the authorizer to the live peer store.
    pub(crate) fn new(peers: Arc<RwLock<PeerStore>>) -> Self {
        Self { peers }
    }

    fn holds(&self, peer: &NodeId, capability: &str) -> bool {
        let Ok(capability) = capability.parse::<CapabilityId>() else {
            return false;
        };
        self.peers
            .read()
            .map(|store| store.authorize(peer, &capability, unix_ms()).is_ok())
            .unwrap_or(false)
    }
}

impl KnowledgeAuthorizer for GrantKnowledgeAuthorizer {
    fn may_query(
        &self,
        peer: &NodeId,
        _class: KnowledgeClass,
        _purpose: KnowledgePurpose,
        _scope: Option<&WorkspaceId>,
    ) -> bool {
        self.holds(peer, QUERY_CAPABILITY)
    }

    fn may_publish(&self, peer: &NodeId) -> bool {
        self.holds(peer, PUBLISH_CAPABILITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use federation::{NodeIdentity, PeerStore};

    fn peer() -> NodeId {
        NodeIdentity::from_seed([77u8; 32]).node_id()
    }

    #[test]
    fn knowledge_requires_a_matching_grant() {
        let peer = peer();
        let store = Arc::new(RwLock::new(PeerStore::new()));
        let authorizer = GrantKnowledgeAuthorizer::new(Arc::clone(&store));
        assert!(!authorizer.may_query(
            &peer,
            KnowledgeClass::Public,
            KnowledgePurpose::Answer,
            None
        ));
        assert!(!authorizer.may_publish(&peer));

        store
            .write()
            .expect("peers")
            .enroll(peer.clone(), unix_ms(), None);
        store
            .write()
            .expect("peers")
            .grant(
                &peer,
                QUERY_CAPABILITY.parse().expect("capability"),
                unix_ms(),
                None,
            )
            .expect("grant");
        assert!(authorizer.may_query(
            &peer,
            KnowledgeClass::Public,
            KnowledgePurpose::Answer,
            None
        ));
        assert!(!authorizer.may_publish(&peer));

        store
            .write()
            .expect("peers")
            .grant(
                &peer,
                PUBLISH_CAPABILITY.parse().expect("capability"),
                unix_ms(),
                None,
            )
            .expect("grant");
        assert!(authorizer.may_publish(&peer));
        store.write().expect("peers").revoke(&peer);
        assert!(!authorizer.may_publish(&peer));
    }
}
