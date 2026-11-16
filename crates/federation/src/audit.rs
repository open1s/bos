//! Federation audit records.
//!
//! One record per cross-node operation, on each side that made a decision.
//! A record carries who, what operation, which capability, and the outcome —
//! never the arguments, the result, or a secret. The caller's record and the
//! provider's record are different rows, because the two decisions are
//! different decisions.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// Which side of the boundary the record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// This node sent the operation.
    Outbound,
    /// This node received and answered the operation.
    Inbound,
}

/// One auditable federation event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationEvent {
    /// When it happened, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Which side this record is.
    pub direction: Direction,
    /// The other Node.
    pub peer: String,
    /// The operation: invoke, delegate, delegation_status, delegation_cancel,
    /// discover, or resource.
    pub operation: String,
    /// The capability involved, when the operation names one.
    pub capability: Option<String>,
    /// The delegation, lease, or identity pair a coordination or data
    /// operation acts on; a rotation names it as "old -> new" (issue #338).
    #[serde(default)]
    pub subject: Option<String>,
    /// The outcome class: ok, denied, invalid, not_found, unavailable,
    /// storage_failed, provider_failed, transport, unknown_outcome.
    pub outcome: String,
    /// Why it was refused or failed, when there is something to say.
    pub reason: Option<String>,
    /// How long it took, on the side that waited.
    pub elapsed_ms: Option<u64>,
    /// Bytes moved, for a resource event.
    #[serde(default)]
    pub bytes: Option<u64>,
    /// The distributed trace this operation belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// The span this operation ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// The task this operation serves, when it serves one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The delegation this operation acts under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<String>,
    /// The ORIGINAL principal, which is not the immediate peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
}

impl FederationEvent {
    /// Build one event.
    pub fn new(
        direction: Direction,
        peer: &str,
        operation: &str,
        capability: Option<&str>,
        outcome: &str,
    ) -> Self {
        Self {
            at_ms: capability::unix_ms(),
            direction,
            peer: peer.to_string(),
            operation: operation.to_string(),
            capability: capability.map(str::to_string),
            subject: None,
            outcome: outcome.to_string(),
            reason: None,
            elapsed_ms: None,
            bytes: None,
            trace_id: None,
            span_id: None,
            task: None,
            delegation: None,
            principal: None,
        }
    }

    /// Attach the trace correlation.
    pub fn with_trace(mut self, trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self.span_id = Some(span_id.into());
        self
    }

    /// Attach the task this operation serves.
    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    /// Attach the delegation this operation acts under.
    pub fn with_delegation(mut self, delegation: impl Into<String>) -> Self {
        self.delegation = Some(delegation.into());
        self
    }

    /// Attach the original principal (not the immediate peer).
    pub fn with_principal(mut self, principal: impl Into<String>) -> Self {
        self.principal = Some(principal.into());
        self
    }

    /// Attach a reason.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Name the delegation or lease this event is about.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }

    /// Name the lease and the stream attempt a resource event is about.
    ///
    /// The subject becomes [`stream_subject`]. A stream open is a distinct
    /// generation even when it reuses a lease id, so the caller's cancellation
    /// record and the provider's terminal record for one stream correlate on
    /// this value, and an older attempt never reads as the live one.
    pub fn with_stream_subject(mut self, lease: impl std::fmt::Display, attempt: u64) -> Self {
        self.subject = Some(stream_subject(lease, attempt));
        self
    }

    /// Attach a byte count.
    pub fn with_bytes(mut self, bytes: u64) -> Self {
        self.bytes = Some(bytes);
        self
    }

    /// Attach a duration.
    pub fn with_elapsed_ms(mut self, elapsed_ms: u64) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }
}

/// The correlated audit subject of one resource stream: `<lease>#attempt=<n>`.
///
/// The same value is used by the caller's resource records and the provider's,
/// so the two sides of one stream attempt can be joined.
pub fn stream_subject(lease: impl std::fmt::Display, attempt: u64) -> String {
    format!("{lease}#attempt={attempt}")
}

/// How a dropped resource stream's cancellation is accounted for.
///
/// Dropping a caller-side stream sends a best-effort `Cancel` for its exact
/// (lease, attempt). The provider does not acknowledge a cancel, so the
/// strongest fact available when the drop settles is whether the peer's
/// transport accepted the signed frame:
///
/// - [CancelConfirmation::Confirmed] means the cancel frame left this node.
///   The provider's own terminal record carries the same `lease#attempt=N`
///   subject, so the two records can be tied together.
/// - [CancelConfirmation::Unconfirmed] means the frame was not accepted (no
///   runtime, an unreachable peer, or a send that did not complete at drop
///   time), so the provider's outcome is unknown and is recorded as such. It is
///   never recorded as "the producer stopped".
///
/// The choice is deterministic: it is derived from the transport's acceptance
/// of the frame, not from timing or from a later inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelConfirmation {
    /// The cancel frame was accepted by the peer's transport.
    Confirmed,
    /// The cancel frame was not accepted; the provider's outcome is unknown.
    Unconfirmed,
}

impl CancelConfirmation {
    /// The audit outcome class: `cancelled` or `cancel_unconfirmed`.
    pub fn outcome(self) -> &'static str {
        match self {
            Self::Confirmed => "cancelled",
            Self::Unconfirmed => "cancel_unconfirmed",
        }
    }

    /// Whether the peer accepted the cancel frame.
    pub fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed)
    }

    /// A sentence for the audit reason field.
    pub fn detail(self) -> &'static str {
        match self {
            Self::Confirmed => "the peer's transport accepted the cancel frame",
            Self::Unconfirmed => {
                "the cancel frame was not accepted; the provider's outcome is unknown"
            }
        }
    }
}

/// Where federation records go. The host decides: a file, a log, or a sink
/// that drops them.
pub trait FederationAudit: Send + Sync {
    /// Record one event.
    fn record(&self, event: FederationEvent);
}

/// An in-memory sink, for tests and for a host that only wants a tail.
#[derive(Debug, Default)]
pub struct MemoryAudit {
    events: Mutex<Vec<FederationEvent>>,
}

impl MemoryAudit {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// The events recorded so far, in order.
    pub fn events(&self) -> Vec<FederationEvent> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl FederationAudit for MemoryAudit {
    fn record(&self, event: FederationEvent) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
    }
}

impl FederationAudit for Arc<MemoryAudit> {
    fn record(&self, event: FederationEvent) {
        (**self).record(event);
    }
}
