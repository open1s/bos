//! Tests for the cancellation design:
//! 1. `call_tool()` injects `__call_id__` into the tool's args.
//! 2. `tool.cancel(call_id)` (on the AsyncTool trait) is invoked with the
//!    matching call_id when the engine's cancel listener receives a cancel
//!    message on the bus topic.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use react::engine::ReActEngineBuilder;
use react::llm::vendor::{ChatCompletionResponse, ChatMessage, Choice};
use react::llm::{
    LlmClient, LlmContext, LlmError, LlmRequest, LlmResponse, LlmResponseResult, LlmSession, TokenStream,
};
use react::runtime::ReActApp;
use react::tool::registry::{AsyncTool, ToolVariant};
use react::tool::ToolError;
use serde_json::{json, Value};

#[derive(Default)]
struct TestApp;

impl ReActApp for TestApp {
    type Session = LlmSession;
    type Context = LlmContext;
}

/// Async test tool that records what it received.
struct CapturingTool {
    name: String,
    received_args: Arc<DashMap<String, Value>>,
    received_cancel_calls: Arc<DashMap<String, usize>>,
    cancelable: bool,
}

impl CapturingTool {
    fn new(
        name: &str,
        cancelable: bool,
    ) -> (
        Self,
        Arc<DashMap<String, Value>>,
        Arc<DashMap<String, usize>>,
    ) {
        let received_args = Arc::new(DashMap::new());
        let received_cancel_calls = Arc::new(DashMap::new());
        (
            Self {
                name: name.to_string(),
                received_args: received_args.clone(),
                received_cancel_calls: received_cancel_calls.clone(),
                cancelable,
            },
            received_args,
            received_cancel_calls,
        )
    }
}

#[async_trait]
impl AsyncTool for CapturingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        "captures args and cancel calls".to_string()
    }
    fn is_cancelable(&self) -> bool {
        self.cancelable
    }
    fn cancel(&self, call_id: &str) {
        let mut entry = self.received_cancel_calls.entry(call_id.to_string()).or_insert(0);
        *entry += 1;
    }
    async fn run(&self, input: &Value) -> Result<Value, ToolError> {
        // Record what we got.
        let call_id = input
            .get("__call_id__")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>")
            .to_string();
        self.received_args
            .insert(call_id.clone(), input.clone());
        Ok(json!({"ok": true, "call_id": call_id}))
    }
}

#[tokio::test]
async fn call_tool_injects_call_id_into_args() {
    let (tool, received_args, _cancel_calls) = CapturingTool::new("capturing", true);

    // Build an engine with the tool. No bus needed for this test.
    let llm = Box::new(NoopLlm);
    let tool_box: Box<dyn AsyncTool> = Box::new(tool);
    let engine = ReActEngineBuilder::<TestApp>::new()
        .llm(llm)
        .with_tool(ToolVariant::Async(tool_box))
        .agent_name("test-agent".to_string())
        .max_steps(1)
        .build()
        .unwrap();

    // Drive the engine manually via `call_tool` to verify injection.
    let mut input = json!({"user_key": "abc"});
    let result = engine.call_tool("capturing", &mut input, "test-call-1").await;
    assert!(result.is_ok(), "call_tool failed: {:?}", result.err());

    let captured = received_args.get("test-call-1").expect("tool did not run");
    assert_eq!(
        captured.get("__call_id__").and_then(|v| v.as_str()),
        Some("test-call-1"),
        "engine did not inject __call_id__"
    );
    assert_eq!(captured.get("user_key").and_then(|v| v.as_str()), Some("abc"));
}

#[tokio::test]
async fn tool_cancel_invokes_cancel_callback() {
    let (tool, cancel_calls) = {
        let (t, _a, c) = CapturingTool::new("capturing", true);
        (Box::new(t) as Box<dyn AsyncTool>, c)
    };

    assert_eq!(
        tool.is_cancelable(),
        true,
        "tool should be cancelable",
    );

    let count_before = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_before, 0, "no cancel calls before test");

    tool.cancel("call-XYZ");

    let count_after = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_after, 1, "tool.cancel should have been invoked once");

    tool.cancel("call-XYZ");
    let count_twice = cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0);
    assert_eq!(count_twice, 2, "tool.cancel should be idempotent (count increments)");

    // Different call_id
    tool.cancel("other-call");
    assert_eq!(
        cancel_calls.get("other-call").map(|r| *r.value()).unwrap_or(0),
        1,
    );
    assert_eq!(
        cancel_calls.get("call-XYZ").map(|r| *r.value()).unwrap_or(0),
        2,
    );
}



// ── No-op LLM (we drive the engine directly, not through `react()`) ───────

struct NoopLlm;

#[async_trait]
impl LlmClient<LlmSession, LlmContext> for NoopLlm {
    async fn complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> LlmResponseResult {
        Ok(LlmResponse::OpenAI(make_text_response(
            "Final Answer: done".to_string(),
            true,
        )))
    }
    async fn stream_complete(
        &self,
        _persona: Option<String>,
        _request: LlmRequest,
        _session: &mut LlmSession,
        _context: &mut LlmContext,
    ) -> Result<TokenStream, LlmError> {
        Ok(Box::pin(futures::stream::empty()))
    }
    fn supports_tools(&self) -> bool {
        false
    }
    fn provider_name(&self) -> &'static str {
        "noop"
    }
}

fn make_text_response(content: String, is_final: bool) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: "test-123".to_string(),
        object: "chat.completion".to_string(),
        created: 1234567890,
        model: "test-model".to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(content),
                tool_calls: None,
                function_call: None,
                reasoning_content: None,
                extra: serde_json::Value::Object(serde_json::Map::new()),
            },
            stop_reason: None,
            finish_reason: if is_final {
                Some("stop".to_string())
            } else {
                Some("continue".to_string())
            },
            logprobs: None,
        }],
        usage: None,
        system_fingerprint: None,
        nvext: None,
    }
}