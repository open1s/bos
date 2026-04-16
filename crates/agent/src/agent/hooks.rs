//! Agent hook system for extensibility
//!
//! Provides a way to register callbacks that get triggered at various points
//! during agent execution (react, run_simple, stream).
//! Also supports publishing events to the bus for external notification.

use async_trait::async_trait;
use bus::Publisher;
use futures::FutureExt;
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Events that can be hooked into during agent execution
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HookEvent {
    /// Before a tool is called
    BeforeToolCall,
    /// After a tool completes execution
    AfterToolCall,
    /// Before an LLM call is made
    BeforeLlmCall,
    /// After an LLM response is received
    AfterLlmCall,
    /// When a message is added to the conversation
    OnMessage,
    /// When the agent completes successfully
    OnComplete,
    /// When an error occurs
    OnError,
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookEvent::BeforeToolCall => write!(f, "before_tool_call"),
            HookEvent::AfterToolCall => write!(f, "after_tool_call"),
            HookEvent::BeforeLlmCall => write!(f, "before_llm_call"),
            HookEvent::AfterLlmCall => write!(f, "after_llm_call"),
            HookEvent::OnMessage => write!(f, "on_message"),
            HookEvent::OnComplete => write!(f, "on_complete"),
            HookEvent::OnError => write!(f, "on_error"),
        }
    }
}

/// Hook event payload for bus publishing
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[qserde::Archive]
#[rkyv(crate = qserde::rkyv)]
pub struct HookPayload {
    /// Event type
    pub event: String,
    /// Agent ID
    pub agent_id: String,
    /// Event-specific data
    pub data: HashMap<String, String>,
    /// Timestamp (Unix epoch milliseconds)
    pub timestamp: u64,
}

impl HookPayload {
    /// Create a new payload from event and context
    pub fn new(event: &HookEvent, context: &HookContext) -> Self {
        Self {
            event: event.to_string(),
            agent_id: context.agent_id.clone(),
            data: context.data.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }
}

/// Context passed to hooks with event-specific data
#[derive(Debug, Clone)]
pub struct HookContext {
    /// ID of the agent triggering the hook
    pub agent_id: String,
    /// Event-specific data (tool name, LLM model, messages, etc.)
    pub data: HashMap<String, String>,
}

impl HookContext {
    /// Create a new context with the given agent ID
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            data: HashMap::new(),
        }
    }

    /// Set a data key-value pair
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.data.insert(key.into(), value.into());
    }

    /// Get a data value by key
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }
}

/// Trait for implementing hooks
#[async_trait]
pub trait AgentHook: Send + Sync + 'static {
    /// Called when a hooked event occurs, returns decision on whether to continue
    async fn on_event(&self, event: HookEvent, context: &HookContext) -> HookDecision;
}

/// Decision returned by hooks to control execution flow
#[derive(Debug, Clone, Default)]
pub enum HookDecision {
    /// Continue with normal execution
    #[default]
    Continue,
    /// Abort execution immediately
    Abort,
    /// Abort with an error
    Error(String),
}

/// Registry for managing hooks
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: Arc<RwLock<HashMap<HookEvent, Vec<Arc<dyn AgentHook>>>>>,
    bus_enabled: Arc<AtomicBool>,
    bus_publishers: Arc<RwLock<HashMap<HookEvent, Publisher>>>,
}

impl HookRegistry {
    /// Create a new empty hook registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hook for an event
    pub async fn register(&self, event: HookEvent, hook: Arc<dyn AgentHook>) {
        let mut hooks = self.hooks.write().await;
        hooks.entry(event).or_insert_with(Vec::new).push(hook);
    }

    pub fn register_blocking(&self, event: HookEvent, hook: Arc<dyn AgentHook>) {
        block_on_future(self.register(event, hook));
    }

    /// Get all hooks registered for an event
    pub async fn get_hooks(&self, event: &HookEvent) -> Vec<Arc<dyn AgentHook>> {
        let hooks = self.hooks.read().await;
        hooks.get(event).cloned().unwrap_or_default()
    }

    pub fn get_hooks_blocking(&self, event: &HookEvent) -> Vec<Arc<dyn AgentHook>> {
        block_on_future(self.get_hooks(event))
    }

    /// Trigger all hooks for an event and aggregate decisions
    pub async fn trigger(&self, event: HookEvent, context: HookContext) -> HookDecision {
        let hooks = self.get_hooks(&event).await;
        let mut final_decision = HookDecision::Continue;
        for hook in hooks {
            let decision = AssertUnwindSafe(hook.on_event(event.clone(), &context))
                .catch_unwind()
                .await;
            let decision = match decision {
                Ok(d) => d,
                Err(_) => {
                    return HookDecision::Error(format!(
                        "hook panicked while handling event '{}'",
                        event
                    ));
                }
            };
            match decision {
                HookDecision::Error(msg) => return HookDecision::Error(msg),
                HookDecision::Abort => final_decision = HookDecision::Abort,
                HookDecision::Continue => {}
            }
        }
        final_decision
    }

    pub fn trigger_blocking(&self, event: HookEvent, context: HookContext) -> HookDecision {
        block_on_future(self.trigger(event, context))
    }

    /// Trigger event to both callbacks and bus (if enabled)
    pub async fn trigger_all(&self, event: HookEvent, context: HookContext) {
        // Trigger callback hooks
        match self.trigger(event.clone(), context.clone()).await {
            HookDecision::Continue => {}
            HookDecision::Abort => {
                log::debug!(
                    "Hook trigger returned Abort during trigger_all for event '{}'",
                    event
                );
            }
            HookDecision::Error(msg) => {
                log::warn!(
                    "Hook trigger returned Error during trigger_all for event '{}': {}",
                    event,
                    msg
                );
            }
        }

        // Publish to bus if enabled
        if self.bus_enabled.load(Ordering::Acquire) {
            self.publish_to_bus(&event, &context).await;
        }
    }

    pub fn trigger_all_blocking(&self, event: HookEvent, context: HookContext) {
        block_on_future(self.trigger_all(event, context));
    }

    /// Enable bus publishing for this registry
    pub async fn enable_bus(&self, enabled: bool) {
        self.bus_enabled.store(enabled, Ordering::Release);
    }

    /// Check if bus is enabled
    pub async fn is_bus_enabled(&self) -> bool {
        self.bus_enabled.load(Ordering::Acquire)
    }

    pub fn is_bus_enabled_fast(&self) -> bool {
        self.bus_enabled.load(Ordering::Acquire)
    }

    /// Register a publisher for a specific event type
    pub async fn register_bus_publisher(&self, event: HookEvent, publisher: Publisher) {
        let mut publishers = self.bus_publishers.write().await;
        publishers.insert(event, publisher);
    }

    /// Publish event to bus
    async fn publish_to_bus(&self, event: &HookEvent, context: &HookContext) {
        let publishers = self.bus_publishers.read().await;
        if let Some(publisher) = publishers.get(event) {
            let payload = HookPayload::new(event, context);
            if let Err(e) = publisher.publish(&payload).await {
                log::warn!("Failed to publish hook event to bus: {}", e);
            }
        }
    }

    /// Unregister all hooks for an event
    pub async fn clear(&self, event: &HookEvent) {
        let mut hooks = self.hooks.write().await;
        hooks.remove(event);
    }

    /// Unregister all hooks
    pub async fn clear_all(&self) {
        let mut hooks = self.hooks.write().await;
        hooks.clear();
    }

    pub fn clear_all_blocking(&self) {
        block_on_future(self.clear_all());
    }
}

fn block_on_future<F: Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if matches!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        ) {
            tokio::task::block_in_place(|| handle.block_on(future))
        } else {
            futures::executor::block_on(future)
        }
    } else {
        futures::executor::block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test hook that tracks events
    #[derive(Debug, Clone)]
    struct TestHook {
        events: Arc<std::sync::Mutex<Vec<HookEvent>>>,
    }

    impl TestHook {
        fn new() -> Self {
            Self {
                events: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl AgentHook for TestHook {
        async fn on_event(&self, event: HookEvent, _context: &HookContext) -> HookDecision {
            self.events.lock().unwrap().push(event);
            HookDecision::Continue
        }
    }

    #[tokio::test]
    async fn test_hook_registry_register() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry
            .register(HookEvent::AfterToolCall, hook.clone())
            .await;

        let hooks = registry.get_hooks(&HookEvent::AfterToolCall).await;
        assert_eq!(hooks.len(), 1);
    }

    #[tokio::test]
    async fn test_hook_registry_multiple_events() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry
            .register(HookEvent::AfterToolCall, hook.clone())
            .await;
        registry.register(HookEvent::OnComplete, hook.clone()).await;

        let tool_hooks = registry.get_hooks(&HookEvent::AfterToolCall).await;
        let complete_hooks = registry.get_hooks(&HookEvent::OnComplete).await;

        assert_eq!(tool_hooks.len(), 1);
        assert_eq!(complete_hooks.len(), 1);
    }

    #[tokio::test]
    async fn test_hook_registry_different_hooks() {
        let registry = HookRegistry::new();
        let hook1 = Arc::new(TestHook::new());
        let hook2 = Arc::new(TestHook::new());

        registry
            .register(HookEvent::AfterToolCall, hook1.clone())
            .await;
        registry
            .register(HookEvent::OnComplete, hook2.clone())
            .await;

        let tool_hooks = registry.get_hooks(&HookEvent::AfterToolCall).await;
        let complete_hooks = registry.get_hooks(&HookEvent::OnComplete).await;

        assert_eq!(tool_hooks.len(), 1);
        assert_eq!(complete_hooks.len(), 1);
    }

    #[tokio::test]
    async fn test_hook_event_variants() {
        let event1 = HookEvent::BeforeToolCall;
        let event2 = HookEvent::BeforeToolCall;
        let event3 = HookEvent::AfterToolCall;

        assert_eq!(event1, event2);
        assert_ne!(event1, event3);
    }

    #[tokio::test]
    async fn test_hook_context_new() {
        let context = HookContext::new("test-agent");

        assert_eq!(context.agent_id, "test-agent");
        assert!(context.data.is_empty());
    }

    #[tokio::test]
    async fn test_hook_context_with_data() {
        let mut context = HookContext::new("test-agent");
        context.set("tool_name", "bash");

        assert_eq!(context.get("tool_name"), Some(&"bash".to_string()));
    }

    #[test]
    fn test_hook_registry_register_blocking() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry.register_blocking(HookEvent::OnMessage, hook);

        let hooks = registry.get_hooks_blocking(&HookEvent::OnMessage);
        assert_eq!(hooks.len(), 1);
    }

    #[tokio::test]
    async fn test_hook_panic_returns_error() {
        #[derive(Debug)]
        struct PanicHook;

        #[async_trait]
        impl AgentHook for PanicHook {
            async fn on_event(&self, _event: HookEvent, _context: &HookContext) -> HookDecision {
                panic!("hook panic");
            }
        }

        let registry = HookRegistry::new();
        registry
            .register(HookEvent::BeforeLlmCall, Arc::new(PanicHook))
            .await;

        let decision = registry
            .trigger(HookEvent::BeforeLlmCall, HookContext::new("agent"))
            .await;

        assert!(matches!(decision, HookDecision::Error(_)));
    }
}
