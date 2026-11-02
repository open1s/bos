//! Service-change driver for live extension overlays.
//!
//! When a provider appears, disappears or is replaced, every activation that
//! declared that service is suspended or resumed without rebuilding the Agent.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::agent::ContributionLease;
use super::composable::ServiceRegistry;

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

fn set_suspended(id: &str, suspended: bool) {
    if let Ok(mut state) = live_state().lock() {
        state.insert(id.to_string(), suspended);
    }
}

/// Start the watcher on the current Tokio runtime.
pub(crate) fn start(
    agent: Arc<::agent::Agent>,
    services: Arc<ServiceRegistry>,
    entries: Vec<LiveEntry>,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        let mut changes = services.subscribe_changes();
        while let Ok(change) = changes.recv().await {
            for entry in &entries {
                if !entry
                    .requirements
                    .iter()
                    .any(|(name, _)| name == &change.name)
                {
                    continue;
                }
                let ready = entry.ready(&services);
                let suspended = entry.suspended.load(Ordering::Acquire);
                if !ready && !suspended {
                    entry.lease.suspend_live(&agent);
                    entry.suspended.store(true, Ordering::Release);
                    set_suspended(&entry.id, true);
                } else if ready && suspended {
                    entry.lease.resume_live(&agent);
                    entry.suspended.store(false, Ordering::Release);
                    set_suspended(&entry.id, false);
                }
            }
        }
    });
}
