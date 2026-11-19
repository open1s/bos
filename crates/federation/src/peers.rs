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
    /// Invited, waiting for the peer or the operator to move it to pending.
    Invited,
    /// Known but not yet approved. It cannot invoke anything.
    Pending,
    /// Approved; grants still decide what it may invoke.
    Trusted,
    /// Temporarily stopped. Grants are retained but unusable until resumed.
    Suspended,
    /// Revoked. It cannot invoke anything, and no grant survives.
    Revoked,
}

/// One capability a peer may invoke.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Grant {
    /// The capability granted.
    pub capability: CapabilityId,
    /// Operations this grant permits; empty means every operation.
    #[serde(default)]
    pub operations: Vec<String>,
    /// Resource selectors this grant permits; empty means every resource.
    ///
    /// A selector is an exact string or a prefix ending in '*'.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Workspace selectors this grant permits; empty means every workspace.
    ///
    /// A call that carries no workspace fails closed under a workspace-scoped
    /// grant: absence is not "any".
    #[serde(default)]
    pub workspaces: Vec<String>,
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

/// One committed key rotation, recorded in the durable rotation log.
///
/// The log is what makes a rotation idempotent across restarts: a replayed
/// record whose (old, new, at_ms) triple is already here is answered as a
/// success without re-applying the transition (issue #338).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RotationEntry {
    /// The identity that was retired.
    pub old: NodeId,
    /// The identity that took its grants.
    pub new: NodeId,
    /// When the old key signed the successor statement, in milliseconds
    /// since the Unix epoch.
    pub at_ms: u64,
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
    /// The peer file could not be durably committed (issue #338).
    ///
    /// The previous file is intact and the live store was not changed; a
    /// retryable failure may succeed once storage recovers.
    #[error("cannot commit the peer store at {path}: {detail}")]
    Storage {
        /// The file the commit targeted.
        path: String,
        /// The durable kernel's account, naming the failed stage.
        detail: String,
        /// Whether retrying the same commit later can succeed.
        retryable: bool,
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
    /// The relationship is not in a state that allows this transition.
    #[error("peer {node} cannot move from {from:?} to {to:?}")]
    Transition {
        /// The peer.
        node: NodeId,
        /// Its current state.
        from: TrustState,
        /// The state the caller asked for.
        to: TrustState,
    },
}

/// The peers this Node knows and what each may do.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerStore {
    peers: BTreeMap<NodeId, PeerRecord>,
    /// The durable log of committed key rotations (issue #338).
    ///
    /// An entry is appended only after the rotated store has been durably
    /// committed, and it travels inside the committed snapshot, so a restart
    /// still answers a replayed rotation record as an idempotent success.
    /// The field is serde-defaulted: peer files written before the log
    /// existed load unchanged, with an empty log.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rotations: Vec<RotationEntry>,
}

impl PeerStore {
    /// Forget one peer record entirely, grants included.
    ///
    /// Revocation keeps the record so an audit can still name the relationship;
    /// removal forgets it. Removing a trusted peer therefore revokes it too, and
    /// a later announcement can enroll it again as a new relationship.
    pub fn remove(&mut self, node: &NodeId) -> bool {
        self.peers.remove(node).is_some()
    }

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

    /// Invite a peer. It holds nothing until it is approved.
    pub fn invite(
        &mut self,
        node: NodeId,
        now_ms: u64,
        note: Option<String>,
    ) -> Result<(), PeerError> {
        if let Some(existing) = self.peers.get(&node) {
            return Err(PeerError::Transition {
                node,
                from: existing.state,
                to: TrustState::Invited,
            });
        }
        self.peers.insert(
            node.clone(),
            PeerRecord {
                node,
                state: TrustState::Invited,
                grants: Vec::new(),
                enrolled_at_ms: now_ms,
                expires_at_ms: None,
                federation: None,
                note,
            },
        );
        Ok(())
    }

    /// Approve an invited or pending peer, making it active.
    pub fn approve(&mut self, node: &NodeId, now_ms: u64) -> Result<(), PeerError> {
        let record = self
            .peers
            .get_mut(node)
            .ok_or_else(|| PeerError::UnknownPeer { node: node.clone() })?;
        if !matches!(record.state, TrustState::Invited | TrustState::Pending) {
            return Err(PeerError::Transition {
                node: node.clone(),
                from: record.state,
                to: TrustState::Trusted,
            });
        }
        record.state = TrustState::Trusted;
        record.enrolled_at_ms = now_ms;
        Ok(())
    }

    /// Suspend an active peer. Its grants are retained but unusable.
    pub fn suspend(&mut self, node: &NodeId) -> Result<(), PeerError> {
        let record = self
            .peers
            .get_mut(node)
            .ok_or_else(|| PeerError::UnknownPeer { node: node.clone() })?;
        if record.state != TrustState::Trusted {
            return Err(PeerError::Transition {
                node: node.clone(),
                from: record.state,
                to: TrustState::Suspended,
            });
        }
        record.state = TrustState::Suspended;
        Ok(())
    }

    /// Resume a suspended peer.
    pub fn resume(&mut self, node: &NodeId) -> Result<(), PeerError> {
        let record = self
            .peers
            .get_mut(node)
            .ok_or_else(|| PeerError::UnknownPeer { node: node.clone() })?;
        if record.state != TrustState::Suspended {
            return Err(PeerError::Transition {
                node: node.clone(),
                from: record.state,
                to: TrustState::Trusted,
            });
        }
        record.state = TrustState::Trusted;
        Ok(())
    }
    /// Grant one capability, with an optional expiry.
    pub fn grant(
        &mut self,
        node: &NodeId,
        capability: CapabilityId,
        now_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> Result<(), PeerError> {
        self.grant_scoped(
            node,
            capability,
            Vec::new(),
            Vec::new(),
            now_ms,
            expires_at_ms,
        )
    }

    /// Grant a capability with explicit operation and resource selectors.
    ///
    /// An empty selector list means "any"; a non-empty list narrows the grant,
    /// and a call that supplies no operation/resource then fails closed.
    pub fn grant_scoped(
        &mut self,
        node: &NodeId,
        capability: CapabilityId,
        operations: Vec<String>,
        resources: Vec<String>,
        now_ms: u64,
        expires_at_ms: Option<u64>,
    ) -> Result<(), PeerError> {
        self.grant_scoped_in(
            node,
            capability,
            operations,
            resources,
            Vec::new(),
            now_ms,
            expires_at_ms,
        )
    }

    /// Grant a capability with operation, resource and workspace selectors.
    // The three selector lists are the scope; keeping them explicit avoids a
    // second parallel struct next to `Grant`.
    #[allow(clippy::too_many_arguments)]
    pub fn grant_scoped_in(
        &mut self,
        node: &NodeId,
        capability: CapabilityId,
        operations: Vec<String>,
        resources: Vec<String>,
        workspaces: Vec<String>,
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
            operations,
            resources,
            workspaces,
            granted_at_ms: now_ms,
            expires_at_ms,
        });
        Ok(())
    }

    /// Grants that still rely on the legacy "no selector means any" rule.
    ///
    /// A report an operator can act on: migrate each with `grant_scoped_in`, or
    /// accept that it admits every operation, resource and workspace.
    pub fn unscoped_grants(&self) -> Vec<(NodeId, CapabilityId)> {
        self.peers
            .values()
            .flat_map(|record| {
                record
                    .grants
                    .iter()
                    .filter(|grant| {
                        grant.operations.is_empty()
                            && grant.resources.is_empty()
                            && grant.workspaces.is_empty()
                    })
                    .map(|grant| (record.node.clone(), grant.capability.clone()))
            })
            .collect()
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
        self.authorize_scoped(node, capability, now_ms, None, None, None)
    }

    /// Authorize a scoped call: the capability AND the operation/resource it
    /// touches. A grant with selectors refuses a call that does not name them.
    pub fn authorize_scoped(
        &self,
        node: &NodeId,
        capability: &CapabilityId,
        now_ms: u64,
        operation: Option<&str>,
        resource: Option<&str>,
        workspace: Option<&str>,
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
        let granted = record.grants.iter().any(|grant| {
            &grant.capability == capability
                && grant.is_valid(now_ms)
                && grant.operations.iter().all(|pattern| {
                    operation.is_some_and(|operation| selector_allows(pattern, operation))
                })
                && grant.resources.iter().all(|pattern| {
                    resource.is_some_and(|resource| selector_allows(pattern, resource))
                })
                && grant.workspaces.iter().all(|pattern| {
                    workspace.is_some_and(|workspace| selector_allows(pattern, workspace))
                })
        });
        if !granted {
            return Err(denied(format!(
                "peer {node} holds no valid grant for {capability} at this scope"
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

/// Whether a grant selector admits a value.
///
/// A selector is exact, or a prefix ending in '*'. The bare "*" admits
/// everything.
fn selector_allows(pattern: &str, value: &str) -> bool {
    pattern == "*"
        || pattern == value
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| value.starts_with(prefix))
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
    ///
    /// The store carries who may invoke what; it is kept as private as the
    /// key on every host (issue #299): Unix mode 0600, Windows a protected
    /// owner-only DACL, applied on creation and on replacement. A failure to
    /// establish the private policy fails the save.
    pub fn save(&self, path: &std::path::Path) -> Result<(), PeerError> {
        self.save_with_gate(path, &hostkit::durable::NoFaults)
    }

    /// Commit the store through the durable kernel with a fault-injection
    /// gate (issue #338).
    ///
    /// The commit walks create-dir, create-temp, write, sync, harden and
    /// replace; a failure at any stage leaves the previous file intact and
    /// is reported as a typed `PeerError::Storage` naming the stage and
    /// whether a retry can succeed. Production callers pass
    /// `hostkit::durable::NoFaults`; tests inject a stage failure.
    pub fn save_with_gate(
        &self,
        path: &std::path::Path,
        gate: &dyn hostkit::durable::CommitGate,
    ) -> Result<(), PeerError> {
        let text = self.to_json().map_err(|error| PeerError::Json {
            message: error.to_string(),
        })?;
        hostkit::durable::commit(
            path,
            text.as_bytes(),
            hostkit::durable::Privacy::OwnerOnly,
            gate,
        )
        .map_err(|error| PeerError::Storage {
            path: path.display().to_string(),
            detail: error.to_string(),
            retryable: error.is_retryable(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::NodeIdentity;

    fn peer() -> NodeId {
        NodeIdentity::from_seed([91u8; 32]).node_id()
    }

    fn capability() -> CapabilityId {
        "builtin/read_file".parse().expect("a capability")
    }

    fn store() -> (PeerStore, NodeId) {
        let peer = peer();
        let mut store = PeerStore::new();
        store.enroll(peer.clone(), 1_000, None);
        (store, peer)
    }

    /// Removal forgets the record, not just its grants: the peer disappears
    /// from the list and a second removal has nothing left to do.
    #[test]
    fn removing_a_peer_forgets_its_record_and_grants() {
        let (mut store, peer) = store();
        store
            .grant(&peer, capability(), 1_000, None)
            .expect("grant");
        assert!(store.remove(&peer));
        assert!(!store.iter().any(|record| record.node == peer));
        assert!(store.authorize(&peer, &capability(), 1_000).is_err());
        assert!(!store.remove(&peer), "a second removal reports nothing");
    }

    /// A capability-only grant keeps its historical meaning: any operation and
    /// any resource under that capability.
    #[test]
    fn a_legacy_grant_allows_every_scope() {
        let (mut store, peer) = store();
        store
            .grant(&peer, capability(), 1_000, None)
            .expect("grant");
        assert!(store.authorize(&peer, &capability(), 1_000).is_ok());
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("write"),
                    Some("/etc/passwd"),
                    None
                )
                .is_ok()
        );
        assert_eq!(
            store.unscoped_grants().len(),
            1,
            "the legacy grant is reported"
        );
    }

    /// A scoped grant admits only the listed operations/resources, and refuses
    /// a call that does not name them.
    #[test]
    fn a_scoped_grant_refuses_anything_outside_its_selectors() {
        let (mut store, peer) = store();
        store
            .grant_scoped(
                &peer,
                capability(),
                vec!["read".to_string()],
                vec!["/workspace/*".to_string()],
                1_000,
                None,
            )
            .expect("grant");
        assert!(
            store.unscoped_grants().is_empty(),
            "a scoped grant is not legacy"
        );
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("read"),
                    Some("/workspace/notes.txt"),
                    None
                )
                .is_ok()
        );
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("write"),
                    Some("/workspace/notes.txt"),
                    None
                )
                .is_err(),
            "an unlisted operation is denied"
        );
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("read"),
                    Some("/etc/passwd"),
                    None
                )
                .is_err(),
            "an unlisted resource is denied"
        );
        assert!(
            store
                .authorize_scoped(&peer, &capability(), 1_000, None, None, None)
                .is_err(),
            "a scoped grant fails closed when the call names no scope"
        );
    }

    /// A workspace selector is one more dimension of the same grant.
    #[test]
    fn a_workspace_scoped_grant_refuses_another_workspace() {
        let (mut store, peer) = store();
        store
            .grant_scoped_in(
                &peer,
                capability(),
                vec!["read".to_string()],
                Vec::new(),
                vec!["ws:research".to_string()],
                1_000,
                None,
            )
            .expect("grant");
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("read"),
                    None,
                    Some("ws:research")
                )
                .is_ok()
        );
        assert!(
            store
                .authorize_scoped(
                    &peer,
                    &capability(),
                    1_000,
                    Some("read"),
                    None,
                    Some("ws:other")
                )
                .is_err(),
            "another workspace is denied"
        );
        assert!(
            store
                .authorize_scoped(&peer, &capability(), 1_000, Some("read"), None, None)
                .is_err(),
            "a call with no workspace fails closed"
        );
    }

    /// Membership follows an explicit lifecycle; every wrong transition is
    /// refused instead of silently taking effect.
    #[test]
    fn membership_follows_an_explicit_lifecycle() {
        let mut store = PeerStore::new();
        let peer = peer();
        store
            .invite(peer.clone(), 1_000, None)
            .expect("an invitation");
        assert!(!store.is_trusted(&peer, 1_000), "an invite is not trust");
        assert!(
            store.invite(peer.clone(), 1_000, None).is_err(),
            "no re-invite"
        );
        assert!(
            store.suspend(&peer).is_err(),
            "an invite cannot be suspended"
        );

        store.approve(&peer, 1_000).expect("approval");
        assert!(store.is_trusted(&peer, 1_000));
        assert!(store.approve(&peer, 1_000).is_err(), "already active");
        store.suspend(&peer).expect("suspension");
        assert!(
            !store.is_trusted(&peer, 1_000),
            "a suspended peer cannot invoke"
        );
        store.resume(&peer).expect("resume");
        assert!(store.is_trusted(&peer, 1_000));
        store.revoke(&peer);
        assert!(
            !store.is_trusted(&peer, 1_000),
            "a revoked peer cannot invoke"
        );
        assert!(
            store.resume(&peer).is_err(),
            "a revoked peer is not resumed"
        );
    }

    /// Issue #299: the authorization store is owner-only on creation, and a
    /// replaced pre-existing world-readable file ends up hardened.
    #[test]
    fn the_peer_store_is_private_and_hardens_a_replaced_file() {
        let dir =
            std::env::temp_dir().join(format!("federation-peers-priv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("peers.json");
        let (store, peer) = store();

        store.save(&path).expect("first save");
        let loaded = PeerStore::load(&path).expect("load");
        assert!(loaded.is_trusted(&peer, 1_000));

        // Simulate a legacy world-readable file, then replace it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
        store.save(&path).expect("replacement save");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the replaced store must be owner-only");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A matching federation id is metadata: it never confers trust.
    #[test]
    fn a_federation_id_does_not_grant_trust() {
        let mut store = PeerStore::new();
        let peer = peer();
        store
            .invite(peer.clone(), 1_000, None)
            .expect("an invitation");
        if let Some(record) = store.peers.get_mut(&peer) {
            record.federation = Some(FederationId::new("fed:test"));
        }
        assert!(!store.is_trusted(&peer, 1_000));
        assert!(store.authorize(&peer, &capability(), 1_000).is_err());
    }

    /// A gate that fails exactly one commit stage (issue #338).
    struct FaultAt(hostkit::durable::IoStage);

    impl hostkit::durable::CommitGate for FaultAt {
        fn pass(
            &self,
            stage: hostkit::durable::IoStage,
            _path: &std::path::Path,
        ) -> std::io::Result<()> {
            if stage == self.0 {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected commit failure",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn rotation_fixture() -> (PeerStore, NodeId, NodeId, RotationRecord) {
        let old = NodeIdentity::from_seed([71u8; 32]);
        let new = NodeIdentity::from_seed([72u8; 32]);
        let mut store = PeerStore::new();
        store.enroll(old.node_id(), 1_000, None);
        store
            .grant(&old.node_id(), capability(), 1_000, None)
            .expect("grant");
        let record = RotationRecord::sign(&old, &new, 2_000, None);
        (store, old.node_id(), new.node_id(), record)
    }

    /// Issue #338: an injected failure at every commit stage leaves the
    /// persisted file at the pre-rotation state and reports a typed,
    /// retryable storage error naming the stage.
    #[test]
    fn a_failed_rotation_commit_keeps_the_previous_file() {
        for stage in [
            hostkit::durable::IoStage::CreateTemp,
            hostkit::durable::IoStage::Write,
            hostkit::durable::IoStage::Sync,
            hostkit::durable::IoStage::Replace,
        ] {
            let dir = std::env::temp_dir().join(format!(
                "federation-rotation-fault-{}-{stage}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("peers.json");

            let (store, old_id, new_id, record) = rotation_fixture();
            store
                .save(&path)
                .expect("the pre-rotation state is committed");

            let mut rotated = store.clone();
            rotated
                .rotate(&record, 2_000)
                .expect("the rotation is valid");
            rotated.rotations.push(RotationEntry {
                old: old_id.clone(),
                new: new_id.clone(),
                at_ms: record.rotated_at_ms,
            });
            let error = rotated
                .save_with_gate(&path, &FaultAt(stage))
                .expect_err("the injected stage must fail");
            match &error {
                PeerError::Storage {
                    path: failed,
                    detail,
                    retryable,
                } => {
                    assert_eq!(failed, &path.display().to_string());
                    assert!(detail.contains(&stage.to_string()), "{detail}");
                    assert!(retryable, "an injected I/O failure is retryable");
                }
                other => panic!("expected a Storage error at {stage}, got {other:?}"),
            }

            let on_disk = PeerStore::load(&path).expect("the previous file is intact");
            assert_eq!(on_disk, store, "stage {stage} changed the durable state");
            assert!(on_disk.is_trusted(&old_id, 2_000));
            assert!(on_disk.record(&new_id).is_none());
            assert!(on_disk.rotations.is_empty());

            // Storage recovery: the same rotated store commits on retry.
            rotated
                .save_with_gate(&path, &hostkit::durable::NoFaults)
                .expect("the retry commits once the fault is gone");
            let recovered = PeerStore::load(&path).expect("load after retry");
            assert!(recovered.is_trusted(&new_id, 2_000));
            assert!(!recovered.is_trusted(&old_id, 2_000));
            assert_eq!(recovered.rotations.len(), 1);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Issue #338: the rotation log travels inside the committed snapshot,
    /// and peer files written before the log existed still load.
    #[test]
    fn the_rotation_log_survives_a_round_trip_and_defaults_for_legacy_files() {
        let dir =
            std::env::temp_dir().join(format!("federation-rotation-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("peers.json");

        let (store, old_id, new_id, record) = rotation_fixture();
        let mut rotated = store.clone();
        rotated.rotate(&record, 2_000).expect("rotation");
        rotated.rotations.push(RotationEntry {
            old: old_id.clone(),
            new: new_id.clone(),
            at_ms: record.rotated_at_ms,
        });
        rotated.save(&path).expect("save");
        let loaded = PeerStore::load(&path).expect("load");
        assert_eq!(loaded, rotated);
        assert_eq!(
            loaded.rotations,
            vec![RotationEntry {
                old: old_id,
                new: new_id,
                at_ms: 2_000,
            }]
        );

        // A legacy document without the rotations field is still readable.
        let legacy = store.to_json().expect("serialize");
        assert!(!legacy.contains("rotations"), "an empty log is not written");
        let restored = PeerStore::from_json(&legacy).expect("legacy load");
        assert!(restored.rotations.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
