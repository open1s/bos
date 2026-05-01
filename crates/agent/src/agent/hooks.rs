//! Agent hook system for extensibility
//!
//! Provides a way to register callbacks that get triggered at various points
//! during agent execution (react, run_simple, stream).
//! Also supports publishing events to the bus for external notification.

use async_trait::async_trait;
use bus::Publisher;
use futures::FutureExt;
use react::runtime::ReActApp;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent::context::{AgentReactContext, AgentSession};

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
/// Re-exports react::runtime::HookDecision for unified type across the codebase.
pub use react::runtime::HookDecision;

/// Registry for managing hooks
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: Arc<Mutex<HashMap<HookEvent, Vec<Arc<dyn AgentHook>>>>>,
    bus_enabled: Arc<AtomicBool>,
    bus_publishers: Arc<Mutex<HashMap<HookEvent, Publisher>>>,
}

impl HookRegistry {
    /// Create a new empty hook registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hook for an event
    pub fn register(&self, event: HookEvent, hook: Arc<dyn AgentHook>) {
        let mut hooks = self.hooks.lock().unwrap();
        hooks.entry(event).or_insert_with(Vec::new).push(hook);
    }

    /// Get all hooks registered for an event
    pub fn get_hooks(&self, event: &HookEvent) -> Vec<Arc<dyn AgentHook>> {
        let hooks = self.hooks.lock().unwrap();
        hooks.get(event).cloned().unwrap_or_default()
    }

    pub fn has_hooks(&self, event: &HookEvent) -> bool {
        let hooks = self.hooks.lock().unwrap();
        hooks.get(event).map(|v| !v.is_empty()).unwrap_or(false)
    }

    pub fn register_blocking(&self, event: HookEvent, hook: Arc<dyn AgentHook>) {
        self.register(event, hook);
    }

    pub fn get_hooks_blocking(&self, event: &HookEvent) -> Vec<Arc<dyn AgentHook>> {
        self.get_hooks(event)
    }

    pub fn has_hooks_blocking(&self, event: &HookEvent) -> bool {
        self.has_hooks(event)
    }

    pub fn trigger_blocking(&self, event: HookEvent, context: HookContext) -> HookDecision {
        block_on_future(self.trigger(event, context))
    }

    /// Trigger all hooks for an event and aggregate decisions
    pub async fn trigger(&self, event: HookEvent, context: HookContext) -> HookDecision {
        let hooks = self.get_hooks(&event);
        let mut final_decision = HookDecision::Continue;
        for hook in hooks {
            let wrapped = AssertUnwindSafe(hook.on_event(event.clone(), &context));
            let decision = wrapped.catch_unwind().await;
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

    /// Trigger event to both callbacks and bus (if enabled)
    pub async fn trigger_all(&self, event: HookEvent, context: HookContext) {
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
    }

    pub fn trigger_all_blocking(&self, event: HookEvent, context: HookContext) {
        block_on_future(self.trigger_all(event, context))
    }

    /// Enable bus publishing for this registry
    pub fn enable_bus(&self, enabled: bool) {
        self.bus_enabled.store(enabled, Ordering::Release);
    }

    /// Check if bus is enabled
    pub fn is_bus_enabled(&self) -> bool {
        self.bus_enabled.load(Ordering::Acquire)
    }

    pub fn is_bus_enabled_fast(&self) -> bool {
        self.bus_enabled.load(Ordering::Acquire)
    }

    /// Register a publisher for a specific event type
    pub fn register_bus_publisher(&self, event: HookEvent, publisher: Publisher) {
        let mut publishers = self.bus_publishers.lock().unwrap();
        publishers.insert(event, publisher);
    }

    /// Unregister all hooks for an event
    pub fn clear(&self, event: &HookEvent) {
        let mut hooks = self.hooks.lock().unwrap();
        hooks.remove(event);
    }

    /// Unregister all hooks
    pub fn clear_all(&self) {
        let mut hooks = self.hooks.lock().unwrap();
        hooks.clear();
    }

    pub fn clear_all_blocking(&self) {
        self.clear_all();
    }
}

impl ReActApp for HookRegistry {
    type Session = AgentSession;
    type Context = AgentReactContext;

    fn name(&self) -> &str {
        "hook_registry"
    }

    async fn before_llm_call(
        &self,
        req: &mut react::llm::LlmRequest,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> HookDecision {
        let mut ctx = HookContext::new("");
        ctx.set("model", &req.model);
        match self.trigger(HookEvent::BeforeLlmCall, ctx).await {
            HookDecision::Continue => HookDecision::Continue,
            HookDecision::Abort => HookDecision::Abort,
            HookDecision::Error(msg) => HookDecision::Error(msg),
        }
    }

    async fn after_llm_response(
        &self,
        _response: &mut react::llm::LlmResponse,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {
        let ctx = HookContext::new("");
        let _ = self.trigger(HookEvent::AfterLlmCall, ctx).await;
    }

    async fn after_llm_response_step(
        &self,
        response_text: &str,
        had_tool_call: bool,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {
        let mut ctx = HookContext::new("");
        ctx.set("response_type", "stream");
        ctx.set("response_text", response_text);
        ctx.set("had_tool_call", &had_tool_call.to_string());
        let _ = self.trigger(HookEvent::AfterLlmCall, ctx).await;
    }

    async fn before_tool_call(
        &self,
        tool_name: &str,
        args: &mut Value,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) -> HookDecision {
        let mut ctx = HookContext::new("");
        ctx.set("tool_name", tool_name);
        ctx.set("tool_args", args.to_string());
        match self.trigger(HookEvent::BeforeToolCall, ctx).await {
            HookDecision::Continue => HookDecision::Continue,
            HookDecision::Abort => HookDecision::Abort,
            HookDecision::Error(msg) => HookDecision::Error(msg),
        }
    }

    async fn after_tool_result(
        &self,
        tool_name: &str,
        result: &mut Result<Value, react::engine::ReactError>,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {
        let mut ctx = HookContext::new("");
        ctx.set("tool_name", tool_name);
        ctx.set(
            "tool_result",
            &result.as_ref().map(|v| v.to_string()).unwrap_or_default(),
        );
        let _ = self.trigger(HookEvent::AfterToolCall, ctx).await;
    }

    async fn on_thought(
        &self,
        thought: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {
        let mut ctx = HookContext::new("");
        ctx.set("thought", thought);
        let _ = self.trigger(HookEvent::OnMessage, ctx).await;
    }

    async fn on_final_answer(
        &self,
        answer: &str,
        _session: &mut Self::Session,
        _context: &mut Self::Context,
    ) {
        let mut ctx = HookContext::new("");
        ctx.set("result", answer);
        let _ = self.trigger(HookEvent::OnComplete, ctx).await;
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

    #[test]
    fn test_hook_registry_register() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry.register(HookEvent::AfterToolCall, hook.clone());

        let hooks = registry.get_hooks(&HookEvent::AfterToolCall);
        assert_eq!(hooks.len(), 1);
    }

    #[test]
    fn test_hook_registry_multiple_events() {
        let registry = HookRegistry::new();
        let hook = Arc::new(TestHook::new());

        registry.register(HookEvent::AfterToolCall, hook.clone());
        registry.register(HookEvent::OnComplete, hook.clone());

        let tool_hooks = registry.get_hooks(&HookEvent::AfterToolCall);
        let complete_hooks = registry.get_hooks(&HookEvent::OnComplete);

        assert_eq!(tool_hooks.len(), 1);
        assert_eq!(complete_hooks.len(), 1);
    }

    #[test]
    fn test_hook_registry_different_hooks() {
        let registry = HookRegistry::new();
        let hook1 = Arc::new(TestHook::new());
        let hook2 = Arc::new(TestHook::new());

        registry.register(HookEvent::AfterToolCall, hook1.clone());
        registry.register(HookEvent::OnComplete, hook2.clone());

        let tool_hooks = registry.get_hooks(&HookEvent::AfterToolCall);
        let complete_hooks = registry.get_hooks(&HookEvent::OnComplete);

        assert_eq!(tool_hooks.len(), 1);
        assert_eq!(complete_hooks.len(), 1);
    }

    #[test]
    fn test_hook_event_variants() {
        let event1 = HookEvent::BeforeToolCall;
        let event2 = HookEvent::BeforeToolCall;
        let event3 = HookEvent::AfterToolCall;

        assert_eq!(event1, event2);
        assert_ne!(event1, event3);
    }

    #[test]
    fn test_hook_context_new() {
        let context = HookContext::new("test-agent");

        assert_eq!(context.agent_id, "test-agent");
        assert!(context.data.is_empty());
    }

    #[test]
    fn test_hook_context_with_data() {
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
        registry.register(HookEvent::BeforeLlmCall, Arc::new(PanicHook));

        let decision = registry
            .trigger(HookEvent::BeforeLlmCall, HookContext::new("agent"))
            .await;

        assert!(matches!(decision, HookDecision::Error(_)));
    }
}
