//! Signed Node key rotation.
//!
//! A NodeId is an Ed25519 public key, so changing the key changes the identity.
//! A rotation record lets the old key authorize the new one, preserving the
//! peer's grants across the change without letting a third party substitute a
//! key. The record is data; a peer must still apply it to its own peer store.

use capability::NodeId;
use serde::{Deserialize, Serialize};

use crate::identity::{self, NodeIdentity};

/// A statement by the old key that a new key is its successor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotationRecord {
    /// The previous identity.
    pub old: NodeId,
    /// The successor identity.
    pub new: NodeId,
    /// When the old key signed this statement.
    pub rotated_at_ms: u64,
    /// Why the key changed, for an auditor.
    pub reason: Option<String>,
    /// Base64 Ed25519 signature by the old key.
    pub signature: String,
}

impl RotationRecord {
    /// Sign a successor statement with the old identity.
    pub fn sign(
        old: &NodeIdentity,
        new: &NodeIdentity,
        rotated_at_ms: u64,
        reason: Option<String>,
    ) -> Self {
        let mut record = Self {
            old: old.node_id(),
            new: new.node_id(),
            rotated_at_ms,
            reason: reason
                .map(|reason| reason.trim().to_string())
                .filter(|reason| !reason.is_empty()),
            signature: String::new(),
        };
        record.signature = old.sign(&record.canonical());
        record
    }

    /// Verify that the old key signed this exact successor statement.
    pub fn verify(&self) -> bool {
        self.old != self.new
            && !self.signature.is_empty()
            && identity::verify(&self.old, &self.canonical(), &self.signature)
    }

    /// Serialize the record for transfer.
    pub fn to_json(&self) -> std::io::Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Parse a record.
    pub fn from_json(text: &str) -> std::io::Result<Self> {
        serde_json::from_str(text)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    fn canonical(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.signature = String::new();
        serde_json::to_vec(&unsigned).expect("a rotation record serializes")
    }
}
