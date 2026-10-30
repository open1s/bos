//! The lifecycle state machine and the per-extension health record.
//!
//! The state machine is deliberately small and explicit. A host that can move
//! an extension from any state to any other has no lifecycle, only a label; an
//! invalid transition is an error the caller has to fix, not a silent rewrite.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::capability::unix_now;
use super::types::{ExtensionError, LifecycleState};

/// Health derived from the lifecycle state and recent failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Health {
    /// Active and no recorded failure since the last start.
    Healthy,
    /// Active but a call failed recently.
    Degraded,
    /// Not active because of a failure.
    Unhealthy,
    /// Not started yet, or deliberately stopped.
    Unknown,
}

impl Health {
    /// A label for status output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }
}

/// One extension's live lifecycle record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LifecycleRecord {
    /// Extension id.
    pub(crate) id: String,
    /// Current state.
    pub(crate) state: LifecycleState,
    /// Whether the reader left it enabled.
    pub(crate) enabled: bool,
    /// The last failure, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_error: Option<String>,
    /// Unix seconds of the last transition.
    pub(crate) updated_at: u64,
    /// Unix seconds of the last successful activation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) activated_at: Option<u64>,
    /// How long the last activation took.
    #[serde(default)]
    pub(crate) startup_ms: u64,
    /// Calls accepted since activation.
    #[serde(default)]
    pub(crate) tool_calls: u64,
    /// Calls that failed since activation.
    #[serde(default)]
    pub(crate) tool_failures: u64,
}

impl LifecycleRecord {
    /// A fresh record for a discovered package.
    pub(crate) fn new(id: &str, enabled: bool) -> Self {
        Self {
            id: id.to_string(),
            state: LifecycleState::Discovered,
            enabled,
            last_error: None,
            updated_at: unix_now(),
            activated_at: None,
            startup_ms: 0,
            tool_calls: 0,
            tool_failures: 0,
        }
    }

    /// Health derived from the current facts.
    pub(crate) fn health(&self) -> Health {
        match self.state {
            LifecycleState::Active if self.tool_failures == 0 => Health::Healthy,
            LifecycleState::Active => Health::Degraded,
            LifecycleState::Failed => Health::Unhealthy,
            LifecycleState::Starting | LifecycleState::Loading => Health::Degraded,
            _ => Health::Unknown,
        }
    }
}

/// The lifecycle manager: one record per extension id.
#[derive(Debug, Clone, Default)]
pub(crate) struct LifecycleManager {
    records: BTreeMap<String, LifecycleRecord>,
}

impl LifecycleManager {
    /// An empty manager.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a discovered package. Registering the same id twice keeps the
    /// existing record but refreshes the enabled flag.
    pub(crate) fn register(&mut self, id: &str, enabled: bool) {
        match self.records.get_mut(id) {
            Some(record) => record.enabled = enabled,
            None => {
                self.records
                    .insert(id.to_string(), LifecycleRecord::new(id, enabled));
            }
        }
    }

    /// Remove an uninstalled package.
    pub(crate) fn unregister(&mut self, id: &str) -> Option<LifecycleRecord> {
        self.records.remove(id)
    }

    /// One record.
    pub(crate) fn get(&self, id: &str) -> Option<&LifecycleRecord> {
        self.records.get(id)
    }

    /// Every record, sorted by id.
    pub(crate) fn records(&self) -> Vec<&LifecycleRecord> {
        self.records.values().collect()
    }

    /// Move an extension to `to`, refusing transitions the machine does not
    /// define.
    pub(crate) fn transition(
        &mut self,
        id: &str,
        to: LifecycleState,
    ) -> Result<&LifecycleRecord, ExtensionError> {
        let Some(record) = self.records.get_mut(id) else {
            return Err(ExtensionError::NotInstalled { id: id.to_string() });
        };
        if record.state == to {
            return Ok(record);
        }
        if !allowed_transition(record.state, to) {
            return Err(ExtensionError::Lifecycle {
                extension: id.to_string(),
                from: record.state,
                to,
            });
        }
        record.state = to;
        record.updated_at = unix_now();
        if to == LifecycleState::Active {
            record.activated_at = Some(unix_now());
        }
        Ok(record)
    }

    /// Mark an extension failed, recording the reason. A failure is always a
    /// legal move: the point is to stop the package, not to police the state it
    /// was in when it broke.
    pub(crate) fn fail(&mut self, id: &str, error: impl Into<String>) -> LifecycleRecord {
        let message = error.into();
        let record = self
            .records
            .entry(id.to_string())
            .or_insert_with(|| LifecycleRecord::new(id, true));
        record.state = LifecycleState::Failed;
        record.last_error = Some(message);
        record.updated_at = unix_now();
        record.clone()
    }

    /// Record a successful activation and how long it took.
    pub(crate) fn activated(
        &mut self,
        id: &str,
        startup_ms: u64,
    ) -> Result<&LifecycleRecord, ExtensionError> {
        let record = self
            .records
            .get_mut(id)
            .ok_or_else(|| ExtensionError::NotInstalled { id: id.to_string() })?;
        record.state = LifecycleState::Active;
        record.last_error = None;
        record.updated_at = unix_now();
        record.activated_at = Some(unix_now());
        record.startup_ms = startup_ms;
        Ok(record)
    }

    /// Change the enabled flag without moving the lifecycle state.
    pub(crate) fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(record) = self.records.get_mut(id) {
            record.enabled = enabled;
            record.updated_at = unix_now();
        }
    }

    /// Count one accepted tool call.
    pub(crate) fn record_call(&mut self, id: &str) {
        if let Some(record) = self.records.get_mut(id) {
            record.tool_calls = record.tool_calls.saturating_add(1);
        }
    }

    /// Count one failed tool call.
    pub(crate) fn record_failure(&mut self, id: &str) {
        if let Some(record) = self.records.get_mut(id) {
            record.tool_failures = record.tool_failures.saturating_add(1);
            record.updated_at = unix_now();
        }
    }
}

/// Whether the state machine defines this edge.
pub(crate) fn allowed_transition(from: LifecycleState, to: LifecycleState) -> bool {
    if to == LifecycleState::Failed {
        return !matches!(from, LifecycleState::Unloaded);
    }
    use LifecycleState::*;
    matches!(
        (from, to),
        (Discovered, Validating)
            | (Discovered, Unloaded)
            | (Validating, Resolved)
            | (Resolved, Installing)
            | (Resolved, Loading)
            | (Installing, Installed)
            | (Installed, Loading)
            | (Installed, Unloaded)
            | (Loading, Starting)
            | (Loading, Unloaded)
            | (Starting, Active)
            | (Starting, Unloaded)
            | (Active, Suspending)
            | (Active, Stopping)
            | (Suspending, Suspended)
            | (Suspending, Stopping)
            | (Suspended, Starting)
            | (Suspended, Stopping)
            | (Suspended, Unloaded)
            | (Stopping, Unloaded)
            | (Unloaded, Loading)
            | (Unloaded, Discovered)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_happy_path_moves_through_the_machine() {
        let mut manager = LifecycleManager::new();
        manager.register("com.example.a", true);
        for state in [
            LifecycleState::Validating,
            LifecycleState::Resolved,
            LifecycleState::Installing,
            LifecycleState::Installed,
            LifecycleState::Loading,
            LifecycleState::Starting,
            LifecycleState::Active,
            LifecycleState::Stopping,
            LifecycleState::Unloaded,
        ] {
            manager
                .transition("com.example.a", state)
                .unwrap_or_else(|error| panic!("{state}: {error}"));
        }
        assert_eq!(
            manager.get("com.example.a").expect("record").state,
            LifecycleState::Unloaded
        );
    }

    #[test]
    fn an_undefined_transition_is_refused() {
        let mut manager = LifecycleManager::new();
        manager.register("com.example.a", true);
        manager
            .transition("com.example.a", LifecycleState::Active)
            .expect_err("discovered cannot activate directly");
        manager
            .transition("com.example.a", LifecycleState::Validating)
            .expect("validate");
        manager
            .transition("com.example.a", LifecycleState::Resolved)
            .expect("resolve");
        assert!(manager
            .transition("com.example.a", LifecycleState::Discovered)
            .is_err());
    }

    #[test]
    fn failure_is_always_allowed_and_changes_health() {
        let mut manager = LifecycleManager::new();
        manager.register("com.example.a", true);
        manager
            .transition("com.example.a", LifecycleState::Validating)
            .expect("validate");
        let record = manager.fail("com.example.a", "manifest is invalid");
        assert_eq!(record.state, LifecycleState::Failed);
        assert_eq!(record.health(), Health::Unhealthy);
        assert_eq!(record.last_error.as_deref(), Some("manifest is invalid"));
    }

    #[test]
    fn call_failures_degrade_an_active_extension() {
        let mut manager = LifecycleManager::new();
        manager.register("com.example.a", true);
        manager.activated("com.example.a", 12).expect("activate");
        manager.record_call("com.example.a");
        assert_eq!(
            manager.get("com.example.a").expect("record").health(),
            Health::Healthy
        );
        manager.record_failure("com.example.a");
        let record = manager.get("com.example.a").expect("record");
        assert_eq!(record.health(), Health::Degraded);
        assert_eq!(record.tool_calls, 1);
        assert_eq!(record.tool_failures, 1);
    }
}
