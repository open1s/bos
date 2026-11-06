//! The capability error taxonomy.
//!
//! A denial, a provider fault, and a transport failure are different facts and
//! stay different here. Collapsing them into one "failed" string is what makes
//! retry safety undecidable later.

use crate::context::PolicyDecisionRef;
use crate::id::CapabilityId;
use serde::{Deserialize, Serialize};

/// Whether a message left this Node.
///
/// The distinction decides whether a retry is safe. A request that was never
/// sent may be retried; one that was sent and not answered has an unknown
/// outcome and must not be replayed blindly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// The message never left this Node.
    NotSent,
    /// The message left this Node; no answer was received.
    Sent,
}

/// One classified capability failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CapabilityError {
    /// The request contradicted the declared contract.
    #[error("invalid capability request: {message}")]
    Invalid {
        /// What was wrong.
        message: String,
    },
    /// A policy refused the call.
    #[error("capability denied: {reason}")]
    Denied {
        /// Why it was refused.
        reason: String,
        /// The decision that refused it, when the kernel records one.
        policy: Option<PolicyDecisionRef>,
    },
    /// The capability does not exist, or was revoked.
    #[error("capability not found: {capability}")]
    NotFound {
        /// The capability that was asked for.
        capability: CapabilityId,
    },
    /// A delegated task does not exist, or was never created.
    #[error("delegation not found: {delegation}")]
    DelegationNotFound {
        /// The delegation that was asked for.
        delegation: String,
    },
    /// The provider cannot serve it right now.
    #[error("capability unavailable: {reason}")]
    Unavailable {
        /// Why it is unavailable.
        reason: String,
    },
    /// A quota or rate limit refused it.
    #[error("capability rate limited")]
    RateLimited {
        /// How long the caller should wait, when the provider says.
        retry_after_ms: Option<u64>,
    },
    /// The deadline expired before the call finished.
    #[error("capability timed out after {elapsed_ms} ms")]
    Timeout {
        /// How long the call ran.
        elapsed_ms: u64,
        /// Whatever was produced before the deadline.
        partial: Option<serde_json::Value>,
    },
    /// The call was cancelled.
    #[error("capability cancelled")]
    Cancelled,
    /// The provider ran and failed.
    #[error("capability provider failed: {message}")]
    ProviderFailed {
        /// The provider's account of the failure.
        message: String,
    },
    /// The transport failed.
    #[error("capability transport failed ({delivered:?}): {message}")]
    Transport {
        /// The transport's account of the failure.
        message: String,
        /// Whether the message left this Node.
        delivered: DeliveryState,
    },
    /// The request was delivered and its outcome is unknown.
    #[error("capability outcome unknown: {message}")]
    UnknownOutcome {
        /// What is known about the failure.
        message: String,
    },
}
