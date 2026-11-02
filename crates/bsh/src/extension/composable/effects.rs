//! Revertible effect scopes.
//!
//! An effect is an owned registration with an inverse. A component registers
//! effects instead of mutating global state directly, and disposing the scope
//! runs every inverse in reverse registration order. Child scopes dispose before
//! their parent, which is the ownership rule Cordis calls temporal
//! composability.
//!
//! The scope is deliberately independent of any runtime: sync cleanups are the
//! common case, async cleanups cover process/WASM/service teardown, and a late
//! registration runs immediately instead of being silently lost.

use std::any::Any;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;

/// A boxed cleanup future. Cleanups must be static because a scope may outlive
/// the activation that registered it.
pub(crate) type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// The default ceiling for one async cleanup. A wedged cleanup must not hold
/// the rest of the scope hostage.
const ASYNC_DISPOSE_TIMEOUT: Duration = Duration::from_secs(5);

enum Cleanup {
    Sync(Box<dyn FnOnce() + Send + 'static>),
    Async(Box<dyn FnOnce() -> BoxFuture + Send + 'static>),
}

struct EffectSlot {
    label: String,
    cleanup: Option<Cleanup>,
}

/// What went wrong while disposing one effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EffectFailure {
    /// Label supplied when the effect was registered.
    pub(crate) label: String,
    /// Panic message or timeout description.
    pub(crate) message: String,
}

/// The outcome of disposing a scope. A failure never stops the remaining
/// cleanups; the report is how the caller learns what was skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DisposalReport {
    /// Cleanups attempted.
    pub(crate) attempted: usize,
    /// Cleanups that panicked.
    pub(crate) failed: Vec<EffectFailure>,
    /// Async cleanups that hit the timeout.
    pub(crate) timed_out: Vec<String>,
    /// Async cleanups that could not be awaited (sync disposal path) and were
    /// spawned instead.
    pub(crate) deferred: usize,
}

/// A handle to one registered effect. Disposing the handle fires the cleanup
/// once; disposing the owning scope later observes that it already fired.
#[derive(Clone)]
pub(crate) struct Disposer {
    slot: Arc<Mutex<EffectSlot>>,
}

impl Disposer {
    /// Run the cleanup now. Returns false when it was already fired.
    pub(crate) fn dispose(&self) -> bool {
        run_slot_sync(&self.slot)
    }

    /// Run the cleanup now, awaiting an async cleanup.
    pub(crate) async fn dispose_async(&self) -> bool {
        run_slot_async(&self.slot).await
    }

    /// Whether the effect is still pending.
    pub(crate) fn is_pending(&self) -> bool {
        self.slot
            .lock()
            .map(|slot| slot.cleanup.is_some())
            .unwrap_or(false)
    }
}

/// A set of effects with one owner.
#[derive(Clone)]
pub(crate) struct EffectScope {
    inner: Arc<EffectScopeInner>,
}

struct EffectScopeInner {
    slots: Mutex<Vec<Arc<Mutex<EffectSlot>>>>,
    disposed: AtomicBool,
}

impl Default for EffectScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EffectScopeInner {
    fn drop(&mut self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }
        let slots = match self.slots.lock() {
            Ok(mut slots) => std::mem::take(&mut *slots),
            Err(_) => Vec::new(),
        };
        for slot in slots.into_iter().rev() {
            let _ = run_slot_sync(&slot);
        }
    }
}

impl EffectScope {
    /// Create an empty scope.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(EffectScopeInner {
                slots: Mutex::new(Vec::new()),
                disposed: AtomicBool::new(false),
            }),
        }
    }

    /// Whether disposal has started.
    pub(crate) fn is_disposed(&self) -> bool {
        self.inner.disposed.load(Ordering::Acquire)
    }

    /// Number of cleanups still waiting.
    pub(crate) fn pending(&self) -> usize {
        self.inner
            .slots
            .lock()
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| {
                        slot.lock()
                            .map(|slot| slot.cleanup.is_some())
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Register a synchronous cleanup.
    ///
    /// An effect registered after disposal runs immediately. That is the same
    /// rule Cordis applies: a late registration is not a silent leak.
    pub(crate) fn push_sync(
        &self,
        label: impl Into<String>,
        cleanup: impl FnOnce() + Send + 'static,
    ) -> Disposer {
        self.push_cleanup(label.into(), Cleanup::Sync(Box::new(cleanup)))
    }

    /// Register an asynchronous cleanup.
    pub(crate) fn push_async(
        &self,
        label: impl Into<String>,
        cleanup: impl FnOnce() -> BoxFuture + Send + 'static,
    ) -> Disposer {
        self.push_cleanup(label.into(), Cleanup::Async(Box::new(cleanup)))
    }

    /// Compatibility helper for the old EffectSet callers.
    pub(crate) fn push(&self, cleanup: impl FnOnce() + Send + 'static) {
        let _ = self.push_sync("effect", cleanup);
    }

    fn push_cleanup(&self, label: String, cleanup: Cleanup) -> Disposer {
        let slot = Arc::new(Mutex::new(EffectSlot {
            label,
            cleanup: Some(cleanup),
        }));
        if self.inner.disposed.load(Ordering::Acquire) {
            // Late effect: run it now instead of storing it in a scope that
            // will never be visited again.
            let _ = run_slot_sync(&slot);
            return Disposer { slot };
        }
        if let Ok(mut slots) = self.inner.slots.lock() {
            // Re-check under the lock: a concurrent dispose may have taken the
            // vector between the flag check and here.
            if self.inner.disposed.load(Ordering::Acquire) {
                drop(slots);
                let _ = run_slot_sync(&slot);
                return Disposer { slot };
            }
            slots.push(Arc::clone(&slot));
        } else {
            let _ = run_slot_sync(&slot);
        }
        Disposer { slot }
    }

    /// Create a child scope whose disposal is an effect of this scope.
    ///
    /// Child effects therefore always run before effects this scope registered
    /// earlier. The child can be disposed independently as well.
    pub(crate) fn child(&self, label: impl Into<String>) -> EffectScope {
        let child = EffectScope::new();
        let cleanup = child.clone();
        self.push_async(label, move || {
            Box::pin(async move {
                cleanup.dispose().await;
            })
        });
        child
    }

    /// Dispose every effect in reverse order, awaiting async cleanups.
    ///
    /// Panics and timeouts are isolated: one effect cannot prevent the rest of
    /// the scope from unwinding.
    pub(crate) async fn dispose(&self) -> DisposalReport {
        if self.inner.disposed.swap(true, Ordering::AcqRel) {
            return DisposalReport::default();
        }
        let slots = match self.inner.slots.lock() {
            Ok(mut slots) => std::mem::take(&mut *slots),
            Err(_) => Vec::new(),
        };

        let mut report = DisposalReport::default();
        for slot in slots.into_iter().rev() {
            let Some(cleanup) = take_cleanup(&slot) else {
                continue;
            };
            report.attempted += 1;
            match cleanup {
                Cleanup::Sync(cleanup) => {
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(cleanup)) {
                        report.failed.push(failure(&slot, payload));
                    }
                }
                Cleanup::Async(cleanup) => {
                    let future = AssertUnwindSafe(cleanup()).catch_unwind();
                    match tokio::time::timeout(ASYNC_DISPOSE_TIMEOUT, future).await {
                        Ok(Ok(())) => {}
                        Ok(Err(payload)) => report.failed.push(failure(&slot, payload)),
                        Err(_) => report.timed_out.push(label_of(&slot)),
                    }
                }
            }
        }
        report
    }

    /// Dispose synchronously, spawning async cleanups that cannot be awaited.
    ///
    /// This is the compatibility path for existing Fiber::unload callers.
    /// Correct async code calls EffectScope::dispose instead.
    pub(crate) fn dispose_sync(&self) -> DisposalReport {
        if self.inner.disposed.swap(true, Ordering::AcqRel) {
            return DisposalReport::default();
        }
        let slots = match self.inner.slots.lock() {
            Ok(mut slots) => std::mem::take(&mut *slots),
            Err(_) => Vec::new(),
        };

        let mut report = DisposalReport::default();
        for slot in slots.into_iter().rev() {
            let Some(cleanup) = take_cleanup(&slot) else {
                continue;
            };
            report.attempted += 1;
            match cleanup {
                Cleanup::Sync(cleanup) => {
                    if let Err(payload) = catch_unwind(AssertUnwindSafe(cleanup)) {
                        report.failed.push(failure(&slot, payload));
                    }
                }
                Cleanup::Async(cleanup) => {
                    run_late(cleanup());
                    report.deferred += 1;
                }
            }
        }
        report
    }

    /// Compatibility helper for the old EffectSet callers. Returns the number
    /// of cleanups attempted.
    pub(crate) fn rollback(&self) -> usize {
        self.dispose_sync().attempted
    }
}

fn take_cleanup(slot: &Arc<Mutex<EffectSlot>>) -> Option<Cleanup> {
    slot.lock().ok()?.cleanup.take()
}

fn label_of(slot: &Arc<Mutex<EffectSlot>>) -> String {
    slot.lock()
        .map(|slot| slot.label.clone())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn failure(slot: &Arc<Mutex<EffectSlot>>, payload: Box<dyn Any + Send>) -> EffectFailure {
    EffectFailure {
        label: label_of(slot),
        message: panic_message(payload),
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "cleanup panicked".to_string()
    }
}

fn run_slot_sync(slot: &Arc<Mutex<EffectSlot>>) -> bool {
    let Some(cleanup) = take_cleanup(slot) else {
        return false;
    };
    match cleanup {
        Cleanup::Sync(cleanup) => {
            let _ = catch_unwind(AssertUnwindSafe(cleanup));
        }
        Cleanup::Async(cleanup) => run_late(cleanup()),
    }
    true
}

async fn run_slot_async(slot: &Arc<Mutex<EffectSlot>>) -> bool {
    let Some(cleanup) = take_cleanup(slot) else {
        return false;
    };
    match cleanup {
        Cleanup::Sync(cleanup) => {
            let _ = catch_unwind(AssertUnwindSafe(cleanup));
        }
        Cleanup::Async(cleanup) => {
            let _ = tokio::time::timeout(ASYNC_DISPOSE_TIMEOUT, cleanup()).await;
        }
    }
    true
}

/// Run an async cleanup from a synchronous context. Inside a Tokio runtime the
/// task is spawned; outside one the future is driven to completion.
fn run_late(future: BoxFuture) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future);
    } else {
        futures::executor::block_on(future);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn child_scope_disposes_before_parent_effects() {
        let parent = EffectScope::new();
        let order = Arc::new(Mutex::new(Vec::new()));

        let first = order.clone();
        parent.push_sync("parent.first", move || {
            first.lock().unwrap().push("first");
        });

        let child = parent.child("ctx.child()");
        let second = order.clone();
        child.push_sync("child.second", move || {
            second.lock().unwrap().push("second");
        });
        let third = order.clone();
        child.push_sync("child.third", move || {
            third.lock().unwrap().push("third");
        });

        let report = parent.dispose().await;
        assert_eq!(report.attempted, 2);
        assert_eq!(*order.lock().unwrap(), vec!["third", "second", "first"]);
        assert!(child.is_disposed());
    }

    #[tokio::test]
    async fn panic_and_timeout_do_not_stop_the_scope() {
        let scope = EffectScope::new();
        let count = Arc::new(AtomicUsize::new(0));

        let first = count.clone();
        scope.push_sync("first", move || {
            first.fetch_add(1, Ordering::SeqCst);
        });
        scope.push_sync("boom", || panic!("cleanup failure"));
        let last = count.clone();
        scope.push_sync("last", move || {
            last.fetch_add(1, Ordering::SeqCst);
        });

        let report = scope.dispose().await;
        assert_eq!(report.attempted, 3);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].label, "boom");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(scope.dispose().await.attempted, 0);
    }

    #[tokio::test]
    async fn async_cleanup_is_awaited() {
        let scope = EffectScope::new();
        let (tx, rx) = tokio::sync::oneshot::channel();

        scope.push_async("async", move || {
            Box::pin(async move {
                let _ = tx.send(());
            })
        });

        let report = scope.dispose().await;
        assert_eq!(report.attempted, 1);
        rx.await.expect("async cleanup ran");
    }

    #[tokio::test]
    async fn late_effect_runs_immediately() {
        let scope = EffectScope::new();
        assert_eq!(scope.dispose().await.attempted, 0);

        let fired = Arc::new(AtomicBool::new(false));
        let marker = fired.clone();
        let disposer = scope.push_sync("late", move || {
            marker.store(true, Ordering::SeqCst);
        });

        assert!(fired.load(Ordering::SeqCst));
        assert!(!disposer.is_pending());
    }

    #[tokio::test]
    async fn disposer_is_idempotent() {
        let scope = EffectScope::new();
        let count = Arc::new(AtomicUsize::new(0));
        let marker = count.clone();
        let disposer = scope.push_sync("once", move || {
            marker.fetch_add(1, Ordering::SeqCst);
        });

        assert!(disposer.dispose());
        assert!(!disposer.dispose());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(scope.dispose().await.attempted, 0);
    }

    #[test]
    fn sync_disposal_defers_async_cleanup() {
        let scope = EffectScope::new();
        scope.push_async("async", || Box::pin(async {}));
        let report = scope.dispose_sync();
        assert_eq!(report.attempted, 1);
        assert_eq!(report.deferred, 1);
    }
}
