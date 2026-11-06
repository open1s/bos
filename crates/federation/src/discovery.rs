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

/// How long a discovery record stays current after it is published.
pub const DISCOVERY_TTL_MS: u64 = 300_000;

/// How far ahead of the local clock a publisher may be before its record is
/// treated as future-dated.
pub const DISCOVERY_CLOCK_SKEW_MS: u64 = 60_000;

/// Why a discovery record is not current enough to trust.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// The record is past its expiry.
    #[error("the discovery record is expired")]
    Expired,
    /// The record claims a time beyond the allowed clock skew.
    #[error("the discovery record is dated in the future")]
    FutureDated,
    /// A newer record from this peer was already accepted.
    #[error("the discovery record is stale: {published_ms} is older than {known_ms}")]
    Stale {
        /// The record's publication time.
        published_ms: u64,
        /// The newest publication this consumer already accepted.
        known_ms: u64,
    },
    /// The signature does not verify.
    #[error("the discovery record's signature does not verify")]
    Invalid,
}

/// What one Node publishes about its capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryRecord {
    /// The publishing Node.
    pub node: NodeId,
    /// The human name the Node gives itself, when it has one.
    ///
    /// It is signed with the rest of the record, so a peer can show it without
    /// inventing a claim about who published it. Absent on an older record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When it was published, in milliseconds since the Unix epoch.
    pub published_at_ms: u64,
    /// When the record stops being current, in milliseconds since the epoch.
    ///
    /// A record is never trusted past this instant. A record written before
    /// expiry existed decodes as 0 and is therefore already stale.
    #[serde(default)]
    pub expires_at_ms: u64,
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
        Self::publish_named(identity, None, descriptors)
    }

    /// Build and sign a record that also carries the Node's display name.
    pub fn publish_named(
        identity: &NodeIdentity,
        name: Option<String>,
        descriptors: Vec<CapabilityDescriptor>,
    ) -> Self {
        let name = name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        let published_at_ms = unix_ms();
        let mut record = Self {
            node: identity.node_id(),
            name,
            published_at_ms,
            expires_at_ms: published_at_ms.saturating_add(DISCOVERY_TTL_MS),
            descriptors: descriptors
                .into_iter()
                .filter(|descriptor| descriptor.visibility == Visibility::Public)
                .collect(),
            signature: String::new(),
        };
        record.signature = identity.sign(&record.canonical());
        record
    }

    /// Check signature, issue time, expiry, and rollback against the newest
    /// publication this consumer already accepted from that peer.
    ///
    /// An older record is a replay and is refused; an equal timestamp is the
    /// same catalogue, so a repeated query stays idempotent.
    pub fn check(&self, now_ms: u64, known_ms: Option<u64>) -> Result<(), DiscoveryError> {
        if !self.verify() {
            return Err(DiscoveryError::Invalid);
        }
        if self.expires_at_ms <= now_ms {
            return Err(DiscoveryError::Expired);
        }
        if self.published_at_ms > now_ms.saturating_add(DISCOVERY_CLOCK_SKEW_MS) {
            return Err(DiscoveryError::FutureDated);
        }
        if let Some(known) = known_ms {
            if self.published_at_ms < known {
                return Err(DiscoveryError::Stale {
                    published_ms: self.published_at_ms,
                    known_ms: known,
                });
            }
        }
        Ok(())
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

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use crate::identity::NodeIdentity;

    fn record(identity: &NodeIdentity, published: u64, expires: u64) -> DiscoveryRecord {
        let mut record = DiscoveryRecord::publish_named(identity, Some("peer".to_string()), vec![]);
        record.published_at_ms = published;
        record.expires_at_ms = expires;
        record.signature = identity.sign(&record.canonical());
        record
    }

    #[test]
    fn a_current_record_passes() {
        let identity = NodeIdentity::from_seed([21; 32]);
        let now = unix_ms();
        let record = record(&identity, now - 1_000, now + 1_000);
        assert_eq!(record.check(now, None), Ok(()));
        assert_eq!(
            record.check(now, Some(now - 1_000)),
            Ok(()),
            "a repeat is idempotent"
        );
    }

    #[test]
    fn an_expired_record_is_refused() {
        let identity = NodeIdentity::from_seed([22; 32]);
        let now = unix_ms();
        let record = record(&identity, now - 2_000, now - 1);
        assert_eq!(record.check(now, None), Err(DiscoveryError::Expired));
    }

    #[test]
    fn a_future_dated_record_is_refused() {
        let identity = NodeIdentity::from_seed([23; 32]);
        let now = unix_ms();
        let future = now + DISCOVERY_CLOCK_SKEW_MS + 1_000;
        let record = record(&identity, future, future + DISCOVERY_TTL_MS);
        assert_eq!(record.check(now, None), Err(DiscoveryError::FutureDated));
    }

    #[test]
    fn an_older_replay_is_refused() {
        let identity = NodeIdentity::from_seed([24; 32]);
        let now = unix_ms();
        let record = record(&identity, now - 2_000, now + 1_000);
        assert_eq!(
            record.check(now, Some(now - 1_000)),
            Err(DiscoveryError::Stale {
                published_ms: now - 2_000,
                known_ms: now - 1_000,
            })
        );
    }

    #[test]
    fn a_tampered_record_is_invalid() {
        let identity = NodeIdentity::from_seed([25; 32]);
        let now = unix_ms();
        let mut record = record(&identity, now - 1_000, now + 1_000);
        record.name = Some("someone-else".to_string());
        assert_eq!(record.check(now, None), Err(DiscoveryError::Invalid));
    }
}
