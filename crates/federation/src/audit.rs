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
    /// The delegation or lease a coordination or data operation acts on.
    #[serde(default)]
    pub subject: Option<String>,
    /// The outcome class: ok, denied, invalid, not_found, unavailable,
    /// provider_failed, transport, unknown_outcome.
    pub outcome: String,
    /// Why it was refused or failed, when there is something to say.
    pub reason: Option<String>,
    /// How long it took, on the side that waited.
    pub elapsed_ms: Option<u64>,
    /// Bytes moved, for a resource event.
    #[serde(default)]
    pub bytes: Option<u64>,
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
        }
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
