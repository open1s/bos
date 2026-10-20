//! The policy audit: a bounded ring of denials and counted allows.
//!
//! The shape follows the existing approval audit in
//! [`crate::security::rules`]: a [`std::sync::Mutex`]-guarded
//! [`std::collections::VecDeque`], oldest first, capped on insert, with a
//! snapshot accessor. What differs is the payload, because a policy decision is
//! about an action and a target rather than an argument digest.
//!
//! Denials are recorded **verbatim** - the rule that refused, the policy's
//! reason, the tool, the principal and the target - because a refused action is
//! the one a reader will ask about later. Allows are only counted: a turn makes
//! thousands of them and none is interesting unless it is missing. No
//! environment value, credential or file content is ever recorded; a target is
//! a path or a host and resolved address.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use super::target::{Action, Decision, PolicyInput};

/// Default size of the bounded denial ring.
pub const DEFAULT_AUDIT_CAPACITY: usize = 128;

/// One denied decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAuditEntry {
    /// Wall-clock time of the decision, in Unix epoch milliseconds.
    pub at_ms: u64,
    /// Session the decision was made for.
    pub session: String,
    /// Agent preset in effect.
    pub preset: String,
    /// Tool that asked.
    pub tool: String,
    /// The effective permission in force when the decision was made.
    ///
    /// The session mode capped by the preset ceiling, copied from
    /// `data.permission` so an audit row can distinguish a read-only floor
    /// from a narrower policy on the same tool.
    pub permission: String,
    /// What it wanted to do.
    pub action: Action,
    /// The path, or the host and resolved address.
    pub target: String,
    /// The rule that refused it, when the policy named one.
    pub rule: Option<String>,
    /// The policy's reason, when it gave one.
    pub reason: Option<String>,
}

/// Counted allows, one total per action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AllowCounts {
    /// Every allowed decision.
    pub total: u64,
    /// Allowed reads.
    pub reads: u64,
    /// Allowed writes.
    pub writes: u64,
    /// Allowed egress checks.
    pub egress: u64,
    /// Allowed spawns.
    pub spawns: u64,
    /// Allowed capability (registration-time) questions.
    pub capabilities: u64,
}

/// The audit ring and its counters.
pub struct PolicyAudit {
    denials: Mutex<VecDeque<PolicyAuditEntry>>,
    capacity: AtomicUsize,
    allows: [AtomicU64; 5],
    allows_total: AtomicU64,
    denials_total: AtomicU64,
}

impl Default for PolicyAudit {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyAudit {
    /// An empty audit with the default capacity.
    pub fn new() -> Self {
        Self {
            denials: Mutex::new(VecDeque::new()),
            capacity: AtomicUsize::new(DEFAULT_AUDIT_CAPACITY),
            allows: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            allows_total: AtomicU64::new(0),
            denials_total: AtomicU64::new(0),
        }
    }

    /// Cap the denial ring at `capacity` entries (0 disables recording).
    pub fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, Ordering::Relaxed);
        if let Ok(mut ring) = self.denials.lock() {
            while ring.len() > capacity {
                ring.pop_front();
            }
        }
    }

    /// The configured denial-ring capacity.
    pub fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Record one decided question, with the effective permission in force.
    pub(crate) fn record(&self, input: &PolicyInput, decision: &Decision, permission: &str) {
        if decision.is_allowed() {
            self.allows_total.fetch_add(1, Ordering::Relaxed);
            self.allows[input.action.index()].fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.denials_total.fetch_add(1, Ordering::Relaxed);
        let capacity = self.capacity.load(Ordering::Relaxed);
        if capacity == 0 {
            return;
        }
        let entry = PolicyAuditEntry {
            at_ms: now_ms(),
            session: input.session.clone(),
            preset: input.preset.clone(),
            tool: input.tool.clone(),
            permission: permission.to_string(),
            action: input.action,
            target: input.target.display(),
            rule: decision.rule.clone(),
            reason: decision.reason.clone(),
        };
        if let Ok(mut ring) = self.denials.lock() {
            while ring.len() >= capacity {
                ring.pop_front();
            }
            ring.push_back(entry);
        }
    }

    /// Empty the ring and reset every counter.
    pub fn clear(&self) {
        if let Ok(mut ring) = self.denials.lock() {
            ring.clear();
        }
        self.allows_total.store(0, Ordering::Relaxed);
        self.denials_total.store(0, Ordering::Relaxed);
        for counter in &self.allows {
            counter.store(0, Ordering::Relaxed);
        }
    }

    /// Snapshot of the denial ring, oldest first.
    pub fn denials(&self) -> Vec<PolicyAuditEntry> {
        self.denials
            .lock()
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The counted allows.
    pub fn allows(&self) -> AllowCounts {
        AllowCounts {
            total: self.allows_total.load(Ordering::Relaxed),
            reads: self.allows[Action::Read.index()].load(Ordering::Relaxed),
            writes: self.allows[Action::Write.index()].load(Ordering::Relaxed),
            egress: self.allows[Action::Egress.index()].load(Ordering::Relaxed),
            spawns: self.allows[Action::Spawn.index()].load(Ordering::Relaxed),
            capabilities: self.allows[Action::Capability.index()].load(Ordering::Relaxed),
        }
    }

    /// Every denial ever counted, including ones the ring has since dropped.
    pub fn denials_total(&self) -> u64 {
        self.denials_total.load(Ordering::Relaxed)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
