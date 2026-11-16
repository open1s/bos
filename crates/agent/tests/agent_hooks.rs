//! Tests for the agent hook system

use agent::agent::hooks::{AgentHook, HookContext, HookDecision, HookEvent, HookRegistry};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Test hook that tracks events
#[derive(Debug, Clone)]
struct TestHook {
    events: Arc<Mutex<Vec<HookEvent>>>,
}

impl TestHook {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
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
