//! Knowledge Federation: governed knowledge exchange over the Federation envelope.
//!
//! This is a separate protocol, not an extension of capability invocation. A
//! query carries its purpose, classification ceiling, scope, and minimisation
//! budget; a publication carries provenance and integrity metadata. Remote
//! knowledge is always returned as remote provenance, never merged into local
//! trusted memory, so a consumer can evaluate it independently.

use std::collections::{BTreeMap, BTreeSet};

use capability::{FederationMessage, NodeId, WorkspaceId};

use crate::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The protocol name and version carried by knowledge messages.
pub const KNOWLEDGE_PROTOCOL: &str = "bsh.fed.knowledge/1";
/// The schema version of the knowledge contracts.
pub const KNOWLEDGE_SCHEMA: u32 = 1;
/// The envelope kind for a knowledge query.
pub const KNOWLEDGE_QUERY_KIND: &str = "knowledge_query";
/// The envelope kind for a knowledge publication.
pub const KNOWLEDGE_PUBLICATION_KIND: &str = "knowledge_publication";
/// The envelope kind for a knowledge response.
pub const KNOWLEDGE_RESPONSE_KIND: &str = "knowledge_response";

/// The largest publication one message may carry.
pub const MAX_KNOWLEDGE_ITEMS: usize = 256;
/// The largest payload one item may carry.
pub const MAX_KNOWLEDGE_BYTES: usize = 256 * 1024;

/// How sensitive one knowledge item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnowledgeClass {
    /// Safe for any authorized peer.
    Public,
    /// Scoped to one workspace.
    Workspace,
    /// Requires an explicit restricted grant.
    Restricted,
}

impl KnowledgeClass {
    /// Whether an item of this class may answer a query with this ceiling.
    pub fn allowed_by(self, ceiling: Self) -> bool {
        self <= ceiling
    }
}

/// Why a peer is asking for knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnowledgePurpose {
    /// Answer a user question.
    Answer,
    /// Verify a claim or a plan.
    Verify,
    /// Compose a new plan.
    Compose,
}

/// One versioned knowledge query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeQuery {
    /// Schema version; only KNOWLEDGE_SCHEMA is accepted.
    pub schema: u32,
    /// Query identity, used to correlate the response.
    pub query_id: String,
    /// Why the peer asks.
    pub purpose: KnowledgePurpose,
    /// The most sensitive class the peer may receive.
    pub max_class: KnowledgeClass,
    /// The workspace the answer is for, when scoped.
    pub scope: Option<WorkspaceId>,
    /// Terms the answer must contain (all of them).
    pub terms: Vec<String>,
    /// At most this many items.
    pub max_items: usize,
    /// Absolute deadline in milliseconds since the Unix epoch.
    pub deadline_ms: Option<u64>,
}

impl KnowledgeQuery {
    /// A query with the given id and purpose, public class, no scope or terms.
    pub fn new(query_id: impl Into<String>, purpose: KnowledgePurpose) -> Self {
        Self {
            schema: KNOWLEDGE_SCHEMA,
            query_id: query_id.into(),
            purpose,
            max_class: KnowledgeClass::Public,
            scope: None,
            terms: Vec::new(),
            max_items: 16,
            deadline_ms: None,
        }
    }

    /// Refuse an unsupported schema.
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.schema != KNOWLEDGE_SCHEMA {
            return Err(KnowledgeError::Schema {
                found: self.schema,
                expected: KNOWLEDGE_SCHEMA,
            });
        }
        if self.max_items == 0 || self.max_items > MAX_KNOWLEDGE_ITEMS {
            return Err(KnowledgeError::Invalid {
                message: format!("max_items must be between 1 and {MAX_KNOWLEDGE_ITEMS}"),
            });
        }
        Ok(())
    }

    /// Whether this query's deadline already passed.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.deadline_ms.is_some_and(|deadline| deadline <= now_ms)
    }

    /// Wrap this query in a Federation message for the authenticated envelope.
    pub fn to_message(&self) -> Result<FederationMessage, KnowledgeError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| KnowledgeError::Invalid {
            message: error.to_string(),
        })?;
        Ok(FederationMessage::new(KNOWLEDGE_QUERY_KIND, bytes))
    }

    /// Read a query from an authenticated Federation message.
    pub fn from_message(message: &FederationMessage) -> Result<Self, KnowledgeError> {
        if message.kind != KNOWLEDGE_QUERY_KIND {
            return Err(KnowledgeError::Invalid {
                message: format!("expected {KNOWLEDGE_QUERY_KIND}, got {}", message.kind),
            });
        }
        let query: Self =
            serde_json::from_slice(&message.payload).map_err(|error| KnowledgeError::Invalid {
                message: error.to_string(),
            })?;
        query.validate()?;
        Ok(query)
    }

    /// Read a query from an already-verified envelope.
    pub fn from_envelope(envelope: &Envelope) -> Result<Self, KnowledgeError> {
        if envelope.kind != KNOWLEDGE_QUERY_KIND {
            return Err(KnowledgeError::Invalid {
                message: format!("expected {KNOWLEDGE_QUERY_KIND}, got {}", envelope.kind),
            });
        }
        let query: Self = serde_json::from_value(envelope.payload.clone()).map_err(|error| {
            KnowledgeError::Invalid {
                message: error.to_string(),
            }
        })?;
        query.validate()?;
        Ok(query)
    }
}

/// Where one knowledge item came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// The Node that published the item.
    pub origin: NodeId,
    /// The source reference inside that Node (a path, record id, or URL).
    pub source: String,
    /// When the source was read, in milliseconds since the Unix epoch.
    pub issued_at_ms: u64,
    /// When the item stops being valid.
    pub expires_at_ms: Option<u64>,
    /// Hex SHA-256 of the canonical payload.
    pub content_hash: String,
}

/// One item of knowledge with its provenance and classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeItem {
    /// Stable item identity.
    pub id: String,
    /// Sensitivity.
    pub class: KnowledgeClass,
    /// The workspace this item belongs to, when it is workspace-bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    /// The knowledge itself. It is data, never instructions.
    pub payload: Value,
    /// Where it came from and how to verify it.
    pub provenance: Provenance,
    /// Optional retention ceiling from the moment of publication.
    pub retention_ms: Option<u64>,
}

impl KnowledgeItem {
    /// Whether this item is fresh at the given instant.
    pub fn fresh(&self, now_ms: u64) -> bool {
        if self
            .provenance
            .expires_at_ms
            .is_some_and(|expiry| expiry <= now_ms)
        {
            return false;
        }
        if let Some(retention) = self.retention_ms
            && self.provenance.issued_at_ms.saturating_add(retention) <= now_ms
        {
            return false;
        }
        true
    }

    /// Verify that the provenance names this item and the hash matches.
    pub fn verify_provenance(&self, publisher: &NodeId) -> Result<(), KnowledgeError> {
        if &self.provenance.origin != publisher {
            return Err(KnowledgeError::BadProvenance {
                item: self.id.clone(),
                reason: "the origin is not the authenticated publisher".to_string(),
            });
        }
        if self.provenance.content_hash != knowledge_hash(&self.payload) {
            return Err(KnowledgeError::BadProvenance {
                item: self.id.clone(),
                reason: "the content hash does not match the payload".to_string(),
            });
        }
        if self.provenance.issued_at_ms == 0 {
            return Err(KnowledgeError::BadProvenance {
                item: self.id.clone(),
                reason: "the issue time is missing".to_string(),
            });
        }
        Ok(())
    }
}

/// One versioned publication.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgePublication {
    /// Schema version; only KNOWLEDGE_SCHEMA is accepted.
    pub schema: u32,
    /// The items being published.
    pub items: Vec<KnowledgeItem>,
}

impl KnowledgePublication {
    /// A publication at the current schema.
    pub fn new(items: Vec<KnowledgeItem>) -> Self {
        Self {
            schema: KNOWLEDGE_SCHEMA,
            items,
        }
    }

    /// Wrap this publication in a Federation message.
    pub fn to_message(&self) -> Result<FederationMessage, KnowledgeError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| KnowledgeError::Invalid {
            message: error.to_string(),
        })?;
        Ok(FederationMessage::new(KNOWLEDGE_PUBLICATION_KIND, bytes))
    }

    /// Read a publication from an authenticated Federation message.
    pub fn from_message(message: &FederationMessage) -> Result<Self, KnowledgeError> {
        if message.kind != KNOWLEDGE_PUBLICATION_KIND {
            return Err(KnowledgeError::Invalid {
                message: format!(
                    "expected {KNOWLEDGE_PUBLICATION_KIND}, got {}",
                    message.kind
                ),
            });
        }
        let publication: Self =
            serde_json::from_slice(&message.payload).map_err(|error| KnowledgeError::Invalid {
                message: error.to_string(),
            })?;
        publication.validate()?;
        Ok(publication)
    }

    /// Read a publication from an already-verified envelope.
    pub fn from_envelope(envelope: &Envelope) -> Result<Self, KnowledgeError> {
        if envelope.kind != KNOWLEDGE_PUBLICATION_KIND {
            return Err(KnowledgeError::Invalid {
                message: format!(
                    "expected {KNOWLEDGE_PUBLICATION_KIND}, got {}",
                    envelope.kind
                ),
            });
        }
        let publication: Self =
            serde_json::from_value(envelope.payload.clone()).map_err(|error| {
                KnowledgeError::Invalid {
                    message: error.to_string(),
                }
            })?;
        publication.validate()?;
        Ok(publication)
    }

    /// Refuse an unsupported or oversized publication.
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.schema != KNOWLEDGE_SCHEMA {
            return Err(KnowledgeError::Schema {
                found: self.schema,
                expected: KNOWLEDGE_SCHEMA,
            });
        }
        if self.items.is_empty() || self.items.len() > MAX_KNOWLEDGE_ITEMS {
            return Err(KnowledgeError::Invalid {
                message: format!("a publication must carry 1..={MAX_KNOWLEDGE_ITEMS} items"),
            });
        }
        Ok(())
    }
}

/// The origin of a response. Remote knowledge is always marked remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnowledgeOrigin {
    /// Knowledge authored and trusted on this Node.
    Local,
    /// Knowledge returned by another Node; never implicitly trusted.
    Remote {
        /// The Node that answered.
        node: NodeId,
    },
}

/// One versioned query response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeResponse {
    /// Schema version; only KNOWLEDGE_SCHEMA is accepted.
    pub schema: u32,
    /// The query this answers.
    pub query_id: String,
    /// The Node that answered.
    pub responder: NodeId,
    /// Local or remote; remote is never implicitly trusted.
    pub origin: KnowledgeOrigin,
    /// The retained items.
    pub items: Vec<KnowledgeItem>,
    /// Whether a larger result was cut to the requested budget.
    pub truncated: bool,
}

/// One audit record for a knowledge query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeAudit {
    /// Who asked.
    pub requester: NodeId,
    /// The query id.
    pub query_id: String,
    /// The purpose.
    pub purpose: KnowledgePurpose,
    /// How many items were returned.
    pub returned: usize,
    /// Whether authorization refused the query.
    pub denied: bool,
}

/// Why a knowledge operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum KnowledgeError {
    /// The schema version is unsupported.
    #[error("unsupported knowledge schema {found}, expected {expected}")]
    Schema {
        /// What arrived.
        found: u32,
        /// What this build accepts.
        expected: u32,
    },
    /// The payload is malformed.
    #[error("invalid knowledge request: {message}")]
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// The peer may not ask this question.
    #[error("peer {peer} may not query {class:?} knowledge for {purpose:?}")]
    Unauthorized {
        /// The refused peer.
        peer: NodeId,
        /// The requested class.
        class: KnowledgeClass,
        /// The requested purpose.
        purpose: KnowledgePurpose,
    },
    /// The query deadline already passed.
    #[error("the knowledge query expired")]
    Stale,
    /// Provenance does not verify.
    #[error("provenance for {item} is invalid: {reason}")]
    BadProvenance {
        /// The item.
        item: String,
        /// What failed.
        reason: String,
    },
    /// The item is already published by another Node.
    #[error("knowledge {id} is already published")]
    Duplicate {
        /// The item id.
        id: String,
    },
    /// The item was revoked and cannot be republished.
    #[error("knowledge {id} was revoked")]
    Revoked {
        /// The item id.
        id: String,
    },
    /// Only the publisher can revoke an item.
    #[error("peer {peer} cannot revoke knowledge {id}")]
    RevokeDenied {
        /// The peer.
        peer: NodeId,
        /// The item.
        id: String,
    },
    /// A payload exceeds the byte ceiling.
    #[error("knowledge item {id} is larger than {MAX_KNOWLEDGE_BYTES} bytes")]
    TooLarge {
        /// The item.
        id: String,
    },
    /// The knowledge snapshot could not be durably committed (issue #339).
    ///
    /// The previous file is intact and the live store was not changed; a
    /// retryable failure may succeed once storage recovers.
    #[error("cannot commit the knowledge store at {path}: {detail}")]
    Storage {
        /// The file the commit targeted.
        path: String,
        /// The durable kernel's account, naming the failed stage.
        detail: String,
        /// Whether retrying the same commit later can succeed.
        retryable: bool,
    },
    /// An existing knowledge file is not a readable snapshot (issue #339).
    ///
    /// Distinct from an invalid request: a host loading this at startup must
    /// fail closed and preserve the file, never substitute an empty store.
    #[error("the knowledge store is not a valid snapshot: {message}")]
    InvalidStore {
        /// What the parser said.
        message: String,
    },
}

/// The host decision for one knowledge query or publication.
///
/// The default is deny: a Node without an authorizer does not answer or accept
/// knowledge messages. The callback sees the authenticated peer, requested
/// class, purpose, and scope, so a grant revocation takes effect on the next
/// query.
pub trait KnowledgeAuthorizer: Send + Sync {
    /// Whether this peer may run this query.
    fn may_query(
        &self,
        peer: &NodeId,
        class: KnowledgeClass,
        purpose: KnowledgePurpose,
        scope: Option<&WorkspaceId>,
    ) -> bool;

    /// Whether this peer may publish knowledge here.
    fn may_publish(&self, peer: &NodeId) -> bool;
}

/// Map a knowledge failure onto the wire capability taxonomy.
impl From<KnowledgeError> for capability::CapabilityError {
    fn from(error: KnowledgeError) -> Self {
        use capability::CapabilityError;
        match error {
            KnowledgeError::Schema { .. } | KnowledgeError::Invalid { .. } => {
                CapabilityError::Invalid {
                    message: error.to_string(),
                }
            }
            KnowledgeError::Unauthorized { .. } | KnowledgeError::RevokeDenied { .. } => {
                CapabilityError::Denied {
                    reason: error.to_string(),
                    policy: None,
                }
            }
            KnowledgeError::Stale => CapabilityError::Timeout {
                elapsed_ms: 0,
                partial: None,
            },
            KnowledgeError::BadProvenance { .. } => CapabilityError::Invalid {
                message: error.to_string(),
            },
            KnowledgeError::Duplicate { .. } | KnowledgeError::Revoked { .. } => {
                CapabilityError::Duplicate {
                    message: error.to_string(),
                }
            }
            KnowledgeError::TooLarge { .. } => CapabilityError::Invalid {
                message: error.to_string(),
            },
            KnowledgeError::Storage { .. } => CapabilityError::Unavailable {
                reason: error.to_string(),
            },
            KnowledgeError::InvalidStore { .. } => CapabilityError::Invalid {
                message: error.to_string(),
            },
        }
    }
}

/// The local authoritative knowledge store.
///
/// Cloning is cheap enough to be the publication seam (issue #339): a
/// publication is applied to a clone, committed, and only then published as
/// the live store, so a failed commit cannot leave uncommitted items behind.
#[derive(Debug, Clone, Default)]
pub struct KnowledgeService {
    items: BTreeMap<String, KnowledgeItem>,
    publisher: BTreeMap<String, NodeId>,
    revoked: BTreeSet<String>,
}

impl KnowledgeService {
    /// An empty service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Serialize the live store for a durable host cache.
    pub fn to_json(&self) -> Result<String, KnowledgeError> {
        #[derive(Serialize)]
        struct Snapshot<'a> {
            items: &'a BTreeMap<String, KnowledgeItem>,
            publisher: &'a BTreeMap<String, NodeId>,
            revoked: &'a BTreeSet<String>,
        }
        serde_json::to_string(&Snapshot {
            items: &self.items,
            publisher: &self.publisher,
            revoked: &self.revoked,
        })
        .map_err(|error| KnowledgeError::Invalid {
            message: error.to_string(),
        })
    }

    /// Load a snapshot written by to_json.
    ///
    /// A parse failure is a typed `KnowledgeError::InvalidStore`, distinct
    /// from an invalid request, so a host can fail closed on a corrupt store
    /// instead of treating it as empty (issue #339).
    pub fn from_json(text: &str) -> Result<Self, KnowledgeError> {
        #[derive(Deserialize)]
        struct Snapshot {
            items: BTreeMap<String, KnowledgeItem>,
            publisher: BTreeMap<String, NodeId>,
            revoked: BTreeSet<String>,
        }
        let snapshot: Snapshot =
            serde_json::from_str(text).map_err(|error| KnowledgeError::InvalidStore {
                message: error.to_string(),
            })?;
        Ok(Self {
            items: snapshot.items,
            publisher: snapshot.publisher,
            revoked: snapshot.revoked,
        })
    }

    /// Write the snapshot atomically, creating the parent directory.
    ///
    /// The store can carry governed material, so it is kept owner-only on
    /// every host (issue #299), the same private policy as the peer store.
    pub fn save(&self, path: &std::path::Path) -> Result<(), KnowledgeError> {
        self.save_with_gate(path, &hostkit::durable::NoFaults)
    }

    /// Commit the snapshot through the durable kernel with a fault-injection
    /// gate (issue #339).
    ///
    /// A failure at any commit stage leaves the previous file intact and is
    /// reported as a typed `KnowledgeError::Storage` naming the stage and
    /// whether a retry can succeed. Production callers pass
    /// `hostkit::durable::NoFaults`; tests inject a stage failure.
    pub fn save_with_gate(
        &self,
        path: &std::path::Path,
        gate: &dyn hostkit::durable::CommitGate,
    ) -> Result<(), KnowledgeError> {
        let text = self.to_json()?;
        hostkit::durable::commit(
            path,
            text.as_bytes(),
            hostkit::durable::Privacy::OwnerOnly,
            gate,
        )
        .map_err(|error| KnowledgeError::Storage {
            path: path.display().to_string(),
            detail: error.to_string(),
            retryable: error.is_retryable(),
        })
    }

    /// Every live item, for a local management surface.
    pub fn items(&self) -> impl Iterator<Item = &KnowledgeItem> {
        self.items.values()
    }

    /// Publish items after verifying provenance, freshness, and size.
    pub fn publish(
        &mut self,
        publisher: &NodeId,
        publication: &KnowledgePublication,
        now_ms: u64,
    ) -> Result<usize, KnowledgeError> {
        publication.validate()?;
        let mut seen = BTreeSet::new();
        for item in &publication.items {
            if item.id.trim().is_empty() {
                return Err(KnowledgeError::Invalid {
                    message: "a knowledge item needs an id".to_string(),
                });
            }
            if serde_json::to_vec(&item.payload)
                .map(|bytes| bytes.len() > MAX_KNOWLEDGE_BYTES)
                .unwrap_or(true)
            {
                return Err(KnowledgeError::TooLarge {
                    id: item.id.clone(),
                });
            }
            if item.class == KnowledgeClass::Workspace && item.workspace.is_none() {
                return Err(KnowledgeError::Invalid {
                    message: format!("workspace knowledge {} names no workspace", item.id),
                });
            }
            item.verify_provenance(publisher)?;
            if !item.fresh(now_ms) {
                return Err(KnowledgeError::Stale);
            }
            if self.revoked.contains(&item.id) {
                return Err(KnowledgeError::Revoked {
                    id: item.id.clone(),
                });
            }
            if !seen.insert(item.id.clone()) || self.publisher.contains_key(&item.id) {
                return Err(KnowledgeError::Duplicate {
                    id: item.id.clone(),
                });
            }
        }
        for item in &publication.items {
            self.publisher.insert(item.id.clone(), publisher.clone());
            self.items.insert(item.id.clone(), item.clone());
        }
        Ok(publication.items.len())
    }

    /// Revoke one of the publisher's items.
    pub fn revoke(&mut self, publisher: &NodeId, id: &str) -> Result<bool, KnowledgeError> {
        if !self.items.contains_key(id) {
            return Ok(false);
        }
        if self.publisher.get(id) != Some(publisher) {
            return Err(KnowledgeError::RevokeDenied {
                peer: publisher.clone(),
                id: id.to_string(),
            });
        }
        self.items.remove(id);
        self.publisher.remove(id);
        self.revoked.insert(id.to_string());
        Ok(true)
    }

    /// One stored item, when it is still live.
    pub fn get(&self, id: &str) -> Option<&KnowledgeItem> {
        self.items.get(id)
    }

    /// Whether an item id was revoked, so a replayed publication cannot revive it.
    pub fn is_revoked(&self, id: &str) -> bool {
        self.revoked.contains(id)
    }

    /// Answer a query under an authorization callback.
    ///
    /// The callback receives the peer, requested class, purpose, and scope; a
    /// grant revocation makes it return false, which refuses the whole query.
    /// Expired items, revoked items, and items above the class ceiling are
    /// dropped. At most max_items fresh items are returned.
    pub fn query(
        &self,
        responder: &NodeId,
        requester: &NodeId,
        query: &KnowledgeQuery,
        now_ms: u64,
        authorize: impl Fn(&NodeId, KnowledgeClass, KnowledgePurpose, Option<&WorkspaceId>) -> bool,
        audit: &mut Vec<KnowledgeAudit>,
    ) -> Result<KnowledgeResponse, KnowledgeError> {
        query.validate()?;
        if query.expired(now_ms) {
            audit.push(KnowledgeAudit {
                requester: requester.clone(),
                query_id: query.query_id.clone(),
                purpose: query.purpose,
                returned: 0,
                denied: false,
            });
            return Err(KnowledgeError::Stale);
        }
        if !authorize(
            requester,
            query.max_class,
            query.purpose,
            query.scope.as_ref(),
        ) {
            audit.push(KnowledgeAudit {
                requester: requester.clone(),
                query_id: query.query_id.clone(),
                purpose: query.purpose,
                returned: 0,
                denied: true,
            });
            return Err(KnowledgeError::Unauthorized {
                peer: requester.clone(),
                class: query.max_class,
                purpose: query.purpose,
            });
        }
        let terms: Vec<String> = query.terms.iter().map(|term| term.to_lowercase()).collect();
        let mut matched: Vec<KnowledgeItem> = self
            .items
            .values()
            .filter(|item| item.class.allowed_by(query.max_class))
            // A workspace-bound item answers only a query for that workspace; a
            // query without a scope sees only unscoped items. Never another
            // workspace's knowledge.
            .filter(|item| match (&query.scope, &item.workspace) {
                (Some(scope), Some(owner)) => owner == scope,
                (Some(_), None) | (None, None) => true,
                (None, Some(_)) => false,
            })
            .filter(|item| item.fresh(now_ms))
            .filter(|item| {
                let payload = item.payload.to_string().to_lowercase();
                terms.iter().all(|term| payload.contains(term))
            })
            .cloned()
            .collect();
        matched.sort_by(|left, right| left.id.cmp(&right.id));
        let truncated = matched.len() > query.max_items;
        matched.truncate(query.max_items);
        let response = KnowledgeResponse {
            schema: KNOWLEDGE_SCHEMA,
            query_id: query.query_id.clone(),
            responder: responder.clone(),
            origin: KnowledgeOrigin::Remote {
                node: responder.clone(),
            },
            items: matched,
            truncated,
        };
        audit.push(KnowledgeAudit {
            requester: requester.clone(),
            query_id: query.query_id.clone(),
            purpose: query.purpose,
            returned: response.items.len(),
            denied: false,
        });
        Ok(response)
    }
}

/// Hex SHA-256 of a canonical JSON payload.
pub fn knowledge_hash(payload: &Value) -> String {
    let bytes = serde_json::to_vec(payload).unwrap_or_default();
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::NodeIdentity;

    fn item(node: &NodeId) -> KnowledgeItem {
        let payload = serde_json::json!({ "fact": "sky is blue" });
        KnowledgeItem {
            id: "k-1".to_string(),
            class: KnowledgeClass::Public,
            workspace: None,
            payload: payload.clone(),
            provenance: Provenance {
                origin: node.clone(),
                source: "test".to_string(),
                issued_at_ms: 1,
                expires_at_ms: None,
                content_hash: knowledge_hash(&payload),
            },
            retention_ms: None,
        }
    }

    #[test]
    fn a_store_survives_a_json_round_trip_and_keeps_revocations() {
        let node = NodeIdentity::from_seed([9u8; 32]).node_id();
        let mut service = KnowledgeService::new();
        let duplicate = KnowledgePublication::new(vec![item(&node), item(&node)]);
        assert!(service.publish(&node, &duplicate, 2).is_err());
        let single = KnowledgePublication::new(vec![item(&node)]);
        assert_eq!(service.publish(&node, &single, 2).expect("published"), 1);
        assert!(service.revoke(&node, "k-1").expect("revoked"));

        let json = service.to_json().expect("serialized");
        let restored = KnowledgeService::from_json(&json).expect("restored");
        assert!(restored.is_revoked("k-1"));
        assert!(restored.get("k-1").is_none());
    }

    /// Issue #339: a restart keeps the item, its publisher, workspace scope,
    /// provenance, expiry and the revoked markers, not just the payloads.
    #[test]
    fn a_publication_round_trips_with_publisher_scope_provenance_and_expiry() {
        let node = NodeIdentity::from_seed([17u8; 32]).node_id();
        let stranger = NodeIdentity::from_seed([18u8; 32]).node_id();
        let workspace = WorkspaceId::new("ws:research").expect("a workspace");
        let payload = serde_json::json!({ "fact": "governed" });
        let live = KnowledgeItem {
            id: "k-live".to_string(),
            class: KnowledgeClass::Workspace,
            workspace: Some(workspace.clone()),
            payload: payload.clone(),
            provenance: Provenance {
                origin: node.clone(),
                source: "memory/k-live".to_string(),
                issued_at_ms: 1_000,
                expires_at_ms: Some(9_000_000),
                content_hash: knowledge_hash(&payload),
            },
            retention_ms: Some(60_000),
        };
        let mut service = KnowledgeService::new();
        service
            .publish(&node, &KnowledgePublication::new(vec![live.clone()]), 2_000)
            .expect("published");
        let doomed = item(&node);
        service
            .publish(&node, &KnowledgePublication::new(vec![doomed]), 2_000)
            .expect("published");
        assert!(service.revoke(&node, "k-1").expect("revoked"));

        let json = service.to_json().expect("serialized");
        let mut restored = KnowledgeService::from_json(&json).expect("restored");

        let kept = restored.get("k-live").expect("the item survived");
        assert_eq!(kept, &live, "payload, scope, provenance and expiry survive");
        assert_eq!(kept.workspace.as_ref(), Some(&workspace));
        assert_eq!(kept.provenance.expires_at_ms, Some(9_000_000));
        // The publisher map survived: only the original publisher may revoke.
        assert!(matches!(
            restored.revoke(&stranger, "k-live"),
            Err(KnowledgeError::RevokeDenied { .. })
        ));
        assert!(
            restored
                .revoke(&node, "k-live")
                .expect("the publisher revokes")
        );
        // The revoked marker survived and still refuses a republication.
        assert!(restored.is_revoked("k-1"));
        assert!(restored.get("k-1").is_none());
        assert!(matches!(
            restored.publish(&node, &KnowledgePublication::new(vec![item(&node)]), 3_000),
            Err(KnowledgeError::Revoked { .. })
        ));
    }

    /// Issue #339: a corrupt snapshot is a distinct error a host can fail
    /// closed on, never an invalid request and never an empty store.
    #[test]
    fn a_corrupt_snapshot_is_a_distinct_invalid_store_error() {
        let error = KnowledgeService::from_json("{not json").expect_err("corrupt");
        assert!(
            matches!(error, KnowledgeError::InvalidStore { .. }),
            "{error:?}"
        );
        let wrong_shape = KnowledgeService::from_json(r#"{"items":{}}"#).expect_err("shape");
        assert!(
            matches!(wrong_shape, KnowledgeError::InvalidStore { .. }),
            "{wrong_shape:?}"
        );
    }

    /// A gate that fails exactly one commit stage (issue #339).
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

    /// Issue #339: an injected failure at every commit stage leaves the
    /// previous snapshot intact and reports a typed, retryable error.
    #[test]
    fn a_failed_snapshot_commit_keeps_the_previous_file() {
        let node = NodeIdentity::from_seed([19u8; 32]).node_id();
        for stage in [
            hostkit::durable::IoStage::CreateTemp,
            hostkit::durable::IoStage::Write,
            hostkit::durable::IoStage::Sync,
            hostkit::durable::IoStage::Replace,
        ] {
            let dir = std::env::temp_dir().join(format!(
                "federation-knowledge-fault-{}-{stage}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("knowledge.json");

            let mut before = KnowledgeService::new();
            before
                .publish(&node, &KnowledgePublication::new(vec![item(&node)]), 2)
                .expect("published");
            before.save(&path).expect("the pre-state is committed");

            let mut candidate = before.clone();
            let second = KnowledgeItem {
                id: "k-2".to_string(),
                ..item(&node)
            };
            candidate
                .publish(&node, &KnowledgePublication::new(vec![second]), 3)
                .expect("published on the clone");
            let error = candidate
                .save_with_gate(&path, &FaultAt(stage))
                .expect_err("the injected stage must fail");
            match &error {
                KnowledgeError::Storage {
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

            let text = std::fs::read_to_string(&path).expect("the previous file is intact");
            let on_disk = KnowledgeService::from_json(&text).expect("still a snapshot");
            assert!(on_disk.get("k-1").is_some());
            assert!(
                on_disk.get("k-2").is_none(),
                "stage {stage} leaked the uncommitted item"
            );

            candidate
                .save_with_gate(&path, &hostkit::durable::NoFaults)
                .expect("the retry commits once the fault is gone");
            let text = std::fs::read_to_string(&path).expect("read after retry");
            let recovered = KnowledgeService::from_json(&text).expect("snapshot");
            assert!(recovered.get("k-2").is_some());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
