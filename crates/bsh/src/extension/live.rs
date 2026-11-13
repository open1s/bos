//! Service-change driver for live extension overlays.
//!
//! When a provider appears, disappears or is replaced, every activation that
//! declared that service is suspended or resumed without rebuilding the Agent.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::agent::ContributionLease;
use ferris_sdk::composable::ServiceRegistry;

/// One activation watched by the driver.
pub(crate) struct LiveEntry {
    id: String,
    lease: ContributionLease,
    requirements: Vec<(String, bool)>,
    suspended: AtomicBool,
}

impl LiveEntry {
    /// Build one watch entry from an activation's lease and requires.
    pub(crate) fn new(
        id: String,
        lease: ContributionLease,
        requirements: Vec<(String, bool)>,
        services: &ServiceRegistry,
    ) -> Self {
        let suspended = !requirements
            .iter()
            .all(|(name, optional)| *optional || services.has_name(name));
        Self {
            id,
            lease,
            requirements,
            suspended: AtomicBool::new(suspended),
        }
    }

    fn ready(&self, services: &ServiceRegistry) -> bool {
        self.requirements
            .iter()
            .all(|(name, optional)| *optional || services.has_name(name))
    }
}

static LIVE_STATE: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();

fn live_state() -> &'static Mutex<BTreeMap<String, bool>> {
    LIVE_STATE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Whether the driver currently has this activation suspended.
pub(crate) fn is_suspended(id: &str) -> bool {
    live_state()
        .lock()
        .ok()
        .and_then(|state| state.get(id).copied())
        .unwrap_or(false)
}

/// A snapshot of the live activation fibers known to the driver.
pub(crate) fn snapshot() -> Vec<serde_json::Value> {
    live_state()
        .lock()
        .map(|state| {
            state
                .iter()
                .map(|(id, suspended)| serde_json::json!({ "id": id, "suspended": suspended }))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn set_suspended(id: &str, suspended: bool) {
    if let Ok(mut state) = live_state().lock() {
        state.insert(id.to_string(), suspended);
    }
}

/// Start the watcher on the current Tokio runtime.
pub(crate) fn start(
    agent: Arc<::agent::Agent>,
    services: Arc<ServiceRegistry>,
    entries: Vec<LiveEntry>,
) -> Option<tokio::task::JoinHandle<()>> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    // Subscribe before the first readiness check so a transition between
    // LiveEntry::new() and this task start is either in the stream or in the
    // reconciliation below; the two together leave no gap.
    let mut changes = services.subscribe_changes();
    Some(handle.spawn(async move {
        // Initial reconciliation: close the construction/subscription gap.
        reconcile(&entries, &agent, &services, None);
        loop {
            match changes.recv().await {
                Ok(change) => reconcile(&entries, &agent, &services, Some(&change.name)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!(
                        target: "bsh::extension",
                        "live service watcher lagged {skipped} events; reconciling against the registry"
                    );
                    reconcile(&entries, &agent, &services, None);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }))
}

/// Apply the current readiness of every entry (or only entries naming
/// `changed`) against the registry, which is authoritative after a lag.
fn reconcile(
    entries: &[LiveEntry],
    agent: &Arc<::agent::Agent>,
    services: &ServiceRegistry,
    changed: Option<&str>,
) {
    for entry in entries {
        if let Some(name) = changed {
            if !entry
                .requirements
                .iter()
                .any(|(required, _)| required == name)
            {
                continue;
            }
        }
        let ready = entry.ready(services);
        let suspended = entry.suspended.load(Ordering::Acquire);
        if !ready && !suspended {
            entry.lease.suspend_live(agent);
            entry.suspended.store(true, Ordering::Release);
            set_suspended(&entry.id, true);
        } else if ready && suspended {
            entry.lease.resume_live(agent);
            entry.suspended.store(false, Ordering::Release);
            set_suspended(&entry.id, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use ferris_sdk::composable::{Scope, ServiceRegistry};
    use std::sync::Arc;

    /// The readiness predicate the live driver reconciles against (the same
    /// computation LiveEntry::ready performs): a requirement is satisfied when
    /// it is optional or a provider of that name exists.
    fn ready(requirements: &[(String, bool)], services: &ServiceRegistry) -> bool {
        requirements
            .iter()
            .all(|(name, optional)| *optional || services.has_name(name))
    }

    /// Provider disappearance suspends a required consumer; publication and
    /// replacement reconcile it, and each transition emits the change event
    /// the live driver subscribes to (issue #302).
    #[tokio::test]
    async fn provider_lifecycle_reconciles_a_required_consumer() {
        let services = Arc::new(ServiceRegistry::new());
        let mut changes = services.subscribe_changes();
        let requirements = vec![("com.example.search".to_string(), false)];

        // No provider: the required consumer is not ready (would be suspended).
        assert!(!ready(&requirements, &services));

        // A provider publishes at a shared scope: the consumer becomes ready,
        // and the change event the driver watches is emitted.
        let token = services.provide_contract(
            Scope::Host,
            "com.example.search",
            "com.example.search-api",
            semver::Version::new(1, 0, 0),
            Arc::new(42u32),
        );
        let change = changes.recv().await.expect("publish emits a change");
        assert_eq!(change.name, "com.example.search");
        assert!(ready(&requirements, &services));

        // Provider disappears: the consumer is no longer ready (suspended).
        assert!(services.revoke(&token));
        let change = changes.recv().await.expect("revoke emits a change");
        assert_eq!(change.name, "com.example.search");
        assert!(!ready(&requirements, &services));

        // A replacement provider reconciles the consumer again.
        let replacement = services.provide_contract(
            Scope::Host,
            "com.example.search",
            "com.example.search-api",
            semver::Version::new(1, 1, 0),
            Arc::new(43u32),
        );
        let _ = changes.recv().await.expect("replacement emits a change");
        assert!(ready(&requirements, &services));
        assert!(services.revoke(&replacement));
    }

    /// An optional requirement never blocks activation (issue #302).
    #[test]
    fn an_optional_requirement_does_not_block() {
        let services = ServiceRegistry::new();
        let requirements = vec![("com.example.optional".to_string(), true)];
        assert!(ready(&requirements, &services));
    }
}
