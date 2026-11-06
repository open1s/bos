//! What one invocation is allowed to know about why it is running.
//!
//! The context is runtime state, not wire state: the deadline is an absolute
//! instant and a cancellation token is an in-process flag, so this type is
//! deliberately not serializable.

use crate::id::{NodeId, WorkspaceId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// One invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InvocationId(String);

impl InvocationId {
    /// Build an invocation id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InvocationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One delegated task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(String);

impl TaskId {
    /// Build a task id.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A reference to the policy decision that admitted a call.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyDecisionRef(String);

impl PolicyDecisionRef {
    /// Build a decision reference.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The reference as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who is acting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// A local turn, identified by its session and preset.
    Local {
        /// The session that is acting.
        session: String,
        /// The preset in effect.
        preset: String,
    },
    /// A peer Node acting through a delegation or a remote binding.
    Remote {
        /// The authenticated peer.
        node: NodeId,
    },
}

/// A per-invocation cancellation flag.
///
/// One flag per invocation, never one per provider: a shared flag cancels every
/// overlapping call, and a later call inherits the earlier cancellation.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal cancellation to whoever is watching.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was signalled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Correlation identifiers for tracing one call across a federation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceContext {
    /// The trace this invocation belongs to.
    pub trace_id: String,
    /// The span of this invocation.
    pub span_id: String,
}

impl TraceContext {
    /// Build a trace context.
    pub fn new(trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
        }
    }
}

/// One hop in a delegation chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationHop {
    /// The Node the task was delegated to.
    pub node: NodeId,
    /// The delegation that carried it.
    pub delegation: String,
}

/// Everything one invocation is allowed to know about why it is running.
#[derive(Debug, Clone)]
pub struct InvocationContext {
    /// This invocation.
    pub invocation: InvocationId,
    /// Who is acting.
    pub principal: Principal,
    /// The workspace the call belongs to. There is exactly one, even when a
    /// binding is remote.
    pub workspace: WorkspaceId,
    /// The delegated task, when the call belongs to one.
    pub task: Option<TaskId>,
    /// Absolute deadline. A duration restarted at each hop is how a federated
    /// call outlives its budget.
    pub deadline: Option<Instant>,
    /// Per-invocation cancellation.
    pub cancel: CancelToken,
    /// Trace correlation.
    pub trace: TraceContext,
    /// Delegation provenance, empty for a local call.
    pub delegation: Vec<DelegationHop>,
    /// The decision that admitted the call.
    pub policy: Option<PolicyDecisionRef>,
}

impl InvocationContext {
    /// Start a local invocation in one workspace.
    pub fn local(invocation: InvocationId, principal: Principal, workspace: WorkspaceId) -> Self {
        Self {
            invocation,
            principal,
            workspace,
            task: None,
            deadline: None,
            cancel: CancelToken::new(),
            trace: TraceContext::default(),
            delegation: Vec::new(),
            policy: None,
        }
    }

    /// Whether the deadline has already passed.
    pub fn expired(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }

    /// Whether the call was cancelled.
    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
