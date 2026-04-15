//! Agent hook system for extensibility
//!
//! Provides a way to register callbacks that get triggered at various points
//! during agent execution (react, run_simple, stream).
//! Also supports publishing events to the bus for external notification.

use async_trait::async_trait;
use bus::Publisher;
use std::collections::HashMap;
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
    bus_enabled: Arc<RwLock<bool>>,
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

    /// Get all hooks registered for an event
    pub async fn get_hooks(&self, event: &HookEvent) -> Vec<Arc<dyn AgentHook>> {
        let hooks = self.hooks.read().await;
        hooks.get(event).cloned().unwrap_or_default()
    }

    /// Trigger all hooks for an event and aggregate decisions
    pub async fn trigger(&self, event: HookEvent, context: HookContext) -> HookDecision {
        let hooks = self.get_hooks(&event).await;
        let mut final_decision = HookDecision::Continue;
        for hook in hooks {
            let decision = hook.on_event(event.clone(), &context).await;
            match decision {
                HookDecision::Error(msg) => return HookDecision::Error(msg),
                HookDecision::Abort => final_decision = HookDecision::Abort,
                HookDecision::Continue => {}
            }
        }
        final_decision
    }

    /// Trigger event to both callbacks and bus (if enabled)
    pub async fn trigger_all(&self, event: HookEvent, context: HookContext) {
        // Trigger callback hooks
        self.trigger(event.clone(), context.clone()).await;

        // Publish to bus if enabled
        let bus_enabled = *self.bus_enabled.read().await;
        if bus_enabled {
            self.publish_to_bus(&event, &context).await;
        }
    }

    /// Enable bus publishing for this registry
    pub async fn enable_bus(&self, enabled: bool) {
        let mut bus_enabled = self.bus_enabled.write().await;
        *bus_enabled = enabled;
    }

    /// Check if bus is enabled
    pub async fn is_bus_enabled(&self) -> bool {
        *self.bus_enabled.read().await
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

        registry.register(HookEvent::AfterToolCall, hook.clone()).await;

        let hooks = registry.get_hooks(&HookEvent::AfterToolCall).await;
        assert_eq!(hooks.len(), 1);
    }

    #[tokio::test]
    async fn test_hook_registry_multiple_events() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry.register(HookEvent::AfterToolCall, hook.clone()).await;
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

        registry.register(HookEvent::AfterToolCall, hook1.clone()).await;
        registry.register(HookEvent::OnComplete, hook2.clone()).await;

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
}