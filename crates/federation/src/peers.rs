//! Peer trust and grants.
//!
//! Trust is explicit and revocable (ADR-006): discovery never implies trust, a
//! signature never implies permission, and a peer holds exactly the grants it
//! was given. Every question here is answered with the current time, so an
//! expired enrollment or grant stops working without a restart.

use std::collections::BTreeMap;

use capability::{CapabilityError, CapabilityId, NodeId};

use crate::membership::FederationId;
use crate::rotation::RotationRecord;

/// Where a peer relationship stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrustState {
    /// Known but not yet approved. It cannot invoke anything.
    Pending,
    /// Approved; grants still decide what it may invoke.
    Trusted,
    /// Revoked. It cannot invoke anything, and no grant survives.
    Revoked,
}

/// One capability a peer may invoke.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Grant {
    /// The capability granted.
    pub capability: CapabilityId,
    /// When it was granted, in milliseconds since the Unix epoch.
    pub granted_at_ms: u64,
    /// When it stops being valid.
    pub expires_at_ms: Option<u64>,
}

impl Grant {
    /// Whether the grant is still valid at a point in time.
    pub fn is_valid(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_none_or(|expires| now_ms < expires)
    }
}

/// One peer relationship.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerRecord {
    /// The peer.
    pub node: NodeId,
    /// Where the relationship stands.
    pub state: TrustState,
    /// Capabilities it may invoke.
    pub grants: Vec<Grant>,
    /// When it was enrolled, in milliseconds since the Unix epoch.
    pub enrolled_at_ms: u64,
    /// When the enrollment itself stops being valid.
    pub expires_at_ms: Option<u64>,
    /// The federation membership this peer was enrolled under, when known.
    ///
    /// Membership is metadata beside the per-peer trust decision; it never
    /// grants a capability by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub federation: Option<FederationId>,
    /// Why the relationship exists, for an auditor.
    pub note: Option<String>,
}

impl PeerRecord {
    /// Whether the enrollment is still valid.
    pub fn enrolled(&self, now_ms: u64) -> bool {
        self.state == TrustState::Trusted
            && self.expires_at_ms.is_none_or(|expires| now_ms < expires)
    }
}

/// Why an operation on the peer store was refused.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The peer was never enrolled.
    #[error("peer {node} is not enrolled")]
    UnknownPeer {
        /// The peer.
        node: NodeId,
    },
    /// The peer file could not be read or written.
    #[error("cannot access {path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The operating system's error.
        source: std::io::Error,
    },
    /// The peer file is not a valid peer document.
    #[error("the peer file is not valid JSON: {message}")]
    Json {
        /// What the parser said.
        message: String,
    },
    /// A rotation record was refused.
    #[error("rotation refused: {message}")]
    Rotation {
        /// Why.
        message: String,
    },
}

/// The peers this Node knows and what each may do.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerStore {
    peers: BTreeMap<NodeId, PeerRecord>,
}

impl PeerStore {
    /// An empty store: nothing is trusted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enroll a peer as trusted, with no grants yet.
    pub fn enroll(&mut self, node: NodeId, now_ms: u64, note: Option<String>) {
        self.enroll_in(node, None, now_ms, note);
    }

    /// Enroll a peer and record the federation it was enrolled under.
    pub fn enroll_in(
        &mut self,
        node: NodeId,
        federation: Option<FederationId>,
        now_ms: u64,
        note: Option<String>,
    ) {
        let record = PeerRecord {
            node: node.clone(),
            state: TrustState::Trusted,
            grants: Vec::new(),
            enrolled_at_ms: now_ms,
            expires_at_ms: None,
            federation,
            note,
        };
        self.peers.insert(node, record);
    }

    /// The federation recorded for a peer, when it has one.
    pub fn federation_of(&self, node: &NodeId) -> Option<&FederationId> {
        self.peers.get(node)?.federation.as_ref()
    }

    /// Every trusted member of one federation, sorted by node id.
    pub fn members_of(&self, federation: &FederationId) -> Vec<&PeerRecord> {
        self.peers
            .values()
            .filter(|record| record.federation.as_ref() == Some(federation))
            .collect()
    }

    /// Insert or replace a peer record, for loading persisted state.
    pub fn upsert(&mut self, record: PeerRecord) {
        self.peers.insert(record.node.clone(), record);
    }

    /// Apply a signed key rotation, moving the old identity's grants to the new.
    ///
    /// The old key must have signed the successor statement and must still be a
    /// trusted peer here. The old record becomes Revoked; an already-trusted new
    /// identity is refused rather than silently merged.
    pub fn rotate(&mut self, record: &RotationRecord, now_ms: u64) -> Result<(), PeerError> {
        if !record.verify() {
            return Err(PeerError::Rotation {
                message: "the old key did not sign this successor".to_string(),
            });
        }
        let Some(old) = self.peers.get(&record.old).cloned() else {
            return Err(PeerError::Rotation {
                message: format!("{} is not an enrolled peer", record.old),
            });
        };
        if !old.enrolled(now_ms) {
            return Err(PeerError::Rotation {
                message: format!("{} is not currently trusted", record.old),
            });
        }
        if self
            .peers
            .get(&record.new)
            .is_some_and(|existing| existing.enrolled(now_ms))
        {
            return Err(PeerError::Rotation {
                message: format!("{} is already a trusted peer", record.new),
            });
        }
        let mut grants = old.grants.clone();
        grants.retain(|grant| grant.is_valid(now_ms));
        self.peers.insert(
            record.new.clone(),
            PeerRecord {
                node: record.new.clone(),
                state: TrustState::Trusted,
                grants,
                enrolled_at_ms: record.rotated_at_ms,
                expires_at_ms: old.expires_at_ms,
                federation: old.federation.clone(),
                note: Some(format!("rotated from {}", record.old)),
            },
        );
        if let Some(previous) = self.peers.get_mut(&record.old) {
            previous.state = TrustState::Revoked;
            previous.grants.clear();
            previous.note = Some(format!("rotated to {}", record.new));
        }
        Ok(())
    }

    /// Revoke a peer. Its grants are dropped with it.
    pub fn revoke(&mut self, node: &NodeId) -> bool {
        match self.peers.get_mut(node) {
            Some(record) => {
                record.state = TrustState::Revoked;
                record.grants.clear();
                true
            }
            None => false,
        }
    }

    /// Grant one capability, with an optional expiry.
    pub fn grant(
        &mut self,
        node: &NodeId,
        capability: CapabilityId,
        now_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> Result<(), PeerError> {
        let record = self
            .peers
            .get_mut(node)
            .ok_or_else(|| PeerError::UnknownPeer { node: node.clone() })?;
        record.grants.retain(|grant| grant.capability != capability);
        record.grants.push(Grant {
            capability,
            granted_at_ms: now_ms,
            expires_at_ms,
        });
        Ok(())
    }

    /// Remove one grant.
    pub fn revoke_grant(&mut self, node: &NodeId, capability: &CapabilityId) -> bool {
        let Some(record) = self.peers.get_mut(node) else {
            return false;
        };
        let before = record.grants.len();
        record
            .grants
            .retain(|grant| &grant.capability != capability);
        record.grants.len() != before
    }

    /// One peer record.
    pub fn record(&self, node: &NodeId) -> Option<&PeerRecord> {
        self.peers.get(node)
    }

    /// Every peer record.
    pub fn iter(&self) -> impl Iterator<Item = &PeerRecord> {
        self.peers.values()
    }

    /// Whether the peer is enrolled and not expired.
    pub fn is_trusted(&self, node: &NodeId, now_ms: u64) -> bool {
        self.peers
            .get(node)
            .is_some_and(|record| record.enrolled(now_ms))
    }

    /// Decide one remote invocation.
    ///
    /// A refusal is a Denied capability error carrying the reason, because the
    /// caller (and the audit log) need to tell "not enrolled" from "no grant".
    pub fn authorize(
        &self,
        node: &NodeId,
        capability: &CapabilityId,
        now_ms: u64,
    ) -> Result<(), CapabilityError> {
        let Some(record) = self.peers.get(node) else {
            return Err(denied(format!("peer {node} is not enrolled")));
        };
        if record.state == TrustState::Revoked {
            return Err(denied(format!("peer {node} is revoked")));
        }
        if !record.enrolled(now_ms) {
            return Err(denied(format!("peer {node} enrollment has expired")));
        }
        let granted = record
            .grants
            .iter()
            .any(|grant| &grant.capability == capability && grant.is_valid(now_ms));
        if !granted {
            return Err(denied(format!(
                "peer {node} holds no valid grant for {capability}"
            )));
        }
        Ok(())
    }
}

fn denied(reason: String) -> CapabilityError {
    CapabilityError::Denied {
        reason,
        policy: None,
    }
}

impl PeerStore {
    /// Serialize the store for persistence.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Read a store from its persisted form.
    pub fn from_json(text: &str) -> Result<Self, PeerError> {
        serde_json::from_str(text).map_err(|error| PeerError::Json {
            message: error.to_string(),
        })
    }

    /// Write the store atomically, creating the parent directory.
    pub fn save(&self, path: &std::path::Path) -> Result<(), PeerError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| PeerError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let text = self.to_json().map_err(|error| PeerError::Json {
            message: error.to_string(),
        })?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text).map_err(|source| PeerError::Io {
            path: temp.display().to_string(),
            source,
        })?;
        // The store carries who may invoke what; keep it as private as the key
        // rather than relying on the process umask.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path).map_err(|source| PeerError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    /// Load the store, or an empty one when the file does not exist.
    pub fn load(path: &std::path::Path) -> Result<Self, PeerError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(source) => Err(PeerError::Io {
                path: path.display().to_string(),
                source,
            }),
        }
    }
}
