//! Delegation: a bounded task request, its durable state, and attenuation.
//!
//! Delegation transfers a task, never control of a Node (RFC section 13). The
//! store keeps the authoritative state; every transition is validated, so a
//! completion cannot overwrite a cancellation and a finished task cannot be
//! cancelled again.
//!
//! Sub-delegation is disabled by default. When a delegation allows it, the
//! child may only carry a subset of the parent's capabilities, target the same
//! Node, and end no later than the parent: forwarding a task must never
//! manufacture authority.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use capability::{CapabilityId, NodeId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A delegation's stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DelegationId(String);

impl DelegationId {
    /// Build a delegation id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DelegationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Where a delegated task stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationState {
    /// Recorded, not yet answered by the target.
    Created,
    /// The target agreed to perform it.
    Accepted,
    /// Work is in progress.
    Running,
    /// Finished with a result.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Cancelled before completion.
    Cancelled,
    /// Refused by the target.
    Rejected,
}

impl DelegationState {
    /// Whether no further transition is possible.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Rejected
        )
    }

    /// Whether work may still be in progress.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Accepted | Self::Running)
    }
}

/// One bounded task request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegationRequest {
    /// Stable id.
    pub id: DelegationId,
    /// The delegation this one was derived from, when it is a sub-delegation.
    pub parent: Option<DelegationId>,
    /// The Node asked to perform it.
    pub target: NodeId,
    /// What the target is asked to achieve.
    pub objective: String,
    /// The capabilities the target may use.
    pub capabilities: Vec<CapabilityId>,
    /// Operations the delegation may use; empty means any.
    #[serde(default)]
    pub operations: Vec<String>,
    /// Resource selectors the delegation may use; empty means any.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Absolute deadline in milliseconds since the Unix epoch.
    pub deadline_ms: Option<u64>,
    /// Whether the target may delegate further. False by default.
    pub allow_sub_delegation: bool,
    /// What a result must satisfy, when the caller states it.
    pub result_contract: Option<String>,
    /// The trace this delegation belongs to, when the caller supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// The caller's span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
}

/// The authoritative record of one delegation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegationRecord {
    /// The authenticated peer that submitted the request.
    ///
    /// Absent on records written before ownership was recorded; those are
    /// owner-only operations nobody can prove, so status and cancel refuse
    /// them rather than guessing.
    #[serde(default)]
    pub origin: Option<NodeId>,
    /// The request as accepted into the store.
    pub request: DelegationRequest,
    /// Current state.
    pub state: DelegationState,
    /// When the state last changed.
    pub updated_at_ms: u64,
    /// The result, once succeeded.
    pub result: Option<Value>,
    /// The failure reason, once failed or rejected.
    pub error: Option<String>,
}

/// Why a delegation operation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegationError {
    /// No delegation with that id.
    #[error("delegation {id} is unknown")]
    Unknown {
        /// The id asked for.
        id: DelegationId,
    },
    /// The transition is not allowed from the current state.
    #[error("delegation {id} cannot go from {from:?} to {to:?}")]
    InvalidTransition {
        /// The id.
        id: DelegationId,
        /// Current state.
        from: DelegationState,
        /// Requested state.
        to: DelegationState,
    },
    /// The request itself is malformed.
    #[error("invalid delegation: {message}")]
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// The parent forbids further delegation.
    #[error("delegation {id} does not allow sub-delegation")]
    SubDelegationDisabled {
        /// The parent.
        id: DelegationId,
    },
    /// A child tried to use authority the parent does not hold.
    #[error("delegation would widen authority with {capability}")]
    NotAttenuated {
        /// The capability that is not in the parent's scope.
        capability: CapabilityId,
    },
    /// A child tried to target a Node the parent was not granted.
    #[error("delegation would forward to {node}, which the parent was not granted")]
    RecipientNotAllowed {
        /// The Node.
        node: NodeId,
    },
    /// A child's deadline outlives the parent's.
    #[error("delegation deadline exceeds the parent's")]
    DeadlineExceedsParent,
    /// The request's deadline has already passed.
    #[error("delegation deadline has already expired")]
    DeadlineExpired,
    /// A child's operation or resource scope widens the parent's.
    #[error("delegation scope exceeds the parent's")]
    ScopeExceedsParent,
}

/// The authoritative delegation records.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DelegationStore {
    records: BTreeMap<DelegationId, DelegationRecord>,
    /// A monotonic count of successful mutations, so a writer can tell whether
    /// its file is stale without comparing every record.
    #[serde(default)]
    revision: u64,
}

impl DelegationStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a new top-level delegation.
    pub fn create(
        &mut self,
        request: DelegationRequest,
        now_ms: u64,
    ) -> Result<DelegationId, DelegationError> {
        self.create_for(request, None, now_ms)
    }

    /// Record a new delegation and its authenticated submitter.
    pub fn create_for(
        &mut self,
        request: DelegationRequest,
        origin: Option<NodeId>,
        now_ms: u64,
    ) -> Result<DelegationId, DelegationError> {
        if request.objective.trim().is_empty() {
            return Err(DelegationError::Invalid {
                message: "a delegation needs an objective".to_string(),
            });
        }
        if self.records.contains_key(&request.id) {
            return Err(DelegationError::Invalid {
                message: format!("delegation {} already exists", request.id),
            });
        }
        // An expired bound is not a bound: refuse it at acceptance rather than
        // letting the execution path treat it as "no deadline".
        if let Some(deadline) = request.deadline_ms {
            if deadline <= now_ms {
                return Err(DelegationError::DeadlineExpired);
            }
        }
        let id = request.id.clone();
        self.records.insert(
            id.clone(),
            DelegationRecord {
                origin,
                request,
                state: DelegationState::Created,
                updated_at_ms: now_ms,
                result: None,
                error: None,
            },
        );
        self.revision += 1;
        Ok(id)
    }

    /// Record a sub-delegation under a parent, enforcing attenuation.
    pub fn delegate(
        &mut self,
        parent: &DelegationId,
        request: DelegationRequest,
        now_ms: u64,
    ) -> Result<DelegationId, DelegationError> {
        self.delegate_for(parent, request, None, now_ms)
    }

    /// Record a sub-delegation, inheriting the parent's owner so the chain
    /// cannot change hands halfway down.
    pub fn delegate_for(
        &mut self,
        parent: &DelegationId,
        request: DelegationRequest,
        origin: Option<NodeId>,
        now_ms: u64,
    ) -> Result<DelegationId, DelegationError> {
        let parent_record = self
            .records
            .get(parent)
            .ok_or_else(|| DelegationError::Unknown { id: parent.clone() })?;
        if !parent_record.state.is_active() {
            return Err(DelegationError::InvalidTransition {
                id: parent.clone(),
                from: parent_record.state,
                to: DelegationState::Created,
            });
        }
        if !parent_record.request.allow_sub_delegation {
            return Err(DelegationError::SubDelegationDisabled { id: parent.clone() });
        }
        if request.target != parent_record.request.target {
            return Err(DelegationError::RecipientNotAllowed {
                node: request.target.clone(),
            });
        }
        let allowed: BTreeSet<&CapabilityId> = parent_record.request.capabilities.iter().collect();
        for capability in &request.capabilities {
            if !allowed.contains(capability) {
                return Err(DelegationError::NotAttenuated {
                    capability: capability.clone(),
                });
            }
        }
        // Scope attenuates exactly like the capability list: a bounded parent
        // does not permit an unbounded child, and a child selector must be
        // covered by one of the parent's selectors.
        if !parent_record.request.operations.is_empty()
            && (request.operations.is_empty()
                || !request.operations.iter().all(|child| {
                    parent_record
                        .request
                        .operations
                        .iter()
                        .any(|parent| selector_covers(parent, child))
                }))
        {
            return Err(DelegationError::ScopeExceedsParent);
        }
        if !parent_record.request.resources.is_empty()
            && (request.resources.is_empty()
                || !request.resources.iter().all(|child| {
                    parent_record
                        .request
                        .resources
                        .iter()
                        .any(|parent| selector_covers(parent, child))
                }))
        {
            return Err(DelegationError::ScopeExceedsParent);
        }
        // A child may not widen the parent's temporal bound: a later deadline
        // is refused, and so is omitting the deadline entirely when the parent
        // has one. Only a parent without a deadline leaves the child free.
        if let Some(parent_deadline) = parent_record.request.deadline_ms {
            match request.deadline_ms {
                Some(child) if child > parent_deadline => {
                    return Err(DelegationError::DeadlineExceedsParent);
                }
                None => return Err(DelegationError::DeadlineExceedsParent),
                Some(_) => {}
            }
        }
        let mut request = request;
        request.parent = Some(parent.clone());
        if request.allow_sub_delegation && !parent_record.request.allow_sub_delegation {
            request.allow_sub_delegation = false;
        }
        // A sub-delegation stays in the caller's trace unless it names its own.
        if request.trace_id.is_none() {
            request.trace_id = parent_record.request.trace_id.clone();
        }
        if request.span_id.is_none() {
            request.span_id = parent_record.request.span_id.clone();
        }
        let origin = parent_record.origin.clone().or(origin);
        self.create_for(request, origin, now_ms)
    }

    /// Cancel every active delegation submitted by one origin.
    ///
    /// A revoked peer must not keep work running that it started; the records
    /// stay for the audit trail, in the cancelled state.
    pub fn cancel_from(&mut self, origin: &NodeId, now_ms: u64) -> usize {
        let ids: Vec<DelegationId> = self
            .records
            .values()
            .filter(|record| record.origin.as_ref() == Some(origin) && record.state.is_active())
            .map(|record| record.request.id.clone())
            .collect();
        let mut cancelled = 0;
        for id in ids {
            if self
                .transition(&id, DelegationState::Cancelled, now_ms)
                .is_ok()
            {
                cancelled += 1;
            }
        }
        cancelled
    }

    /// Move one delegation to a new state.
    pub fn transition(
        &mut self,
        id: &DelegationId,
        to: DelegationState,
        now_ms: u64,
    ) -> Result<(), DelegationError> {
        let record = self
            .records
            .get_mut(id)
            .ok_or_else(|| DelegationError::Unknown { id: id.clone() })?;
        if !allowed(record.state, to) {
            return Err(DelegationError::InvalidTransition {
                id: id.clone(),
                from: record.state,
                to,
            });
        }
        record.state = to;
        record.updated_at_ms = now_ms;
        self.revision += 1;
        Ok(())
    }

    /// Record a successful result.
    pub fn succeed(
        &mut self,
        id: &DelegationId,
        result: Value,
        now_ms: u64,
    ) -> Result<(), DelegationError> {
        self.transition(id, DelegationState::Succeeded, now_ms)?;
        if let Some(record) = self.records.get_mut(id) {
            record.result = Some(result);
        }
        Ok(())
    }

    /// Record a failure or a refusal.
    pub fn fail(
        &mut self,
        id: &DelegationId,
        reason: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), DelegationError> {
        self.transition(id, DelegationState::Failed, now_ms)?;
        if let Some(record) = self.records.get_mut(id) {
            record.error = Some(reason.into());
        }
        Ok(())
    }

    /// Refuse a delegation the target will not perform.
    pub fn reject(
        &mut self,
        id: &DelegationId,
        reason: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), DelegationError> {
        self.transition(id, DelegationState::Rejected, now_ms)?;
        if let Some(record) = self.records.get_mut(id) {
            record.error = Some(reason.into());
        }
        Ok(())
    }

    /// One record.
    pub fn record(&self, id: &DelegationId) -> Option<&DelegationRecord> {
        self.records.get(id)
    }

    /// Every record.
    pub fn iter(&self) -> impl Iterator<Item = &DelegationRecord> {
        self.records.values()
    }

    /// The provenance chain from the root to this delegation.
    pub fn chain(&self, id: &DelegationId) -> Vec<&DelegationRecord> {
        let mut chain = Vec::new();
        let mut cursor = self.records.get(id);
        let mut depth = 0;
        while let Some(record) = cursor {
            chain.push(record);
            depth += 1;
            if depth > 16 {
                break;
            }
            cursor = record
                .request
                .parent
                .as_ref()
                .and_then(|parent| self.records.get(parent));
        }
        chain.reverse();
        chain
    }

    /// The mutation counter, for a writer deciding whether its file is stale.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Remove every terminal record; active records stay.
    pub fn clear_finished(&mut self) -> usize {
        let before = self.records.len();
        self.records.retain(|_, record| !record.state.is_terminal());
        let removed = before.saturating_sub(self.records.len());
        if removed > 0 {
            self.revision += 1;
        }
        removed
    }

    /// Remove one record by id; returns whether it existed.
    pub fn clear_one(&mut self, id: &DelegationId) -> bool {
        let removed = self.records.remove(id).is_some();
        if removed {
            self.revision += 1;
        }
        removed
    }

    /// Remove every record, including active ones.
    pub fn clear_all(&mut self) -> usize {
        let removed = self.records.len();
        self.records.clear();
        if removed > 0 {
            self.revision += 1;
        }
        removed
    }

    /// Read a store from its persistence file; a missing file is empty.
    ///
    /// Recovery is explicit: a delegation that was still Created, Accepted, or
    /// Running when the process stopped is marked Failed with an indeterminate-
    /// outcome reason. The rewritten file is saved before the store is returned.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let mut store = Self::load_read_only(path)?;
        if store.recover(capability::unix_ms()) > 0 {
            store.save(path)?;
        }
        Ok(store)
    }

    /// Read a store without the restart recovery pass.
    ///
    /// Read-only commands must use this: loading a shared file must not turn a
    /// live Accepted/Running delegation into a Failed one behind the owning
    /// process.
    pub fn load_read_only(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_json(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(error) => Err(error),
        }
    }

    /// Mark every non-terminal delegation as failed after a restart.
    ///
    /// Returns how many records were changed. The outcome is deliberately
    /// indeterminate rather than guessed: a completed call whose result was
    /// never recorded must not be reported as success.
    pub fn recover(&mut self, now_ms: u64) -> usize {
        let mut changed = 0;
        for record in self.records.values_mut() {
            if record.state.is_terminal() {
                continue;
            }
            record.state = DelegationState::Failed;
            record.error =
                Some("interrupted by a Node restart; the outcome is indeterminate".to_string());
            record.updated_at_ms = now_ms;
            changed += 1;
        }
        if changed > 0 {
            self.revision += 1;
        }
        changed
    }

    /// Parse a store, refusing a file that is not a store.
    pub fn from_json(text: &str) -> std::io::Result<Self> {
        serde_json::from_str(text)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Serialize the store for its persistence file.
    pub fn to_json(&self) -> std::io::Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    /// Write the store atomically, private to the account.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, self.to_json()?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path)
    }
}

fn allowed(from: DelegationState, to: DelegationState) -> bool {
    matches!(
        (from, to),
        (DelegationState::Created, DelegationState::Accepted)
            | (DelegationState::Created, DelegationState::Rejected)
            | (DelegationState::Created, DelegationState::Cancelled)
            | (DelegationState::Accepted, DelegationState::Running)
            | (DelegationState::Accepted, DelegationState::Failed)
            | (DelegationState::Accepted, DelegationState::Cancelled)
            | (DelegationState::Running, DelegationState::Succeeded)
            | (DelegationState::Running, DelegationState::Failed)
            | (DelegationState::Running, DelegationState::Cancelled)
    )
}

/// Whether a parent selector covers a child selector.
///
/// Both are exact strings or prefixes ending in '*'. The bare "*" covers
/// everything; a child wildcard is only covered by a parent wildcard whose
/// prefix contains it.
fn selector_covers(parent: &str, child: &str) -> bool {
    if parent == "*" {
        return true;
    }
    match child.strip_suffix('*') {
        Some(child_prefix) => parent
            .strip_suffix('*')
            .is_some_and(|parent_prefix| child_prefix.starts_with(parent_prefix)),
        None => {
            parent == child
                || parent
                    .strip_suffix('*')
                    .is_some_and(|prefix| child.starts_with(prefix))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use capability::NodeId;

    fn request(id: &str, target: &NodeId, deadline: Option<u64>) -> DelegationRequest {
        DelegationRequest {
            id: DelegationId::new(id),
            parent: None,
            target: target.clone(),
            objective: "audit".to_string(),
            capabilities: Vec::new(),
            operations: Vec::new(),
            resources: Vec::new(),
            deadline_ms: deadline,
            allow_sub_delegation: true,
            result_contract: None,
            trace_id: None,
            span_id: None,
        }
    }

    /// An already-expired bound is refused at acceptance.
    #[test]
    fn an_expired_delegation_is_refused() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let error = store
            .create(request("d-1", &target, Some(1_000)), 1_000)
            .expect_err("expired");
        assert!(matches!(error, DelegationError::DeadlineExpired));
    }

    /// A child may not drop or extend the parent's deadline.
    #[test]
    fn a_child_cannot_widen_the_parent_deadline() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let parent = store
            .create_for(request("d-parent", &target, Some(10_000)), None, 1_000)
            .expect("parent");
        store
            .transition(&parent, DelegationState::Accepted, 1_000)
            .expect("accepted");

        let error = store
            .delegate_for(&parent, request("d-child-none", &target, None), None, 2_000)
            .expect_err("missing deadline");
        assert!(
            matches!(error, DelegationError::DeadlineExceedsParent),
            "got {error:?}"
        );

        let error = store
            .delegate_for(
                &parent,
                request("d-child-late", &target, Some(20_000)),
                None,
                2_000,
            )
            .expect_err("later deadline");
        assert!(matches!(error, DelegationError::DeadlineExceedsParent));

        let exact = store
            .delegate_for(
                &parent,
                request("d-child-exact", &target, Some(10_000)),
                None,
                2_000,
            )
            .expect("equal deadline");
        let earlier = store
            .delegate_for(
                &parent,
                request("d-child-early", &target, Some(5_000)),
                None,
                2_000,
            )
            .expect("earlier deadline");
        assert!(store.record(&exact).is_some());
        assert!(store.record(&earlier).is_some());
        store
            .transition(&exact, DelegationState::Accepted, 2_000)
            .expect("accepted child");

        // A grandchild inherits through the child: still no unbounded deadline.
        let error = store
            .delegate_for(&exact, request("d-grandchild", &target, None), None, 3_000)
            .expect_err("grandchild missing");
        assert!(matches!(error, DelegationError::DeadlineExceedsParent));
    }

    /// A child may not widen the parent's operation or resource scope.
    #[test]
    fn a_child_cannot_widen_the_parent_scope() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let mut parent_request = request("d-parent", &target, None);
        parent_request.operations = vec!["read".to_string()];
        parent_request.resources = vec!["/ws/*".to_string()];
        let parent = store
            .create_for(parent_request, None, 1_000)
            .expect("parent");
        store
            .transition(&parent, DelegationState::Accepted, 1_000)
            .expect("accepted");

        let unbounded = request("d-child-none", &target, None);
        assert!(matches!(
            store
                .delegate_for(&parent, unbounded, None, 2_000)
                .expect_err("unbounded child"),
            DelegationError::ScopeExceedsParent
        ));

        let mut wider = request("d-child-wide", &target, None);
        wider.operations = vec!["write".to_string()];
        wider.resources = vec!["/ws/*".to_string()];
        assert!(matches!(
            store
                .delegate_for(&parent, wider, None, 2_000)
                .expect_err("wider operation"),
            DelegationError::ScopeExceedsParent
        ));

        let mut narrower = request("d-child-ok", &target, None);
        narrower.operations = vec!["read".to_string()];
        narrower.resources = vec!["/ws/notes/*".to_string()];
        assert!(store.delegate_for(&parent, narrower, None, 2_000).is_ok());
    }

    /// Revoking an origin cancels the delegations it submitted, and only those.
    #[test]
    fn revoking_an_origin_cancels_its_active_delegations() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let origin = NodeId::new("node-owner").expect("node id");
        let other = NodeId::new("node-other").expect("node id");
        let mine = store
            .create_for(
                request("d-mine", &target, None),
                Some(origin.clone()),
                1_000,
            )
            .expect("mine");
        let theirs = store
            .create_for(
                request("d-theirs", &target, None),
                Some(other.clone()),
                1_000,
            )
            .expect("theirs");
        store
            .transition(&mine, DelegationState::Accepted, 1_000)
            .expect("accepted");
        store
            .transition(&theirs, DelegationState::Accepted, 1_000)
            .expect("accepted");

        assert_eq!(store.cancel_from(&origin, 2_000), 1);
        assert_eq!(
            store.record(&mine).expect("mine").state,
            DelegationState::Cancelled
        );
        assert_eq!(
            store.record(&theirs).expect("theirs").state,
            DelegationState::Accepted
        );
    }

    /// A child joins the parent's trace unless it names its own.
    #[test]
    fn a_sub_delegation_inherits_the_parent_trace() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let mut parent_request = request("d-parent", &target, None);
        parent_request.trace_id = Some("trace-1".to_string());
        parent_request.span_id = Some("span-parent".to_string());
        let parent = store.create(parent_request, 1_000).expect("parent");
        store
            .transition(&parent, DelegationState::Accepted, 1_000)
            .expect("accepted");

        let inherited = store
            .delegate_for(&parent, request("d-child", &target, None), None, 1_000)
            .expect("child");
        let record = store.record(&inherited).expect("record");
        assert_eq!(record.request.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(record.request.span_id.as_deref(), Some("span-parent"));

        let mut own = request("d-own", &target, None);
        own.trace_id = Some("trace-2".to_string());
        let explicit = store
            .delegate_for(&parent, own, None, 1_000)
            .expect("own trace");
        assert_eq!(
            store
                .record(&explicit)
                .expect("record")
                .request
                .trace_id
                .as_deref(),
            Some("trace-2")
        );
    }

    #[test]
    fn clear_one_removes_one_and_clear_all_removes_every_record() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let done = store
            .create(request("done", &target, Some(10_000)), 1_000)
            .expect("done");
        store
            .transition(&done, DelegationState::Accepted, 1_000)
            .expect("accepted");
        store
            .transition(&done, DelegationState::Running, 1_000)
            .expect("running");
        store
            .transition(&done, DelegationState::Succeeded, 1_000)
            .expect("succeeded");
        let live = store
            .create(request("live", &target, Some(10_000)), 1_000)
            .expect("live");
        store
            .transition(&live, DelegationState::Accepted, 1_000)
            .expect("accepted");

        assert_eq!(store.clear_finished(), 1);
        assert!(store.record(&live).is_some());
        assert!(store.clear_one(&live), "clear one removes the live record");
        assert!(store.iter().next().is_none());
        let again = store
            .create(request("again", &target, Some(10_000)), 1_000)
            .expect("again");
        assert_eq!(store.clear_all(), 1);
        assert!(store.record(&again).is_none());
    }

    #[test]
    fn read_only_load_does_not_recover_live_delegations() {
        let mut store = DelegationStore::new();
        let target = NodeId::new("node-a").expect("node id");
        let id = store
            .create(request("live", &target, Some(10_000)), 1_000)
            .expect("created");
        store
            .transition(&id, DelegationState::Accepted, 1_000)
            .expect("accepted");
        let path = std::env::temp_dir().join(format!(
            "bos-delegation-readonly-{}.json",
            std::process::id()
        ));
        store.save(&path).expect("saved");

        let read_only = DelegationStore::load_read_only(&path).expect("read only");
        assert_eq!(
            read_only.record(&id).expect("record").state,
            DelegationState::Accepted,
            "a status reader must not settle a live delegation"
        );
        let recovered = DelegationStore::load(&path).expect("recovered");
        assert_eq!(
            recovered.record(&id).expect("record").state,
            DelegationState::Failed,
            "the owning load still performs recovery"
        );
        let _ = std::fs::remove_file(path);
    }
}
