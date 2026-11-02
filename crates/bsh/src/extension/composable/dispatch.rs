//! In-process event dispatch with Cordis's five modes.
//!
//! The remote extension EventBus is a bounded JSON pub/sub queue for observers.
//! This dispatcher is the in-process seam: it carries typed JSON payloads,
//! supports registration order and prepend, and implements emit, parallel,
//! serial, bail, and waterfall. A listener is an effect; dropping its
//! Subscription removes it.
//!
//! Parallel is synchronous here. A host that needs true concurrency can call
//! the same handlers from its own task pool; the ordering semantics remain the
//! same.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde_json::Value;

use super::effects::EffectScope;

/// Checks whether an owner may register a waterfall interceptor.
pub(crate) trait InterceptorPolicy: Send + Sync {
    /// Allow or deny one interceptor registration.
    fn authorize(&self, owner: &str, event: &str) -> Result<(), String>;
}

/// How an event is dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DispatchMode {
    /// Observe; return value ignored.
    Emit,
    /// Invoke all observers; return value ignored.
    Parallel,
    /// Ordered until a listener returns a value.
    Serial,
    /// Ordered until a listener returns a value.
    Bail,
    /// Around-middleware; each listener may call next().
    Waterfall,
}

type Handler = Arc<dyn Fn(Value, Chain) -> Option<Value> + Send + Sync>;

struct RegisteredListener {
    id: u64,
    order: i32,
    mode: DispatchMode,
    handler: Handler,
}

impl Clone for RegisteredListener {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            order: self.order,
            mode: self.mode,
            handler: Arc::clone(&self.handler),
        }
    }
}

/// The continuation passed to a listener.
#[derive(Clone)]
pub(crate) struct Chain {
    listeners: Arc<Vec<RegisteredListener>>,
    index: usize,
    terminal: Arc<dyn Fn(Value) -> Option<Value> + Send + Sync>,
}

impl Chain {
    /// Run the next listener (or the terminal).
    pub(crate) fn next(&self, value: Value) -> Option<Value> {
        let Some(listener) = self.listeners.get(self.index).cloned() else {
            return (self.terminal)(value);
        };
        let next = Chain {
            listeners: Arc::clone(&self.listeners),
            index: self.index + 1,
            terminal: Arc::clone(&self.terminal),
        };
        (listener.handler)(value, next)
    }
}

struct Inner {
    listeners: RwLock<BTreeMap<String, Vec<RegisteredListener>>>,
    next_id: AtomicU64,
    interceptor_policy: RwLock<Option<Arc<dyn InterceptorPolicy>>>,
}

/// A dispatcher shared by contexts and hosts.
#[derive(Clone)]
pub(crate) struct EventDispatcher {
    inner: Arc<Inner>,
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl EventDispatcher {
    /// Create an empty dispatcher.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                listeners: RwLock::new(BTreeMap::new()),
                next_id: AtomicU64::new(1),
                interceptor_policy: RwLock::new(None),
            }),
        }
    }

    /// Register a listener. Lower order runs first; equal order keeps
    /// registration order.
    pub(crate) fn on<F>(
        &self,
        name: impl Into<String>,
        mode: DispatchMode,
        order: i32,
        handler: F,
    ) -> Subscription
    where
        F: Fn(Value, Chain) -> Option<Value> + Send + Sync + 'static,
    {
        let name = name.into();
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut listeners) = self.inner.listeners.write() {
            let entry = listeners.entry(name.clone()).or_default();
            entry.push(RegisteredListener {
                id,
                order,
                mode,
                handler: Arc::new(handler),
            });
            entry.sort_by_key(|listener| (listener.order, listener.id));
        }

        let inner = Arc::clone(&self.inner);
        let scope = EffectScope::new();
        let cleanup_name = name.clone();
        scope.push_sync("event.unsubscribe", move || {
            if let Ok(mut listeners) = inner.listeners.write() {
                if let Some(entry) = listeners.get_mut(&cleanup_name) {
                    entry.retain(|listener| listener.id != id);
                    if entry.is_empty() {
                        listeners.remove(&cleanup_name);
                    }
                }
            }
        });
        Subscription { scope }
    }

    /// Create a dispatcher that gates waterfall interceptor registration.
    pub(crate) fn with_interceptor_policy(policy: Arc<dyn InterceptorPolicy>) -> Self {
        Self {
            inner: Arc::new(Inner {
                listeners: RwLock::new(BTreeMap::new()),
                next_id: AtomicU64::new(1),
                interceptor_policy: RwLock::new(Some(policy)),
            }),
        }
    }

    /// Install or replace the interceptor policy.
    pub(crate) fn set_interceptor_policy(&self, policy: Arc<dyn InterceptorPolicy>) {
        if let Ok(mut slot) = self.inner.interceptor_policy.write() {
            *slot = Some(policy);
        }
    }

    /// Register a waterfall interceptor only when the policy permits the owner.
    /// Observers use on(); this is the capability-bearing path.
    pub(crate) fn on_interceptor<F>(
        &self,
        owner: &str,
        name: impl Into<String>,
        order: i32,
        handler: F,
    ) -> Result<Subscription, String>
    where
        F: Fn(Value, Chain) -> Option<Value> + Send + Sync + 'static,
    {
        let name = name.into();
        let policy = self
            .inner
            .interceptor_policy
            .read()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(|| "no interceptor policy is installed".to_string())?;
        policy.authorize(owner, &name)?;
        Ok(self.on(name, DispatchMode::Waterfall, order, handler))
    }

    /// Dispatch only listeners registered for observe mode.
    pub(crate) fn emit(&self, name: &str, payload: Value) {
        for listener in self.snapshot(name, DispatchMode::Emit) {
            let chain = self.chain_for(&listener, 1);
            let _ = (listener.handler)(payload.clone(), chain);
        }
    }

    /// Dispatch only parallel-observer listeners.
    ///
    /// This in-process adapter currently performs synchronous fan-out because
    /// handlers are synchronous Rust closures; it does not provide async
    /// concurrency. The mode remains isolated from ordinary emit listeners.
    pub(crate) fn parallel(&self, name: &str, payload: Value) {
        for listener in self.snapshot(name, DispatchMode::Parallel) {
            let chain = self.chain_for(&listener, 1);
            let _ = (listener.handler)(payload.clone(), chain);
        }
    }

    /// Dispatch serial listeners in registration order until one bails.
    ///
    /// Async waiting is not available in this synchronous Rust adapter, so
    /// serial currently has the same execution timing as bail.
    pub(crate) fn serial(&self, name: &str, payload: Value) -> Option<Value> {
        self.first_value(name, DispatchMode::Serial, payload)
    }

    /// Dispatch synchronous bail listeners in registration order.
    pub(crate) fn bail(&self, name: &str, payload: Value) -> Option<Value> {
        self.first_value(name, DispatchMode::Bail, payload)
    }

    /// Dispatch as an around-middleware chain.
    pub(crate) fn waterfall(&self, name: &str, payload: Value) -> Option<Value> {
        let listeners = self.snapshot(name, DispatchMode::Waterfall);
        let chain = Chain {
            listeners: Arc::new(listeners),
            index: 0,
            terminal: Arc::new(Some),
        };
        chain.next(payload)
    }

    fn first_value(
        &self,
        name: &str,
        mode: DispatchMode,
        payload: Value,
    ) -> Option<Value> {
        for listener in self.snapshot(name, mode) {
            let chain = self.chain_for(&listener, 1);
            if let Some(value) = (listener.handler)(payload.clone(), chain) {
                if is_bail_value(&value) {
                    return Some(value);
                }
            }
        }
        None
    }

    fn chain_for(&self, listener: &RegisteredListener, index: usize) -> Chain {
        Chain {
            listeners: Arc::new(vec![listener.clone()]),
            index,
            terminal: Arc::new(Some),
        }
    }

    fn snapshot(&self, name: &str, mode: DispatchMode) -> Vec<RegisteredListener> {
        self.inner
            .listeners
            .read()
            .ok()
            .and_then(|listeners| listeners.get(name).cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|listener| listener.mode == mode)
            .collect()
    }

    /// Number of listeners for one event.
    pub(crate) fn listener_count(&self, name: &str) -> usize {
        self.inner
            .listeners
            .read()
            .ok()
            .and_then(|listeners| listeners.get(name).map(Vec::len))
            .unwrap_or(0)
    }
}

/// Cordis treats null, false and undefined as "continue" values.
fn is_bail_value(value: &Value) -> bool {
    !value.is_null() && value != &Value::Bool(false)
}

/// Dropping a subscription removes its listener.
pub(crate) struct Subscription {
    scope: EffectScope,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.scope.dispose_sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "release benchmark; run with --release --ignored"]
    fn gold_standard_release_event_budget() {
        let dispatcher = EventDispatcher::new();
        let count = Arc::new(AtomicU64::new(0));
        let marker = Arc::clone(&count);
        let _subscription = dispatcher.on("tick", DispatchMode::Emit, 0, move |_value, _chain| {
            marker.fetch_add(1, Ordering::Relaxed);
            None
        });
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            dispatcher.emit("tick", serde_json::json!({ "n": 1 }));
        }
        let elapsed = start.elapsed();
        assert_eq!(count.load(Ordering::Relaxed), 100_000);
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "100k dispatches took {elapsed:?} in release"
        );
    }

    struct DenyInterceptors;

    impl InterceptorPolicy for DenyInterceptors {
        fn authorize(&self, _owner: &str, _event: &str) -> Result<(), String> {
            Err("interceptor capability is not granted".to_string())
        }
    }

    struct AllowInterceptors;

    impl InterceptorPolicy for AllowInterceptors {
        fn authorize(&self, _owner: &str, _event: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn interceptor_registration_is_capability_gated() {
        let denied = EventDispatcher::with_interceptor_policy(Arc::new(DenyInterceptors));
        assert!(denied
            .on_interceptor("ext", "tool.before", 0, |_value, _next| None)
            .is_err());

        let allowed = EventDispatcher::with_interceptor_policy(Arc::new(AllowInterceptors));
        let _subscription = allowed
            .on_interceptor("ext", "tool.before", 0, |_value, next| {
                next.next(serde_json::json!({ "ok": true }))
            })
            .expect("allowed");
        assert_eq!(allowed.listener_count("tool.before"), 1);
    }

    #[test]
    fn dispatch_methods_only_invoke_their_registered_mode() {
        let dispatcher = EventDispatcher::new();
        let calls = Arc::new(RwLock::new(Vec::new()));
        let mut subscriptions = Vec::new();

        for (mode, label) in [
            (DispatchMode::Emit, "emit"),
            (DispatchMode::Parallel, "parallel"),
            (DispatchMode::Serial, "serial"),
            (DispatchMode::Bail, "bail"),
            (DispatchMode::Waterfall, "waterfall"),
        ] {
            let calls = Arc::clone(&calls);
            subscriptions.push(dispatcher.on(
                "modes",
                mode,
                0,
                move |value, next| {
                    calls.write().unwrap().push(label);
                    if mode == DispatchMode::Waterfall {
                        next.next(value)
                    } else if mode == DispatchMode::Serial || mode == DispatchMode::Bail {
                        Some(Value::Null)
                    } else {
                        None
                    }
                },
            ));
        }

        dispatcher.emit("modes", json!({}));
        assert_eq!(*calls.read().unwrap(), vec!["emit"]);
        calls.write().unwrap().clear();

        dispatcher.parallel("modes", json!({}));
        assert_eq!(*calls.read().unwrap(), vec!["parallel"]);
        calls.write().unwrap().clear();

        dispatcher.serial("modes", json!({}));
        assert_eq!(*calls.read().unwrap(), vec!["serial"]);
        calls.write().unwrap().clear();

        dispatcher.bail("modes", json!({}));
        assert_eq!(*calls.read().unwrap(), vec!["bail"]);
        calls.write().unwrap().clear();

        dispatcher.waterfall("modes", json!({}));
        assert_eq!(*calls.read().unwrap(), vec!["waterfall"]);
    }

    #[test]
    fn bail_and_serial_continue_after_null_or_false() {
        for mode in [DispatchMode::Serial, DispatchMode::Bail] {
            let dispatcher = EventDispatcher::new();
            let calls = Arc::new(AtomicU64::new(0));

            let first = Arc::clone(&calls);
            let _a = dispatcher.on("decision", mode, 0, move |_value, _chain| {
                first.fetch_add(1, Ordering::SeqCst);
                Some(json!(false))
            });
            let second = Arc::clone(&calls);
            let _b = dispatcher.on("decision", mode, 1, move |_value, _chain| {
                second.fetch_add(1, Ordering::SeqCst);
                Some(json!(null))
            });
            let third = Arc::clone(&calls);
            let _c = dispatcher.on("decision", mode, 2, move |_value, _chain| {
                third.fetch_add(1, Ordering::SeqCst);
                Some(json!("accepted"))
            });

            let result = if mode == DispatchMode::Serial {
                dispatcher.serial("decision", json!({}))
            } else {
                dispatcher.bail("decision", json!({}))
            };
            assert_eq!(result, Some(json!("accepted")));
            assert_eq!(calls.load(Ordering::SeqCst), 3);
        }
    }

    #[test]
    fn emit_calls_every_listener() {
        let dispatcher = EventDispatcher::new();
        let count = Arc::new(AtomicU64::new(0));
        let mut subscriptions = Vec::new();
        for _ in 0..3 {
            let count = Arc::clone(&count);
            subscriptions.push(dispatcher.on(
                "tick",
                DispatchMode::Emit,
                0,
                move |_value, _chain| {
                    count.fetch_add(1, Ordering::SeqCst);
                    None
                },
            ));
        }
        dispatcher.emit("tick", json!({ "n": 1 }));
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn bail_stops_at_the_first_value() {
        let dispatcher = EventDispatcher::new();
        let seen = Arc::new(AtomicU64::new(0));

        let first = Arc::clone(&seen);
        let _a = dispatcher.on("policy", DispatchMode::Bail, 0, move |_value, _chain| {
            first.fetch_add(1, Ordering::SeqCst);
            Some(json!({ "decision": "allow" }))
        });
        let second = Arc::clone(&seen);
        let _b = dispatcher.on("policy", DispatchMode::Bail, 1, move |_value, _chain| {
            second.fetch_add(1, Ordering::SeqCst);
            Some(json!({ "decision": "deny" }))
        });

        let result = dispatcher.bail("policy", json!({})).expect("bail value");
        assert_eq!(result["decision"], "allow");
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn waterfall_wraps_a_value() {
        let dispatcher = EventDispatcher::new();
        let _a = dispatcher.on("number", DispatchMode::Waterfall, 0, |value, next| {
            let value = value.as_i64().unwrap_or(0) + 1;
            next.next(json!(value))
        });
        let _b = dispatcher.on("number", DispatchMode::Waterfall, 1, |value, next| {
            let value = value.as_i64().unwrap_or(0) * 2;
            next.next(json!(value))
        });

        let result = dispatcher.waterfall("number", json!(1)).expect("value");
        assert_eq!(result, json!(4));
    }

    #[test]
    fn waterfall_can_short_circuit() {
        let dispatcher = EventDispatcher::new();
        let _a = dispatcher.on("number", DispatchMode::Waterfall, 0, |_value, _next| {
            Some(json!(99))
        });
        let _b = dispatcher.on("number", DispatchMode::Waterfall, 1, |_value, next| {
            next.next(json!(0))
        });

        assert_eq!(dispatcher.waterfall("number", json!(1)), Some(json!(99)));
    }

    #[test]
    fn dropping_a_subscription_removes_the_listener() {
        let dispatcher = EventDispatcher::new();
        let count = Arc::new(AtomicU64::new(0));
        let marker = Arc::clone(&count);
        let subscription = dispatcher.on("tick", DispatchMode::Emit, 0, move |_value, _chain| {
            marker.fetch_add(1, Ordering::SeqCst);
            None
        });
        assert_eq!(dispatcher.listener_count("tick"), 1);
        dispatcher.emit("tick", json!({}));
        drop(subscription);
        assert_eq!(dispatcher.listener_count("tick"), 0);
        dispatcher.emit("tick", json!({}));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn order_is_deterministic() {
        let dispatcher = EventDispatcher::new();
        let order = Arc::new(RwLock::new(Vec::new()));

        let later = Arc::clone(&order);
        let _b = dispatcher.on("tick", DispatchMode::Emit, 10, move |_value, _chain| {
            later.write().unwrap().push("later");
            None
        });
        let earlier = Arc::clone(&order);
        let _a = dispatcher.on("tick", DispatchMode::Emit, -10, move |_value, _chain| {
            earlier.write().unwrap().push("earlier");
            None
        });

        dispatcher.emit("tick", json!({}));
        assert_eq!(*order.read().unwrap(), vec!["earlier", "later"]);
    }
}
