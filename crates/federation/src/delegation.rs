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
    /// Absolute deadline in milliseconds since the Unix epoch.
    pub deadline_ms: Option<u64>,
    /// Whether the target may delegate further. False by default.
    pub allow_sub_delegation: bool,
    /// What a result must satisfy, when the caller states it.
    pub result_contract: Option<String>,
}

/// The authoritative record of one delegation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegationRecord {
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
}

/// The authoritative delegation records.
#[derive(Debug, Clone, Default)]
pub struct DelegationStore {
    records: BTreeMap<DelegationId, DelegationRecord>,
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
        let id = request.id.clone();
        self.records.insert(
            id.clone(),
            DelegationRecord {
                request,
                state: DelegationState::Created,
                updated_at_ms: now_ms,
                result: None,
                error: None,
            },
        );
        Ok(id)
    }

    /// Record a sub-delegation under a parent, enforcing attenuation.
    pub fn delegate(
        &mut self,
        parent: &DelegationId,
        request: DelegationRequest,
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
        if let (Some(child), Some(parent_deadline)) =
            (request.deadline_ms, parent_record.request.deadline_ms)
        {
            if child > parent_deadline {
                return Err(DelegationError::DeadlineExceedsParent);
            }
        }
        let mut request = request;
        request.parent = Some(parent.clone());
        if request.allow_sub_delegation && !parent_record.request.allow_sub_delegation {
            request.allow_sub_delegation = false;
        }
        self.create(request, now_ms)
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
