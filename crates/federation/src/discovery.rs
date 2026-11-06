//! Signed discovery records.
//!
//! A record says what one Node currently offers. It is signed by that Node, so
//! forged discovery metadata fails verification, and it contains only
//! descriptors marked Public: everything else needs a grant before it is even
//! named (ADR-009). A valid record proves what a Node claims; it never proves
//! that a later call will be authorized or will succeed.

use capability::{unix_ms, CapabilityDescriptor, CapabilityId, NodeId, Visibility};
use serde::{Deserialize, Serialize};

use crate::identity::{self, NodeIdentity};

/// What one Node publishes about its capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    /// The publishing Node.
    pub node: NodeId,
    /// When it was published, in milliseconds since the Unix epoch.
    pub published_at_ms: u64,
    /// Public descriptors only.
    pub descriptors: Vec<CapabilityDescriptor>,
    /// Base64 Ed25519 signature over the canonical form.
    pub signature: String,
}

impl DiscoveryRecord {
    /// Build and sign a record from a Node's current catalogue.
    ///
    /// Descriptors that are not Public are dropped before signing: a record
    /// must not name a capability the issuer would not show to a stranger.
    pub fn publish(identity: &NodeIdentity, descriptors: Vec<CapabilityDescriptor>) -> Self {
        let mut record = Self {
            node: identity.node_id(),
            published_at_ms: unix_ms(),
            descriptors: descriptors
                .into_iter()
                .filter(|descriptor| descriptor.visibility == Visibility::Public)
                .collect(),
            signature: String::new(),
        };
        record.signature = identity.sign(&record.canonical());
        record
    }

    /// Verify that the record was signed by the Node it names.
    pub fn verify(&self) -> bool {
        identity::verify(&self.node, &self.canonical(), &self.signature)
    }

    /// The descriptors this record makes visible.
    pub fn visible(&self) -> &[CapabilityDescriptor] {
        &self.descriptors
    }

    /// One visible descriptor by id.
    pub fn find(&self, capability: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| &descriptor.id == capability)
    }

    /// Serialize the record for a transport frame.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Decode a record from a transport frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    fn canonical(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = String::new();
        serde_json::to_vec(&unsigned).expect("a discovery record serializes")
    }
}
