//! Hook provider seam for runtime observers.

use std::sync::{Arc, OnceLock, RwLock};

/// One runtime attached to a hook point.
pub struct HookTarget {
    /// Extension id.
    pub id: String,
    /// Runtime that receives the batch.
    pub runtime: Arc<dyn crate::contract::Runtime>,
}

/// Host-provided hook target resolution.
pub trait HookProvider: Send + Sync {
    /// Runtimes attached to one point.
    fn targets(&self, point: &str) -> Vec<HookTarget>;
    /// Whether any extension declared a hook.
    fn attached(&self) -> bool;
}

static PROVIDER: OnceLock<RwLock<Option<Arc<dyn HookProvider>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<dyn HookProvider>>> {
    PROVIDER.get_or_init(|| RwLock::new(None))
}

/// Install the host provider.
pub fn install(provider: Arc<dyn HookProvider>) {
    if let Ok(mut guard) = slot().write() {
        *guard = Some(provider);
    }
}

/// Resolve targets for one point.
pub fn targets(point: &str) -> Vec<HookTarget> {
    slot()
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|provider| provider.targets(point)))
        .unwrap_or_default()
}

/// Whether any hook is attached.
pub fn attached() -> bool {
    slot()
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|provider| provider.attached()))
        .unwrap_or(false)
}
